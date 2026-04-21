//! Source Chain Checkpoint Coordinator Functions
//!
//! Implements automatic checkpoint creation and retrieval for scalability.
//! Checkpoints enable validators to skip historical validation, reducing
//! verification overhead from O(chain_length) to O(checkpoint_interval).

use hdk::prelude::*;
use transaction_integrity::checkpoint::{
    compute_checkpoint_evidence_hash, ChainCheckpoint, ContractSummary, TrustSummary,
};
use transaction_integrity::debt_contract::{ContractStatus, DebtContract};
use transaction_integrity::types::constants::*;
use transaction_integrity::types::timestamp_to_epoch;
use transaction_integrity::*;

use crate::capacity::compute_credit_capacity;
use crate::contracts;
use crate::trust::{self, get_acquaintances};
use crate::vouch;

// =========================================================================
//  Checkpoint Link Tag
// =========================================================================

/// Tag for AgentToCheckpoint links containing epoch and sequence for lookup.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone)]
pub struct CheckpointLinkTag {
    pub epoch: u64,
    pub sequence: u64,
}

// =========================================================================
//  Checkpoint Creation
// =========================================================================

/// Check if a checkpoint should be created based on configured intervals.
/// Returns (should_create, entries_since_last, epochs_since_last).
fn should_create_checkpoint(agent: AgentPubKey) -> ExternResult<(bool, u64, u64)> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    // Get the latest checkpoint
    let latest = get_latest_checkpoint(agent)?;

    match latest {
        None => {
            // No checkpoint yet - check if we have enough chain entries
            let chain_length = get_chain_length()?;
            Ok((chain_length >= CHECKPOINT_INTERVAL_ENTRIES, chain_length, 0))
        }
        Some((_, checkpoint)) => {
            let epochs_since = current_epoch.saturating_sub(checkpoint.epoch);
            let chain_length = get_chain_length()?;
            let entries_since = chain_length.saturating_sub(checkpoint.chain_length);

            let should_create =
                epochs_since >= CHECKPOINT_INTERVAL_EPOCHS || entries_since >= CHECKPOINT_INTERVAL_ENTRIES;

            Ok((should_create, entries_since, epochs_since))
        }
    }
}

/// Get the current source chain length.
///
/// NOTE: The HDK does not expose a count-only query, so this loads all action
/// hashes from the local source chain to count them. For agents with long chains
/// this is O(n) in memory. A future HDK version may expose a cheaper count API.
fn get_chain_length() -> ExternResult<u64> {
    let filter = ChainQueryFilter::new();
    let actions = query(filter)?;
    Ok(actions.len() as u64)
}

/// Get the hash of the last action on the source chain.
fn get_last_action_hash() -> ExternResult<ActionHash> {
    // Query in descending order and take just one
    let filter = ChainQueryFilter::new();
    let actions = query(filter)?;

    actions
        .last()
        .map(|r| r.action_address().clone())
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_checkpoint_error::CHAIN_EMPTY.to_string())))
}

/// Create a new checkpoint for the current agent.
/// This is called automatically when checkpoint thresholds are reached.
#[hdk_extern]
pub fn create_checkpoint(_: ()) -> ExternResult<Option<Record>> {
    let agent = agent_info()?.agent_initial_pubkey;
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    // Check if we should create a checkpoint
    let (should_create, _, _) = should_create_checkpoint(agent.clone())?;
    if !should_create {
        return Ok(None);
    }

    // Get previous checkpoint info (hash + sequence + evidence_hash for chaining).
    let prev_checkpoint = get_latest_checkpoint(agent.clone())?;
    let (prev_hash, prev_sequence, prev_evidence_hash) = match prev_checkpoint {
        Some((hash, cp)) => (Some(hash.into()), cp.sequence, Some(cp.evidence_hash)),
        None => (None, 0, None),
    };

    // Compute contract summary
    let contract_summary = compute_contract_summary(agent.clone())?;

    // Compute trust summary
    let trust_summary = compute_trust_summary(agent.clone())?;

    // Get chain info
    let chain_length = get_chain_length()?;
    let last_action_hash = get_last_action_hash()?;

    // Build the checkpoint with a sentinel evidence_hash, compute the real
    // hash over the complete struct, then set the final evidence_hash.
    // The helper does not read `checkpoint.evidence_hash`, so the sentinel
    // value does not affect the computed hash. We use the genesis zero-hash
    // as the sentinel for clarity.
    let sentinel_hash = {
        use hdi::prelude::holo_hash::hash_type;
        HoloHash::from_raw_36_and_type(vec![0u8; 36], hash_type::External)
    };
    let mut checkpoint = ChainCheckpoint {
        agent: agent.clone().into(),
        epoch: current_epoch,
        sequence: prev_sequence + 1,
        contract_summary,
        trust_summary,
        last_action_hash: last_action_hash.into(),
        chain_length,
        prev_checkpoint_hash: prev_hash,
        evidence_hash: sentinel_hash,
    };
    checkpoint.evidence_hash = compute_checkpoint_evidence_hash(prev_evidence_hash.as_ref(), &checkpoint);

    // Create the entry
    let checkpoint_hash = create_entry(&EntryTypes::ChainCheckpoint(checkpoint.clone()))?;

    // Create link for discovery
    let tag = CheckpointLinkTag { epoch: current_epoch, sequence: checkpoint.sequence };
    let tag_bytes = SerializedBytes::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    create_link(agent, checkpoint_hash.clone(), LinkTypes::AgentToCheckpoint, LinkTag(tag_bytes.bytes().clone()))?;

    get(checkpoint_hash, GetOptions::default())
}

/// Compute contract summary for checkpoint.
///
/// If a previous checkpoint exists, we use the running debt balance (O(1))
/// for current_debt and only need to count contract statuses.
fn compute_contract_summary(agent: AgentPubKey) -> ExternResult<ContractSummary> {
    // Use the O(1) running debt balance for current_debt
    let current_debt = contracts::get_total_debt(agent.clone())?;

    // Count contracts by status. We still need to scan to get counts,
    // but this is cheaper than deserializing every contract.
    let all_contracts = contracts::get_all_contracts_as_debtor(agent)?;

    let mut total_created = 0u64;
    let mut total_transferred = 0u64;
    let mut total_expired = 0u64;
    let mut total_archived = 0u64;

    for record in all_contracts {
        if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
            total_created += 1;

            match contract.status {
                ContractStatus::Active => {}
                ContractStatus::Transferred => {
                    total_transferred += 1;
                }
                ContractStatus::Expired => {
                    total_expired += 1;
                }
                ContractStatus::Archived => {
                    total_archived += 1;
                }
            }
        }
    }

    Ok(ContractSummary { total_created, total_transferred, total_expired, total_archived, current_debt })
}

/// Compute trust summary for checkpoint.
fn compute_trust_summary(agent: AgentPubKey) -> ExternResult<TrustSummary> {
    // Get acquaintances
    let acquaintances = get_acquaintances(())?;
    let acquaintance_count = acquaintances.len() as u64;

    // Compute actual S/F totals from contract history
    let sf_counters = trust::compute_sf_counters(agent.clone())?;
    let mut total_satisfaction = 0.0f64;
    let mut total_failure = 0.0f64;

    for counters in sf_counters.values() {
        total_satisfaction += counters.satisfaction;
        total_failure += counters.failure;
    }

    // Get reputation for self (requires full computation)
    let rep = trust::get_subjective_reputation(agent)?;
    let last_trust_score = rep.trust;
    let mut visited = Vec::new();
    let vouched = vouch::query_vouched_capacity(agent_info()?.agent_initial_pubkey, &mut visited)?;
    let last_capacity = compute_credit_capacity(rep.trust, rep.acquaintance_count, vouched);

    Ok(TrustSummary { acquaintance_count, total_satisfaction, total_failure, last_trust_score, last_capacity })
}

// =========================================================================
//  Checkpoint Retrieval
// =========================================================================

/// Get the latest checkpoint for an agent.
///
/// Uses the sequence number in the link tag to find the most recent checkpoint
/// without needing to fetch and deserialize every checkpoint record.
#[hdk_extern]
pub fn get_latest_checkpoint(agent: AgentPubKey) -> ExternResult<Option<(ActionHash, ChainCheckpoint)>> {
    let links = get_links(LinkQuery::try_new(agent, LinkTypes::AgentToCheckpoint)?, GetStrategy::default())?;

    // Find the link with the highest sequence number using only the tag
    let mut best_link: Option<(u64, AnyLinkableHash)> = None;

    for link in &links {
        let sequence = if let Ok(tag) =
            CheckpointLinkTag::try_from(SerializedBytes::from(UnsafeBytes::from(link.tag.clone().into_inner())))
        {
            tag.sequence
        } else {
            0
        };

        if best_link.as_ref().is_none_or(|(seq, _)| sequence > *seq) {
            best_link = Some((sequence, link.target.clone()));
        }
    }

    // Only fetch the single best record
    if let Some((_, target)) = best_link {
        if let Some(action_hash) = target.into_action_hash() {
            if let Some(record) = get(action_hash.clone(), GetOptions::default())? {
                if let Some(checkpoint) = record.entry().to_app_option::<ChainCheckpoint>().ok().flatten() {
                    return Ok(Some((action_hash, checkpoint)));
                }
            }
        }
    }

    Ok(None)
}

/// Get a checkpoint by sequence number.
#[hdk_extern]
pub fn get_checkpoint_by_sequence(input: GetCheckpointBySequenceInput) -> ExternResult<Option<ChainCheckpoint>> {
    let links = get_links(LinkQuery::try_new(input.agent, LinkTypes::AgentToCheckpoint)?, GetStrategy::default())?;

    for link in links {
        // Parse the tag to get sequence number
        if let Ok(tag) = CheckpointLinkTag::try_from(SerializedBytes::from(UnsafeBytes::from(link.tag.into_inner()))) {
            if tag.sequence == input.sequence {
                if let Some(action_hash) = link.target.into_action_hash() {
                    if let Some(record) = get(action_hash, GetOptions::default())? {
                        return Ok(record.entry().to_app_option::<ChainCheckpoint>().ok().flatten());
                    }
                }
            }
        }
    }

    Ok(None)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GetCheckpointBySequenceInput {
    pub agent: AgentPubKey,
    pub sequence: u64,
}

/// Ensure checkpoint is up-to-date.
/// Called automatically during major operations to maintain checkpoint freshness.
pub fn ensure_checkpoint_fresh() -> ExternResult<()> {
    let agent = agent_info()?.agent_initial_pubkey;
    let (should_create, _, _) = should_create_checkpoint(agent)?;

    if should_create {
        create_checkpoint(())?;
    }

    Ok(())
}

// =========================================================================
//  Checkpoint Verification
// =========================================================================

/// Verify that the current state matches the latest checkpoint plus delta.
/// Returns true if state is consistent, false if corruption detected.
#[hdk_extern]
pub fn verify_checkpoint_consistency(_: ()) -> ExternResult<bool> {
    let agent = agent_info()?.agent_initial_pubkey;

    // Get latest checkpoint
    let checkpoint = match get_latest_checkpoint(agent.clone())? {
        Some((_, cp)) => cp,
        None => return Ok(true), // No checkpoint to verify against
    };

    // Recompute current summaries
    let current_contract_summary = compute_contract_summary(agent.clone())?;
    let current_trust_summary = compute_trust_summary(agent)?;

    // Verify contract counts are monotonically increasing
    if current_contract_summary.total_created < checkpoint.contract_summary.total_created {
        return Ok(false);
    }
    if current_contract_summary.total_transferred < checkpoint.contract_summary.total_transferred {
        return Ok(false);
    }
    if current_contract_summary.total_expired < checkpoint.contract_summary.total_expired {
        return Ok(false);
    }
    if current_contract_summary.total_archived < checkpoint.contract_summary.total_archived {
        return Ok(false);
    }

    // Verify chain length hasn't decreased
    let current_chain_length = get_chain_length()?;
    if current_chain_length < checkpoint.chain_length {
        return Ok(false);
    }

    // Verify acquaintance count is reasonable (can decrease due to removals)
    // but shouldn't be drastically different.
    //
    // Bug fix: the original check `acq_diff > count / 2` produces false positives
    // when the checkpoint was created with a small acquaintance count. For example,
    // if count = 2, the threshold is 2/2 = 1, meaning any change of more than 1
    // acquaintance triggers a failure — too strict for early-stage nodes.
    //
    // Fix: use a minimum absolute floor of MIN_ACQ_ABSOLUTE_DIFF (10) so that low-count
    // checkpoints are not erroneously invalidated by normal acquaintance churn.
    const MIN_ACQ_ABSOLUTE_DIFF: u64 = 10;
    let acq_diff = (current_trust_summary.acquaintance_count as i64
        - checkpoint.trust_summary.acquaintance_count as i64)
        .unsigned_abs();
    let acq_threshold = (checkpoint.trust_summary.acquaintance_count / 2).max(MIN_ACQ_ABSOLUTE_DIFF);
    if acq_diff > acq_threshold {
        // More than 50% change (or more than MIN_ACQ_ABSOLUTE_DIFF for small counts) is suspicious
        return Ok(false);
    }

    Ok(true)
}
