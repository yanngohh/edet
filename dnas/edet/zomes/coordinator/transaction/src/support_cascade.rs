use hdk::prelude::*;
// Release-build no-op logging macros are defined in lib.rs (#[macro_export]).
// Do NOT re-define them here — duplicate macro definitions cause shadowing
// and maintenance confusion. The crate-root macros are always in scope.

use transaction_integrity::types::constants::{
    coordinator_cascade_error, coordinator_support_error, DUST_THRESHOLD, MAX_CASCADE_DEPTH,
};
use transaction_integrity::{DrainMetadata, TransactionStatus};

use crate::contracts;

/// Alias to the canonical DUST_THRESHOLD constant for cascade operations.
/// Amounts at or below this are treated as dust and ignored to prevent
/// infinite spillover loops on floating-point residuals.
const CASCADE_DUST_THRESHOLD: f64 = DUST_THRESHOLD;

/// Maximum number of waterfilling passes before the cascade is forcibly terminated
/// and the remaining balance becomes genesis debt.
const CASCADE_MAX_ITERATIONS: usize = 100;

// =========================================================================
//  Support Cascade (Whitepaper Section 5.2 — Recursive Spillover Cascading)
//
//  PHASE 1 (synchronous, on seller's cell during reify_side_effects):
//    1. The seller drains only their OWN debt pool (local transfer_debt call).
//    2. For each beneficiary in the breakdown: fire-and-forget a pending drain
//       Transaction on the beneficiary's cell via call_remote(create_drain_request).
//    3. Beneficiaries are immediately treated as "dry" for this cascade pass.
//       The remaining amount becomes genesis debt absorbed by the buyer's DebtContract.
//
//  PHASE 2 (async, on each beneficiary's cell after they moderate the drain):
//    When a beneficiary approves their drain tx (update_transaction → Accepted):
//      - reify_transaction_side_effects detects drain_metadata.is_some()
//      - Runs transfer_debt locally (reduces beneficiary's own existing debt)
//      - Fires pending drain txs to the beneficiary's own beneficiaries
//      - No new DebtContract is created — only existing contracts are reduced.
//
//  This gives beneficiaries full moderation power over drains while keeping the
//  originating transaction non-blocking. Unresolved drain amounts are genesis debt.
// =========================================================================

/// Result of a support cascade operation.
#[derive(Serialize, Deserialize, Debug)]
pub struct SupportCascadeResult {
    /// Amount transferred from seller's own contracts.
    pub own_transferred: f64,
    /// Amount sent as pending drain requests to beneficiaries (async, not yet resolved).
    pub beneficiary_requests_sent: f64,
    /// Amount of genesis debt created (remaining after own drain; beneficiary drains are async).
    pub genesis_amount: f64,
    /// Per-creditor S increments from seller's own debt transfer.
    pub own_creditor_transfers: Vec<(AgentPubKeyB64, f64)>,
    /// Per-beneficiary amounts for which drain requests were sent.
    pub beneficiary_drains: Vec<(AgentPubKeyB64, f64)>,
}

/// Input to create a pending drain transaction on a beneficiary's cell.
/// Sent via call_remote to the beneficiary's `create_drain_request` extern.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateDrainRequestInput {
    /// The supporter (buyer in the drain transaction) who is requesting the drain.
    pub requester: AgentPubKeyB64,
    /// The beneficiary (seller in the drain transaction, author of this cell).
    pub beneficiary: AgentPubKeyB64,
    /// Amount to drain.
    pub amount: f64,
    /// Drain metadata for the pending transaction.
    pub drain_metadata: DrainMetadata,
    /// The requester's own SupportBreakdown record for authentication.
    pub requester_breakdown_record: Record,
    /// Pre-computed status override (set by create_drain_request after risk eval).
    /// If None, create_drain_transaction will compute status itself.
    pub status: Option<TransactionStatus>,
}

/// Intermediate type to deserialize support breakdown from cross-zome call.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone)]
pub struct SupportBreakdownData {
    pub owner: AgentPubKeyB64,
    pub addresses: Vec<AgentPubKeyB64>,
    pub coefficients: Vec<f64>,
}

/// Execute the support cascade for a seller (Whitepaper Section 5.2).
///
/// Phase 1 only: drains the seller's OWN debt pool synchronously, then sends
/// fire-and-forget pending drain requests to each beneficiary via call_remote.
/// Beneficiaries are treated as immediately dry for the remaining-amount calculation
/// (their drains resolve asynchronously; unresolved amounts become genesis debt).
///
/// `visited` carries the cycle-detection set (agents already in the cascade chain).
/// `parent_tx` is the ActionHash of the originating buyer→seller transaction.
pub fn execute_support_cascade(
    seller: AgentPubKey,
    amount: f64,
    mut visited: Vec<AgentPubKeyB64>,
    parent_tx: ActionHash,
    cascade_depth: u32,
) -> ExternResult<SupportCascadeResult> {
    // Hard depth limit: each cascade hop fires a call_remote to the next cell.
    // Without this guard, a pathological acyclic support chain (A→B→C→…) of
    // unbounded length would consume proportional network resources per transaction.
    // Any uncleared amount at the depth limit becomes genesis debt on the buyer —
    // the same outcome as a "dry" support chain, so correctness is preserved.
    if cascade_depth >= MAX_CASCADE_DEPTH {
        warn!(
            "CASCADE depth limit {} reached at {}. Remaining {} becomes genesis debt.",
            MAX_CASCADE_DEPTH, seller, amount
        );
        return Ok(SupportCascadeResult {
            own_transferred: 0.0,
            own_creditor_transfers: vec![],
            beneficiary_requests_sent: 0.0,
            beneficiary_drains: vec![],
            genesis_amount: amount,
        });
    }
    let mut remaining = amount;
    let mut own_transferred = 0.0;
    let mut own_creditor_transfers: Vec<(AgentPubKeyB64, f64)> = Vec::new();
    let mut beneficiary_requests_sent = 0.0;
    let mut beneficiary_drains: Vec<(AgentPubKeyB64, f64)> = Vec::new();

    let seller_key: AgentPubKeyB64 = seller.clone().into();
    visited.push(seller_key.clone());

    info!("CASCADE [{}]: started — amount={}, depth={}, visited={:?}", seller_key, amount, cascade_depth, visited);

    // ── Build participant set W ──────────────────────────────────────────────
    let (breakdown, maybe_breakdown_record) = match get_support_breakdown_for_agent(seller.clone())? {
        Some((data, record)) => {
            info!(
                "CASCADE [{}]: breakdown FOUND — addresses={:?}, coefficients={:?}",
                seller_key, data.addresses, data.coefficients
            );
            (data, Some(record))
        }
        None => {
            info!("CASCADE [{}]: breakdown NOT FOUND — using default seller-only breakdown", seller_key);
            (
                SupportBreakdownData {
                    owner: seller_key.clone(),
                    addresses: vec![seller_key.clone()],
                    coefficients: vec![1.0],
                },
                None,
            )
        }
    };

    // (key, coef, is_seller)
    let mut active: Vec<(AgentPubKeyB64, f64, bool)> = Vec::new();
    for (i, addr) in breakdown.addresses.iter().enumerate() {
        let coef = breakdown.coefficients[i];
        if coef <= 0.0 {
            continue;
        }
        if *addr == seller_key {
            active.push((seller_key.clone(), coef, true));
        } else if !visited.contains(addr) {
            active.push((addr.clone(), coef, false));
        }
    }

    debug!("CASCADE [{}]: W = {:?}", seller_key, active.iter().map(|(k, c, s)| (k, *c, *s)).collect::<Vec<_>>());

    // ── Separate seller bucket from beneficiary buckets ──────────────────────
    let mut seller_entry: Option<(AgentPubKeyB64, f64)> = None;
    let mut beneficiary_entries: Vec<(AgentPubKeyB64, f64)> = Vec::new();

    for (key, coef, is_seller) in active {
        if is_seller {
            seller_entry = Some((key, coef));
        } else {
            beneficiary_entries.push((key, coef));
        }
    }

    // ── Step 1: Drain seller's OWN debt first (synchronous waterfill) ────────
    //
    // The seller absorbs as much of `remaining` as their debt allows.
    // Iterate like the old loop so multi-pass waterfilling still works for the
    // seller bucket (seller coef determines their priority share, then any
    // saturation spills to the next pass). When no beneficiaries exist the
    // entire genesis goes to the seller synchronously.
    if let Some((seller_node, seller_coef)) = seller_entry {
        let total_coef = seller_coef + beneficiary_entries.iter().map(|(_, c)| c).sum::<f64>();
        // Seller's proportional ceiling: what the seller would absorb if fully wet
        let seller_ceiling = if total_coef > 0.0 { remaining * (seller_coef / total_coef) } else { 0.0 };

        if seller_ceiling > CASCADE_DUST_THRESHOLD {
            debug!("CASCADE [{}]: own drain — target={}", seller_key, seller_ceiling);
            let own_result = contracts::transfer_debt((seller.clone(), seller_ceiling))?;
            let drained = own_result.transferred;
            debug!("CASCADE [{}]: own drain returned transferred={}", seller_key, drained);

            if drained > CASCADE_DUST_THRESHOLD {
                own_transferred += drained;
                remaining -= drained;
                for (creditor, amt) in own_result.creditor_transfers {
                    if let Some(e) = own_creditor_transfers.iter_mut().find(|(c, _)| *c == creditor) {
                        e.1 += amt;
                    } else {
                        own_creditor_transfers.push((creditor, amt));
                    }
                }
            }

            // If the seller fully absorbed their ceiling, try additional passes
            // (multi-pass waterfill: seller stays in the pool if they could take more).
            // In practice this handles cases where the seller's debt pool is not
            // yet exhausted — keep draining until dry.
            let mut iters = 0;
            while drained >= seller_ceiling - CASCADE_DUST_THRESHOLD
                && remaining > CASCADE_DUST_THRESHOLD
                && iters < CASCADE_MAX_ITERATIONS
            {
                iters += 1;
                debug!("CASCADE [{}]: seller wet — extra drain pass, remaining={}", seller_key, remaining);
                let extra = contracts::transfer_debt((seller.clone(), remaining))?;
                if extra.transferred <= CASCADE_DUST_THRESHOLD {
                    break;
                }
                own_transferred += extra.transferred;
                remaining -= extra.transferred;
                for (creditor, amt) in extra.creditor_transfers {
                    if let Some(e) = own_creditor_transfers.iter_mut().find(|(c, _)| *c == creditor) {
                        e.1 += amt;
                    } else {
                        own_creditor_transfers.push((creditor, amt));
                    }
                }
            }
        }
        let _ = seller_node; // silence unused warning
    }

    // ── Step 2: Distribute remainder to beneficiaries ────────────────────────
    //
    // After the seller absorbs what they can, `remaining` is the unabsorbed
    // surplus. This is split among beneficiaries in proportion to their
    // relative coefficients (seller quota no longer in denominator — it has
    // already been consumed or found dry).
    //
    // Waterfill semantics: if A has 0 debt and B has coefficient 1.0, B gets
    // min(remaining, B_available_debt). The rest becomes genesis.
    if remaining > CASCADE_DUST_THRESHOLD && !beneficiary_entries.is_empty() {
        let total_bcoef: f64 = beneficiary_entries.iter().map(|(_, c)| c).sum();
        if total_bcoef > 0.0 {
            if let Some(ref breakdown_record) = maybe_breakdown_record {
                // Multi-pass waterfill within a single cell's beneficiary bucket.
                // The whitepaper §5.2 waterfilling algorithm re-allocates a "dry"
                // beneficiary's unmet quota to the remaining "wet" beneficiaries.
                //
                // Phase-1 single-pass: allocate proportionally by coefficient.
                // Then collect failed (dry) beneficiaries and re-distribute their
                // unmet amounts among the beneficiaries that accepted.
                //
                // "Dry" = call_remote returned an error or 0 allocation.
                let mut dry_amount = 0.0f64;
                let mut wet_coefs_remaining: Vec<(&AgentPubKeyB64, f64)> = Vec::new();

                for (key, coef) in &beneficiary_entries {
                    let target_amount = remaining * (coef / total_bcoef);
                    if target_amount <= CASCADE_DUST_THRESHOLD {
                        dry_amount += target_amount; // too small to drain — redistribute
                        continue;
                    }
                    let drain_meta = DrainMetadata {
                        parent_tx: parent_tx.clone(),
                        cascade_depth,
                        allocated_amount: target_amount,
                        visited: visited.clone(),
                    };
                    let input = CreateDrainRequestInput {
                        requester: seller_key.clone(),
                        beneficiary: key.clone(),
                        amount: target_amount,
                        drain_metadata: drain_meta,
                        requester_breakdown_record: breakdown_record.clone(),
                        status: None,
                    };
                    let target_agent: AgentPubKey = key.clone().into();
                    let response =
                        call_remote(target_agent, zome_info()?.name, "create_drain_request".into(), None, input);
                    match response {
                        Ok(ZomeCallResponse::Ok(_)) => {
                            info!(
                                "CASCADE [{}]: successfully fired drain request to {} for amount={}",
                                seller_key, key, target_amount
                            );
                            beneficiary_requests_sent += target_amount;
                            if let Some(existing) = beneficiary_drains.iter_mut().find(|(k, _)| *k == *key) {
                                existing.1 += target_amount;
                            } else {
                                beneficiary_drains.push((key.clone(), target_amount));
                            }
                            wet_coefs_remaining.push((key, *coef));
                        }
                        other => {
                            error!("CASCADE [{}]: FAILED to fire drain request to {}: {:?}", seller_key, key, other);
                            dry_amount += target_amount; // collect for re-distribution
                        }
                    }
                }

                // Re-distribute dry allocations among the wet beneficiaries (second pass).
                if dry_amount > CASCADE_DUST_THRESHOLD && !wet_coefs_remaining.is_empty() {
                    let wet_total: f64 = wet_coefs_remaining.iter().map(|(_, c)| c).sum();
                    if wet_total > 0.0 {
                        for (key, coef) in &wet_coefs_remaining {
                            let redistrib = dry_amount * (coef / wet_total);
                            if redistrib <= CASCADE_DUST_THRESHOLD {
                                continue;
                            }
                            let drain_meta = DrainMetadata {
                                parent_tx: parent_tx.clone(),
                                cascade_depth,
                                allocated_amount: redistrib,
                                visited: visited.clone(),
                            };
                            let input = CreateDrainRequestInput {
                                requester: seller_key.clone(),
                                beneficiary: (*key).clone(),
                                amount: redistrib,
                                drain_metadata: drain_meta,
                                requester_breakdown_record: breakdown_record.clone(),
                                status: None,
                            };
                            let target_agent: AgentPubKey = (*key).clone().into();
                            let response = call_remote(
                                target_agent,
                                zome_info()?.name,
                                "create_drain_request".into(),
                                None,
                                input,
                            );
                            match response {
                                Ok(ZomeCallResponse::Ok(_)) => {
                                    info!(
                                        "CASCADE [{}]: redistrib drain to {} for amount={}",
                                        seller_key, key, redistrib
                                    );
                                    beneficiary_requests_sent += redistrib;
                                    if let Some(existing) = beneficiary_drains.iter_mut().find(|(k, _)| *k == **key) {
                                        existing.1 += redistrib;
                                    } else {
                                        beneficiary_drains.push(((*key).clone(), redistrib));
                                    }
                                    // Reduce remaining by re-distributed amount
                                    remaining = (remaining - redistrib).max(0.0);
                                }
                                other => {
                                    error!("CASCADE [{}]: redistrib drain FAILED to {}: {:?}", seller_key, key, other);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    info!(
        "CASCADE [{}]: FINISHED phase-1 — own_transferred={}, beneficiary_requests_sent={}, genesis={}",
        seller_key,
        own_transferred,
        beneficiary_requests_sent,
        remaining.max(0.0)
    );

    Ok(SupportCascadeResult {
        own_transferred,
        beneficiary_requests_sent,
        genesis_amount: remaining.max(0.0),
        own_creditor_transfers,
        beneficiary_drains,
    })
}

/// Handle an incoming request to create a pending drain transaction on this cell.
///
/// Called via `call_remote` by a supporter whose cascade allocated a portion
/// to this agent. Creates a Pending Transaction with drain_metadata set,
/// which the beneficiary can then approve or reject like any other pending transaction.
///
/// AUTHENTICATION: The requester must have a valid SupportBreakdown listing this
/// agent as a beneficiary with a positive coefficient.
#[hdk_extern]
pub fn create_drain_request(input: CreateDrainRequestInput) -> ExternResult<Record> {
    let my_agent = agent_info()?.agent_initial_pubkey;
    let my_key: AgentPubKeyB64 = my_agent.clone().into();

    // Rate-limit: a single cascade chain should never produce more than one drain
    // request to this cell within a short window. A 5-second cooldown stops a
    // malicious or buggy supporter from flooding this cell with drain requests.
    let now_secs = sys_time()?.as_seconds_and_nanos().0 as u64;
    if !crate::trust_cache::check_and_set_rate_limit("create_drain_request", 5, now_secs) {
        warn!("DRAIN [{}]: rate-limited create_drain_request from {}", my_key, input.requester);
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_cascade_error::REQUESTER_NOT_SUPPORTER.to_string())));
    }

    debug!(
        "DRAIN [{}]: received create_drain_request from {} — amount={}, depth={}",
        my_key, input.requester, input.amount, input.drain_metadata.cascade_depth
    );

    // Cycle detection: if we are already in the visited set, skip
    if input.drain_metadata.visited.contains(&my_key) {
        debug!("DRAIN [{}]: CYCLE detected — already in visited set, ignoring", my_key);
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_cascade_error::REQUESTER_NOT_SUPPORTER.to_string())));
    }

    // AUTHENTICATION: Verify the requester's SupportBreakdown lists us as a beneficiary.
    // We DHT-verify the record to prevent forged breakdown payloads that were never
    // actually committed to the network.
    let requester_agent: AgentPubKey = input.requester.clone().into();
    let record_author = input.requester_breakdown_record.action().author().clone();
    if record_author != requester_agent {
        warn!("DRAIN [{}]: AUTH FAILED — record author {:?} != requester {:?}", my_key, record_author, requester_agent);
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_cascade_error::REQUESTER_NOT_SUPPORTER.to_string())));
    }

    // DHT-verify: ensure the breakdown record actually exists on the DHT
    let record_hash = input.requester_breakdown_record.action_address().clone();
    let dht_record = must_get_valid_record(record_hash.clone()).map_err(|_| {
        wasm_error!(WasmErrorInner::Guest(format!(
            "DRAIN [{my_key}]: AUTH FAILED — breakdown record {record_hash} not found on DHT (possible forgery)"
        )))
    })?;

    let breakdown_from_record: Option<SupportBreakdownData> = dht_record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(WasmErrorInner::Guest(format!("Deserialize error: {e:?}"))))?;

    let is_listed = breakdown_from_record
        .as_ref()
        .map(|bd| {
            bd.addresses
                .iter()
                .zip(bd.coefficients.iter())
                .any(|(addr, &coef)| *addr == my_key && coef > 0.0)
        })
        .unwrap_or(false);

    if !is_listed {
        warn!("DRAIN [{}]: AUTH FAILED — not listed in requester's breakdown", my_key);
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_cascade_error::REQUESTER_NOT_SUPPORTER.to_string())));
    }

    // ── Risk assessment: decide status using same logic as purchases ─────
    // Beneficiary (seller) evaluates the SUPPORTER (buyer/requester) using their own wallet thresholds.
    // Low risk  → auto-accept (drain runs immediately via reify_side_effects).
    // High risk → auto-reject (drain recorded but skipped).
    // In between → Pending for manual moderation by the beneficiary (seller).
    let (_, maybe_my_wallet_record) = crate::wallet::get_wallet_for_agent(my_agent.clone())?;
    let my_wallet: crate::Wallet = maybe_my_wallet_record
        .as_ref()
        .and_then(|r| r.entry().to_app_option::<crate::Wallet>().ok().flatten())
        .unwrap_or_else(|| crate::Wallet::new(&my_key));

    let risk = crate::trust::compute_full_risk_score(
        requester_agent.clone(),
        my_agent.clone(),
        my_wallet.auto_reject_threshold,
    )?;
    info!(
        "DRAIN [{}]: risk score for requester {} = {}, accept_threshold={}, reject_threshold={}",
        my_key, input.requester, risk, my_wallet.auto_accept_threshold, my_wallet.auto_reject_threshold
    );
    let status = TransactionStatus::from_risk_score_for_wallet(risk, my_wallet);

    use crate::transaction::create_drain_transaction;
    create_drain_transaction(crate::support_cascade::CreateDrainRequestInput { status: Some(status), ..input })
}

pub fn get_support_breakdown_for_agent(agent: AgentPubKey) -> ExternResult<Option<(SupportBreakdownData, Record)>> {
    let agent_key: AgentPubKeyB64 = agent.clone().into();
    debug!("get_support_breakdown_for_agent: looking up breakdown for {}", agent_key);
    let response = call(CallTargetCell::Local, "support", "get_support_breakdown_for_owner".into(), None, agent)?;

    match response {
        ZomeCallResponse::Ok(result) => {
            // The response is (Option<ActionHash>, Option<Record>)
            let (_, maybe_record): (Option<ActionHash>, Option<Record>) = result
                .decode()
                .map_err(|e| wasm_error!(WasmErrorInner::Guest(format!("Decode error: {e:?}"))))?;

            match maybe_record {
                Some(record) => {
                    let breakdown: SupportBreakdownData = record
                        .entry()
                        .to_app_option()
                        .map_err(|e| wasm_error!(WasmErrorInner::Guest(format!("Deserialize error: {e:?}"))))?
                        .ok_or(wasm_error!(WasmErrorInner::Guest(
                            coordinator_support_error::BREAKDOWN_NO_ENTRY.to_string()
                        )))?;
                    debug!(
                        "get_support_breakdown_for_agent: FOUND for {} — owner={}, addresses={:?}",
                        agent_key, breakdown.owner, breakdown.addresses
                    );
                    Ok(Some((breakdown, record)))
                }
                None => {
                    debug!("get_support_breakdown_for_agent: NONE for {}", agent_key);
                    Ok(None)
                }
            }
        }
        ZomeCallResponse::NetworkError(err) => {
            error!("get_support_breakdown_for_agent: NETWORK ERROR for {}: {}", agent_key, err);
            Ok(None)
        }
        other => {
            warn!("get_support_breakdown_for_agent: UNEXPECTED RESPONSE for {}: {:?}", agent_key, other);
            Ok(None)
        }
    }
}
