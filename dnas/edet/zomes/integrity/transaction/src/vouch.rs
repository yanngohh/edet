use crate::types::constants::{
    vouch_validation_error::*, ACQ_SATURATION, BASE_CAPACITY, CAPACITY_BETA, EIGENTRUST_ALPHA, MAX_ACQUAINTANCES,
    MAX_THEORETICAL_CAPACITY,
};
use crate::LinkTypes;
use hdi::prelude::*;

use crate::debt_contract::{ContractStatus, DebtContract};

/// Integrity-zome capacity ceiling derived from the agent's net acquaintance
/// count. Mirrors `debt_contract::cap_integrity` so that the vouch-creation
/// capacity check uses the same upper-bound formula as debt-contract creation.
///
///   Cap_int(n) = V_base + β × ln(max(1, (1−α)·n/α)) × (1 − e^{−n/n₀})
///
/// At n = 0 returns BASE_CAPACITY (the genesis floor); clamped to
/// MAX_THEORETICAL_CAPACITY as a hard backstop.
fn cap_integrity(n: usize) -> f64 {
    if n == 0 {
        return BASE_CAPACITY;
    }
    let n_f = n as f64;
    let t_max = 1.0 - EIGENTRUST_ALPHA;
    let t_baseline = EIGENTRUST_ALPHA / n_f;
    let rel_rep = t_max / t_baseline;
    let saturation = 1.0 - (-n_f / ACQ_SATURATION).exp();
    let cap = BASE_CAPACITY + CAPACITY_BETA * rel_rep.max(1.0).ln() * saturation;
    cap.min(MAX_THEORETICAL_CAPACITY)
}

/// Scan the sponsor's source chain to compute their total committed exposure
/// for the sponsor-capacity check on vouch creation:
///
///   committed = Σ (active_vouch_amount − slashed_amount   ; agent as sponsor)
///             + Σ (debt_contract.amount                    ; agent as debtor)
///   acquaintances = #(net AgentToAcquaintance CreateLinks)
///
/// Returns `(committed, net_acq)`.
///
/// Walks the chain ONCE and aggregates all three classes simultaneously.
/// This deliberately matches the pattern used in `debt_contract::scan_chain`
/// so the ceiling check is consistent across both validation paths.
fn scan_sponsor_chain(sponsor: &AgentPubKey, chain_top: &ActionHash) -> ExternResult<(f64, usize)> {
    let activity = must_get_agent_activity(sponsor.clone(), ChainFilter::new(chain_top.clone()))?;

    let mut committed: f64 = 0.0;
    let mut acq_creates: std::collections::HashSet<ActionHash> = std::collections::HashSet::new();
    let mut acq_deletes: std::collections::HashSet<ActionHash> = std::collections::HashSet::new();

    // Track latest known state per (vouch entry) to handle update chains.
    // Map from original-action-hash → (amount, slashed_amount, status).
    // Updates supersede creates (we use the most recent action's view).
    let mut vouch_states: std::collections::HashMap<ActionHash, (f64, f64, VouchStatus)> =
        std::collections::HashMap::new();

    for item in &activity {
        let action = item.action.action();
        match action {
            Action::Create(create) => {
                if let Some(entry_hash) = action.entry_hash() {
                    if let Ok(entry) = must_get_entry(entry_hash.clone()) {
                        // Try Vouch (sponsor scenario): only count vouches where
                        // this agent is the sponsor.
                        if let Ok(vouch) = Vouch::try_from(entry.content.clone()) {
                            if vouch.sponsor == *sponsor {
                                vouch_states.insert(
                                    item.action.as_hash().clone(),
                                    (vouch.amount, vouch.slashed_amount, vouch.status.clone()),
                                );
                            }
                            continue;
                        }
                        // Try DebtContract (debtor scenario): committed = sum of amounts.
                        if let Ok(contract) = DebtContract::try_from(entry.content) {
                            if AgentPubKey::from(contract.debtor.clone()) == *sponsor {
                                committed += contract.amount;
                            }
                            continue;
                        }
                    }
                }
                let _ = create;
            }
            Action::Update(update) => {
                // Apply updates to vouch states only (debt-contract updates
                // can only decrease amount; a conservative sum from Creates
                // already over-counts, which is the safe direction).
                let original = update.original_action_address.clone();
                if let Some(entry_hash) = action.entry_hash() {
                    if let Ok(entry) = must_get_entry(entry_hash.clone()) {
                        if let Ok(vouch) = Vouch::try_from(entry.content) {
                            if vouch.sponsor == *sponsor {
                                vouch_states
                                    .insert(original, (vouch.amount, vouch.slashed_amount, vouch.status.clone()));
                            }
                        }
                    }
                }
            }
            Action::CreateLink(create_link) => {
                if let Ok(Some(LinkTypes::AgentToAcquaintance)) =
                    LinkTypes::from_type(create_link.zome_index, create_link.link_type)
                {
                    acq_creates.insert(item.action.as_hash().clone());
                }
            }
            Action::DeleteLink(delete_link) => {
                acq_deletes.insert(delete_link.link_add_address.clone());
            }
            _ => {}
        }
    }

    // Sum only Active vouches (Released and Slashed are no longer committing capacity).
    for (amount, slashed, status) in vouch_states.values() {
        if matches!(status, VouchStatus::Active) {
            let live = (amount - slashed).max(0.0);
            committed += live;
        }
    }

    let net_acq = acq_creates.difference(&acq_deletes).count().min(MAX_ACQUAINTANCES);
    Ok((committed, net_acq))
}

/// Status of a vouch stake.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum VouchStatus {
    /// Stake is active - sponsor's capacity is locked.
    Active,
    /// Stake was slashed due to entrant default. The penalty was transferred
    /// to the creditor(s) who suffered the default.
    Slashed,
    /// Stake was released (e.g., sponsor reclaimed after entrant built reputation).
    Released,
}

#[derive(Clone, PartialEq)]
#[hdk_entry_helper]
pub struct Vouch {
    pub sponsor: AgentPubKey,
    pub entrant: AgentPubKey,
    pub amount: f64,
    /// Status of this vouch stake.
    pub status: VouchStatus,
    /// Total amount slashed from this vouch (if any).
    /// Accumulated over multiple defaults if the vouch isn't fully slashed at once.
    pub slashed_amount: f64,
    /// True when this is a genesis (bootstrap) vouch that skips the sponsor capacity check.
    /// Genesis vouches are only valid during the founding epoch window
    /// (action timestamp epoch <= GENESIS_VOUCH_CUTOFF_EPOCH).
    /// Immutably set at creation time.
    #[serde(default)]
    pub is_genesis: bool,
    /// Proof of the expired contract that justifies a debtor-initiated slash update.
    /// Must be `Some` when the debtor (entrant) increases `slashed_amount`, and must
    /// reference a DebtContract with status Expired or Archived whose `debtor` field
    /// matches the vouch's `entrant`.  Sponsors may leave this `None` for sponsor-side
    /// updates (release, etc.).
    #[serde(default)]
    pub expired_contract_hash: Option<ActionHash>,
}

/// Maximum amount a single vouch can stake. Set to BASE_CAPACITY to prevent
/// a single sponsor from granting outsized capacity to an entrant.
pub const MAX_VOUCH_AMOUNT: f64 = BASE_CAPACITY;

pub fn validate_create_vouch(action: EntryCreationAction, vouch: Vouch) -> ExternResult<ValidateCallbackResult> {
    // Only the sponsor can create a vouch (prevents forging vouches from other agents)
    if action.author() != &AgentPubKey::from(vouch.sponsor.clone()) {
        return Ok(ValidateCallbackResult::Invalid(AUTHOR_NOT_SPONSOR.to_string()));
    }

    // Amount must be a finite positive number.
    // NaN and ±Infinity must be rejected explicitly because Rust float comparisons
    // with NaN always return false: `NaN <= 0.0` is false AND `NaN > MAX_VOUCH_AMOUNT`
    // is false, so without this guard a NaN amount would pass both downstream checks.
    if !vouch.amount.is_finite() {
        return Ok(ValidateCallbackResult::Invalid(AMOUNT_NOT_POSITIVE.to_string()));
    }

    // Amount must be positive
    if vouch.amount <= 0.0 {
        return Ok(ValidateCallbackResult::Invalid(AMOUNT_NOT_POSITIVE.to_string()));
    }

    // Amount must not exceed maximum
    if vouch.amount > MAX_VOUCH_AMOUNT {
        return Ok(ValidateCallbackResult::Invalid(AMOUNT_EXCEEDS_MAXIMUM.to_string()));
    }

    // Sponsor cannot vouch for themselves
    if vouch.sponsor == vouch.entrant {
        return Ok(ValidateCallbackResult::Invalid(SELF_VOUCH_NOT_ALLOWED.to_string()));
    }

    // New vouches must be Active with 0 slashed
    if vouch.status != VouchStatus::Active {
        return Ok(ValidateCallbackResult::Invalid(NEW_VOUCH_STATUS_NOT_ACTIVE.to_string()));
    }
    if vouch.slashed_amount != 0.0 {
        return Ok(ValidateCallbackResult::Invalid(NEW_VOUCH_SLASHED_NOT_ZERO.to_string()));
    }

    // Genesis vouches (is_genesis=true) only exist in test-epoch builds.
    #[cfg(feature = "test-epoch")]
    if vouch.is_genesis {
        use crate::types::constants::GENESIS_VOUCH_CUTOFF_EPOCH;
        use crate::types::timestamp_to_epoch;
        let action_epoch = timestamp_to_epoch(action.timestamp().to_owned());
        #[allow(clippy::absurd_extreme_comparisons)]
        if action_epoch > GENESIS_VOUCH_CUTOFF_EPOCH {
            return Ok(ValidateCallbackResult::Invalid(GENESIS_VOUCH_AFTER_CUTOFF.to_string()));
        }
    }
    // In production builds (without test-epoch feature), unconditionally
    // reject any vouch with is_genesis=true. The genesis_vouch coordinator function
    // is #[cfg(feature="test-epoch")] and cannot be called in production, but a
    // node from a test build could theoretically propagate a genesis vouch to the
    // production DHT. This guard ensures production validators always reject them.
    #[cfg(not(feature = "test-epoch"))]
    if vouch.is_genesis {
        return Ok(ValidateCallbackResult::Invalid(
            crate::types::constants::vouch_validation_error::AUTHOR_NOT_SPONSOR.to_string(),
        ));
    }

    // Sponsor capacity check. Whitepaper Theorem 2.2 requires every vouch
    // be backed by real staked capacity — without this check, a Sybil with no
    // history can issue arbitrary vouches and grant unbacked capacity to
    // entrants, defeating the protocol's Sybil resistance.
    //
    // The integrity zome cannot run EigenTrust, so it uses the same dynamic
    // ceiling formula as `debt_contract::cap_integrity` (substituting
    // t = 1 − α, the Perron-Frobenius upper bound on any agent's score).
    //
    // Genesis vouches are exempted: they exist only in test-epoch builds and
    // are explicitly the founder bootstrap mechanism (no prior capacity).
    if !vouch.is_genesis {
        if let EntryCreationAction::Create(_) = action {
            let (committed, net_acq) = scan_sponsor_chain(action.author(), action.prev_action())?;
            let ceiling = cap_integrity(net_acq);
            if committed + vouch.amount > ceiling {
                error!(
                    "Validation failed: sponsor {} capacity exceeded (committed={}, new_vouch={}, ceiling={})",
                    action.author(),
                    committed,
                    vouch.amount,
                    ceiling
                );
                return Ok(ValidateCallbackResult::Invalid(SPONSOR_CAPACITY_INSUFFICIENT.to_string()));
            }
        }
    }

    Ok(ValidateCallbackResult::Valid)
}

pub fn validate_update_vouch(
    action: Update,
    vouch: Vouch,
    _original_action: EntryCreationAction,
    original_vouch: Vouch,
) -> ExternResult<ValidateCallbackResult> {
    // Either the sponsor OR the debtor (entrant) can update a vouch.
    // The debtor is allowed to perform slash updates (i.e., increase slashed_amount
    // or mark status as Slashed) because process_contract_expirations runs on the
    // debtor's cell and must be able to slash vouches without a call_remote to the sponsor.
    let is_sponsor_update = action.author == original_vouch.sponsor;
    let is_debtor_slash =
        action.author == original_vouch.entrant && vouch.slashed_amount > original_vouch.slashed_amount;

    if !is_sponsor_update && !is_debtor_slash {
        return Ok(ValidateCallbackResult::Invalid(UPDATE_AUTHOR_NOT_SPONSOR.to_string()));
    }

    // Debtor-initiated slash: require proof of an expired/archived contract.
    // This prevents the entrant from slashing their own sponsors without cause.
    if is_debtor_slash {
        match &vouch.expired_contract_hash {
            None => {
                return Ok(ValidateCallbackResult::Invalid(SLASH_MISSING_PROOF.to_string()));
            }
            Some(contract_hash) => {
                // Retrieve and validate the referenced contract.
                let contract_record = match must_get_valid_record(contract_hash.clone()) {
                    Ok(r) => r,
                    Err(_) => {
                        return Ok(ValidateCallbackResult::Invalid(SLASH_PROOF_CONTRACT_NOT_FOUND.to_string()));
                    }
                };
                let contract: DebtContract =
                    match contract_record.entry().to_app_option().map_err(|e| wasm_error!(e))? {
                        Some(c) => c,
                        None => {
                            return Ok(ValidateCallbackResult::Invalid(SLASH_PROOF_NOT_CONTRACT.to_string()));
                        }
                    };
                // Contract must be expired or archived
                if contract.status != ContractStatus::Expired && contract.status != ContractStatus::Archived {
                    return Ok(ValidateCallbackResult::Invalid(SLASH_PROOF_CONTRACT_NOT_EXPIRED.to_string()));
                }
                // Debtor on the contract must match the vouch's entrant
                let debtor_key: AgentPubKey = contract.debtor.clone().into();
                if debtor_key != original_vouch.entrant {
                    return Ok(ValidateCallbackResult::Invalid(SLASH_PROOF_DEBTOR_MISMATCH.to_string()));
                }
            }
        }
    }

    // Immutable fields
    if vouch.sponsor != original_vouch.sponsor {
        return Ok(ValidateCallbackResult::Invalid(SPONSOR_CHANGED.to_string()));
    }
    if vouch.entrant != original_vouch.entrant {
        return Ok(ValidateCallbackResult::Invalid(ENTRANT_CHANGED.to_string()));
    }
    if vouch.amount != original_vouch.amount {
        return Ok(ValidateCallbackResult::Invalid(AMOUNT_CHANGED.to_string()));
    }
    if vouch.is_genesis != original_vouch.is_genesis {
        return Ok(ValidateCallbackResult::Invalid(IS_GENESIS_CHANGED.to_string()));
    }

    // Valid status transitions:
    // Active -> Slashed (on entrant default)
    // Active -> Released (sponsor reclaims after reputation built)
    // Slashed -> Slashed (partial slash, increase slashed_amount)
    match (&original_vouch.status, &vouch.status) {
        (VouchStatus::Active, VouchStatus::Active) => {
            // Active→Active: the only legitimate reason to stay Active while updating
            // is a partial slash that does not yet reach the threshold to flip to Slashed.
            // Any such update must still increase slashed_amount (never decrease it) and
            // must not exceed the original staked amount.
            if vouch.slashed_amount < original_vouch.slashed_amount {
                return Ok(ValidateCallbackResult::Invalid(SLASH_CANNOT_DECREASE.to_string()));
            }
            if vouch.slashed_amount > vouch.amount {
                return Ok(ValidateCallbackResult::Invalid(SLASH_EXCEEDS_AMOUNT.to_string()));
            }
        }
        (VouchStatus::Active, VouchStatus::Slashed) => {
            // Slashing must increase slashed_amount
            if vouch.slashed_amount <= original_vouch.slashed_amount {
                return Ok(ValidateCallbackResult::Invalid(SLASH_MUST_INCREASE.to_string()));
            }
            // Slashed amount cannot exceed original amount
            if vouch.slashed_amount > vouch.amount {
                return Ok(ValidateCallbackResult::Invalid(SLASH_EXCEEDS_AMOUNT.to_string()));
            }
        }
        (VouchStatus::Active, VouchStatus::Released) => {
            // Release is allowed (sponsor reclaims capacity after entrant has built reputation).
        }
        (VouchStatus::Slashed, VouchStatus::Slashed) => {
            // Additional slashing (should increase slashed_amount)
            if vouch.slashed_amount < original_vouch.slashed_amount {
                return Ok(ValidateCallbackResult::Invalid(SLASH_CANNOT_DECREASE.to_string()));
            }
        }
        _ => {
            // NOTE: Released→Slashed is intentionally omitted here and falls to Invalid.
            // The coordinator's `release_vouch` function prevents releasing a vouch while
            // the entrant still has active contracts, so the scenario "release then get
            // slashed for a pre-release contract" cannot arise in normal protocol operation.
            // Any attempt to perform Released→Slashed directly (bypassing the coordinator)
            // is treated as an invalid transition.
            return Ok(ValidateCallbackResult::Invalid(INVALID_STATUS_TRANSITION.to_string()));
        }
    }

    Ok(ValidateCallbackResult::Valid)
}

pub fn validate_delete_vouch(
    _action: Delete,
    _original_action: EntryCreationAction,
    _original_vouch: Vouch,
) -> ExternResult<ValidateCallbackResult> {
    // Vouches cannot be deleted - they can only be Released or Slashed
    Ok(ValidateCallbackResult::Invalid(VOUCH_NOT_DELETABLE.to_string()))
}
