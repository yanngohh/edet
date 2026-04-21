//! Source Chain Checkpoint Entry
//!
//! Checkpoints are periodic summaries of an agent's source chain state that
//! allow validators to skip historical validation. Instead of validating
//! thousands of entries, validators can verify that the chain matches a
//! recent checkpoint and validate only entries since then.
//!
//! This is Phase 5 of the scalability architecture, enabling billion-scale
//! networks by reducing validation overhead from O(chain_length) to O(checkpoint_interval).
//!
//! # Structure
//!
//! A checkpoint contains:
//! - Summary of all debt contracts (total created, transferred, expired, archived)
//! - Summary of trust state (acquaintance count, total S/F counters)
//! - Hash of the last entry in the checkpointed range
//! - Sequence number for ordering
//!
//! # Creation
//!
//! Checkpoints are created automatically when:
//! - CHECKPOINT_INTERVAL_EPOCHS epochs have passed since the last checkpoint, OR
//! - CHECKPOINT_INTERVAL_ENTRIES entries have been created since the last checkpoint
//!
//! # Validation
//!
//! Validators can use checkpoints to:
//! 1. Skip validation of entries before the checkpoint
//! 2. Verify that current state matches checkpoint + delta
//! 3. Detect state corruption by comparing checkpoint hashes

use hdi::prelude::*;

use crate::types::constants::*;

/// Summary of contract state at checkpoint time.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
pub struct ContractSummary {
    /// Total number of contracts ever created by this agent as debtor.
    pub total_created: u64,
    /// Number of contracts successfully transferred.
    pub total_transferred: u64,
    /// Number of contracts that expired.
    pub total_expired: u64,
    /// Number of contracts archived.
    pub total_archived: u64,
    /// Total debt amount currently outstanding (active contracts).
    pub current_debt: f64,
}

impl Default for ContractSummary {
    fn default() -> Self {
        ContractSummary {
            total_created: 0,
            total_transferred: 0,
            total_expired: 0,
            total_archived: 0,
            current_debt: 0.0,
        }
    }
}

/// Summary of trust state at checkpoint time.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
pub struct TrustSummary {
    /// Number of acquaintances.
    pub acquaintance_count: u64,
    /// Total satisfaction amount (sum of all S_ij).
    pub total_satisfaction: f64,
    /// Total failure amount (sum of all F_ij).
    pub total_failure: f64,
    /// Last computed trust score (from most recent reputation computation).
    pub last_trust_score: f64,
    /// Last computed credit capacity.
    pub last_capacity: f64,
}

impl Default for TrustSummary {
    fn default() -> Self {
        TrustSummary {
            acquaintance_count: 0,
            total_satisfaction: 0.0,
            total_failure: 0.0,
            last_trust_score: 0.0,
            last_capacity: BASE_CAPACITY,
        }
    }
}

/// A source chain checkpoint entry.
#[derive(Clone, PartialEq)]
#[hdk_entry_helper]
pub struct ChainCheckpoint {
    /// The agent this checkpoint belongs to.
    pub agent: AgentPubKeyB64,

    /// Epoch when this checkpoint was created.
    pub epoch: u64,

    /// Checkpoint sequence number (monotonically increasing).
    pub sequence: u64,

    /// Summary of contract state.
    pub contract_summary: ContractSummary,

    /// Summary of trust state.
    pub trust_summary: TrustSummary,

    /// Hash of the last action in the checkpointed range.
    /// Used to verify chain integrity.
    pub last_action_hash: ActionHashB64,

    /// Total number of entries on the source chain at checkpoint time.
    pub chain_length: u64,

    /// Hash of the previous checkpoint (for checkpoint chain integrity).
    /// None for the first checkpoint.
    pub prev_checkpoint_hash: Option<ActionHashB64>,

    /// Cryptographic binding of the summary fields to the underlying
    /// source chain range. Blake2b-256 hash over the canonical encoding of
    /// (prev_evidence_hash || last_action_hash || contract_summary ||
    ///  trust_summary || epoch || sequence || chain_length). Without this,
    /// a malicious author can forge arbitrary S/F/acq values in the summary
    /// fields and validators who trust the checkpoint would skip historical
    /// validation against the forged state.
    ///
    /// This field is MANDATORY for every checkpoint. The validator
    /// recomputes it from the committed fields and rejects the entry if
    /// the hashes differ. For the first checkpoint in an agent's chain
    /// (prev_checkpoint_hash is None), the `prev_evidence_hash` seed is
    /// a 39-byte zero buffer.
    pub evidence_hash: ExternalHash,
}

/// Compute blake2b hash of bytes and return as ExternalHash for evidence
/// verification. Mirrors the helper in reputation_claim.rs so both evidence
/// schemes use identical hashing (Blake2b-256 with Holochain's 4-byte
/// DHT-location prefix, producing a 39-byte ExternalHash).
fn compute_evidence_hash(data: &[u8]) -> ExternalHash {
    use hdi::prelude::holo_hash::{encode, hash_type};

    let hash_core = encode::blake2b_256(data);
    let mut hash_bytes: Vec<u8> = Vec::with_capacity(36);
    hash_bytes.extend_from_slice(&encode::holo_dht_location_bytes(&hash_core));
    hash_bytes.extend_from_slice(&hash_core);

    HoloHash::from_raw_36_and_type(hash_bytes, hash_type::External)
}

/// Canonical byte encoding of the checkpoint fields that participate in the
/// evidence hash. The encoding is deterministic (fixed-width little-endian
/// for integers, fixed-size IEEE 754 bits for f64, raw 39-byte hash buffers
/// for hashes). Any two callers computing over equal logical content
/// produce byte-identical output.
///
/// Layout (field order is part of the spec — NEVER reorder):
///   [0..39]       prev_evidence_hash (39 zero bytes for the genesis
///                 checkpoint, i.e. prev_checkpoint_hash is None)
///   [39..78]      last_action_hash (raw_39)
///   [78..86]      epoch (u64 LE)
///   [86..94]      sequence (u64 LE)
///   [94..102]     chain_length (u64 LE)
///   [102..110]    contract_summary.total_created (u64 LE)
///   [110..118]    contract_summary.total_transferred (u64 LE)
///   [118..126]    contract_summary.total_expired (u64 LE)
///   [126..134]    contract_summary.total_archived (u64 LE)
///   [134..142]    contract_summary.current_debt (f64 bits LE)
///   [142..150]    trust_summary.acquaintance_count (u64 LE)
///   [150..158]    trust_summary.total_satisfaction (f64 bits LE)
///   [158..166]    trust_summary.total_failure (f64 bits LE)
///   [166..174]    trust_summary.last_trust_score (f64 bits LE)
///   [174..182]    trust_summary.last_capacity (f64 bits LE)
/// Total: 182 bytes.
fn serialize_checkpoint_evidence(prev_evidence_hash_bytes: &[u8; 39], checkpoint: &ChainCheckpoint) -> Vec<u8> {
    let mut buf = Vec::with_capacity(182);
    buf.extend_from_slice(prev_evidence_hash_bytes);

    // last_action_hash as raw 39 bytes
    let last_action: ActionHash = checkpoint.last_action_hash.clone().into();
    buf.extend_from_slice(last_action.get_raw_39());

    buf.extend_from_slice(&checkpoint.epoch.to_le_bytes());
    buf.extend_from_slice(&checkpoint.sequence.to_le_bytes());
    buf.extend_from_slice(&checkpoint.chain_length.to_le_bytes());

    let cs = &checkpoint.contract_summary;
    buf.extend_from_slice(&cs.total_created.to_le_bytes());
    buf.extend_from_slice(&cs.total_transferred.to_le_bytes());
    buf.extend_from_slice(&cs.total_expired.to_le_bytes());
    buf.extend_from_slice(&cs.total_archived.to_le_bytes());
    buf.extend_from_slice(&cs.current_debt.to_bits().to_le_bytes());

    let ts = &checkpoint.trust_summary;
    buf.extend_from_slice(&ts.acquaintance_count.to_le_bytes());
    buf.extend_from_slice(&ts.total_satisfaction.to_bits().to_le_bytes());
    buf.extend_from_slice(&ts.total_failure.to_bits().to_le_bytes());
    buf.extend_from_slice(&ts.last_trust_score.to_bits().to_le_bytes());
    buf.extend_from_slice(&ts.last_capacity.to_bits().to_le_bytes());

    buf
}

/// Compute the evidence hash for a given ChainCheckpoint, given the previous
/// checkpoint's evidence hash (or `None` if this is the genesis checkpoint).
/// Callers MUST use this exact function on both the coordinator (commit)
/// and integrity (validation) sides to guarantee agreement.
pub fn compute_checkpoint_evidence_hash(
    prev_evidence: Option<&ExternalHash>,
    checkpoint: &ChainCheckpoint,
) -> ExternalHash {
    let zero_bytes = [0u8; 39];
    let prev_bytes: [u8; 39] = match prev_evidence {
        Some(h) => {
            let raw = h.get_raw_39();
            let mut arr = [0u8; 39];
            arr.copy_from_slice(raw);
            arr
        }
        None => zero_bytes,
    };
    let buf = serialize_checkpoint_evidence(&prev_bytes, checkpoint);
    compute_evidence_hash(&buf)
}

/// Validate checkpoint creation.
pub fn validate_create_checkpoint(
    action: EntryCreationAction,
    checkpoint: ChainCheckpoint,
) -> ExternResult<ValidateCallbackResult> {
    // Only the agent can create their own checkpoint
    let author: AgentPubKeyB64 = action.author().clone().into();
    if author != checkpoint.agent {
        return Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::AUTHOR_NOT_AGENT.to_string()));
    }

    // Epoch must not be in the future
    let action_epoch = crate::types::timestamp_to_epoch(*action.timestamp());
    if checkpoint.epoch > action_epoch {
        return Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::EPOCH_IN_FUTURE.to_string()));
    }

    // Sequence must be positive (0 is reserved for "no checkpoint")
    if checkpoint.sequence == 0 {
        return Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::INVALID_SEQUENCE.to_string()));
    }

    // Current debt must not be negative
    if checkpoint.contract_summary.current_debt < 0.0 {
        return Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::NEGATIVE_DEBT.to_string()));
    }

    // All float fields must be finite (no NaN / ±Infinity). Without this
    // guard a malicious author can write NaN to total_satisfaction, last_trust_score,
    // etc. A NaN propagated through checkpoint-consuming code would produce
    // unpredictable behaviour in any downstream computation.
    let float_fields = [
        checkpoint.contract_summary.current_debt,
        checkpoint.trust_summary.total_satisfaction,
        checkpoint.trust_summary.total_failure,
        checkpoint.trust_summary.last_trust_score,
        checkpoint.trust_summary.last_capacity,
    ];
    for &f in &float_fields {
        if !f.is_finite() {
            return Ok(ValidateCallbackResult::Invalid(
                checkpoint_validation_error::FLOAT_FIELD_NOT_FINITE.to_string(),
            ));
        }
    }
    // Trust score must be in [0, 1].
    if !(0.0..=1.0).contains(&checkpoint.trust_summary.last_trust_score) {
        return Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::FLOAT_FIELD_NOT_FINITE.to_string()));
    }
    // Capacity must be non-negative.
    if checkpoint.trust_summary.last_capacity < 0.0 {
        return Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::FLOAT_FIELD_NOT_FINITE.to_string()));
    }

    // Validate checkpoint chain integrity + evidence hash.
    // `prev_evidence_for_hash` is the ExternalHash to seed `compute_checkpoint_evidence_hash`.
    // It's `None` for the genesis checkpoint and `Some(prev.evidence_hash)` otherwise.
    let prev_evidence_for_hash: Option<ExternalHash> = if let Some(ref prev_hash) = checkpoint.prev_checkpoint_hash {
        // Fetch previous checkpoint to validate monotonicity and to
        // obtain its evidence_hash for the chain-binding verification.
        let prev_record = must_get_valid_record(ActionHash::from(prev_hash.clone()))?;
        let prev_checkpoint = prev_record.entry().to_app_option::<ChainCheckpoint>().ok().flatten();
        if let Some(prev_checkpoint) = prev_checkpoint {
            // Sequence must be exactly prev + 1
            if checkpoint.sequence != prev_checkpoint.sequence + 1 {
                return Ok(ValidateCallbackResult::Invalid(
                    checkpoint_validation_error::SEQUENCE_NOT_MONOTONIC.to_string(),
                ));
            }
            // Epoch must not decrease
            if checkpoint.epoch < prev_checkpoint.epoch {
                return Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::EPOCH_DECREASED.to_string()));
            }
            // Chain length must not decrease
            if checkpoint.chain_length < prev_checkpoint.chain_length {
                return Ok(ValidateCallbackResult::Invalid(
                    checkpoint_validation_error::CHAIN_LENGTH_DECREASED.to_string(),
                ));
            }
            Some(prev_checkpoint.evidence_hash)
        } else {
            // If the referenced entry isn't a ChainCheckpoint, treat as
            // missing and bail with an explicit error (prevents an
            // attacker linking to a non-checkpoint entry).
            return Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::PREV_NOT_CHECKPOINT.to_string()));
        }
    } else {
        // First checkpoint must have sequence 1
        if checkpoint.sequence != 1 {
            return Ok(ValidateCallbackResult::Invalid(
                checkpoint_validation_error::FIRST_CHECKPOINT_WRONG_SEQUENCE.to_string(),
            ));
        }
        None
    };

    // Evidence-hash binding: recompute from the committed fields and compare.
    // This is the critical anti-forgery check — a modified conductor cannot
    // publish a ChainCheckpoint with tampered summary fields without also
    // forging a new evidence_hash, which would be detected here.
    let expected_hash = compute_checkpoint_evidence_hash(prev_evidence_for_hash.as_ref(), &checkpoint);
    if checkpoint.evidence_hash != expected_hash {
        return Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::EVIDENCE_HASH_MISMATCH.to_string()));
    }

    Ok(ValidateCallbackResult::Valid)
}

/// Validate checkpoint updates (not allowed - checkpoints are immutable).
pub fn validate_update_checkpoint(
    _action: Update,
    _checkpoint: ChainCheckpoint,
    _original_action: EntryCreationAction,
    _original_checkpoint: ChainCheckpoint,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::UPDATE_NOT_ALLOWED.to_string()))
}

/// Validate checkpoint deletion (not allowed - checkpoints must be preserved).
pub fn validate_delete_checkpoint(
    _action: Delete,
    _original_action: EntryCreationAction,
    _original_checkpoint: ChainCheckpoint,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(checkpoint_validation_error::DELETE_NOT_ALLOWED.to_string()))
}

pub mod checkpoint_validation_error {
    pub const AUTHOR_NOT_AGENT: &str = "EV800000";
    pub const EPOCH_IN_FUTURE: &str = "EV800001";
    pub const INVALID_SEQUENCE: &str = "EV800002";
    pub const NEGATIVE_DEBT: &str = "EV800003";
    pub const UPDATE_NOT_ALLOWED: &str = "EV800004";
    pub const DELETE_NOT_ALLOWED: &str = "EV800005";
    pub const SEQUENCE_NOT_MONOTONIC: &str = "EV800006";
    pub const EPOCH_DECREASED: &str = "EV800007";
    pub const CHAIN_LENGTH_DECREASED: &str = "EV800008";
    pub const FIRST_CHECKPOINT_WRONG_SEQUENCE: &str = "EV800009";
    /// A float field in the checkpoint (total_satisfaction, total_failure,
    /// last_trust_score, last_capacity, or current_debt) is NaN, ±Infinity,
    /// out of range, or negative where non-negative is required.
    pub const FLOAT_FIELD_NOT_FINITE: &str = "EV800010";
    /// evidence_hash does not match the hash recomputed from the committed
    /// fields. The checkpoint's summary fields, last_action_hash, or
    /// prev_evidence_hash chaining have been tampered with.
    pub const EVIDENCE_HASH_MISMATCH: &str = "EV800011";
    /// The prev_checkpoint_hash points to an action whose target entry is
    /// not a ChainCheckpoint. Attempts to link a checkpoint to an arbitrary
    /// entry are rejected.
    pub const PREV_NOT_CHECKPOINT: &str = "EV800012";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contract_summary_default() {
        let summary = ContractSummary::default();
        assert_eq!(summary.total_created, 0);
        assert_eq!(summary.total_transferred, 0);
        assert_eq!(summary.total_expired, 0);
        assert_eq!(summary.total_archived, 0);
        assert_eq!(summary.current_debt, 0.0);
    }

    #[test]
    fn test_trust_summary_default() {
        let summary = TrustSummary::default();
        assert_eq!(summary.acquaintance_count, 0);
        assert_eq!(summary.total_satisfaction, 0.0);
        assert_eq!(summary.total_failure, 0.0);
        assert_eq!(summary.last_trust_score, 0.0);
        assert_eq!(summary.last_capacity, BASE_CAPACITY);
    }

    #[test]
    fn test_contract_summary_equality() {
        let summary1 = ContractSummary {
            total_created: 10,
            total_transferred: 5,
            total_expired: 2,
            total_archived: 3,
            current_debt: 100.0,
        };

        let summary2 = ContractSummary {
            total_created: 10,
            total_transferred: 5,
            total_expired: 2,
            total_archived: 3,
            current_debt: 100.0,
        };

        assert_eq!(summary1, summary2);

        let summary3 = ContractSummary { total_created: 11, ..summary1.clone() };
        assert_ne!(summary1, summary3);
    }

    #[test]
    fn test_trust_summary_equality() {
        let summary1 = TrustSummary {
            acquaintance_count: 10,
            total_satisfaction: 500.0,
            total_failure: 50.0,
            last_trust_score: 0.8,
            last_capacity: 2000.0,
        };

        let summary2 = TrustSummary {
            acquaintance_count: 10,
            total_satisfaction: 500.0,
            total_failure: 50.0,
            last_trust_score: 0.8,
            last_capacity: 2000.0,
        };

        assert_eq!(summary1, summary2);

        let summary3 = TrustSummary { acquaintance_count: 15, ..summary1.clone() };
        assert_ne!(summary1, summary3);
    }

    #[test]
    fn test_contract_summary_clone() {
        let summary = ContractSummary {
            total_created: 10,
            total_transferred: 5,
            total_expired: 2,
            total_archived: 3,
            current_debt: 100.0,
        };

        let cloned = summary.clone();
        assert_eq!(summary, cloned);
    }

    #[test]
    fn test_trust_summary_clone() {
        let summary = TrustSummary {
            acquaintance_count: 10,
            total_satisfaction: 500.0,
            total_failure: 50.0,
            last_trust_score: 0.8,
            last_capacity: 2000.0,
        };

        let cloned = summary.clone();
        assert_eq!(summary, cloned);
    }

    #[test]
    fn test_validation_error_codes() {
        // Ensure error codes are unique and properly formatted
        let codes = [
            checkpoint_validation_error::AUTHOR_NOT_AGENT,
            checkpoint_validation_error::EPOCH_IN_FUTURE,
            checkpoint_validation_error::INVALID_SEQUENCE,
            checkpoint_validation_error::NEGATIVE_DEBT,
            checkpoint_validation_error::UPDATE_NOT_ALLOWED,
            checkpoint_validation_error::DELETE_NOT_ALLOWED,
            checkpoint_validation_error::SEQUENCE_NOT_MONOTONIC,
            checkpoint_validation_error::EPOCH_DECREASED,
            checkpoint_validation_error::CHAIN_LENGTH_DECREASED,
            checkpoint_validation_error::FIRST_CHECKPOINT_WRONG_SEQUENCE,
        ];

        // All codes should start with EV8 (checkpoint error namespace)
        for code in &codes {
            assert!(code.starts_with("EV8"), "Error code {code} should start with EV8");
        }

        // All codes should be unique
        let mut unique_codes = std::collections::HashSet::new();
        for code in &codes {
            assert!(unique_codes.insert(*code), "Duplicate error code: {code}");
        }
    }
}
