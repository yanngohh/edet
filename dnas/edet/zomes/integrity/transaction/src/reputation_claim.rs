use hdi::prelude::*;

use crate::debt_contract::*;
use crate::types::constants::{
    reputation_claim_validation_error, DUST_THRESHOLD, EPOCH_DURATION_SECS, MAX_CLAIM_STALENESS_SECONDS,
    MAX_CONTRACTS_PER_CLAIM_SCAN, MAX_THEORETICAL_CAPACITY,
};

/// Cumulative statistics carried forward across claim epochs.
/// This enables incremental updates without scanning the entire contract history.
#[derive(Clone, PartialEq, Serialize, Deserialize, SerializedBytes, Debug)]
pub struct ClaimCumulativeStats {
    /// Total number of contracts processed (created) up to and including this claim.
    pub total_contracts_processed: u64,
    /// Total number of contracts that reached `ContractStatus::Transferred` (fully repaid).
    /// This is the primary trust signal used in `successful_transfers` on the claim.
    /// Distinct from `total_contracts_processed` which counts all created contracts
    /// regardless of outcome.
    #[serde(default)]
    pub total_successful_transfers: u64,
    /// Total debt amount successfully transferred (sum of all transferred contracts).
    pub total_amount_transferred: f64,
    /// Total debt amount that expired without transfer (sum of all expired contracts).
    pub total_amount_expired: f64,
    /// Set of all distinct counterparty keys seen across all claim epochs.
    /// Carried forward and merged with new counterparties each claim to prevent
    /// overcounting when the same counterparty appears in multiple claim windows.
    #[serde(default)]
    pub counterparty_set: Vec<AgentPubKeyB64>,
}

/// Result of scanning an agent's source chain for contract statistics.
pub struct ChainScanResult {
    pub contracts_count: u64,
    /// Number of contracts that reached ContractStatus::Transferred (fully repaid).
    /// Used to verify `claim.successful_transfers` and
    /// `claim.cumulative_stats.total_successful_transfers`.
    pub successful_transfers_count: u64,
    pub amount_transferred: f64,
    pub amount_expired: f64,
    pub distinct_counterparties_count: u64,
    pub contract_hashes: Vec<Vec<u8>>,
}

impl Default for ClaimCumulativeStats {
    fn default() -> Self {
        ClaimCumulativeStats {
            total_contracts_processed: 0,
            total_successful_transfers: 0,
            total_amount_transferred: 0.0,
            total_amount_expired: 0.0,
            counterparty_set: Vec::new(),
        }
    }
}

/// A verifiable claim about an agent's creditworthiness for first-contact transactions.
///
/// When transacting with a new counterparty (no prior bilateral history), the buyer
/// presents a ReputationClaim so the seller can assess risk without performing a full
/// EigenTrust graph traversal.
///
/// The claim is validated by DHT validators who cross-check it against the agent's
/// actual DebtContract entries. Capacity and debt are expressed as conservative bounds
/// (capacity rounded down, debt rounded up) so that any risk score derived from the
/// claim is at least as high as the true risk score. This is consistent with the
/// protocol's radical transparency model: all source-chain data is publicly traversable
/// on the DHT, so the bounds serve risk-score safety, not confidentiality.
///
/// For REPEAT transactions (existing bilateral history), the seller still uses
/// the full S/F model — this claim is only for first-contact.
///
/// ## Incremental Updates
///
/// To support billion-scale networks, claims support incremental updates:
/// - `last_processed_contract` tracks the last contract included in this claim
/// - `cumulative_stats` carries forward historical totals
/// - New claims only need to scan contracts created since the previous claim
#[derive(Clone, PartialEq)]
#[hdk_entry_helper]
pub struct ReputationClaim {
    /// The agent this claim is about (must be the author).
    pub agent: AgentPubKeyB64,

    /// Lower bound on credit capacity (actual capacity >= this value).
    /// Rounded down to ensure the claim-based risk score is a conservative
    /// upper bound on the true risk score (R^claim >= R^true).
    pub capacity_lower_bound: f64,

    /// Upper bound on current total debt (actual debt <= this value).
    /// Rounded up to ensure the claim-based risk score is a conservative
    /// upper bound on the true risk score (R^claim >= R^true).
    pub debt_upper_bound: f64,

    /// Number of fully transferred (successful) contracts as debtor.
    /// Evidence of positive transaction history.
    pub successful_transfers: u64,

    /// Number of distinct creditors the agent has transacted with.
    /// Total number of distinct counterparties in the claim's history.
    pub distinct_counterparties: u64,

    /// Timestamp (in seconds) when this claim was computed. Claims become stale after
    /// MAX_CLAIM_STALENESS_SECONDS.
    pub timestamp: u64,

    /// Blake2b hash of the sorted contract action hashes used to compute this claim.
    /// Validators can cross-check this against the agent's actual contracts.
    pub evidence_hash: ExternalHash,

    // =========================================================================
    //  Incremental Update Fields (for scalability)
    // =========================================================================
    /// Action hash of the last contract processed when creating this claim.
    /// Subsequent claims only need to scan contracts after this point.
    /// None for the first claim (no prior contracts).
    pub last_processed_contract: Option<ActionHash>,

    /// Cumulative statistics from all contracts up to this claim.
    /// Carried forward and incremented with each new claim.
    pub cumulative_stats: ClaimCumulativeStats,

    /// Action hash of the previous ReputationClaim in the chain.
    /// Essential for incremental validation by neighbors.
    pub prev_claim_hash: Option<ActionHash>,
}

/// Validation rules for ReputationClaim creation.
///
/// Key validations:
/// 1. Author must be the agent in the claim (you can only publish your own claim)
/// 2. Epoch must be current or recent (no stale backdating)
/// 3. Bounds must be plausible (within protocol limits)
/// 4. At most one claim per epoch per agent
///
/// NOTE: We cannot fully verify capacity_lower_bound here because it requires
/// EigenTrust computation over the trust graph (which needs data from other agents).
/// We verify it's within plausible bounds [0, MAX_THEORETICAL_CAPACITY]
/// and rely on the seller to fall back to full computation for high-value transactions.
pub fn validate_create_reputation_claim(
    action: EntryCreationAction,
    claim: ReputationClaim,
) -> ExternResult<ValidateCallbackResult> {
    // 1. Author must be the agent in the claim
    if action.author() != &AgentPubKey::from(claim.agent.clone()) {
        return Ok(ValidateCallbackResult::Invalid(reputation_claim_validation_error::AUTHOR_NOT_AGENT.to_string()));
    }

    // 2. Freshness must be plausible (not in the future relative to action timestamp, not too old)
    let action_time_secs = match action.timestamp().as_seconds_and_nanos() {
        (s, _) if s < 0 => 0,
        (s, _) => s as u64,
    };

    if claim.timestamp > action_time_secs {
        return Ok(ValidateCallbackResult::Invalid(reputation_claim_validation_error::EPOCH_IN_FUTURE.to_string()));
    }
    if action_time_secs.saturating_sub(claim.timestamp) > MAX_CLAIM_STALENESS_SECONDS {
        return Ok(ValidateCallbackResult::Invalid(reputation_claim_validation_error::EPOCH_TOO_OLD.to_string()));
    }

    // 3. Capacity bound must be within protocol limits.
    // The lower bound is 0: agents with small vouches and no trust may have
    // capacity below BASE_CAPACITY, which is legitimate after the self-trust
    // inflation fix (subgraph < 2 → trust = 0).
    if claim.capacity_lower_bound < 0.0 {
        return Ok(ValidateCallbackResult::Invalid(
            reputation_claim_validation_error::CAPACITY_BELOW_MINIMUM.to_string(),
        ));
    }
    if claim.capacity_lower_bound > MAX_THEORETICAL_CAPACITY {
        return Ok(ValidateCallbackResult::Invalid(
            reputation_claim_validation_error::CAPACITY_ABOVE_MAXIMUM.to_string(),
        ));
    }

    // 4. Debt bound must be non-negative
    if claim.debt_upper_bound < 0.0 {
        return Ok(ValidateCallbackResult::Invalid(reputation_claim_validation_error::DEBT_NEGATIVE.to_string()));
    }

    // 5. Claimed remaining capacity must be non-negative (within rounding tolerance).
    // Since capacity_lower_bound is rounded DOWN and debt_upper_bound is rounded UP,
    // a small overlap is acceptable. The tolerance allows honest agents whose true
    // capacity exceeds true debt to still publish valid claims when conservative
    // rounding pushes the bounds past each other.
    let remaining_capacity_tolerance = 0.1 * claim.debt_upper_bound.max(1.0);
    if claim.capacity_lower_bound + remaining_capacity_tolerance < claim.debt_upper_bound {
        return Ok(ValidateCallbackResult::Invalid(
            reputation_claim_validation_error::DEBT_EXCEEDS_CAPACITY.to_string(),
        ));
    }

    // 6. At most one claim per epoch (scan chain for duplicates)
    let activity = must_get_agent_activity(action.author().clone(), ChainFilter::new(action.prev_action().clone()))?;
    let claim_epoch = claim.timestamp / EPOCH_DURATION_SECS;
    for item in &activity {
        let item_action = item.action.action();
        if item_action.action_type() != ActionType::Create {
            continue;
        }
        if item_action.entry_type().is_none_or(|et| et != action.entry_type()) {
            continue;
        }
        // Found another ReputationClaim create - check if same epoch
        if let Some(entry_hash) = item_action.entry_hash() {
            if let Ok(entry) = must_get_entry(entry_hash.clone()) {
                if let Ok(existing_claim) = ReputationClaim::try_from(entry.content) {
                    let existing_epoch = existing_claim.timestamp / EPOCH_DURATION_SECS;
                    if existing_epoch == claim_epoch {
                        return Ok(ValidateCallbackResult::Invalid(
                            reputation_claim_validation_error::DUPLICATE_EPOCH.to_string(),
                        ));
                    }
                }
            }
        }
    }

    // 7. Validate first claim or incremental claim stats against actual source chain data.
    //    For first claims (no prev_claim_hash), verify cumulative stats from scratch.
    //    For incremental claims, verify the delta against contracts since the previous claim.

    // Helper: scan contracts from an agent's chain to verify claim stats.
    // Walks backward from `start_hash` and collects debt contract stats as debtor.
    let scan_contracts_on_chain = |start_hash: Option<ActionHash>,
                                   stop_hash: Option<&ActionHash>,
                                   author: &AgentPubKeyB64|
     -> ExternResult<ChainScanResult> {
        let mut contracts_count = 0u64;
        let mut successful_transfers_count = 0u64;
        let mut amount_transferred = 0.0;
        let mut amount_expired = 0.0;
        let mut counterparties: std::collections::HashSet<AgentPubKeyB64> = std::collections::HashSet::new();
        let mut contract_hashes: Vec<Vec<u8>> = Vec::new();

        if let Some(mut current_hash) = start_hash {
            // `entries_walked` counts every source chain entry visited, not just
            // DebtContracts.  Agents with long chains have many non-contract entries
            // (Transactions, Wallets, Vouches, etc.) interspersed with contracts.
            // The limit is on total entries walked — the expensive `must_get_valid_record`
            // calls — to bound validator CPU and network work. MAX_CONTRACTS_PER_CLAIM_SCAN
            // is set large enough (50,000) to accommodate the realistic worst case:
            //   150 acquaintances × 100 transactions/epoch × 3 entries/tx ≈ 45,000
            // An agent who approaches this limit must publish claims more frequently.
            let mut entries_walked = 0u64;
            loop {
                if stop_hash.is_some_and(|h| *h == current_hash) {
                    break;
                }
                if entries_walked >= MAX_CONTRACTS_PER_CLAIM_SCAN {
                    // Safety cap: reject claims that require walking more source chain
                    // entries than MAX_CONTRACTS_PER_CLAIM_SCAN.  The publisher must
                    // issue claims more frequently so each delta window stays within
                    // this limit.
                    return Err(wasm_error!(WasmErrorInner::Guest(format!(
                        "Chain scan exceeded {MAX_CONTRACTS_PER_CLAIM_SCAN} entries — publish claims more frequently"
                    ))));
                }

                let record = must_get_valid_record(current_hash.clone())?;
                entries_walked += 1;

                // Gracefully skip entries that aren't DebtContracts (e.g. Transaction,
                // Wallet, Vouch entries on the same source chain).
                if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
                    if contract.debtor == *author {
                        let action_type = record.action().action_type();

                        // Only count new contract creations (Phase 3 Fix)
                        if action_type == ActionType::Create {
                            contracts_count += 1;
                        }

                        counterparties.insert(contract.creditor.clone());
                        contract_hashes.push(current_hash.get_raw_39().to_vec());

                        match contract.status {
                            ContractStatus::Transferred => {
                                // Use (original_amount - residual) to match the coordinator's
                                // computation in reputation.rs (publish_reputation_claim).
                                amount_transferred += (contract.original_amount - contract.amount).max(0.0);
                                // Count the Transferred contract toward n_S so that
                                // claim.successful_transfers can be verified against the chain.
                                successful_transfers_count += 1;
                            }
                            ContractStatus::Expired => amount_expired += contract.amount,
                            ContractStatus::Archived => {
                                // Archived contracts are ignored to prevent double-counting
                                // across incremental claim windows.
                            }
                            _ => {}
                        }
                    }
                }

                let action = record.action();
                match action.prev_action() {
                    Some(prev) => current_hash = prev.clone(),
                    None => break,
                }
            }
        }

        Ok(ChainScanResult {
            contracts_count,
            successful_transfers_count,
            amount_transferred,
            amount_expired,
            distinct_counterparties_count: counterparties.len() as u64,
            contract_hashes,
        })
    };

    if let Some(prev_hash) = claim.prev_claim_hash.clone() {
        // INCREMENTAL CLAIM: Validate delta against previous claim
        let prev_record = must_get_valid_record(prev_hash)?;
        let prev_claim = match prev_record
            .entry()
            .to_app_option::<ReputationClaim>()
            .map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?
        {
            Some(c) => c,
            None => {
                return Ok(ValidateCallbackResult::Invalid(
                    reputation_claim_validation_error::PREV_CLAIM_NOT_FOUND.to_string(),
                ))
            }
        };

        // 7a. Verify epoch monotonicity
        if prev_claim.timestamp >= claim.timestamp {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::PREV_CLAIM_TIMESTAMP_INVALID.to_string(),
            ));
        }

        // 7b. Verify delta from contracts since previous claim
        let scan_result = scan_contracts_on_chain(
            claim.last_processed_contract.clone(),
            prev_claim.last_processed_contract.as_ref(),
            &claim.agent,
        )?;

        if claim.cumulative_stats.total_contracts_processed
            != prev_claim.cumulative_stats.total_contracts_processed + scan_result.contracts_count
        {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }

        // Verify successful_transfers count matches cumulative chain evidence.
        // An attacker setting claim.successful_transfers = 1_000_000 would otherwise
        // make hat_t_claim ≈ 1 and suppress the risk score for first-contact transactions,
        // bypassing the whitepaper's claim-risk bound.
        let expected_total_successful =
            prev_claim.cumulative_stats.total_successful_transfers + scan_result.successful_transfers_count;
        if claim.cumulative_stats.total_successful_transfers != expected_total_successful {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }
        // claim.successful_transfers must equal the verified cumulative count.
        if claim.successful_transfers != claim.cumulative_stats.total_successful_transfers {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }

        // Use relative tolerance to prevent accumulated floating-point drift
        // across long incremental claim chains from causing spurious rejections.
        let transferred_tolerance = DUST_THRESHOLD
            .max(1e-6 * (prev_claim.cumulative_stats.total_amount_transferred + scan_result.amount_transferred).abs());
        if (claim.cumulative_stats.total_amount_transferred
            - (prev_claim.cumulative_stats.total_amount_transferred + scan_result.amount_transferred))
            .abs()
            > transferred_tolerance
        {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }

        let expired_tolerance = DUST_THRESHOLD
            .max(1e-6 * (prev_claim.cumulative_stats.total_amount_expired + scan_result.amount_expired).abs());
        if (claim.cumulative_stats.total_amount_expired
            - (prev_claim.cumulative_stats.total_amount_expired + scan_result.amount_expired))
            .abs()
            > expired_tolerance
        {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }

        // 7b-iv. Verify distinct_counterparties for incremental claims.
        // The new claim's counterparty count must be the union of the previous set and the
        // counterparties found in the delta scan. Since we only have counts (not the full
        // delta set), we verify the count is monotonically non-decreasing and bounded by
        // the additive upper bound (prev_count + delta_count).
        {
            let prev_count = prev_claim.cumulative_stats.counterparty_set.len() as u64;
            let delta_count = scan_result.distinct_counterparties_count;
            let claimed_count = claim.distinct_counterparties;

            // The claimed count must be >= max(prev, delta) and <= prev + delta
            if claimed_count < prev_count.max(delta_count) || claimed_count > prev_count + delta_count {
                return Ok(ValidateCallbackResult::Invalid(
                    reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
                ));
            }
        }

        // 7c. Verify evidence_hash: H(prev_evidence_hash || H(new_contract_hashes))
        let mut sorted_hashes = scan_result.contract_hashes;
        sorted_hashes.sort();
        let new_evidence_bytes: Vec<u8> = sorted_hashes.into_iter().flatten().collect();
        let new_hash = compute_evidence_hash(&new_evidence_bytes);

        let mut combined: Vec<u8> = Vec::new();
        combined.extend_from_slice(prev_claim.evidence_hash.get_raw_39());
        combined.extend_from_slice(new_hash.get_raw_39());
        let expected_hash = compute_evidence_hash(&combined);

        if claim.evidence_hash != expected_hash {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::EVIDENCE_HASH_MISMATCH.to_string(),
            ));
        }
    } else {
        // FIRST CLAIM: Validate cumulative stats from scratch by scanning the full chain.
        let scan_result = scan_contracts_on_chain(claim.last_processed_contract.clone(), None, &claim.agent)?;

        if claim.cumulative_stats.total_contracts_processed != scan_result.contracts_count {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }

        // Verify successful_transfers for first claim.
        if claim.cumulative_stats.total_successful_transfers != scan_result.successful_transfers_count {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }
        if claim.successful_transfers != claim.cumulative_stats.total_successful_transfers {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }

        if (claim.cumulative_stats.total_amount_transferred - scan_result.amount_transferred).abs() > DUST_THRESHOLD {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }

        if (claim.cumulative_stats.total_amount_expired - scan_result.amount_expired).abs() > DUST_THRESHOLD {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }

        // Verify distinct_counterparties
        if claim.distinct_counterparties != scan_result.distinct_counterparties_count {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::CUMULATIVE_STATS_MISMATCH.to_string(),
            ));
        }

        // Verify evidence_hash for first claim: H(sorted contract action hashes)
        let mut sorted_hashes = scan_result.contract_hashes;
        sorted_hashes.sort();
        let evidence_bytes: Vec<u8> = sorted_hashes.into_iter().flatten().collect();
        let expected_hash = compute_evidence_hash(&evidence_bytes);

        if claim.evidence_hash != expected_hash {
            return Ok(ValidateCallbackResult::Invalid(
                reputation_claim_validation_error::EVIDENCE_HASH_MISMATCH.to_string(),
            ));
        }
    }

    Ok(ValidateCallbackResult::Valid)
}

/// Compute blake2b hash of bytes and return as ExternalHash for evidence verification.
fn compute_evidence_hash(data: &[u8]) -> ExternalHash {
    use hdi::prelude::holo_hash::{encode, hash_type};

    let hash_core = encode::blake2b_256(data);
    let mut hash_bytes: Vec<u8> = Vec::with_capacity(36);
    hash_bytes.extend_from_slice(&encode::holo_dht_location_bytes(&hash_core));
    hash_bytes.extend_from_slice(&hash_core);

    HoloHash::from_raw_36_and_type(hash_bytes, hash_type::External)
}

/// ReputationClaim entries cannot be updated - create a new one for a new epoch instead.
pub fn validate_update_reputation_claim(
    _action: Update,
    _claim: ReputationClaim,
    _original_action: EntryCreationAction,
    _original_claim: ReputationClaim,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(reputation_claim_validation_error::UPDATE_NOT_ALLOWED.to_string()))
}

/// ReputationClaim entries cannot be deleted.
pub fn validate_delete_reputation_claim(
    _action: Delete,
    _original_action: EntryCreationAction,
    _original_claim: ReputationClaim,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(reputation_claim_validation_error::DELETE_NOT_ALLOWED.to_string()))
}
