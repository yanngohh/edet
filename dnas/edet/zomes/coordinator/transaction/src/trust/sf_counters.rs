use std::collections::HashMap;

use hdk::prelude::*;
use transaction_integrity::debt_contract::{ContractStatus, DebtContract};
use transaction_integrity::types::constants::*;
use transaction_integrity::types::SupportSatisfactionTag;
use transaction_integrity::*;

use crate::trust_cache::{cache_sf_counters, get_cached_sf_counters};

/// Per-debtor satisfaction and failure counters.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SFCounters {
    /// S_ij: cumulative successful debt transfer amount.
    pub satisfaction: f64,
    /// F_ij: cumulative failed (expired) debt amount.
    pub failure: f64,
    /// Earliest epoch in which we observed the debtor's first contract.
    /// Used to compute `age = current_epoch - first_seen_epoch` for the
    /// time-weighted volume maturation cap (matching sim: n_mat_eff = min(n_ij, MAX_VOLUME * age)).
    pub first_seen_epoch: u64,
    /// S in the last RECENT_WINDOW_K epochs (for behavioral switch detection).
    pub recent_satisfaction: f64,
    /// F in the last RECENT_WINDOW_K epochs (for behavioral switch detection).
    pub recent_failure: f64,
}

/// Compute S/F counters for the current agent as creditor.
/// Scans all contracts (active, transferred, expired) where this agent is creditor,
/// plus support satisfaction links from accepted drain transactions.
/// - Transferred contracts contribute their original amount minus remaining to S
/// - Expired contracts contribute their remaining amount to F
/// - Active contracts with partial transfer contribute transferred portion to S
/// - Support satisfaction links contribute drained amount as S from the supporter
pub fn compute_sf_counters(creditor: AgentPubKey) -> ExternResult<HashMap<AgentPubKeyB64, SFCounters>> {
    // Check cache first
    if let Some(cached) = get_cached_sf_counters(&creditor)? {
        let mut result: HashMap<AgentPubKeyB64, SFCounters> = HashMap::new();
        for (key, (satisfaction, failure, first_seen_epoch, recent_satisfaction, recent_failure)) in cached {
            result.insert(
                key,
                SFCounters { satisfaction, failure, first_seen_epoch, recent_satisfaction, recent_failure },
            );
        }
        return Ok(result);
    }

    let links =
        get_links(LinkQuery::try_new(creditor.clone(), LinkTypes::CreditorToContracts)?, GetStrategy::default())?;

    let get_inputs: Vec<GetInput> = links
        .into_iter()
        .filter_map(|link| {
            link.target
                .into_action_hash()
                .map(|hash| GetInput::new(hash.into(), GetOptions::default()))
        })
        .collect();

    let records: Vec<Record> = HDK.with(|hdk| hdk.borrow().get(get_inputs))?.into_iter().flatten().collect();

    let mut counters: HashMap<AgentPubKeyB64, SFCounters> = HashMap::new();
    let mut epoch_volume: HashMap<(AgentPubKeyB64, u64), f64> = HashMap::new();

    // Compute current epoch for recent window filtering
    let current_epoch = transaction_integrity::types::timestamp_to_epoch(sys_time()?);
    let recent_cutoff = current_epoch.saturating_sub(RECENT_WINDOW_K);

    // Helper closure to add S with epoch cap
    let mut add_satisfaction =
        |debtor: AgentPubKeyB64, amount: f64, epoch: u64, counters: &mut HashMap<AgentPubKeyB64, SFCounters>| {
            if amount <= DUST_THRESHOLD {
                return;
            }
            let current_volume = *epoch_volume.get(&(debtor.clone(), epoch)).unwrap_or(&0.0);
            let allowed = (MAX_VOLUME_PER_EPOCH - current_volume).max(0.0);
            let actual_s = amount.min(allowed);

            if actual_s > 0.0 {
                epoch_volume.insert((debtor.clone(), epoch), current_volume + actual_s);
                let entry = counters.entry(debtor).or_insert(SFCounters {
                    satisfaction: 0.0,
                    failure: 0.0,
                    first_seen_epoch: epoch,
                    recent_satisfaction: 0.0,
                    recent_failure: 0.0,
                });
                entry.satisfaction += actual_s;
                // Track earliest epoch seen for age-based volume maturation
                if epoch < entry.first_seen_epoch {
                    entry.first_seen_epoch = epoch;
                }
                // Recent window: accumulate S in last RECENT_WINDOW_K epochs
                if epoch >= recent_cutoff {
                    entry.recent_satisfaction += actual_s;
                }
            }
        };

    // Helper closure to add F with contagion to co-signers
    let add_failure = |debtor: AgentPubKeyB64,
                       amount: f64,
                       epoch: u64,
                       contract: &DebtContract,
                       counters: &mut HashMap<AgentPubKeyB64, SFCounters>| {
        if amount <= DUST_THRESHOLD {
            return;
        }

        // 1. Penalize the direct debtor
        let entry = counters.entry(debtor.clone()).or_insert(SFCounters {
            satisfaction: 0.0,
            failure: 0.0,
            first_seen_epoch: contract.start_epoch,
            recent_satisfaction: 0.0,
            recent_failure: 0.0,
        });
        entry.failure += amount;
        if contract.start_epoch < entry.first_seen_epoch {
            entry.first_seen_epoch = contract.start_epoch;
        }
        // Recent window: accumulate F in last RECENT_WINDOW_K epochs
        if epoch >= recent_cutoff {
            entry.recent_failure += amount;
        }

        // 2. Support Co-Signing Contagion: Penalize the co-signers proportionally.
        //
        // Note on normalization — we use (coef / total_coef) for the penalty
        // fraction, which normalizes the distribution to sum to `amount` regardless
        // of whether co-signer coefficients sum to < 1, = 1, or > 1.
        // Whitepaper Lemma `lem:cascade_contagion` uses the raw coefficient c_v
        // (not normalized), assuming the support-breakdown's coefficients sum to 1.
        // Since the integrity zome for SupportBreakdown enforces sum-to-1 at
        // creation (support_breakdown.rs:32-53), normalization is idempotent for
        // honest nodes. The normalized form is more robust against any floating-point
        // drift and is equivalent for sum-to-1 inputs.
        if let Some(co_signers) = &contract.co_signers {
            let total_coef: f64 = co_signers.iter().map(|(_, c)| *c).sum();
            if total_coef > 0.0 {
                for (co_signer, coef) in co_signers {
                    if *co_signer != debtor {
                        // Don't double-penalize the debtor
                        let penalty = amount * (coef / total_coef);
                        if penalty > DUST_THRESHOLD {
                            let cs_entry = counters.entry(co_signer.clone()).or_insert(SFCounters {
                                satisfaction: 0.0,
                                failure: 0.0,
                                first_seen_epoch: contract.start_epoch,
                                recent_satisfaction: 0.0,
                                recent_failure: 0.0,
                            });
                            cs_entry.failure += penalty;
                            if contract.start_epoch < cs_entry.first_seen_epoch {
                                cs_entry.first_seen_epoch = contract.start_epoch;
                            }
                            if epoch >= recent_cutoff {
                                cs_entry.recent_failure += penalty;
                            }
                        }
                    }
                }
            }
        }
    };

    for record in records {
        // Follow update chain to get latest version
        let latest = get_latest_contract_record(record.action_address().clone())?;
        let contract: DebtContract = match latest.entry().to_app_option::<DebtContract>().ok().flatten() {
            Some(c) => c,
            None => continue,
        };

        // Get the original contract to know the initial amount
        let original: DebtContract = match record.entry().to_app_option::<DebtContract>().ok().flatten() {
            Some(c) => c,
            None => continue,
        };

        match contract.status {
            ContractStatus::Transferred => {
                // Use current_epoch (when the transfer is being observed) rather
                // than original.start_epoch (when the contract was created) for the
                // volume-cap bucket and recent-window classification.
                // Old behaviour used start_epoch, so a contract created 20 epochs ago and
                // transferred today would be epoch-stamped 20 epochs in the past:
                //  - The per-epoch volume cap (epoch_volume key) would hit the 20-epoch-old
                //    bucket (probably empty → cap not enforced for old contracts).
                //  - The recent-window S check (epoch >= recent_cutoff) would use start_epoch
                //    instead of the actual satisfaction event epoch, systematically
                //    under-weighting S from long-maturity contracts and biasing r_recent upward.
                add_satisfaction(contract.debtor.clone(), original.amount, current_epoch, &mut counters);
            }
            ContractStatus::Expired => {
                // Expired: transferred portion is satisfaction, remaining is failure.
                // Use current_epoch for both events (transfer and expiry both happened now).
                let transferred = original.amount - contract.amount;
                add_satisfaction(contract.debtor.clone(), transferred, current_epoch, &mut counters);
                // Use current_epoch for failure: it EXPIRED now, regardless of when it started
                add_failure(contract.debtor.clone(), contract.amount, current_epoch, &original, &mut counters);

                // VOUCH SPONSOR CONTAGION (Whitepaper §2.4):
                // When a vouchee defaults, the creditor also records F against the sponsor.
                // "The default δ is directly counted as a failure F for the sponsor s by the creditor."
                // Use current_epoch for sponsor recent-window.
                // Old behaviour used original.start_epoch, so long-maturity defaults escaped
                // the recent-window detection if the contract was created > K_w epochs ago.
                // The expiry EVENT happened now, so recent-window should use current_epoch.
                let debtor_key: AgentPubKey = contract.debtor.clone().into();
                if let Ok(sponsors) = crate::vouch::get_all_sponsors_for_entrant(debtor_key) {
                    for sponsor in sponsors {
                        let sponsor_key: AgentPubKeyB64 = sponsor;
                        let entry = counters.entry(sponsor_key).or_insert(SFCounters {
                            satisfaction: 0.0,
                            failure: 0.0,
                            first_seen_epoch: u64::MAX,
                            recent_satisfaction: 0.0,
                            recent_failure: 0.0,
                        });
                        entry.failure += contract.amount;
                        // Compare current_epoch (default event epoch), not start_epoch
                        if current_epoch >= recent_cutoff {
                            entry.recent_failure += contract.amount;
                        }
                    }
                }
            }
            ContractStatus::Active => {
                // CREDITOR-SIDE INDEPENDENT EXPIRATION DETECTION (Gap 5 Prong 2):
                if current_epoch >= original.start_epoch + original.maturity {
                    // Past maturity, treat as expired.
                    // Use current_epoch for both satisfaction and failure events.
                    let transferred = original.amount - contract.amount;
                    add_satisfaction(contract.debtor.clone(), transferred, current_epoch, &mut counters);
                    // Detected expiration: count as current failure
                    add_failure(contract.debtor.clone(), contract.amount, current_epoch, &original, &mut counters);

                    // VOUCH SPONSOR CONTAGION (Whitepaper §2.4): same as Expired arm.
                    // Use current_epoch for sponsor recent-window.
                    let debtor_key: AgentPubKey = contract.debtor.clone().into();
                    if let Ok(sponsors) = crate::vouch::get_all_sponsors_for_entrant(debtor_key) {
                        for sponsor in sponsors {
                            let sponsor_key: AgentPubKeyB64 = sponsor;
                            let entry = counters.entry(sponsor_key).or_insert(SFCounters {
                                satisfaction: 0.0,
                                failure: 0.0,
                                first_seen_epoch: u64::MAX,
                                recent_satisfaction: 0.0,
                                recent_failure: 0.0,
                            });
                            entry.failure += contract.amount;
                            // Compare current_epoch, not start_epoch
                            if current_epoch >= recent_cutoff {
                                entry.recent_failure += contract.amount;
                            }
                        }
                    }
                } else {
                    // Active, count only transferred portion.
                    // Use current_epoch.
                    let transferred = original.amount - contract.amount;
                    add_satisfaction(contract.debtor.clone(), transferred, current_epoch, &mut counters);
                }
            }
            ContractStatus::Archived => {
                // Archived contracts are already counted - skip to avoid double counting
            }
        }
    }
    debug!("SF Counters for {:?}: {:?}", creditor, counters);

    // ── Support Satisfaction: scan drain events where this agent was beneficiary ──
    // When a drain successfully reduces the beneficiary's debt, a
    // AgentToSupportSatisfaction link is created (see side_effects.rs).
    // The beneficiary records S from the supporter, populating the pre-trust
    // vector so EigenTrust can propagate trust back to the beneficiary.
    // This fixes zero-capacity for pure buyers whose debt was extinguished via support.
    let support_links = get_links(
        LinkQuery::try_new(creditor.clone(), LinkTypes::AgentToSupportSatisfaction)?,
        GetStrategy::default(),
    )?;

    for link in support_links {
        let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.into_inner()));
        if let Ok(sat_tag) = SupportSatisfactionTag::try_from(tag_bytes) {
            add_satisfaction(sat_tag.supporter, sat_tag.amount, sat_tag.epoch, &mut counters);
        }
    }

    // ── Repayment Satisfaction: scan contracts where this agent was the DEBTOR ──
    // When a debtor successfully repays (Transferred), they record satisfaction for
    // the creditor. This ensures the debtor includes their repayment witnesses in
    // their pre-trust vector, enabling EigenTrust trust loops.
    // This fixes zero-capacity for pure buyers who have repaid their debt.
    let debtor_links =
        get_links(LinkQuery::try_new(creditor.clone(), LinkTypes::DebtorToContracts)?, GetStrategy::default())?;

    let debtor_inputs: Vec<GetInput> = debtor_links
        .into_iter()
        .filter_map(|link| {
            link.target
                .into_action_hash()
                .map(|hash| GetInput::new(hash.into(), GetOptions::default()))
        })
        .collect();

    let debtor_records: Vec<Record> = HDK.with(|hdk| hdk.borrow().get(debtor_inputs))?.into_iter().flatten().collect();

    for record in debtor_records {
        let latest = get_latest_contract_record(record.action_address().clone())?;
        let contract: DebtContract = match latest.entry().to_app_option::<DebtContract>().ok().flatten() {
            Some(c) => c,
            None => continue,
        };

        if contract.status == ContractStatus::Transferred {
            // Get original to know the amount and epoch
            let original: DebtContract = match record.entry().to_app_option::<DebtContract>().ok().flatten() {
                Some(c) => c,
                None => continue,
            };
            // Note: add_satisfaction takes the AgentPubKey of the TRUSTED party.
            // Here, the debtor trusts the creditor.
            add_satisfaction(contract.creditor.clone(), original.amount, original.start_epoch, &mut counters);
        }
    }

    // Cache the computed counters (including recent window data)
    let cache_map: HashMap<AgentPubKeyB64, (f64, f64, u64, f64, f64)> = counters
        .iter()
        .map(|(k, v)| {
            (k.clone(), (v.satisfaction, v.failure, v.first_seen_epoch, v.recent_satisfaction, v.recent_failure))
        })
        .collect();
    let _ = cache_sf_counters(creditor, cache_map);

    Ok(counters)
}

/// Follow the update chain of a contract to get the latest version.
/// Uses get_details to find the latest update action for the original entry.
pub(crate) fn get_latest_contract_record(action_hash: ActionHash) -> ExternResult<Record> {
    match get_details(action_hash.clone(), GetOptions::default())? {
        Some(Details::Record(record_details)) => {
            if record_details.updates.is_empty() {
                Ok(record_details.record)
            } else {
                // Get the latest update
                let latest_update = record_details
                    .updates
                    .into_iter()
                    .max_by(|a, b| a.action().timestamp().cmp(&b.action().timestamp()))
                    .unwrap();

                // Fetch the actual record for the update
                get(latest_update.hashed.hash, GetOptions::default())?.ok_or(wasm_error!(WasmErrorInner::Guest(
                    coordinator_trust_error::CONTRACT_RECORD_NOT_FOUND.to_string()
                )))
            }
        }
        _ => get(action_hash, GetOptions::default())?
            .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_trust_error::CONTRACT_RECORD_NOT_FOUND.to_string()))),
    }
}

#[hdk_extern]
pub fn get_my_sf_counters(_: ()) -> ExternResult<HashMap<AgentPubKeyB64, SFCounters>> {
    let agent = agent_info()?.agent_initial_pubkey;
    compute_sf_counters(agent)
}
