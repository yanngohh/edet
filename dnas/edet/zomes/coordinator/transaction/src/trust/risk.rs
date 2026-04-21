use std::marker::PhantomData;

use hdk::prelude::*;
use transaction_integrity::debt_contract::DebtContract;
use transaction_integrity::types::constants::*;
use transaction_integrity::types::timestamp_to_epoch;
use transaction_integrity::types::TransactionStatusTag;
use transaction_integrity::*;

use crate::contracts;
use crate::vouch::get_vouched_capacity;

use super::reputation::{compute_risk_from_claim, get_reputation_claim, is_claim_fresh};
use crate::capacity::compute_credit_capacity;

// =========================================================================
//  Risk Score (Whitepaper Definition 16 / Section 5.3)
//
//  R_s(b, delta) = 1 - t_hat_b * (Cap_b - Debt_b) / Cap_b * lambda_b
//
//  lambda_b = 0.5 + 0.5 * min(1, D_out_b / D_in_b)
//
//  D_in_b  = total debt acquired by b in the current epoch (contracts created as debtor)
//  D_out_b = total debt extinguished by b in the current epoch (same-epoch Transferred contracts)
//
//  Same-epoch approximation: D_out uses contracts created AND Transferred within the
//  current epoch. This is conservative (understates D_out), which means lambda_b is
//  slightly pessimistic for agents who repay across epoch boundaries -- acceptable
//  because cross-epoch repayment is rare and the bound is provably conservative.
// =========================================================================

/// Compute the full risk score for a transaction from the seller's perspective.
/// R = 1 - t_hat_b * (Cap_b - Debt_b) / Cap_b * lambda_b
///
/// Returns value in [0, 1]. 0 = zero risk, 1 = maximum risk.
///
/// `seller_reject_threshold`: the seller's configured `auto_reject_threshold`.
/// Used to cap fallback "blind observer" and "graduated n_S=0" cases to the
/// Pending band without hardcoding WALLET_DEFAULT_AUTO_REJECT_THRESHOLD
/// (sellers with custom, stricter thresholds are respected).
pub fn compute_full_risk_score(
    buyer: AgentPubKey,
    observer: AgentPubKey,
    seller_reject_threshold: f64,
) -> ExternResult<f64> {
    // SECURITY/OPTIMIZATION: Self-risk is always zero.
    // This also ensures that drain transactions (where beneficiary=buyer)
    // are auto-accepted by the beneficiary without triggering a blind-observer
    // fallback to Pending status.
    if buyer == observer {
        debug!("compute_full_risk_score: self-risk bypass for agent {}", buyer);
        return Ok(0.0);
    }

    // Determine which path to take: Path 1 (Claim) or Path 2 (Full EigenTrust)
    let bilateral = super::check_bilateral_history_as_observer(buyer.clone(), observer.clone())?;
    debug!("compute_full_risk_score: bilateral_history={}", bilateral);
    if bilateral {
        // PATH 2: Repeat transaction - use full subjective EigenTrust
        compute_risk_path_2_as_observer(buyer, observer)
    } else {
        // PATH 1: First-contact - try to use ReputationClaim O(1)
        let claim_result = get_reputation_claim(buyer.clone())?;
        let claim_fresh = claim_result.as_ref().map(|(_, c)| is_claim_fresh(c)).transpose()?;
        debug!("compute_full_risk_score: claim_exists={}, claim_fresh={:?}", claim_result.is_some(), claim_fresh);
        match claim_result {
            Some((_, claim)) if claim_fresh == Some(true) => {
                let current_debt = contracts::get_total_debt(buyer.clone())?;
                let score = compute_risk_from_claim(&claim, current_debt);
                debug!("compute_full_risk_score: PATH 1 claim score={}, n_S={}", score, claim.successful_transfers);
                // Claim-based score is 1.0 when n_S=0 (whitepaper: no completed transfers).
                // But a buyer with active contracts (graduated, not fresh newcomer) and
                // positive vouched capacity is not a cold-starter — they have real economic
                // skin in the game. Auto-rejecting them conflates "unproven" with "bad actor".
                // Cap to Pending band so the seller can manually review.
                // score=1.0 when n_S=0. But if the buyer has active debt (current_debt>0),
                // they are a graduated agent (DebtContract exists, just not yet repaid) —
                // not a cold-starter. Cap to Pending so the seller can decide manually.
                // We do NOT check capacity_lower_bound here because unvouched buyers have
                // cap=0 yet can still be graduated (their trial was approved, debt is real).
                if score >= 1.0 && current_debt > 0.0 {
                    debug!(
                        "compute_full_risk_score: graduated buyer n_S=0 with active debt={}, capping to Pending score",
                        current_debt
                    );
                    // Use the seller's configured reject threshold, not a global default.
                    return Ok(seller_reject_threshold - f64::EPSILON);
                }
                Ok(score)
            }
            _ => {
                // No valid claim: fallback to PATH 2.
                // PATH 2 gives risk = 1 - rel_trust * ...; if rel_trust == 0 the result is
                // always 1.0, which auto-rejects. rel_trust is 0 whenever the observer's
                // subgraph yields trust=0 for the buyer — this happens both when the seller
                // has zero acquaintances AND when the seller's subgraph is too small (<2 nodes).
                // In either case the seller has no meaningful signal about the buyer.
                // Auto-rejecting in the absence of signal conflates "unknown" with "bad actor".
                // Cap the score in the Pending band so the seller can manually review.
                let rep = super::get_subjective_reputation_as_observer(buyer.clone(), observer.clone())?;
                debug!("compute_full_risk_score: PATH 2 fallback, observer_trust_for_buyer={}", rep.trust);
                if rep.trust == 0.0 {
                    debug!("compute_full_risk_score: blind observer (trust=0 for buyer), forcing Pending score");
                    // Use seller's configured reject threshold
                    return Ok(seller_reject_threshold - f64::EPSILON);
                }
                compute_risk_path_2_as_observer(buyer, observer)
            }
        }
    }
}

/// PATH 2: Full EigenTrust Risk Score
/// R_s(b, delta) = 1 - t_hat_b * (Cap_b - Debt_b) / Cap_b * lambda_b
fn compute_risk_path_2_as_observer(buyer: AgentPubKey, observer: AgentPubKey) -> ExternResult<f64> {
    let rep = super::get_subjective_reputation_as_observer(buyer.clone(), observer)?;
    let vouched = get_vouched_capacity(buyer.clone())?;
    let base = vouched.max(0.0);
    // Whitepaper Def 2.4 uses the TARGET's (buyer's) acquaintance count
    // for the saturation factor (1 - exp(-|A_buyer|/n0)). The `rep.acquaintance_count`
    // returned by `get_subjective_reputation_as_observer` is the OBSERVER's (seller's)
    // count because the subgraph is built from the observer's perspective.
    // We fetch the buyer's acquaintance count via DHT link query for the capacity formula.
    let buyer_acq_count = {
        let links =
            get_links(LinkQuery::try_new(buyer.clone(), LinkTypes::AgentToAcquaintance)?, GetStrategy::default())
                .unwrap_or_default();
        if links.is_empty() {
            // Fallback to observer's count on DHT miss — conservative (over-estimates
            // capacity only when observer has more acquaintances than buyer, which is
            // the common case for new buyers, producing a more forgiving risk score).
            rep.acquaintance_count
        } else {
            links.len()
        }
    };
    let cap = compute_credit_capacity(rep.trust, buyer_acq_count, base);
    let current_debt = contracts::get_total_debt(buyer.clone())?;

    let remaining_ratio = if cap > 0.0 { ((cap - current_debt) / cap).max(0.0) } else { 0.0 };

    // Normalize trust via saturating sigmoid on relative reputation.
    let t_baseline = EIGENTRUST_ALPHA / rep.acquaintance_count.max(1) as f64;
    let rel_rep = if t_baseline > 0.0 { rep.trust / t_baseline } else { 0.0 };
    let rel_trust = rel_rep / (rel_rep + RISK_SIGMOID_K);

    // Debt velocity factor lambda_b (Whitepaper Definition 16):
    // Penalizes pure buyers whose debt grows faster than they extinguish it.
    //
    // D_in and D_out are computed from TWO SEPARATE QUERIES so that
    // long-maturity contracts (created in a previous epoch, transferred today)
    // correctly contribute to D_out without affecting D_in.
    //
    //   D_in  = Σ original_amount  for contracts with start_epoch == current_epoch
    //           (debt *acquired* this epoch, regardless of current transfer status)
    //   D_out = Σ (original - residual) for contracts whose latest update is
    //           ContractStatus::Transferred AND whose update action epoch == current_epoch
    //           (debt *extinguished* this epoch, regardless of when they were created)
    //
    // Previously both used the same `get_contracts_in_epoch_range(current, current)`
    // query, which missed old contracts transferred today (because their start_epoch
    // was earlier) and thus under-reported D_out — making lambda_b ≈ 1 for most honest
    // traffic and effectively disabling the build-then-betray detection.
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    // D_in: contracts created this epoch (start_epoch == current_epoch).
    // resolve_latest=false: we want original create records (original_amount accuracy).
    let new_contracts = contracts::get_contracts_in_epoch_range(buyer.clone(), current_epoch, current_epoch, false)?;
    let d_in: f64 = new_contracts
        .iter()
        .filter_map(|r| r.entry().to_app_option::<DebtContract>().ok().flatten())
        .map(|c| c.original_amount)
        .sum();

    // D_out: contracts transferred (extinguished) this epoch.
    // We fetch ALL contracts as debtor, resolve each to latest, and keep those
    // that are currently Transferred AND whose last-update action falls in the
    // current epoch. This uses the DebtorToContracts index (all contracts ever)
    // rather than the epoch-bucket index so we catch old contracts repaid today.
    let d_out: f64 = {
        let all_links =
            get_links(LinkQuery::try_new(buyer.clone(), LinkTypes::DebtorToContracts)?, GetStrategy::default())
                .unwrap_or_default();
        let mut total = 0.0f64;
        for link in &all_links {
            if let Some(hash) = link.target.clone().into_action_hash() {
                if let Ok(Some(record)) = contracts::get_latest_debt_contract_record(hash) {
                    if let Some(c) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
                        if c.status == transaction_integrity::debt_contract::ContractStatus::Transferred {
                            // Check that the update action's epoch matches current_epoch.
                            let update_epoch = timestamp_to_epoch(record.action().timestamp());
                            if update_epoch == current_epoch {
                                total += (c.original_amount - c.amount).max(0.0);
                            }
                        }
                    }
                }
            }
        }
        total
    };
    let lambda_b = if d_in > 0.0 { 0.5 + 0.5 * (d_out / d_in).min(1.0) } else { 1.0 };

    let risk_score = 1.0 - rel_trust * remaining_ratio * lambda_b;
    info!(
        "compute_risk_path_2: trust={}, acquaintances={}, vouched={}, cap={}, debt={}, rem_ratio={}, rel_rep={}, rel_trust={}, d_in={}, d_out={}, lambda_b={}, risk={}",
        rep.trust, rep.acquaintance_count, vouched, cap, current_debt, remaining_ratio, rel_rep, rel_trust, d_in, d_out, lambda_b, risk_score
    );

    Ok(risk_score)
}

/// Check if a transaction amount qualifies as a trial-sized amount.
/// This tests the AMOUNT threshold only (amount < eta * V_base).
/// For PATH 0 eligibility (bypassing risk assessment), the buyer must ALSO
/// pass `is_bootstrap_eligible` — see `compute_transaction_status`.
pub fn is_trial_transaction(amount: f64) -> bool {
    amount < TRIAL_FRACTION * BASE_CAPACITY
}

/// Zome extern: check if a buyer is bootstrap-eligible (PATH 0 candidate).
///
/// This is the UI-facing version. It adds one extra check on top of the
/// internal `is_bootstrap_eligible` check:
///
/// Pending-trial scan: hides the banner immediately after the buyer creates
/// a trial (before seller approval). Intentionally NOT in the internal check
/// because the whitepaper allows simultaneous trials to DIFFERENT sellers
/// while n_S == 0.
///
/// Once the trial is approved and a DebtContract exists, `is_bootstrap_eligible`
/// itself returns false (DebtorToContracts check), so no extra handling is needed.
#[hdk_extern]
pub fn check_bootstrap_eligible(buyer: AgentPubKey) -> ExternResult<bool> {
    // First apply the authoritative internal check (whitepaper definition).
    if !is_bootstrap_eligible(buyer.clone())? {
        return Ok(false);
    }
    // UI extra 1: if the buyer already has a pending trial in-flight, hide the banner.
    use crate::ranking_index::{GetRankingCursor, GetRankingDirection};
    use crate::transaction::get_wallet_transactions_index;
    let pending_tag = SerializedBytes::try_from(TransactionStatusTag::Pending)
        .map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    let pending = get_wallet_transactions_index()?.get_ranking_chunk(
        GetRankingDirection::Descendent,
        10,
        Some(GetRankingCursor {
            from_ranking: sys_time()?.as_millis(),
            tag: Some(pending_tag),
            tag_type: PhantomData::<TransactionStatusTag>,
            agent_pubkey: buyer.clone(),
        }),
    )?;
    for hashes in pending.values() {
        for hash in hashes {
            if let Some(dht_hash) = hash.hash.clone().into_any_dht_hash() {
                if let Some(record) = get(dht_hash, GetOptions::default())? {
                    if let Some(tx) = record.entry().to_app_option::<Transaction>().ok().flatten() {
                        if tx.is_trial && Into::<AgentPubKey>::into(tx.buyer.pubkey.clone()) == buyer {
                            return Ok(false);
                        }
                    }
                }
            }
        }
    }
    Ok(true)
}

/// Internal: check if a buyer is bootstrap-eligible (PATH 0 candidate).
///
/// A buyer is bootstrap-eligible iff they have no economic footprint:
///   - effective_cap == 0 (unvouched or fully slashed), OR
///   - no debt contracts exist yet (no economic history at all).
///
/// Once a buyer has BOTH cap > 0 AND any debt contract on record (Active,
/// Transferred, or Expired), they are a graduated participant and small
/// transactions go through PATH 1/2 with full risk assessment.
///
/// n_S counts successful transfers as DEBTOR (buyer role only).
/// Being a creditor (seller) does NOT affect bootstrap eligibility —
/// sellers who accepted trials are NOT graduated as buyers.
///
/// Evaluation priority:
///   1. Fast path: if a fresh ReputationClaim with n_S > 0 exists → not eligible.
///   2. Fallback: scan DebtorToContracts links directly. Any contract (regardless
///      of status) means the buyer has economic history. This covers the window
///      between first trial acceptance and first ReputationClaim publication.
///
/// This is O(links) for the vouch check + O(1) claim lookup + O(contracts) fallback.
pub fn is_bootstrap_eligible(buyer: AgentPubKey) -> ExternResult<bool> {
    // Whitepaper §5.3: Bootstrap-eligible iff Cap_b == 0 OR n_S == 0.
    // Unvouched agents (Cap_b = 0) are always eligible for trials because they
    // have 0 non-trial capacity.
    let vouched_cap = get_vouched_capacity(buyer.clone())?;
    if vouched_cap <= 0.0 {
        return Ok(true);
    }

    // High-capacity agents: check successful transfer history (n_S).
    // Fast path: claim exists with confirmed transfer history -> graduated.
    if let Some((_, claim)) = get_reputation_claim(buyer.clone())? {
        if claim.successful_transfers > 0 {
            return Ok(false);
        }
    }

    // Whitepaper §5.3: eligibility requires n_S == 0 (zero SUCCESSFUL transfers),
    // not just "any contract exists". The previous fallback used any DebtorToContracts
    // link (including Active unrepaid contracts) to graduate a buyer. This blocked buyers
    // who had an accepted-but-not-yet-repaid trial from further trials even though they
    // technically still have n_S == 0 per the whitepaper definition.
    //
    // New logic: scan DebtorToContracts links and resolve to latest. A buyer is
    // graduated (not eligible) only if at least one contract has status == Transferred.
    let debtor_links =
        get_links(LinkQuery::try_new(buyer.clone(), LinkTypes::DebtorToContracts)?, GetStrategy::default())?;
    for link in &debtor_links {
        if let Some(hash) = link.target.clone().into_action_hash() {
            if let Ok(Some(record)) = contracts::get_latest_debt_contract_record(hash) {
                if let Some(c) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
                    if c.status == transaction_integrity::debt_contract::ContractStatus::Transferred {
                        // n_S > 0: buyer has at least one successful transfer history.
                        return Ok(false);
                    }
                }
            }
        }
    }

    // Final fallback: a DebtorToBlockedTrialSeller link means the buyer defaulted on a
    // past trial. Even if the expired contract has been archived (removing the
    // DebtorToContracts link), the permanent block link persists. A buyer with any
    // blocked pair is not bootstrap-eligible — they have established economic history.
    let blocked_links =
        get_links(LinkQuery::try_new(buyer.clone(), LinkTypes::DebtorToBlockedTrialSeller)?, GetStrategy::default())?;
    if !blocked_links.is_empty() {
        return Ok(false);
    }

    // No debt history and no transfers — bootstrap eligible.
    Ok(true)
}

/// Compute the full transaction status based on EigenTrust risk assessment.
///
/// The `seller_wallet` parameter is used for risk thresholds (auto_accept/auto_reject)
/// and for trial velocity enforcement (trial_tx_count, last_trial_epoch).
pub fn compute_transaction_status(
    buyer: AgentPubKey,
    debt: f64,
    seller_wallet: &Wallet,
    observer: AgentPubKey,
    current_epoch: u64,
) -> ExternResult<TransactionStatus> {
    debug!("compute_transaction_status: buyer={}, observer={}, debt={}", buyer, observer, debt);
    // PATH 0: Trial transactions bypass risk assessment (Theorem 3: Fair Bootstrapping).
    //
    // PATH 0 fires ONLY for buyers with no economic footprint:
    //   amount < eta * V_base  AND  (effective_cap == 0 OR n_S_global == 0).
    // Once a buyer has both capacity and transfer history, even small transactions
    // go through PATH 1/2 with full risk assessment.
    if is_trial_transaction(debt) && is_bootstrap_eligible(buyer.clone())? {
        // Enforce trial velocity limit to prevent Sybil flood attacks.
        let effective_count =
            if seller_wallet.last_trial_epoch == current_epoch { seller_wallet.trial_tx_count } else { 0 };
        if effective_count >= TRIAL_VELOCITY_LIMIT_PER_EPOCH {
            debug!(
                "Trial velocity limit exceeded: count={}, limit={}",
                effective_count, TRIAL_VELOCITY_LIMIT_PER_EPOCH
            );
            return Ok(TransactionStatus::Rejected);
        }
        // Trial transactions are ALWAYS Pending — the seller must approve manually.
        // Returning Accepted here would allow any caller of compute_transaction_status
        // (including get_transaction_status_from_simulation) to auto-accept trials,
        // bypassing the mandatory seller review that PATH 0 requires.
        // The caller (create_transaction) stamps is_trial=true and enforces Pending
        // status, but that guard must not be the only protection.
        return Ok(TransactionStatus::Pending);
    }

    let risk_score = compute_full_risk_score(buyer, observer, seller_wallet.auto_reject_threshold)?;
    let status = TransactionStatus::from_risk_score_for_wallet(risk_score, seller_wallet.clone());
    debug!(
        "Standard risk assessment: risk_score={}, accept_threshold={}, reject_threshold={}, result={:?}",
        risk_score, seller_wallet.auto_accept_threshold, seller_wallet.auto_reject_threshold, status
    );
    Ok(status)
}

// =========================================================================
//  Public extern: get_risk_score
//
//  Exposed as a production zome function so the UI can display the risk score
//  for a given buyer as seen by the calling agent (observer = self).
//  Previously gated behind `#[cfg(feature = "test-epoch")]`; promoted to
//  production because WalletDetail.svelte calls it directly to show the
//  risk gauge and the function is read-only with no side effects.
// =========================================================================

/// Compute and return the full risk score for the given buyer as seen by the
/// calling agent (observer = self).
///
/// Returns a value in [0, 1]. 0 = zero risk, 1 = maximum risk.
#[hdk_extern]
pub fn get_risk_score(buyer: AgentPubKey) -> ExternResult<f64> {
    let observer = agent_info()?.agent_initial_pubkey;
    // Use default threshold for the UI/external call path (no seller wallet context here).
    compute_full_risk_score(buyer, observer, WALLET_DEFAULT_AUTO_REJECT_THRESHOLD)
}
