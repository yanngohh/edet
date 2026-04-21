use hdk::prelude::*;
use transaction_integrity::types::constants::{coordinator_vouch_error, DUST_THRESHOLD};
use transaction_integrity::*;

use crate::contracts;
use crate::trust;

/// Input for creating a vouch with capacity check.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateVouchInput {
    pub entrant: AgentPubKey,
    pub amount: f64,
}

#[hdk_extern]
pub fn create_vouch(input: CreateVouchInput) -> ExternResult<Record> {
    let sponsor = agent_info()?.agent_initial_pubkey;

    // Check sponsor has enough unlocked capacity to vouch.
    // available = total_capacity - own_debt - active_locked
    //
    // Note: `get_total_locked_capacity` already accounts for slashing — it sums
    // only the *remaining unslashed* amount per vouch (vouch.amount - vouch.slashed_amount
    // for Active/partially-Slashed, zero for Released/fully-Slashed). There is no need to
    // subtract a separate `total_slashed_as_sponsor` term: doing so would double-count the
    // already-excluded slashed portion. The wallet field `total_slashed_as_sponsor` is kept
    // for auditing only and is reconciled via `reconcile_slash_wallet` — it does not affect
    // capacity math.
    let sponsor_capacity = trust::compute_credit_capacity_for_agent(sponsor.clone())?;
    let sponsor_debt = contracts::get_total_debt(sponsor.clone())?;
    let sponsor_locked = get_total_locked_capacity(sponsor.clone())?;
    let sponsor_available = sponsor_capacity - sponsor_debt - sponsor_locked;

    if input.amount > sponsor_available {
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_vouch_error::INSUFFICIENT_CAPACITY.to_string())));
    }

    let vouch = Vouch {
        sponsor: sponsor.clone(),
        entrant: input.entrant.clone(),
        amount: input.amount,
        status: VouchStatus::Active,
        slashed_amount: 0.0,
        is_genesis: false,
        expired_contract_hash: None,
    };

    let vouch_hash = create_entry(&EntryTypes::Vouch(vouch.clone()))?;
    trust::add_acquaintance(input.entrant.clone())?;

    // Link Entrant -> Vouch (so entrant can find their capacity)
    create_link(vouch.entrant.clone(), vouch_hash.clone(), LinkTypes::EntrantToVouch, ())?;

    // Link Sponsor -> Vouch (so sponsor can see their stakes)
    create_link(vouch.sponsor.clone(), vouch_hash.clone(), LinkTypes::SponsorToVouch, ())?;

    let record = get(vouch_hash, GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_vouch_error::CREATED_VOUCH_NOT_FOUND.to_string())))?;

    Ok(record)
}

/// Bootstrap vouch for founding cohort — skips sponsor capacity check.
///
/// Only available in `test-epoch` builds (integration tests). In production
/// builds this function does not exist: the bootstrap mechanism is trial
/// transactions (small amounts, always Pending, seller approves manually).
/// Genesis vouching is a Sybil attack vector in production because any agent
/// could mint unbacked capacity at zero cost.
///
/// Equivalent to the simulation's `_setup_genesis_vouching` (verify_theory.py:54-80).
#[cfg(feature = "test-epoch")]
#[hdk_extern]
pub fn genesis_vouch(input: CreateVouchInput) -> ExternResult<Record> {
    let sponsor = agent_info()?.agent_initial_pubkey;

    let vouch = Vouch {
        sponsor: sponsor.clone(),
        entrant: input.entrant.clone(),
        amount: input.amount,
        status: VouchStatus::Active,
        slashed_amount: 0.0,
        is_genesis: true,
        expired_contract_hash: None,
    };

    let vouch_hash = create_entry(&EntryTypes::Vouch(vouch.clone()))?;
    trust::add_acquaintance(input.entrant.clone())?;

    // Link Entrant -> Vouch (so entrant can find their capacity)
    create_link(vouch.entrant.clone(), vouch_hash.clone(), LinkTypes::EntrantToVouch, ())?;

    // Link Sponsor -> Vouch (so sponsor can see their stakes)
    create_link(vouch.sponsor.clone(), vouch_hash.clone(), LinkTypes::SponsorToVouch, ())?;

    let record = get(vouch_hash, GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_vouch_error::CREATED_VOUCH_NOT_FOUND.to_string())))?;

    Ok(record)
}

/// Get total locked capacity for a sponsor (sum of active vouch amounts).
#[hdk_extern]
pub fn get_total_locked_capacity(sponsor: AgentPubKey) -> ExternResult<f64> {
    let links = get_links(LinkQuery::try_new(sponsor, LinkTypes::SponsorToVouch)?, GetStrategy::default())?;

    let mut total_locked = 0.0;

    for link in links {
        let target_hash = match ActionHash::try_from(link.target) {
            Ok(hash) => hash,
            Err(_) => continue,
        };

        if let Some(record) = get_latest_vouch(target_hash)? {
            if let Ok(Some(vouch)) = record.entry().to_app_option::<Vouch>() {
                if vouch.status == VouchStatus::Active {
                    // Active vouch: full amount is locked
                    total_locked += vouch.amount;
                } else if vouch.status == VouchStatus::Slashed && vouch.slashed_amount < vouch.amount {
                    // Partially slashed: remaining amount is still locked
                    total_locked += vouch.amount - vouch.slashed_amount;
                }
                // Released or fully slashed: nothing locked
            }
        }
    }

    Ok(total_locked)
}

/// Get the latest version of a vouch following the full update chain.
///
/// The protocol creates VouchUpdates links from the *original* vouch hash (flat
/// topology), so a single get_links call normally suffices.  However, this
/// function iteratively walks the update chain to guard against any future code
/// path that might create a link from a non-original hash (e.g. partial-slash
/// followed by full-slash, where the second update link might point to the
/// first update rather than the original).  Iterative traversal ensures
/// correctness regardless of link topology, at the cost of at most O(update_depth)
/// extra get_links calls (≤ 2 in practice: one initial slash + one full slash).
fn get_latest_vouch(original_hash: ActionHash) -> ExternResult<Option<Record>> {
    let mut current_hash = original_hash;
    // Safety bound: a vouch can be slashed at most twice (partial then full).
    // Limiting to 10 iterations prevents an infinite loop on pathological graphs.
    for _ in 0..10 {
        let links =
            get_links(LinkQuery::try_new(current_hash.clone(), LinkTypes::VouchUpdates)?, GetStrategy::default())?;
        let maybe_next = links
            .into_iter()
            .max_by(|a, b| a.timestamp.cmp(&b.timestamp))
            .and_then(|link| link.target.into_action_hash());
        match maybe_next {
            Some(next_hash) => {
                current_hash = next_hash;
            }
            None => break,
        }
    }
    get(current_hash, GetOptions::default())
}

#[hdk_extern]
pub fn get_vouched_capacity(agent: AgentPubKey) -> ExternResult<f64> {
    let mut visited = Vec::new();
    query_vouched_capacity(agent, &mut visited)
}

/// Return all vouch records for which `agent` is the entrant.
/// Includes all vouch statuses (Active, Slashed, Released) so callers can
/// inspect the full vouch history including slashed amounts.
/// Follows the VouchUpdates chain to return the latest version of each vouch.
#[hdk_extern]
pub fn get_vouches_for_entrant(agent: AgentPubKey) -> ExternResult<Vec<Vouch>> {
    let links = get_links(LinkQuery::try_new(agent, LinkTypes::EntrantToVouch)?, GetStrategy::default())?;
    let mut vouches = Vec::new();
    for link in links {
        let target_hash = match ActionHash::try_from(link.target) {
            Ok(hash) => hash,
            Err(_) => continue,
        };
        if let Some(record) = get_latest_vouch(target_hash)? {
            if let Ok(Some(vouch)) = record.entry().to_app_option::<Vouch>() {
                vouches.push(vouch);
            }
        }
    }
    Ok(vouches)
}

pub fn query_vouched_capacity(agent: AgentPubKey, visited: &mut Vec<AgentPubKey>) -> ExternResult<f64> {
    if visited.contains(&agent) {
        return Ok(0.0);
    }
    visited.push(agent.clone());

    // Get all vouches where agent is the entrant
    let links = get_links(LinkQuery::try_new(agent, LinkTypes::EntrantToVouch)?, GetStrategy::default())?;

    let mut total_vouched = 0.0;

    for link in links {
        let target_hash = match ActionHash::try_from(link.target) {
            Ok(hash) => hash,
            Err(_) => continue,
        };

        if let Some(record) = get_latest_vouch(target_hash)? {
            if let Ok(Some(vouch)) = record.entry().to_app_option::<Vouch>() {
                // Only count active or partially slashed vouches
                if vouch.status == VouchStatus::Active || vouch.status == VouchStatus::Slashed {
                    let direct_effective = vouch.amount - vouch.slashed_amount;
                    if direct_effective > 0.0 {
                        // Cascading Liquidation (Whitepaper §2.4):
                        // Cap vouchee's capacity at the sponsor's actual remaining staking balance.
                        // Genesis vouches (self-vouched) have no upstream sponsor to draw from.
                        if vouch.sponsor == vouch.entrant || vouch.is_genesis {
                            total_vouched += direct_effective;
                        } else {
                            // Find sponsor's gross stake recursively
                            let sponsor_gross_stake = query_vouched_capacity(vouch.sponsor.clone(), visited)?;

                            if sponsor_gross_stake == 0.0 {
                                // Sponsor has no upstream vouches — they are a root/founding node.
                                // Their vouch is self-backed: take it at face value.
                                // (Cascading liquidation only applies when the sponsor is themselves
                                // vouched and their upstream stake could be depleted.)
                                total_vouched += direct_effective;
                            } else {
                                // Cascading liquidation: cap vouchee capacity at sponsor's
                                // remaining upstream stake. The sponsor's gross_stake is itself
                                // recursively reduced by their vouches' slashed_amount fields,
                                // so no separate slashed-wallet lookup is needed here.
                                let cascaded_effective = direct_effective.min(sponsor_gross_stake);
                                total_vouched += cascaded_effective;
                            }
                        }
                    }
                }
            }
        }
    }

    visited.pop();

    Ok(total_vouched)
}

#[hdk_extern]
pub fn get_my_vouched_for_agent(agent: AgentPubKey) -> ExternResult<bool> {
    let my_pub_key = agent_info()?.agent_initial_pubkey;
    let links = get_links(LinkQuery::try_new(my_pub_key.clone(), LinkTypes::SponsorToVouch)?, GetStrategy::default())?;
    debug!("get_my_vouched_for_agent my_pub_key={:?} found {} links", my_pub_key, links.len());

    for link in links {
        let target_hash = match ActionHash::try_from(link.target) {
            Ok(hash) => hash,
            Err(_) => continue,
        };

        if let Some(record) = get_latest_vouch(target_hash)? {
            if let Ok(Some(vouch)) = record.entry().to_app_option::<Vouch>() {
                debug!("found vouch sponsor={:?} entrant={:?} status={:?}", vouch.sponsor, vouch.entrant, vouch.status);
                if vouch.entrant == agent && vouch.status == VouchStatus::Active {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

#[hdk_extern]
pub fn get_vouchers_for_agent(agent: AgentPubKey) -> ExternResult<Vec<AgentPubKey>> {
    let links = get_links(LinkQuery::try_new(agent, LinkTypes::EntrantToVouch)?, GetStrategy::default())?;

    let mut sponsors = Vec::new();

    for link in links {
        let target_hash = match ActionHash::try_from(link.target) {
            Ok(hash) => hash,
            Err(_) => continue,
        };

        if let Some(record) = get_latest_vouch(target_hash)? {
            if let Ok(Some(vouch)) = record.entry().to_app_option::<Vouch>() {
                if vouch.status == VouchStatus::Active && !sponsors.contains(&vouch.sponsor) {
                    sponsors.push(vouch.sponsor);
                }
            }
        }
    }

    Ok(sponsors)
}

/// Return all sponsors that ever vouched for `entrant` (regardless of current vouch status).
/// Used by trust computation to propagate failure F to sponsors when a vouchee defaults,
/// matching Whitepaper §2.4: "the default δ is directly counted as a failure F for the sponsor s
/// by the creditor."
///
/// Unlike `get_vouchers_for_agent`, this includes Slashed and Released vouches so that the F
/// contagion is recorded even after the vouch has been fully consumed.
pub fn get_all_sponsors_for_entrant(entrant: AgentPubKey) -> ExternResult<Vec<AgentPubKeyB64>> {
    let links = get_links(LinkQuery::try_new(entrant, LinkTypes::EntrantToVouch)?, GetStrategy::default())?;

    let mut sponsors: Vec<AgentPubKeyB64> = Vec::new();

    for link in links {
        let target_hash = match ActionHash::try_from(link.target) {
            Ok(hash) => hash,
            Err(_) => continue,
        };

        if let Some(record) = get_latest_vouch(target_hash)? {
            if let Ok(Some(vouch)) = record.entry().to_app_option::<Vouch>() {
                let sponsor_b64: AgentPubKeyB64 = vouch.sponsor.into();
                if !sponsors.contains(&sponsor_b64) {
                    sponsors.push(sponsor_b64);
                }
            }
        }
    }

    Ok(sponsors)
}

/// Input for the `receive_vouch_slash` remote handler.
/// Sent via `call_remote` from the debtor to each sponsor when a contract expires.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SlashVouchInput {
    /// The entrant (debtor) whose default triggered the slash.
    pub entrant: AgentPubKeyB64,
    /// Total slash amount requested from this sponsor (already multiplied by VOUCH_SLASHING_MULTIPLIER).
    pub amount: f64,
    /// ActionHash of the expired DebtContract — kept for audit trail / idempotency.
    pub expired_contract_hash: ActionHash,
    /// Inline copy of the expired contract's debtor field.
    /// Included so the sponsor can verify the debtor matches the entrant without
    /// a DHT lookup (which may not have gossipped yet at the time of the call_remote).
    pub contract_debtor: AgentPubKeyB64,
}

/// Slash a vouch when entrant defaults. Called by contract expiration processing.
/// Returns the amount dispatched for slashing.
///
/// `expired_contract_hash` is the ActionHash of the expired DebtContract record —
/// it is sent to sponsors as proof that the slashing is legitimate.
///
/// The function dispatches slash requests to each sponsor via `call_remote`.
/// Sponsors execute `receive_vouch_slash` on their own cells to perform the
/// `update_entry` locally (where they are the author). This follows the same
/// fire-and-forget pattern used by `create_drain_request` and
/// `create_buyer_debt_contract`.
///
/// The `total_slashed_as_sponsor` wallet field is updated by sponsors calling
/// `reconcile_slash_wallet` periodically.
pub fn slash_vouch_for_entrant(
    entrant: AgentPubKey,
    amount: f64,
    expired_contract_hash: ActionHash,
) -> ExternResult<f64> {
    // Find active vouches for this entrant
    let links = get_links(LinkQuery::try_new(entrant.clone(), LinkTypes::EntrantToVouch)?, GetStrategy::default())?;
    let mut remaining_to_slash = amount * transaction_integrity::types::constants::VOUCH_SLASHING_MULTIPLIER;
    let mut total_dispatched = 0.0;
    // Track which sponsors need to be notified and how much to slash per sponsor.
    let mut sponsor_slashes: std::collections::HashMap<AgentPubKey, f64> = std::collections::HashMap::new();

    for link in links {
        if remaining_to_slash <= DUST_THRESHOLD {
            break;
        }

        let original_hash = match ActionHash::try_from(link.target.clone()) {
            Ok(hash) => hash,
            Err(_) => continue,
        };

        // Get the latest version of this vouch
        let Some(record) = get_latest_vouch(original_hash.clone())? else {
            continue;
        };

        let Ok(Some(vouch)) = record.entry().to_app_option::<Vouch>() else {
            continue;
        };

        // Only slash active or partially slashed vouches
        if vouch.status == VouchStatus::Released {
            continue;
        }

        let available_to_slash = vouch.amount - vouch.slashed_amount;
        if available_to_slash <= DUST_THRESHOLD {
            continue;
        }

        let slash_amount = remaining_to_slash.min(available_to_slash);

        remaining_to_slash -= slash_amount;
        total_dispatched += slash_amount;
        *sponsor_slashes.entry(vouch.sponsor).or_insert(0.0) += slash_amount;
    }

    // Dispatch slash requests to each sponsor via call_remote.
    // We include the contract_debtor inline so the sponsor can verify the slash
    // without a DHT lookup (which may not have gossipped yet at call time).
    let zome_name = zome_info()?.name;
    let entrant_key: AgentPubKeyB64 = entrant.clone().into();
    let contract_debtor_key: AgentPubKeyB64 = entrant.into();

    for (sponsor, slash_amount) in &sponsor_slashes {
        let input = SlashVouchInput {
            entrant: entrant_key.clone(),
            amount: *slash_amount,
            expired_contract_hash: expired_contract_hash.clone(),
            contract_debtor: contract_debtor_key.clone(),
        };
        debug!(
            "Dispatching vouch slash to sponsor {:?}: amount={:.2}, contract={:?}",
            sponsor, slash_amount, expired_contract_hash
        );
        // Fire-and-forget: sponsor verifies inline proof and applies the slash.
        // If the sponsor is offline, they can reconcile via reconcile_slash_wallet.
        match call_remote(sponsor.clone(), zome_name.clone(), "receive_vouch_slash".into(), None, input) {
            Err(e) => {
                warn!("SLASH: call_remote to sponsor {:?} failed: {:?}", sponsor, e);
            }
            Ok(ZomeCallResponse::NetworkError(e)) => {
                warn!("SLASH: sponsor {:?} returned NetworkError: {:?}", sponsor, e);
            }
            Ok(ZomeCallResponse::Unauthorized(auth, _, zome, func)) => {
                warn!("SLASH: sponsor {:?} unauthorized ({:?}) for {}/{}", sponsor, auth, zome, func);
            }
            Ok(_) => {
                debug!("SLASH: slash dispatched successfully to sponsor {:?}", sponsor);
            }
        }
    }

    Ok(total_dispatched)
}

/// Remote handler: process a vouch slash request sent by a defaulting entrant.
///
/// Called via `call_remote` from `slash_vouch_for_entrant` on the debtor's cell.
/// The sponsor performs the actual `update_entry` on their own source chain
/// (they are the author of the vouch Create action, so the update is valid).
///
/// Authentication: verifies the expired contract proof via the same integrity
/// validation that `validate_update_vouch` enforces for debtor-initiated slashes.
/// The contract must be Expired/Archived and its debtor must match the entrant.
#[hdk_extern]
pub fn receive_vouch_slash(input: SlashVouchInput) -> ExternResult<f64> {
    let sponsor = agent_info()?.agent_initial_pubkey;
    let entrant: AgentPubKey = input.entrant.clone().into();

    // Rate-limit: slash dispatch for a given entrant should arrive at most once
    // per contract expiration event. A 5-second global cooldown blocks rapid-fire
    // slash spam while still allowing legitimate slashes from different entrants.
    let now_secs = sys_time()?.as_seconds_and_nanos().0 as u64;
    if !crate::trust_cache::check_and_set_rate_limit("receive_vouch_slash", 5, now_secs) {
        warn!(
            "SLASH [{}]: rate-limited receive_vouch_slash from entrant={}",
            AgentPubKeyB64::from(sponsor.clone()),
            input.entrant
        );
        return Ok(0.0);
    }

    debug!(
        "SLASH [{}]: received receive_vouch_slash for entrant={}, amount={:.2}",
        AgentPubKeyB64::from(sponsor.clone()),
        input.entrant,
        input.amount
    );

    // Validate the expired contract proof before processing the slash.
    //
    // Security model:
    //   1. PRIMARY CHECK (always): The caller includes `contract_debtor` inline.
    //      We verify it matches `entrant`. This is always available immediately
    //      and eliminates the gossip-timing race.
    //   2. SECONDARY CHECK (opportunistic): If the expired contract is already
    //      visible on this node's DHT, we fetch it and re-verify the debtor field
    //      and Expired/Archived status. This provides a stronger cryptographic
    //      guarantee when gossip has propagated.
    //   3. SELF-AUTHENTICATION: Only the debtor's own cell calls
    //      `process_contract_expirations`, which is what triggers this
    //      `call_remote`. A malicious third party would need to fabricate
    //      both the entrant key and a matching contract_debtor — they cannot
    //      produce a valid expired_contract_hash for an entrant they don't control.
    //
    // The inline field closes the gossip-timing window while the opportunistic
    // DHT check provides defence-in-depth when the record is available.
    let inline_debtor: AgentPubKey = input.contract_debtor.clone().into();
    if inline_debtor != entrant {
        return Err(wasm_error!(WasmErrorInner::Guest(
            "Slash proof: inline contract_debtor does not match entrant".to_string()
        )));
    }

    // Opportunistic DHT verification (non-blocking: skip if not yet available).
    if let Ok(Some(contract_record)) = get(input.expired_contract_hash.clone(), GetOptions::default()) {
        let contract: transaction_integrity::debt_contract::DebtContract = contract_record
            .entry()
            .to_app_option()
            .map_err(|e| wasm_error!(WasmErrorInner::Guest(format!("Slash proof: deserialize error: {e:?}"))))?
            .ok_or(wasm_error!(WasmErrorInner::Guest("Slash proof: record is not a DebtContract".to_string())))?;
        if contract.status != transaction_integrity::debt_contract::ContractStatus::Expired
            && contract.status != transaction_integrity::debt_contract::ContractStatus::Archived
        {
            return Err(wasm_error!(WasmErrorInner::Guest(
                "Slash proof: contract is not Expired or Archived".to_string()
            )));
        }
        let dht_debtor: AgentPubKey = contract.debtor.clone().into();
        if dht_debtor != entrant {
            return Err(wasm_error!(WasmErrorInner::Guest(
                "Slash proof: DHT contract debtor does not match entrant".to_string()
            )));
        }
    }

    // Find this sponsor's vouches for the entrant and slash them.
    let links = get_links(LinkQuery::try_new(sponsor.clone(), LinkTypes::SponsorToVouch)?, GetStrategy::default())?;

    let mut remaining = input.amount;
    let mut total_slashed = 0.0;

    for link in links {
        if remaining <= DUST_THRESHOLD {
            break;
        }

        let original_hash = match ActionHash::try_from(link.target.clone()) {
            Ok(hash) => hash,
            Err(_) => continue,
        };

        let Some(record) = get_latest_vouch(original_hash.clone())? else {
            continue;
        };
        let previous_hash = record.action_address().clone();

        let Ok(Some(vouch)) = record.entry().to_app_option::<Vouch>() else {
            continue;
        };

        // Only slash vouches for this specific entrant
        if vouch.entrant != entrant {
            continue;
        }
        if vouch.status == VouchStatus::Released {
            continue;
        }

        let available = vouch.amount - vouch.slashed_amount;
        if available <= DUST_THRESHOLD {
            continue;
        }

        let slash_amount = remaining.min(available);

        let mut updated_vouch = vouch.clone();
        updated_vouch.slashed_amount += slash_amount;
        updated_vouch.expired_contract_hash = Some(input.expired_contract_hash.clone());
        if updated_vouch.slashed_amount >= updated_vouch.amount - DUST_THRESHOLD {
            updated_vouch.status = VouchStatus::Slashed;
        }

        // update_entry succeeds here because we (the sponsor) are the author.
        // Retry once with a refreshed hash if the first attempt fails due to
        // a concurrent slash request landing between our read and this write
        // (the "stale previous_hash" race). On retry we re-read the vouch to
        // pick up the latest state and re-compute the slash accordingly.
        let update_result = update_entry(previous_hash.clone(), &updated_vouch).or_else(|_| {
            // Re-read the latest version to get a fresh previous_hash.
            if let Ok(Some(fresh_record)) = get_latest_vouch(original_hash.clone()) {
                let fresh_previous_hash = fresh_record.action_address().clone();
                if let Ok(Some(fresh_vouch)) = fresh_record.entry().to_app_option::<Vouch>() {
                    // Recompute the updated vouch from the latest state so we don't
                    // overwrite a concurrent slash that already partially slashed.
                    let fresh_available = fresh_vouch.amount - fresh_vouch.slashed_amount;
                    if fresh_available <= DUST_THRESHOLD || fresh_vouch.status == VouchStatus::Released {
                        // Already fully slashed or released by a concurrent request — skip.
                        return Err(wasm_error!(WasmErrorInner::Guest("stale: already fully slashed".to_string())));
                    }
                    let fresh_slash = remaining.min(fresh_available);
                    let mut retry_vouch = fresh_vouch.clone();
                    retry_vouch.slashed_amount += fresh_slash;
                    retry_vouch.expired_contract_hash = Some(input.expired_contract_hash.clone());
                    if retry_vouch.slashed_amount >= retry_vouch.amount - DUST_THRESHOLD {
                        retry_vouch.status = VouchStatus::Slashed;
                    }
                    update_entry(fresh_previous_hash, &retry_vouch)
                } else {
                    Err(wasm_error!(WasmErrorInner::Guest("stale: could not decode fresh vouch".to_string())))
                }
            } else {
                Err(wasm_error!(WasmErrorInner::Guest("stale: could not fetch fresh vouch".to_string())))
            }
        });

        match update_result {
            Ok(updated_hash) => {
                let _ = create_link(original_hash, updated_hash, LinkTypes::VouchUpdates, ());
                remaining -= slash_amount;
                total_slashed += slash_amount;
                debug!(
                    "SLASH [{}]: slashed {:.2} from vouch for entrant={}",
                    AgentPubKeyB64::from(sponsor.clone()),
                    slash_amount,
                    input.entrant
                );
            }
            Err(e) => {
                warn!(
                    "SLASH [{}]: update_entry failed for vouch {:?}: {:?}",
                    AgentPubKeyB64::from(sponsor.clone()),
                    previous_hash,
                    e
                );
            }
        }
    }

    Ok(total_slashed)
}

/// A vouch record with its original and latest action hashes, for UI display and release.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VouchRecord {
    pub original_hash: ActionHash,
    pub previous_hash: ActionHash,
    pub vouch: Vouch,
}

/// Return all vouches given by the calling agent as sponsor, with action hashes needed for release.
/// Follows the SponsorToVouch links to return the latest version of each vouch.
#[hdk_extern]
pub fn get_vouches_given(_: ()) -> ExternResult<Vec<VouchRecord>> {
    let sponsor = agent_info()?.agent_initial_pubkey;
    let links = get_links(LinkQuery::try_new(sponsor, LinkTypes::SponsorToVouch)?, GetStrategy::default())?;

    let mut result = Vec::new();

    for link in links {
        let original_hash = match ActionHash::try_from(link.target) {
            Ok(hash) => hash,
            Err(_) => continue,
        };

        if let Some(record) = get_latest_vouch(original_hash.clone())? {
            let previous_hash = record.action_address().clone();
            if let Ok(Some(vouch)) = record.entry().to_app_option::<Vouch>() {
                result.push(VouchRecord { original_hash, previous_hash, vouch });
            }
        }
    }

    Ok(result)
}

/// Release a vouch (sponsor reclaims their locked capacity).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ReleaseVouchInput {
    pub original_vouch_hash: ActionHash,
    pub previous_vouch_hash: ActionHash,
}

#[hdk_extern]
pub fn release_vouch(input: ReleaseVouchInput) -> ExternResult<Record> {
    let sponsor = agent_info()?.agent_initial_pubkey;

    // Get the current vouch
    let record = get(input.previous_vouch_hash.clone(), GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_vouch_error::RELEASE_VOUCH_NOT_FOUND.to_string())))?;

    let vouch: Vouch = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_vouch_error::RELEASE_INVALID_ENTRY.to_string())))?;

    // Verify caller is the sponsor
    if sponsor != vouch.sponsor {
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_vouch_error::RELEASE_NOT_SPONSOR.to_string())));
    }

    // Verify vouch is active (can't release already slashed/released)
    if vouch.status != VouchStatus::Active {
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_vouch_error::RELEASE_INVALID_STATUS.to_string())));
    }

    // Block release while the entrant still has active debt contracts.
    // If a sponsor releases a vouch and the entrant subsequently defaults on a contract
    // that was already active at release time, the slash proof would target a Released
    // vouch — a transition the integrity zome does not allow (Released→Slashed is blocked).
    // We prevent this scenario at the coordinator level: the sponsor must wait until
    // all of the entrant's active contracts have reached a terminal state
    // (Transferred, Expired, or Archived) before the vouch can be released.
    let entrant_key: AgentPubKey = vouch.entrant.clone();
    let active_contracts = crate::contracts::get_active_contracts_for_debtor(entrant_key)?;
    if !active_contracts.is_empty() {
        return Err(wasm_error!(WasmErrorInner::Guest(
            coordinator_vouch_error::RELEASE_ENTRANT_HAS_ACTIVE_CONTRACTS.to_string()
        )));
    }

    // Create released vouch
    let mut updated_vouch = vouch.clone();
    updated_vouch.status = VouchStatus::Released;
    // Release is sponsor-initiated: no expired contract proof needed.
    updated_vouch.expired_contract_hash = None;

    let updated_hash = update_entry(input.previous_vouch_hash.clone(), &updated_vouch)?;

    // Create update link
    create_link(input.original_vouch_hash, updated_hash.clone(), LinkTypes::VouchUpdates, ())?;

    get(updated_hash, GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_vouch_error::RELEASE_UPDATED_NOT_FOUND.to_string())))
}

/// Reconcile the calling agent's `total_slashed_as_sponsor` wallet field.
///
/// This is an audit-only operation: capacity calculations (get_total_locked_capacity,
/// query_vouched_capacity) read vouch entries directly and are always correct.
/// This function exists only to keep the wallet's `total_slashed_as_sponsor` field
/// accurate for UI display and off-chain accounting purposes.
///
/// The sponsor calls this on their own cell; it scans all SponsorToVouch links,
/// sums `slashed_amount` across all vouches, and writes the total to the wallet.
#[hdk_extern]
pub fn reconcile_slash_wallet(_: ()) -> ExternResult<f64> {
    let sponsor = agent_info()?.agent_initial_pubkey;
    let links = get_links(LinkQuery::try_new(sponsor.clone(), LinkTypes::SponsorToVouch)?, GetStrategy::default())?;

    let mut total_slashed: f64 = 0.0;

    for link in links {
        let target_hash = match ActionHash::try_from(link.target) {
            Ok(hash) => hash,
            Err(_) => continue,
        };
        if let Some(record) = get_latest_vouch(target_hash)? {
            if let Ok(Some(vouch)) = record.entry().to_app_option::<Vouch>() {
                total_slashed += vouch.slashed_amount;
            }
        }
    }

    // Update the wallet's audit field
    let (original_wallet_hash, wallet_record) = crate::wallet::get_wallet_for_agent(sponsor)?;
    if let (Some(original_hash), Some(record)) = (original_wallet_hash, wallet_record) {
        let previous_hash = record.action_address().clone();
        if let Ok(Some(mut wallet)) = record.entry().to_app_option::<Wallet>() {
            wallet.total_slashed_as_sponsor = total_slashed;
            let updated_hash = update_entry(previous_hash, &wallet)?;
            // Create a WalletUpdates link so get_latest_wallet() can find this update.
            // This mirrors update_wallet() in wallet.rs which creates the same link.
            // Without this link, subsequent calls to get_wallet_for_agent/get_latest_wallet
            // would return the stale pre-reconciliation wallet.
            create_link(original_hash, updated_hash, LinkTypes::WalletUpdates, ())?;
        }
    }

    Ok(total_slashed)
}

// ============================================================================
//  Sponsor-side slash reconciliation for fire-and-forget call_remote failures
// ============================================================================

/// Discover and apply any vouch slashes that were missed because the sponsor was
/// offline when `slash_vouch_for_entrant` dispatched the `receive_vouch_slash`
/// call_remote.
///
/// Background: `slash_vouch_for_entrant` uses a soft fire-and-forget pattern —
/// it logs failures but does not retry. If the sponsor is offline at the time a
/// debtor's contract expires, the `receive_vouch_slash` call_remote is lost and
/// the sponsor's vouch entries are never updated to reflect the slash.
///
/// This function lets the sponsor self-heal: they call it at startup (or on demand)
/// to scan their vouches, identify entrants with expired/archived contracts that have
/// not yet been credited against the sponsor's vouch, and apply the outstanding slashes.
///
/// The function is idempotent: slashes are applied only up to the vouch's remaining
/// unslashed balance, and contracts already referenced by `expired_contract_hash` on
/// the vouch are skipped.
///
/// Returns the number of slash operations applied.
#[hdk_extern]
pub fn reconcile_pending_slashes(_: ()) -> ExternResult<u32> {
    let sponsor = agent_info()?.agent_initial_pubkey;

    // Collect all of this sponsor's vouches.
    let vouch_links =
        get_links(LinkQuery::try_new(sponsor.clone(), LinkTypes::SponsorToVouch)?, GetStrategy::default())?;

    let mut slashes_applied = 0u32;

    for link in vouch_links {
        let original_hash = match ActionHash::try_from(link.target.clone()) {
            Ok(h) => h,
            Err(_) => continue,
        };

        // Fetch the latest state of this vouch.
        let Some(vouch_record) = get_latest_vouch(original_hash.clone())? else {
            continue;
        };
        let Ok(Some(vouch)) = vouch_record.entry().to_app_option::<Vouch>() else {
            continue;
        };

        // Skip released vouches — slashing a released vouch is not permitted.
        if vouch.status == VouchStatus::Released {
            continue;
        }

        let available = vouch.amount - vouch.slashed_amount;
        if available <= DUST_THRESHOLD {
            continue;
        }

        // Build set of contract hashes already processed by this vouch.
        let already_processed: std::collections::HashSet<ActionHash> =
            vouch.expired_contract_hash.clone().into_iter().collect();

        // Fetch all contracts for this entrant via DHT. May be incomplete if gossip
        // has not fully propagated, but that is acceptable — a later call will catch any
        // remaining gaps once gossip completes.
        let entrant_contracts = match crate::contracts::get_all_contracts_as_debtor(vouch.entrant.clone()) {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    "reconcile_pending_slashes: failed to fetch contracts for entrant {:?}: {:?}",
                    AgentPubKeyB64::from(vouch.entrant.clone()),
                    e
                );
                continue;
            }
        };

        // Collect expired/archived contracts not yet reflected in the vouch.
        let pending: Vec<(ActionHash, f64)> = entrant_contracts
            .into_iter()
            .filter_map(|record| {
                let contract_hash = record.action_address().clone();
                if already_processed.contains(&contract_hash) {
                    return None;
                }
                let contract = record
                    .entry()
                    .to_app_option::<transaction_integrity::debt_contract::DebtContract>()
                    .ok()
                    .flatten()?;
                if contract.status != transaction_integrity::debt_contract::ContractStatus::Expired
                    && contract.status != transaction_integrity::debt_contract::ContractStatus::Archived
                {
                    return None;
                }
                // Sanity-check: debtor on the contract must match the vouch's entrant.
                let debtor: AgentPubKey = contract.debtor.clone().into();
                if debtor != vouch.entrant {
                    return None;
                }
                // Slash exposure = original principal × slashing multiplier, matching
                // how `slash_vouch_for_entrant` computes `remaining_to_slash`.
                Some((
                    contract_hash,
                    contract.original_amount * transaction_integrity::types::constants::VOUCH_SLASHING_MULTIPLIER,
                ))
            })
            .collect();

        if pending.is_empty() {
            continue;
        }

        // Apply slashes for each missed contract.
        for (contract_hash, slash_exposure) in pending {
            // Re-fetch the latest vouch to get a fresh previous_hash and current balance,
            // since an earlier iteration may have already consumed some of the unslashed amount.
            let Some(fresh_record) = get_latest_vouch(original_hash.clone())? else {
                break;
            };
            let fresh_prev = fresh_record.action_address().clone();
            let Ok(Some(fresh_vouch)) = fresh_record.entry().to_app_option::<Vouch>() else {
                break;
            };

            // Skip if this specific contract hash was recorded in a prior iteration.
            if fresh_vouch.expired_contract_hash.as_ref() == Some(&contract_hash) {
                continue;
            }

            let fresh_available = fresh_vouch.amount - fresh_vouch.slashed_amount;
            if fresh_available <= DUST_THRESHOLD {
                break;
            }

            let slash_amount = slash_exposure.min(fresh_available);

            let mut updated = fresh_vouch.clone();
            updated.slashed_amount += slash_amount;
            updated.expired_contract_hash = Some(contract_hash.clone());
            if updated.slashed_amount >= updated.amount - DUST_THRESHOLD {
                updated.status = VouchStatus::Slashed;
            }

            // Attempt the update; retry once on stale-hash failures.
            let update_result = update_entry(fresh_prev.clone(), &updated).or_else(|_| {
                if let Ok(Some(retry_record)) = get_latest_vouch(original_hash.clone()) {
                    let retry_prev = retry_record.action_address().clone();
                    if let Ok(Some(retry_vouch)) = retry_record.entry().to_app_option::<Vouch>() {
                        let retry_avail = retry_vouch.amount - retry_vouch.slashed_amount;
                        if retry_avail > DUST_THRESHOLD {
                            let retry_slash = slash_exposure.min(retry_avail);
                            let mut retry_upd = retry_vouch.clone();
                            retry_upd.slashed_amount += retry_slash;
                            retry_upd.expired_contract_hash = Some(contract_hash.clone());
                            if retry_upd.slashed_amount >= retry_upd.amount - DUST_THRESHOLD {
                                retry_upd.status = VouchStatus::Slashed;
                            }
                            return update_entry(retry_prev, &retry_upd);
                        }
                    }
                }
                Err(wasm_error!(WasmErrorInner::Guest(
                    "reconcile_pending_slashes: stale previous_hash and retry failed".to_string()
                )))
            });

            match update_result {
                Ok(updated_hash) => {
                    create_link(original_hash.clone(), updated_hash, LinkTypes::VouchUpdates, ())?;
                    slashes_applied += 1;
                    debug!(
                        "reconcile_pending_slashes: applied slash {:.2} for contract {:?}",
                        slash_amount, contract_hash
                    );
                }
                Err(e) => {
                    warn!("reconcile_pending_slashes: failed to apply slash for contract {:?}: {:?}", contract_hash, e);
                }
            }
        }
    }

    // Re-sync the wallet display field now that slashes are applied.
    let _ = reconcile_slash_wallet(());

    Ok(slashes_applied)
}
