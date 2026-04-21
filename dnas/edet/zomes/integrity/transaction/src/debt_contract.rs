use hdi::prelude::*;

use crate::types::constants::{
    debt_contract_validation_error, ACQ_SATURATION, BASE_CAPACITY, CAPACITY_BETA, DUST_THRESHOLD, EIGENTRUST_ALPHA,
    MAX_ACQUAINTANCES, MAX_THEORETICAL_CAPACITY, MIN_MATURITY, TRIAL_FRACTION,
};
use crate::types::{timestamp_near_epoch_boundary, timestamp_to_epoch};
use crate::LinkTypes;

/// Status of a debt contract in its lifecycle.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum ContractStatus {
    /// Contract is active, debt has not yet been fully transferred or expired.
    Active,
    /// Debt has been fully transferred (amount reached 0 through selling).
    Transferred,
    /// Contract maturity elapsed with remaining debt (failure event).
    Expired,
    /// Contract has been archived for scalability. Archived contracts are
    /// excluded from active scans but their S/F contributions are preserved.
    /// Contracts can only be archived after ARCHIVE_AFTER_EPOCHS have passed
    /// since they were Transferred or Expired.
    Archived,
}

/// Compute the integrity-layer capacity ceiling for an agent given the number of
/// acquaintances observed on their source chain.
///
/// Uses the same formula as the coordinator (capacity.rs) but substituted with
/// t = 1 - α (the Perron-Frobenius upper bound on any single agent's EigenTrust
/// score) to produce the tightest *provable* ceiling without running EigenTrust:
///
///   Cap_integrity(n) = V_base + β × ln((1−α) / (α/n)) × (1 − e^{−n/n₀})
///
/// This is strictly tighter than the static MAX_THEORETICAL_CAPACITY (which used
/// n = MAX_ACQUAINTANCES unconditionally) for every agent with fewer than
/// MAX_ACQUAINTANCES acquaintances — which is the common case.
///
/// When n = 0 (no acquaintances yet), returns BASE_CAPACITY (vouched/trial floor).
/// The result is clamped to MAX_THEORETICAL_CAPACITY as a hard compile-time backstop.
fn cap_integrity(n: usize) -> f64 {
    if n == 0 {
        return BASE_CAPACITY;
    }
    let n_f = n as f64;
    let t_max = 1.0 - EIGENTRUST_ALPHA; // Perron-Frobenius upper bound = 0.92
    let t_baseline = EIGENTRUST_ALPHA / n_f;
    let rel_rep = t_max / t_baseline; // = n * (1-α) / α
    let saturation = 1.0 - (-n_f / ACQ_SATURATION).exp();
    let cap = BASE_CAPACITY + CAPACITY_BETA * rel_rep.max(1.0).ln() * saturation;
    cap.min(MAX_THEORETICAL_CAPACITY)
}

/// Single-pass scan of the debtor's source chain that simultaneously:
///   1. Sums all DebtContract Create amounts (conservative total debt).
///   2. Counts net AgentToAcquaintance links (CreateLink minus DeleteLink).
///
/// Both pieces of data are needed for the capacity ceiling check and are
/// available in the same `must_get_agent_activity` call, so we combine them
/// into one scan to avoid walking the chain twice.
///
/// Returns `(total_debt, net_acquaintances)`.
fn scan_chain(author: &AgentPubKey, chain_top: &ActionHash, entry_type: &EntryType) -> ExternResult<(f64, usize)> {
    let activity = must_get_agent_activity(author.clone(), ChainFilter::new(chain_top.clone()))?;

    let mut total_debt = 0.0f64;

    // Track acquaintance link creates and deletes in a single pass.
    // We collect the ActionHash of every AgentToAcquaintance CreateLink, then
    // subtract any that have been deleted (via DeleteLink.link_add_address).
    // Using ActionHash as key is correct: each CreateLink has a unique hash, and
    // DeleteLink.link_add_address references the exact hash it reverses.
    let mut acq_creates: std::collections::HashSet<ActionHash> = std::collections::HashSet::new();
    let mut acq_deletes: std::collections::HashSet<ActionHash> = std::collections::HashSet::new();

    for item in &activity {
        let action = item.action.action();

        match action {
            Action::Create(create) => {
                // Only count DebtContract Create entries for the debt total.
                if create.entry_type == *entry_type {
                    if let Some(entry_hash) = action.entry_hash() {
                        if let Ok(entry) = must_get_entry(entry_hash.clone()) {
                            if let Ok(contract) = DebtContract::try_from(entry.content) {
                                total_debt += contract.amount;
                            }
                        }
                    }
                }
            }
            Action::CreateLink(create_link) => {
                // Identify AgentToAcquaintance links by resolving the raw
                // (zome_index, link_type) pair to the typed LinkTypes variant.
                // from_type returns None for foreign-zome links; we skip those.
                if let Ok(Some(LinkTypes::AgentToAcquaintance)) =
                    LinkTypes::from_type(create_link.zome_index, create_link.link_type)
                {
                    acq_creates.insert(item.action.as_hash().clone());
                }
            }
            Action::DeleteLink(delete_link) => {
                // Record the hash of the CreateLink being reversed.
                // We don't need to check the link type here: we only care about
                // deletes that reference a hash we already have in acq_creates.
                acq_deletes.insert(delete_link.link_add_address.clone());
            }
            _ => {}
        }
    }

    // Net acquaintances = creates that have not been deleted.
    // Clamped to MAX_ACQUAINTANCES: more links than the cap is either a
    // validator-side anomaly or a modified conductor; the cap is the protocol limit.
    let net_acq = acq_creates.difference(&acq_deletes).count().min(MAX_ACQUAINTANCES);

    Ok((total_debt, net_acq))
}

/// A debt contract (delta, M, t0, creditor) per Definition 1 of the whitepaper.
///
/// Created when a transaction is accepted. The debtor must transfer the debt
/// by selling goods/services before maturity expires. Successful transfer
/// increments S_ij; expiration increments F_ij.
#[derive(Clone, PartialEq)]
#[hdk_entry_helper]
pub struct DebtContract {
    /// Principal debt amount. Decreases as debt is transferred (debtor sells).
    pub amount: f64,
    /// Original principal at contract creation. Immutable; used to compute
    /// debt-velocity factor (D_in / D_out) in the risk score (Whitepaper Def 16).
    pub original_amount: f64,
    /// Maturity in epochs. Contract expires at start_epoch + maturity.
    pub maturity: u64,
    /// Epoch when the contract was created. E = floor(unix_secs / 86400).
    pub start_epoch: u64,
    /// The creditor (seller who originated the debt).
    pub creditor: AgentPubKeyB64,
    /// The debtor (buyer who contracted the debt).
    pub debtor: AgentPubKeyB64,
    /// Reference to the originating transaction.
    pub transaction_hash: ActionHash,
    /// Co-signers (beneficiaries drained via support cascade) and their exposed amounts.
    pub co_signers: Option<Vec<(AgentPubKeyB64, f64)>>,
    /// Current lifecycle status.
    pub status: ContractStatus,
    /// Whether the originating transaction was a trial (bootstrap) transaction.
    pub is_trial: bool,
}

pub fn validate_create_debt_contract(
    action: EntryCreationAction,
    contract: DebtContract,
) -> ExternResult<ValidateCallbackResult> {
    // Only debtor can create contracts on their source chain
    if action.author() != &AgentPubKey::from(contract.debtor.clone()) {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::AUTHOR_NOT_DEBTOR.to_string()));
    }

    // Amount must be a finite positive number.
    // NaN and ±Infinity are rejected explicitly because `NaN <= 0.0` is false
    // in Rust, so a NaN amount would otherwise pass the guard below.
    if !contract.amount.is_finite() {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::AMOUNT_NOT_POSITIVE.to_string()));
    }

    // Amount must be positive
    if contract.amount <= 0.0 {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::AMOUNT_NOT_POSITIVE.to_string()));
    }

    // original_amount must equal amount at creation (it's the immutable reference principal)
    if (contract.original_amount - contract.amount).abs() > f64::EPSILON * contract.amount.max(1.0) {
        return Ok(ValidateCallbackResult::Invalid(
            debt_contract_validation_error::ORIGINAL_AMOUNT_MISMATCH_ON_CREATE.to_string(),
        ));
    }

    // Maturity must meet minimum
    if contract.maturity < MIN_MATURITY {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::MATURITY_TOO_LOW.to_string()));
    }

    // Creditor and debtor must be different agents.
    if contract.creditor == contract.debtor {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::CREDITOR_IS_DEBTOR.to_string()));
    }

    // New contracts must be Active
    if contract.status != ContractStatus::Active {
        return Ok(ValidateCallbackResult::Invalid(
            debt_contract_validation_error::INVALID_STATUS_TRANSITION.to_string(),
        ));
    }

    // Trial debt contracts must have amount strictly below η · V_base
    // (Whitepaper §2.3, Theorem `thm:bootstrap`). Enforced at the integrity
    // layer so that a modified conductor cannot publish an `is_trial=true`
    // contract with an arbitrarily large amount, which would bypass the
    // trial-slot extraction bound.
    let trial_amount_cap = TRIAL_FRACTION * BASE_CAPACITY;
    if contract.is_trial && contract.amount >= trial_amount_cap {
        return Ok(ValidateCallbackResult::Invalid(
            debt_contract_validation_error::TRIAL_AMOUNT_EXCEEDS_CAP.to_string(),
        ));
    }

    // Capacity upper-bound check (Whitepaper Section 5.1, Step 2).
    //
    // The integrity zome cannot run EigenTrust, so it uses a dynamic ceiling
    // derived from the agent's net acquaintance count on their source chain:
    //
    //   Cap_integrity(n) = V_base + β × ln((1−α)·n / α) × (1 − e^{−n/n₀})
    //
    // This substitutes t = 1−α (the Perron-Frobenius upper bound on any single
    // agent's EigenTrust score) into the coordinator's capacity formula.  The
    // result is always ≥ the real coordinator capacity for the same n, so honest
    // agents are never falsely rejected.  The ceiling tightens automatically as
    // agents with few acquaintances try to over-leverage.
    //
    // Acquaintances are counted from CreateLink / DeleteLink actions on the
    // source chain — no extra DHT fetches, same single scan as the debt total.
    //
    // Gameability note: a modified conductor can write fake AgentToAcquaintance
    // links, but doing so only raises the ceiling (not the debt), and the
    // ceiling is clamped to MAX_THEORETICAL_CAPACITY regardless.  An attacker
    // with a modified conductor is still bounded by that hard cap.
    if action.action_type() == ActionType::Create {
        let (existing_debt, net_acq) = scan_chain(action.author(), action.prev_action(), action.entry_type())?;
        let ceiling = cap_integrity(net_acq);
        if existing_debt + contract.amount > ceiling {
            return Ok(ValidateCallbackResult::Invalid(
                debt_contract_validation_error::DEBT_EXCEEDS_CAPACITY.to_string(),
            ));
        }
    }

    Ok(ValidateCallbackResult::Valid)
}

pub fn validate_update_debt_contract(
    action: Update,
    contract: DebtContract,
    _original_action: EntryCreationAction,
    original_contract: DebtContract,
) -> ExternResult<ValidateCallbackResult> {
    let is_debtor_update = action.author == AgentPubKey::from(contract.debtor.clone());
    let is_creditor_archival = {
        // The creditor may perform Transferred→Archived or Expired→Archived transitions.
        // This unblocks the cross-agent archival deadlock: if the debtor goes offline
        // permanently the creditor would otherwise be unable to clean up their
        // CreditorToContracts link (which requires the contract to be Archived).
        let is_creditor = action.author == AgentPubKey::from(contract.creditor.clone());
        let is_terminal_to_archived = matches!(
            (&original_contract.status, &contract.status),
            (ContractStatus::Transferred, ContractStatus::Archived)
                | (ContractStatus::Expired, ContractStatus::Archived)
        );
        is_creditor && is_terminal_to_archived
    };

    if !is_debtor_update && !is_creditor_archival {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::AUTHOR_NOT_DEBTOR.to_string()));
    }

    // Immutable fields: creditor, debtor, maturity, start_epoch, transaction_hash, co_signers, is_trial, original_amount
    if contract.creditor != original_contract.creditor {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::CREDITOR_CHANGED.to_string()));
    }
    if contract.debtor != original_contract.debtor {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::DEBTOR_CHANGED.to_string()));
    }
    if contract.maturity != original_contract.maturity {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::MATURITY_CHANGED.to_string()));
    }
    if contract.start_epoch != original_contract.start_epoch {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::START_EPOCH_CHANGED.to_string()));
    }
    if contract.transaction_hash != original_contract.transaction_hash {
        return Ok(ValidateCallbackResult::Invalid(
            debt_contract_validation_error::TRANSACTION_HASH_CHANGED.to_string(),
        ));
    }
    if contract.co_signers != original_contract.co_signers {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::CO_SIGNERS_CHANGED.to_string()));
    }
    if contract.is_trial != original_contract.is_trial {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::IS_TRIAL_CHANGED.to_string()));
    }
    // original_amount is immutable — it records the principal at creation for debt-velocity calculations
    if (contract.original_amount - original_contract.original_amount).abs()
        > f64::EPSILON * original_contract.original_amount.max(1.0)
    {
        return Ok(ValidateCallbackResult::Invalid(
            debt_contract_validation_error::ORIGINAL_AMOUNT_CHANGED.to_string(),
        ));
    }

    // Amount can only decrease (debt transfer reduces it)
    if contract.amount > original_contract.amount {
        return Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::AMOUNT_INCREASED.to_string()));
    }

    // Valid status transitions:
    // - Active -> Active (partial transfer)
    // - Active -> Transferred (fully transferred)
    // - Active -> Expired (maturity elapsed)
    // - Transferred -> Archived (for scalability)
    // - Expired -> Archived (for scalability)
    match (&original_contract.status, &contract.status) {
        (ContractStatus::Active, ContractStatus::Active) => {}
        (ContractStatus::Active, ContractStatus::Transferred) => {
            // Amount should be ~0 when marking as transferred
            if contract.amount > DUST_THRESHOLD {
                return Ok(ValidateCallbackResult::Invalid(
                    debt_contract_validation_error::INVALID_STATUS_TRANSITION.to_string(),
                ));
            }
        }
        (ContractStatus::Active, ContractStatus::Expired) => {
            // Verify that the contract has actually reached maturity.
            // The action's timestamp is used as the reference point (available in integrity zome).
            // This prevents premature expiration attacks where a debtor marks contracts
            // as expired before maturity to avoid legitimate debt transfer obligations.
            //
            // BOUNDARY-WINDOW HANDLING (Whitepaper Property: Epoch Unambiguity):
            // When the action timestamp is within CLOCK_DRIFT_MAX_SECS of an epoch boundary,
            // the author's epoch assignment could legitimately be either E or E+1 due to clock
            // drift. In this case, we also accept expiration in the preceding epoch (i.e.,
            // action_epoch >= maturity_epoch - 1 when near a boundary). This prevents spurious
            // PREMATURE_EXPIRATION rejections for contracts that expired right at midnight.
            let action_epoch = timestamp_to_epoch(action.timestamp);
            let near_boundary = timestamp_near_epoch_boundary(action.timestamp);
            // Use checked_add to prevent silent u64 wrap-around if a
            // malicious debtor sets start_epoch = u64::MAX - 5 and maturity = 30.
            // Wrapping would produce a very small maturity_epoch, making
            // `action_epoch >= maturity_epoch` trivially true and allowing premature expiry.
            let maturity_epoch = match original_contract.start_epoch.checked_add(original_contract.maturity) {
                Some(e) => e,
                None => {
                    return Ok(ValidateCallbackResult::Invalid(
                        debt_contract_validation_error::PREMATURE_EXPIRATION.to_string(),
                    ));
                }
            };

            let epoch_ok = action_epoch >= maturity_epoch || (near_boundary && action_epoch + 1 >= maturity_epoch);
            if !epoch_ok {
                return Ok(ValidateCallbackResult::Invalid(
                    debt_contract_validation_error::PREMATURE_EXPIRATION.to_string(),
                ));
            }
            // Expiration is valid regardless of remaining amount
        }
        (ContractStatus::Transferred, ContractStatus::Archived) => {
            // Archival of transferred contracts is allowed
            // Note: ARCHIVE_AFTER_EPOCHS check is done in coordinator (needs sys_time)
        }
        (ContractStatus::Expired, ContractStatus::Archived) => {
            // Archival of expired contracts is allowed
        }
        _ => {
            return Ok(ValidateCallbackResult::Invalid(
                debt_contract_validation_error::INVALID_STATUS_TRANSITION.to_string(),
            ));
        }
    }

    Ok(ValidateCallbackResult::Valid)
}

pub fn validate_delete_debt_contract(
    _action: Delete,
    _original_action: EntryCreationAction,
    _original_contract: DebtContract,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(debt_contract_validation_error::CONTRACT_NOT_DELETABLE.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contract_status_transitions() {
        // Test valid transitions
        let valid_transitions = [
            (ContractStatus::Active, ContractStatus::Active),
            (ContractStatus::Active, ContractStatus::Transferred),
            (ContractStatus::Active, ContractStatus::Expired),
            (ContractStatus::Transferred, ContractStatus::Archived),
            (ContractStatus::Expired, ContractStatus::Archived),
        ];

        for (from, to) in &valid_transitions {
            assert!(is_valid_status_transition(from, to), "Transition from {from:?} to {to:?} should be valid");
        }

        // Test invalid transitions
        let invalid_transitions = [
            (ContractStatus::Transferred, ContractStatus::Active),
            (ContractStatus::Transferred, ContractStatus::Expired),
            (ContractStatus::Expired, ContractStatus::Active),
            (ContractStatus::Expired, ContractStatus::Transferred),
            (ContractStatus::Archived, ContractStatus::Active),
            (ContractStatus::Archived, ContractStatus::Transferred),
            (ContractStatus::Archived, ContractStatus::Expired),
            (ContractStatus::Active, ContractStatus::Archived),
        ];

        for (from, to) in &invalid_transitions {
            assert!(!is_valid_status_transition(from, to), "Transition from {from:?} to {to:?} should be invalid");
        }
    }

    fn is_valid_status_transition(from: &ContractStatus, to: &ContractStatus) -> bool {
        matches!(
            (from, to),
            (ContractStatus::Active, ContractStatus::Active)
                | (ContractStatus::Active, ContractStatus::Transferred)
                | (ContractStatus::Active, ContractStatus::Expired)
                | (ContractStatus::Transferred, ContractStatus::Archived)
                | (ContractStatus::Expired, ContractStatus::Archived)
        )
    }

    #[test]
    fn test_contract_status_equality() {
        assert_eq!(ContractStatus::Active, ContractStatus::Active);
        assert_eq!(ContractStatus::Transferred, ContractStatus::Transferred);
        assert_eq!(ContractStatus::Expired, ContractStatus::Expired);
        assert_eq!(ContractStatus::Archived, ContractStatus::Archived);

        assert_ne!(ContractStatus::Active, ContractStatus::Transferred);
        assert_ne!(ContractStatus::Active, ContractStatus::Expired);
        assert_ne!(ContractStatus::Active, ContractStatus::Archived);
    }

    #[test]
    fn test_contract_status_clone() {
        let status = ContractStatus::Active;
        let cloned = status.clone();
        assert_eq!(status, cloned);

        let status = ContractStatus::Archived;
        let cloned = status.clone();
        assert_eq!(status, cloned);
    }
}
