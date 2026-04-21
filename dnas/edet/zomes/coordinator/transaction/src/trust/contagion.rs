use std::collections::HashSet;

use hdk::prelude::*;
use transaction_integrity::*;

// =========================================================================
//  Failure Observation Contagion (Witness-Based Tau Penalty)
//
//  When a creditor observes a debtor default (F > S), they publish a failure
//  observation to the DHT. Other nodes can query these observations to apply
//  a stricter tau_eff for the debtor, even without direct bilateral evidence.
//
//  This addresses selective defaulting attacks: an attacker who defaults on
//  some creditors but trades honestly with others would normally retain trust
//  from the honest partners. With contagion, community observations propagate.
// =========================================================================

/// Root path prefix for failure observation index (reverse lookup).
/// Per-debtor anchor: "failure_observations/{debtor_b64}" avoids scanning
/// the entire global observation set (O(N_global)) on every witness query.
pub(crate) const FAILURE_OBSERVATION_INDEX_PREFIX: &str = "failure_observations";

/// Publish a failure observation to the DHT with global index for reverse lookup.
/// Called when a contract expires (debtor defaulted).
///
/// The `creditor` parameter is the agent who suffered the default (the witness).
/// This function is typically called from the debtor's cell (which processes expirations),
/// so we must pass the creditor explicitly instead of using agent_info().
///
/// Creates two links:
/// 1. AgentToFailureObservation: creditor -> debtor with `expired_contract_hash` in tag
///    (verifiable proof of default; integrity validation checks the contract on DHT)
/// 2. FailureObservationIndex: path -> debtor with creditor in tag (reverse lookup)
pub fn publish_failure_observation(
    creditor: AgentPubKey,
    debtor: AgentPubKey,
    amount: f64,
    epoch: u64,
    expired_contract_hash: ActionHash,
    witness_bilateral_rate: f64,
) -> ExternResult<()> {
    // 1. AgentToFailureObservation: creditor -> debtor with proof in tag
    let obs_tag = transaction_integrity::types::FailureObservationTag {
        amount,
        epoch,
        expired_contract_hash: expired_contract_hash.clone(),
        witness_bilateral_rate,
    };
    let obs_tag_bytes = SerializedBytes::try_from(obs_tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    create_link(
        creditor.clone(),
        debtor.clone(),
        LinkTypes::AgentToFailureObservation,
        LinkTag(obs_tag_bytes.bytes().clone()),
    )?;

    // 2. FailureObservationIndex: per-debtor path -> debtor for reverse lookup
    let debtor_key: AgentPubKeyB64 = debtor.clone().into();
    let path = Path::from(format!("{FAILURE_OBSERVATION_INDEX_PREFIX}/{debtor_key}"));
    let typed_path = path.typed(LinkTypes::FailureObservationIndex)?;
    typed_path.ensure()?;

    let index_tag = transaction_integrity::types::FailureObservationIndexTag {
        creditor: creditor.into(),
        epoch,
        witness_bilateral_rate,
    };
    let index_tag_bytes =
        SerializedBytes::try_from(index_tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    create_link(
        typed_path.path_entry_hash()?,
        debtor,
        LinkTypes::FailureObservationIndex,
        LinkTag(index_tag_bytes.bytes().clone()),
    )?;

    Ok(())
}

/// Get failure witnesses for a debtor via per-debtor index path.
/// Returns list of unique creditors who have observed this debtor default.
#[hdk_extern]
pub fn get_failure_witnesses(debtor: AgentPubKey) -> ExternResult<Vec<AgentPubKeyB64>> {
    let debtor_key: AgentPubKeyB64 = debtor.clone().into();
    let path = Path::from(format!("{FAILURE_OBSERVATION_INDEX_PREFIX}/{debtor_key}"));
    let typed_path = path.typed(LinkTypes::FailureObservationIndex)?;

    // Check if path exists
    let path_hash = match typed_path.path_entry_hash() {
        Ok(h) => h,
        Err(_) => return Ok(Vec::new()), // No observations published yet
    };

    let links = get_links(LinkQuery::try_new(path_hash, LinkTypes::FailureObservationIndex)?, GetStrategy::default())?;

    let mut witnesses: Vec<AgentPubKeyB64> = Vec::new();
    let mut seen_creditors: HashSet<AgentPubKeyB64> = HashSet::new();

    for link in links {
        // All links under this per-debtor path are for our target debtor,
        // so we only need to extract and deduplicate the creditor from the tag.
        let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.into_inner()));
        if let Ok(index_tag) = transaction_integrity::types::FailureObservationIndexTag::try_from(tag_bytes) {
            if !seen_creditors.contains(&index_tag.creditor) {
                seen_creditors.insert(index_tag.creditor.clone());
                witnesses.push(index_tag.creditor);
            }
        }
    }

    Ok(witnesses)
}

/// Compute the median bilateral failure rate across all failure witnesses for a debtor.
/// Returns 0.0 if fewer than MIN_CONTAGION_WITNESSES exist.
///
/// This closes the selective defaulting gap: when the observer has zero bilateral
/// failures with the debtor, the median of witnesses' bilateral rates (discounted
/// by WITNESS_DISCOUNT in the attenuation function) provides a nonzero floor for r_eff.
#[hdk_extern]
pub fn get_aggregate_witness_rate(debtor: AgentPubKey) -> ExternResult<f64> {
    let (_, rate) = get_witness_contagion_data(&debtor)?;
    Ok(rate)
}

/// Get count of failure witnesses for a debtor (for contagion penalty calculation).
/// Only counts observations from the last WITNESS_RELEVANCE_EPOCHS epochs (= 100,
/// now defined in constants.rs as WITNESS_RELEVANCE_EPOCHS) to prevent
/// permanently penalising reformed debtors.
pub fn get_failure_witness_count(debtor: &AgentPubKey) -> ExternResult<u32> {
    let (count, _) = get_witness_contagion_data(debtor)?;
    Ok(count)
}

/// Epochs beyond which failure observations are considered stale.
// WITNESS_RELEVANCE_EPOCHS is now defined in transaction_integrity::types::constants.
// The value 100 (epochs ≈ 100 days) is still used below via the constants module.
/// Combined witness contagion query: returns `(witness_count, aggregate_rate)` with
/// a **single** `get_links` call. This replaces the previous separate
/// `get_failure_witness_count` and `get_aggregate_witness_rate_inner` functions
/// which each independently fetched the same links — a 2× redundancy.
///
/// The result should be cached in `TrustCache.witness_contagion` to avoid
/// repeated DHT queries for the same debtor within a trust computation cycle.
///
/// - `witness_count`: number of unique recent creditors who observed the debtor default.
/// - `aggregate_rate`: median of witnesses' bilateral F/(S+F) rates, or 0.0 if
///   fewer than `MIN_CONTAGION_WITNESSES` witnesses exist.
pub fn get_witness_contagion_data(debtor: &AgentPubKey) -> ExternResult<(u32, f64)> {
    use transaction_integrity::types::constants::MIN_CONTAGION_WITNESSES;

    let debtor_key: AgentPubKeyB64 = debtor.clone().into();
    let path = Path::from(format!("{FAILURE_OBSERVATION_INDEX_PREFIX}/{debtor_key}"));
    let typed_path = path.typed(LinkTypes::FailureObservationIndex)?;

    let path_hash = match typed_path.path_entry_hash() {
        Ok(h) => h,
        Err(_) => return Ok((0, 0.0)),
    };

    let links = get_links(LinkQuery::try_new(path_hash, LinkTypes::FailureObservationIndex)?, GetStrategy::default())?;

    let current_epoch = if let Ok(now) = sys_time() {
        transaction_integrity::types::timestamp_to_epoch(now)
    } else {
        // On clock failure, return (0, 0.0) instead of the full link
        // count as witnesses.
        // Returning 0 witnesses is the safe conservative choice: it means "we
        // cannot assess contagion right now" and leaves τ_eff at its bilateral
        // value rather than spiking it to effectively zero.
        warn!("contagion: sys_time() failed — returning 0 witnesses (fail-safe)");
        return Ok((0, 0.0));
    };

    let cutoff_epoch = current_epoch.saturating_sub(transaction_integrity::types::constants::WITNESS_RELEVANCE_EPOCHS);
    let mut rates: Vec<f64> = Vec::new();
    let mut seen_creditors: HashSet<AgentPubKeyB64> = HashSet::new();

    for link in links {
        let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.into_inner()));
        if let Ok(index_tag) = transaction_integrity::types::FailureObservationIndexTag::try_from(tag_bytes) {
            if index_tag.epoch >= cutoff_epoch && !seen_creditors.contains(&index_tag.creditor) {
                seen_creditors.insert(index_tag.creditor);
                rates.push(index_tag.witness_bilateral_rate);
            }
        }
    }

    let witness_count = seen_creditors.len().min(u32::MAX as usize) as u32;

    // Aggregate rate: median of bilateral rates if enough witnesses
    let aggregate_rate = if (rates.len() as u32) >= MIN_CONTAGION_WITNESSES {
        rates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = rates.len() / 2;
        if rates.len() % 2 == 1 {
            rates[mid]
        } else {
            (rates[mid - 1] + rates[mid]) / 2.0
        }
    } else {
        0.0
    };

    Ok((witness_count, aggregate_rate))
}
