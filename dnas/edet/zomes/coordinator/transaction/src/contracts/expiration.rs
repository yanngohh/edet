use hdk::prelude::*;
use transaction_integrity::debt_contract::{ContractStatus, DebtContract};
use transaction_integrity::types::constants::*;
use transaction_integrity::types::timestamp_to_epoch;
use transaction_integrity::*;

use super::debt_balance::update_debt_balance;
use super::get_active_contracts_for_debtor;

/// Result of processing contract expirations.
#[derive(Serialize, Deserialize, Debug)]
pub struct ExpirationResult {
    /// Per-creditor breakdown: (creditor, expired_amount).
    /// These should be used to update F counters.
    pub creditor_failures: Vec<(AgentPubKeyB64, f64)>,
    /// Total amount of expired debt.
    pub total_expired: f64,
    /// Total vouch stake dispatched for slashing across all sponsors.
    pub total_slashed_dispatched: f64,
}

/// The next debt expiration deadline and total amount due.
#[derive(Serialize, Deserialize, Debug)]
pub struct NextDeadline {
    pub timestamp: Timestamp,
    pub amount: f64,
}

/// Process contract expirations for the current agent.
/// Checks all active contracts where the agent is debtor.
/// For contracts past maturity with remaining debt, marks them as Expired
/// and returns the per-creditor failure amounts for F counter updates.
///
/// When a **trial** contract (amount < eta * V_base) expires, a permanent
/// `DebtorToBlockedTrialSeller` link is written. This blocks the (buyer, seller)
/// pair from any future trial transactions (Change 1 — Permanent Trial Block).
///
/// Failure observations for community-wide contagion are also published here
/// (moved from publish_trust_row, which no longer handles F>S pruning).
///
/// This should be called lazily before trust computation or transaction creation.
#[hdk_extern]
pub fn process_contract_expirations(_: ()) -> ExternResult<ExpirationResult> {
    let agent = agent_info()?.agent_initial_pubkey;
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    let contracts = super::get_active_contracts_with_original_for_debtor(agent.clone())?;
    let mut creditor_failures: Vec<(AgentPubKeyB64, f64)> = Vec::new();
    let mut total_expired = 0.0;
    let mut total_expired_debt = 0.0f64;
    let mut expired_contract_count = 0i64;
    let mut total_slashed_dispatched = 0.0f64;

    for (original_action_hash, record) in contracts {
        let contract: DebtContract = match record.entry().to_app_option::<DebtContract>().ok().flatten() {
            Some(c) if c.status == ContractStatus::Active => c,
            _ => continue,
        };

        // Check if contract has expired
        if current_epoch >= contract.start_epoch + contract.maturity {
            // Mark contract as expired FIRST so the slash proof hash references an Expired entry.
            let latest_action_hash = record.action_address().clone();
            let expired_contract = DebtContract { status: ContractStatus::Expired, ..contract.clone() };
            let updated_hash = update_entry(latest_action_hash.clone(), &expired_contract)?;
            create_link(original_action_hash.clone(), updated_hash.clone(), LinkTypes::DebtContractUpdates, ())?;

            if contract.amount > DUST_THRESHOLD {
                // Record failure
                let creditor = contract.creditor.clone();
                if let Some(existing) = creditor_failures.iter_mut().find(|(c, _)| *c == creditor) {
                    existing.1 += contract.amount;
                } else {
                    creditor_failures.push((creditor, contract.amount));
                }
                total_expired += contract.amount;

                // PERMANENT TRIAL BLOCK (Change 1):
                // When a trial contract expires, permanently block the (buyer, seller) pair.
                // This prevents Sybil identity cycling via repeated trial defaults.
                // Uses the contract's is_trial flag (set at creation time) rather than
                // an amount threshold, which could give false positives after partial transfers.
                if contract.is_trial {
                    let seller_agent: AgentPubKey = contract.creditor.clone().into();
                    create_link(agent.clone(), seller_agent, LinkTypes::DebtorToBlockedTrialSeller, ())?;
                }

                // FAILURE OBSERVATION PUBLICATION:
                // Publish failure observation so other nodes can query who defaulted on whom.
                // The witness_bilateral_rate is the fraction of this contract that expired
                // as failure. For a fully-expired contract this is 1.0; for partially
                // transferred contracts (partial S, partial F) we'd need the creditor's
                // full bilateral history which isn't available on the debtor's cell.
                // Using 1.0 is conservative: the creditor observed a full default on this
                // contract. The aggregate median across multiple witnesses smooths this out.
                let creditor_agent: AgentPubKey = contract.creditor.clone().into();
                let debtor_agent_obs: AgentPubKey = contract.debtor.clone().into();
                crate::trust::publish_failure_observation(
                    creditor_agent,
                    debtor_agent_obs,
                    contract.amount,
                    current_epoch,
                    updated_hash.clone(),
                    1.0, // witness_bilateral_rate: contract fully defaulted
                )?;

                // VOUCH STAKE SLASHING (Whitepaper Definition - Vouch Transaction):
                // When entrant defaults, sponsors lose their staked capacity proportionally.
                // We pass `updated_hash` (the expired contract action) as proof so integrity
                // validation can verify the slash is legitimate.
                let debtor_agent: AgentPubKey = contract.debtor.clone().into();
                let slashed = crate::vouch::slash_vouch_for_entrant(debtor_agent, contract.amount, updated_hash)?;
                total_slashed_dispatched += slashed;
                if slashed > 0.0 {
                    debug!("Slashed {:.2} from vouches for debtor {:?}", slashed, contract.debtor);
                }

                // SUPPORT ESCROW CONTAGION (Whitepaper Section 5.1):
                // Co-signers who benefited from the support cascade share the failure risk.
                if let Some(co_signers) = &contract.co_signers {
                    let total_coef: f64 = co_signers.iter().map(|(_, coef)| *coef).sum();
                    if total_coef > 0.0 {
                        for (cosigner, coef) in co_signers {
                            let penalty = contract.amount * (coef / total_coef);
                            if penalty > DUST_THRESHOLD {
                                if let Some(existing) = creditor_failures.iter_mut().find(|(c, _)| c == cosigner) {
                                    existing.1 += penalty;
                                } else {
                                    creditor_failures.push((cosigner.clone(), penalty));
                                }
                            }
                        }
                    }
                }
            }

            total_expired_debt += expired_contract.amount;
            expired_contract_count += 1;
        }
    }

    // Update running debt balance: remove expired contract debt
    if expired_contract_count > 0 {
        update_debt_balance(agent, -total_expired_debt, -expired_contract_count)?;
        // Publish updated trust row after F accumulation so that other
        // observers see the freshest S/F distribution immediately. Without this,
        // the published trust row (read from DHT by peers) can lag behind the
        // actual failure data by one full reputation-computation cycle.
        let _ = crate::trust::publish_trust_row(());
    }

    Ok(ExpirationResult { creditor_failures, total_expired, total_slashed_dispatched })
}

/// Get the earliest expiration timestamp and total amount due for any active debt
/// where the current agent is the debtor.
/// Returns None if there is no active debt.
#[hdk_extern]
pub fn get_next_debt_expiration(debtor: AgentPubKey) -> ExternResult<Option<NextDeadline>> {
    let records = get_active_contracts_for_debtor(debtor)?;
    let mut earliest_maturity_epoch: Option<u64> = None;
    let mut amount_due: f64 = 0.0;

    for record in records {
        let contract: DebtContract = match record.entry().to_app_option::<DebtContract>().ok().flatten() {
            Some(c) if c.status == ContractStatus::Active && c.amount > DUST_THRESHOLD => c,
            _ => continue,
        };

        let maturity_epoch = contract.start_epoch + contract.maturity;
        if let Some(current_earliest) = earliest_maturity_epoch {
            if maturity_epoch < current_earliest {
                earliest_maturity_epoch = Some(maturity_epoch);
                amount_due = contract.amount;
            } else if maturity_epoch == current_earliest {
                amount_due += contract.amount;
            }
        } else {
            earliest_maturity_epoch = Some(maturity_epoch);
            amount_due = contract.amount;
        }
    }

    match earliest_maturity_epoch {
        Some(epoch) => {
            let timestamp_secs = epoch * EPOCH_DURATION_SECS;
            Ok(Some(NextDeadline {
                timestamp: Timestamp::from_micros(timestamp_secs as i64 * 1_000_000),
                amount: amount_due,
            }))
        }
        None => Ok(None),
    }
}
