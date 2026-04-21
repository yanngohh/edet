use std::collections::HashSet;

use hdk::prelude::*;
use transaction_integrity::debt_contract::{ContractStatus, DebtContract};
use transaction_integrity::reputation_claim::{ClaimCumulativeStats, ReputationClaim};
use transaction_integrity::types::constants::{coordinator_trust_error, *};
use transaction_integrity::types::timestamp_to_epoch;
use transaction_integrity::*;

use crate::contracts;
use crate::vouch::get_vouched_capacity;

use crate::capacity::compute_credit_capacity;

// =========================================================================
//  ReputationClaim: Conservative First-Contact Verification
//
//  For first-contact transactions (no bilateral history), the buyer presents
//  a ReputationClaim providing a conservative, O(1) summary of their
//  creditworthiness, avoiding the expensive full EigenTrust traversal.
//
//  The seller verifies the claim's bounds and freshness. Capacity and debt
//  are expressed as conservative bounds (capacity rounded down, debt rounded
//  up) so that any derived risk score is at least as high as the true risk
//  score — consistent with the protocol's radical transparency model.
//
//  For REPEAT transactions (existing bilateral history), the full S/F model
//  is still used -- claims are only for first-contact optimization.
//
//  INCREMENTAL UPDATES: Claims now support incremental updates to scale to
//  billions of actors. Each claim records the last processed contract, and
//  subsequent claims only scan contracts created since then.
// =========================================================================

/// Tag for AgentToReputationClaim links containing the epoch for efficient lookup.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone)]
pub struct ReputationClaimLinkTag {
    pub epoch: u64,
}

/// Publish a ReputationClaim for the current agent.
///
/// Computes the agent's reputation, capacity, and debt, then creates a
/// ReputationClaim entry on their source chain with a link for discovery.
///
/// ## Incremental Updates
///
/// For scalability, this function uses incremental updates:
/// 1. Fetches the previous claim (if any)
/// 2. Only scans contracts created since the last claim
/// 3. Carries forward cumulative statistics
///
/// This reduces claim creation from O(all_contracts) to O(new_contracts).
///
/// ## Automatic Archival
///
/// Also triggers automatic archival of old contracts to keep active
/// query sets small.
#[hdk_extern]
pub fn publish_reputation_claim(_: ()) -> ExternResult<Record> {
    let agent = agent_info()?.agent_initial_pubkey;
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    // IDEMPOTENCY CHECK
    let filter = ChainQueryFilter::new().include_entries(true);
    let my_claims = query(filter)?;
    for record in my_claims {
        if let Some(claim) = record.entry().to_app_option::<ReputationClaim>().ok().flatten() {
            let claim_epoch = timestamp_to_epoch(Timestamp::from_micros(claim.timestamp as i64 * 1000000));
            if claim_epoch >= current_epoch {
                return Ok(record);
            }
        }
    }

    // Automatic archival trigger
    let _ = contracts::archive_old_contracts(())?;

    // Get previous claim
    let previous_claim = get_reputation_claim(agent.clone())?;
    let previous_claim_hash = previous_claim.as_ref().map(|(h, _)| h.clone());

    // Compute reputation & capacity
    let rep = super::get_subjective_reputation(agent.clone())?;
    let vouched = get_vouched_capacity(agent.clone())?;
    let capacity = compute_credit_capacity(rep.trust, rep.acquaintance_count, vouched);
    let current_debt = contracts::get_total_debt(agent.clone())?;

    // Get contracts to process
    let (contracts_to_process, prev_cumulative, prev_evidence_hash): (
        Vec<Record>,
        ClaimCumulativeStats,
        Option<ExternalHash>,
    ) = if let Some((ref _prev_hash, ref prev)) = previous_claim {
        let prev_epoch = prev.timestamp / transaction_integrity::types::constants::EPOCH_DURATION_SECS;

        // Window 1: Newly created contracts in buckets
        // resolve_latest=false: return original create-action records so that the
        // ActionType::Create counter here matches the integrity validator's chain-scan.
        let mut delta_contracts =
            contracts::get_contracts_in_epoch_range(agent.clone(), prev_epoch, current_epoch, false)?;

        // Window 2: ALL currently active/unarchived contracts (Phase 3 Auto-Recovery Fix)
        // We scan these specifically to catch "status flips" (Active -> Transferred)
        // for contracts created in OLD windows that were previously missed.
        let active_contracts = contracts::get_active_contracts_for_debtor(agent.clone())?;

        // Merge without duplicates
        let mut seen = std::collections::HashSet::new();
        for r in &delta_contracts {
            seen.insert(r.action_address().clone());
        }
        for r in active_contracts {
            if seen.insert(r.action_address().clone()) {
                delta_contracts.push(r);
            }
        }

        (delta_contracts, prev.cumulative_stats.clone(), Some(prev.evidence_hash.clone()))
    } else {
        // First claim: full scan remains unchanged
        let all_contracts = contracts::get_all_contracts_as_debtor(agent.clone())?;
        let mut latest_records = Vec::new();
        for record in all_contracts {
            let action_hash = record.action_address().clone();
            if let Some(latest) = contracts::get_latest_debt_contract_record(action_hash)? {
                latest_records.push(latest);
            }
        }
        (latest_records, ClaimCumulativeStats::default(), None)
    };

    // Process new contracts incrementally
    // `successful_transfers` tracks ONLY contracts that reached ContractStatus::Transferred.
    // This is distinct from `total_contracts_processed` (all created contracts) and is the
    // trust signal used in the claim's sigmoid heuristic: n_S / (n_S + K_claim).
    let mut successful_transfers = prev_cumulative.total_successful_transfers;
    let mut total_transferred = prev_cumulative.total_amount_transferred;
    let mut total_expired = prev_cumulative.total_amount_expired;
    let mut counterparties: HashSet<AgentPubKeyB64> = HashSet::new();
    let mut last_contract_hash: Option<ActionHash> =
        previous_claim.as_ref().and_then(|(_, p)| p.last_processed_contract.clone());

    // Track new contracts processed this time
    let mut new_contracts_count = 0u64;

    for record in &contracts_to_process {
        if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
            counterparties.insert(contract.creditor.clone());

            // Only increment contract count for creation actions (Phase 3 Fix)
            if record.action().action_type() == ActionType::Create {
                new_contracts_count += 1;
            }

            match contract.status {
                ContractStatus::Transferred => {
                    successful_transfers += 1;
                    // For a fully transferred contract, contract.amount is the residual debt (~0).
                    // Use (original_amount - amount) to get the transferred portion.
                    // original_amount is set at contract creation and is immutable, so it always
                    // equals the initial principal regardless of how many partial-transfer updates
                    // preceded the final Transferred status. This matches the integrity zome's
                    // chain-scan formula exactly, eliminating the EV600012 mismatch.
                    let transferred_amount = (contract.original_amount - contract.amount).max(0.0);
                    total_transferred += transferred_amount;
                }
                ContractStatus::Expired => {
                    total_expired += contract.amount;
                }
                ContractStatus::Active => {
                    // Active contracts don't contribute to cumulative stats yet
                }
                ContractStatus::Archived => {
                    // Archived contracts were already processed in previous claims
                }
            }

            // Track the last processed action (even if it's an update)
            last_contract_hash = Some(record.action_address().clone());
        }
    }

    // Add counterparties from previous claims and merge with new ones.
    // Use the cumulative set from the previous claim to avoid overcounting
    // when the same counterparty appears in multiple claim windows.
    let merged_counterparty_set: Vec<AgentPubKeyB64> = {
        let mut set: HashSet<AgentPubKeyB64> = if let Some((_, ref prev)) = previous_claim {
            prev.cumulative_stats.counterparty_set.iter().cloned().collect()
        } else {
            HashSet::new()
        };
        set.extend(counterparties);
        set.into_iter().collect()
    };
    let distinct_counterparties = merged_counterparty_set.len() as u64;

    // Compute incremental evidence hash: H(prev_hash || H(new_contract_hashes))
    // This avoids the O(all_contracts) full scan that defeated incrementality.
    let evidence_hash = {
        let mut new_hashes: Vec<ActionHash> = contracts_to_process.iter().map(|r| r.action_address().clone()).collect();
        new_hashes.sort();
        let new_evidence_bytes: Vec<u8> = new_hashes.iter().flat_map(|h| h.get_raw_39().to_vec()).collect();
        let new_hash = compute_evidence_external_hash(&new_evidence_bytes);

        match prev_evidence_hash {
            Some(prev_hash) => {
                // Chain: H(prev_hash || new_hash)
                let mut combined: Vec<u8> = Vec::new();
                combined.extend_from_slice(prev_hash.get_raw_39());
                combined.extend_from_slice(new_hash.get_raw_39());
                compute_evidence_external_hash(&combined)
            }
            None => new_hash,
        }
    };

    // Update cumulative stats
    let cumulative_stats = ClaimCumulativeStats {
        total_contracts_processed: prev_cumulative.total_contracts_processed + new_contracts_count,
        total_successful_transfers: successful_transfers,
        total_amount_transferred: total_transferred,
        total_amount_expired: total_expired,
        counterparty_set: merged_counterparty_set,
    };

    // Round capacity down and debt up for conservativeness: ensures R^claim >= R^true
    let capacity_lower_bound = (capacity / 100.0).floor() * 100.0;
    let debt_upper_bound = (current_debt / 100.0).ceil() * 100.0;

    let claim = ReputationClaim {
        agent: agent.clone().into(),
        capacity_lower_bound,
        debt_upper_bound,
        successful_transfers,
        distinct_counterparties,
        timestamp: now.as_seconds_and_nanos().0 as u64,
        evidence_hash,
        last_processed_contract: last_contract_hash,
        cumulative_stats,
        prev_claim_hash: previous_claim_hash,
    };

    // Pre-validate the claim to prevent poisoning the Holochain scratch space.
    // An agent with 0 capacity but active debt (e.g. from an unvouched trial)
    // cannot publish a valid claim because capacity < debt is rejected by integrity validation.
    // Throwing an error here causes ensure_fresh_claim to gracefully ignore it,
    // allowing the agent to transact via PATH 0/2 instead of rolling back the entire tx.
    let remaining_capacity_tolerance = 0.1 * claim.debt_upper_bound.max(1.0);
    if claim.capacity_lower_bound + remaining_capacity_tolerance < claim.debt_upper_bound {
        return Err(wasm_error!(WasmErrorInner::Guest(
            "Cannot publish ReputationClaim: Debt exceeds capacity bounds".to_string()
        )));
    }

    // Delete old claim links (keep only current epoch)
    let existing_links =
        get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToReputationClaim)?, GetStrategy::default())?;
    for link in existing_links {
        delete_link(link.create_link_hash, GetOptions::default())?;
    }

    // Create the claim entry
    info!(
        "Publishing ReputationClaim for agent {}: cap_lower={}, debt_upper={}, success={}",
        agent, claim.capacity_lower_bound, claim.debt_upper_bound, claim.successful_transfers
    );
    let claim_action_hash = create_entry(&EntryTypes::ReputationClaim(claim.clone()))?;

    // Create link for discovery
    let tag = ReputationClaimLinkTag { epoch: current_epoch };
    let tag_bytes = SerializedBytes::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    create_link(
        agent,
        claim_action_hash.clone(),
        LinkTypes::AgentToReputationClaim,
        LinkTag(tag_bytes.bytes().clone()),
    )?;

    get(claim_action_hash, GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_trust_error::REPUTATION_CLAIM_NOT_FOUND.to_string())))
}

/// Compute blake2b hash of bytes and return as ExternalHash.
pub(crate) fn compute_evidence_external_hash(data: &[u8]) -> ExternalHash {
    use hdk::prelude::holo_hash::{encode, hash_type};

    // blake2b_256 produces 32 bytes
    let hash_core = encode::blake2b_256(data);

    // Build the 36-byte format: 3 bytes DHT location + 32 bytes hash + type marker
    let mut hash_bytes: Vec<u8> = Vec::with_capacity(36);
    hash_bytes.extend_from_slice(&encode::holo_dht_location_bytes(&hash_core));
    hash_bytes.extend_from_slice(&hash_core);

    HoloHash::from_raw_36_and_type(hash_bytes, hash_type::External)
}

/// Get the latest ReputationClaim for an agent.
///
/// Returns None if the agent has never published a claim.
#[hdk_extern]
pub fn get_reputation_claim(agent: AgentPubKey) -> ExternResult<Option<(ActionHash, ReputationClaim)>> {
    let my_agent = agent_info()?.agent_initial_pubkey;
    let strategy = if agent == my_agent { GetStrategy::Local } else { GetStrategy::Network };
    let links = get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToReputationClaim)?, strategy)?;

    // Find the most recent claim by epoch
    let mut best_claim: Option<(u64, ReputationClaim, ActionHash)> = None;

    for link in links {
        if let Some(action_hash) = link.target.into_action_hash() {
            if let Some(record) = get(action_hash.clone(), GetOptions::default())? {
                if let Some(claim) = record.entry().to_app_option::<ReputationClaim>().ok().flatten() {
                    let claim_epoch = timestamp_to_epoch(Timestamp::from_micros(claim.timestamp as i64 * 1000000));
                    if best_claim.as_ref().is_none_or(|(epoch, _, _)| claim_epoch > *epoch) {
                        info!("Found valid ReputationClaim at epoch {} for agent {}", claim_epoch, agent);
                        best_claim = Some((claim_epoch, claim, action_hash));
                    }
                }
            }
        }
    }

    Ok(best_claim.map(|(_, claim, hash)| (hash, claim)))
}

/// Get the ActionHash of a specific ReputationClaim by epoch.
/// Helper for linking previous claims.
pub fn get_reputation_claim_action_hash(agent: AgentPubKey, epoch: u64) -> Option<ActionHash> {
    // This is inefficient (re-fetches), but robust. Optimization: return hash from get_reputation_claim
    let links =
        get_links(LinkQuery::try_new(agent, LinkTypes::AgentToReputationClaim).ok()?, GetStrategy::default()).ok()?;

    for link in links {
        if let Some(action_hash) = link.target.into_action_hash() {
            if let Ok(Some(record)) = get(action_hash.clone(), GetOptions::default()) {
                if let Ok(Some(claim)) = record.entry().to_app_option::<ReputationClaim>() {
                    let claim_epoch = claim.timestamp / transaction_integrity::types::constants::EPOCH_DURATION_SECS;
                    if claim_epoch == epoch {
                        return Some(action_hash);
                    }
                }
            }
        }
    }
    None
}

/// Returns true if the claim was computed within the freshness window limit (`MAX_CLAIM_STALENESS_SECONDS`).
/// For a Flash Loan mitigation, we only accept claims minted recently in real-time.
pub fn is_claim_fresh(claim: &ReputationClaim) -> ExternResult<bool> {
    let now = sys_time()?;
    let (now_secs, _) = now.as_seconds_and_nanos();
    let now_secs = if now_secs < 0 { 0 } else { now_secs as u64 };

    // Prevent future timestamps
    if claim.timestamp > now_secs {
        return Ok(false);
    }

    // Check if within the seconds limit
    Ok(now_secs.saturating_sub(claim.timestamp) <= transaction_integrity::types::constants::MAX_CLAIM_STALENESS_SECONDS)
}

/// Compute risk score from a ReputationClaim (conservative fallback for subgraph-unreachable buyers).
///
/// Uses the claim's bounds and the whitepaper claim-based risk formula:
///   R^claim = 1 - hat_t_claim * (Cap_lower - Debt_upper) / Cap_lower
/// where:
///   hat_t_claim = n_S / (n_S + K_claim)    (sigmoid over successful transfers)
///
/// This is provably conservative: capacity bounds are understated (lower bound) and
/// debt bounds are overstated (upper bound), so R^claim >= R^true (Theorem claim_conservative).
/// See whitepaper Section 7.1 (ReputationClaim) and Theorem claim_conservative.
///
/// Note: the debt velocity factor lambda_b is intentionally omitted in the claim-based
/// path (Path 1). The claim does not embed epoch-scoped D_in/D_out metrics, and
/// computing them separately would require an additional DHT round-trip that defeats
/// the O(1) purpose of the claim path. lambda_b = 1 (no penalty) is therefore applied
/// here; the full lambda_b is applied in Path 2 (full EigenTrust, see risk.rs).
pub fn compute_risk_from_claim(claim: &ReputationClaim, current_debt: f64) -> f64 {
    // Use max(current_debt, claim.debt_upper_bound) to restore the
    // conservativeness guarantee of Theorem `thm:claim_conservative`.
    //
    // The whitepaper requires R^claim >= R^true at all times. For this to hold,
    // the debt term must be an upper bound on the true debt:
    //   R^claim = 1 - t̂ · (cap_lb − D̄) / cap_lb  with  D̄ >= D_true
    //
    // Using `current_debt` (live DHT query) is more precise when fresh, but can
    // violate the bound if the DHT view is stale and the buyer has acquired new
    // debt since the claim was published. The claim's `debt_upper_bound` was
    // computed conservatively (rounded UP to the nearest unit) at publish time and
    // is guaranteed to be >= debt at publish time. Taking max(current, claim_bound)
    // ensures we always use the tighter (larger) of the two.
    //
    // For honest buyers who have repaid debt since the claim was published,
    // `current_debt` will be lower than `debt_upper_bound`, so we use current_debt
    // — the bound never exceeds capacity in normal operation because the integrity
    // zome enforces debt < capacity at contract creation.
    let effective_debt = current_debt.max(claim.debt_upper_bound);
    let remaining_ratio = if claim.capacity_lower_bound > 0.0 {
        ((claim.capacity_lower_bound - effective_debt) / claim.capacity_lower_bound).max(0.0)
    } else {
        0.0
    };

    // Claim-based trust: sigmoid over successful_transfers per whitepaper Eq. claim_risk.
    // hat_t_claim = n_S / (n_S + K_claim)
    // K_claim = 20 by default (reaching 0.5 at 20 successful transfers).
    let n_s = claim.successful_transfers as f64;
    let trust_heuristic = n_s / (n_s + K_CLAIM_SIGMOID);

    1.0 - trust_heuristic * remaining_ratio
}

/// Ensure the current agent has a fresh ReputationClaim.
///
/// If no claim exists for the current epoch, computes and publishes one.
/// This is called lazily on the first transaction of each epoch.
pub fn ensure_fresh_claim() -> ExternResult<()> {
    // publish_reputation_claim is now idempotent for the current epoch,
    // so we can simply call it. If a fresh claim exists, it returns immediately;
    // otherwise, it computes and publishes a new one.
    let _ = publish_reputation_claim(())?;
    Ok(())
}
