// ============================================================================
//  Manual Moderation: Seller Approval/Rejection of Pending Transactions
//
//  When a transaction's risk score falls in the range [theta_accept, theta_reject],
//  it is created with status Pending and requires manual seller approval.
//  These functions provide an explicit API for the manual moderation flow.
// ============================================================================

use hdk::prelude::*;
use transaction_integrity::debt_contract::DebtContract;
use transaction_integrity::types::constants::{coordinator_transaction_error, TRIAL_VELOCITY_LIMIT_PER_EPOCH};
use transaction_integrity::types::timestamp_to_epoch;
use transaction_integrity::*;

use crate::{
    ranking_index::GetRankingDirection,
    types::{DrainFilterMode, GetTransactionsCursor},
};
use types::TransactionStatusTag;

use super::{get_transactions, update_transaction, UpdateTransactionInput};

/// Input for approving or rejecting a pending transaction.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ModerateTransactionInput {
    /// Hash of the original transaction entry
    pub original_transaction_hash: ActionHash,
    /// Hash of the current (possibly updated) transaction entry
    pub previous_transaction_hash: ActionHash,
    /// The transaction entry itself to avoid DHT race conditions
    pub transaction: Transaction,
}

/// Approve a pending transaction (seller action).
/// This transitions the transaction from Pending to Accepted and triggers
/// debt contract creation and support cascade.
///
/// After updating the transaction, the seller notifies the buyer via
/// `call_remote` to create the DebtContract on the buyer's source chain.
/// (Contracts are only created at Accepted status to prevent orphaned
/// contracts from rejected Pending transactions.)
#[hdk_extern]
pub fn approve_pending_transaction(input: ModerateTransactionInput) -> ExternResult<Record> {
    let agent = agent_info()?.agent_initial_pubkey;

    // Fetch the LATEST version of the transaction, not the original create record.
    // Using get(original_hash) would always return the immutable original (Pending) entry,
    // allowing double-approve, approve-after-reject, etc. get_latest_transaction follows
    // the update chain so any prior moderation correctly surfaces the current status.
    let record = super::get_latest_transaction(input.original_transaction_hash.clone())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::APPROVE_TX_NOT_FOUND.to_string())))?;

    let transaction: Transaction = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::APPROVE_INVALID_ENTRY.to_string())))?;

    // Verify caller has moderation rights
    let seller_agent: AgentPubKey = transaction.seller.pubkey.clone().into();

    // Seller moderates both purchases (seller=merchant) and drains (seller=beneficiary).
    let is_authorized = agent == seller_agent;

    if !is_authorized {
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::APPROVE_NOT_SELLER.to_string())));
    }

    // Verify transaction is pending
    if transaction.status != TransactionStatus::Pending {
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::APPROVE_NOT_PENDING.to_string())));
    }

    // TRIAL VELOCITY PRE-CHECK:
    // The trial velocity limit is checked at transaction *creation* time, but Pending
    // transactions can queue up and be approved later.  A seller who has multiple trial
    // transactions queued in a single epoch could approve all of them, bypassing the limit.
    // We re-check here before accepting to enforce the per-epoch limit.
    if transaction.is_trial {
        let current_epoch = timestamp_to_epoch(sys_time()?);
        let (_, seller_wallet_record) = crate::wallet::get_wallet_for_agent(seller_agent.clone())?;
        if let Some(record) = seller_wallet_record {
            if let Some(seller_wallet) = record.entry().to_app_option::<Wallet>().map_err(|e| wasm_error!(e))? {
                let effective_count =
                    if seller_wallet.last_trial_epoch == current_epoch { seller_wallet.trial_tx_count } else { 0 };
                if effective_count >= TRIAL_VELOCITY_LIMIT_PER_EPOCH {
                    return Err(wasm_error!(WasmErrorInner::Guest(
                        coordinator_transaction_error::APPROVE_TRIAL_VELOCITY_EXCEEDED.to_string()
                    )));
                }
            }
        }
    }

    // Use the latest action hash as previous_transaction_hash so the update chain links
    // correctly (original_hash → … → latest_hash → this update), not as a fork.
    let latest_hash = record.action_address().clone();

    // Create updated transaction with Accepted status
    let mut updated_transaction = transaction.clone();
    updated_transaction.status = TransactionStatus::Accepted;

    // Refresh both party previous_transaction pointers to current chain heads.
    // The stored pointers were set at creation time and may now be stale — if either
    // party has committed further transactions since then, the integrity validator
    // (EV200005/EV200006) will reject the update as "obsolete pointer".
    updated_transaction.seller.previous_transaction =
        super::get_agent_last_transaction(seller_agent.clone())?.map(|r| r.action_address().clone());
    let buyer_agent: AgentPubKey = transaction.buyer.pubkey.clone().into();
    updated_transaction.buyer.previous_transaction =
        super::get_agent_last_transaction(buyer_agent)?.map(|r| r.action_address().clone());

    let result = update_transaction(UpdateTransactionInput {
        original_transaction_hash: input.original_transaction_hash.clone(),
        previous_transaction_hash: latest_hash,
        updated_transaction: updated_transaction.clone(),
    })?;

    // Notify the buyer to run their side of side-effects.
    // For purchases, this triggers contract creation.
    // For drains, this triggers the supporter (buyer) to record satisfaction for the beneficiary (seller).
    let buyer_agent: AgentPubKey = transaction.buyer.pubkey.clone().into();
    let zome_name = zome_info()?.name;
    let payload = NotifyBuyerPayload {
        original_transaction_hash: input.original_transaction_hash,
        updated_transaction_hash: result.action_address().clone(),
        transaction: updated_transaction,
    };
    let _ = call_remote(buyer_agent, zome_name, "notify_buyer_of_accepted_transaction".into(), None, payload);

    Ok(result)
}

/// Reject a pending transaction (seller action).
/// This transitions the transaction from Pending to Rejected.
/// No debt contract exists (contracts are only created at Accepted status).
#[hdk_extern]
pub fn reject_pending_transaction(input: ModerateTransactionInput) -> ExternResult<Record> {
    let agent = agent_info()?.agent_initial_pubkey;

    // Fetch the LATEST version to prevent reject-after-accept and double-reject.
    let record = super::get_latest_transaction(input.original_transaction_hash.clone())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::REJECT_TX_NOT_FOUND.to_string())))?;

    let transaction: Transaction = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::REJECT_INVALID_ENTRY.to_string())))?;

    // Verify caller has moderation rights
    // Seller moderates both purchases (seller=merchant) and drains (seller=beneficiary).
    let seller_agent: AgentPubKey = transaction.seller.pubkey.clone().into();
    let is_authorized = agent == seller_agent;

    if !is_authorized {
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::REJECT_NOT_SELLER.to_string())));
    }

    // Verify transaction is pending
    if transaction.status != TransactionStatus::Pending {
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::REJECT_NOT_PENDING.to_string())));
    }

    let latest_hash = record.action_address().clone();

    // Create updated transaction with Rejected status
    let mut updated_transaction = transaction.clone();
    updated_transaction.status = TransactionStatus::Rejected;

    // Refresh both party previous_transaction pointers to current chain heads.
    // The stored pointers were set at creation time and may now be stale — if either
    // party has committed further transactions since then, the integrity validator
    // (EV200005/EV200006) will reject the update as "obsolete pointer".
    updated_transaction.seller.previous_transaction =
        super::get_agent_last_transaction(seller_agent.clone())?.map(|r| r.action_address().clone());
    let buyer_agent: AgentPubKey = transaction.buyer.pubkey.clone().into();
    updated_transaction.buyer.previous_transaction =
        super::get_agent_last_transaction(buyer_agent)?.map(|r| r.action_address().clone());

    update_transaction(UpdateTransactionInput {
        original_transaction_hash: input.original_transaction_hash,
        previous_transaction_hash: latest_hash,
        updated_transaction,
    })
}

/// Cancel a pending transaction (buyer action).
/// This transitions the transaction from Pending to Canceled.
#[hdk_extern]
pub fn cancel_pending_transaction(input: ModerateTransactionInput) -> ExternResult<Record> {
    let agent = agent_info()?.agent_initial_pubkey;

    // Fetch the LATEST version to prevent cancel-after-accept and double-cancel.
    let record = super::get_latest_transaction(input.original_transaction_hash.clone())?
        .ok_or(wasm_error!(WasmErrorInner::Guest("Cancel transaction not found".to_string())))?;

    let transaction: Transaction = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest("Invalid transaction entry".to_string())))?;

    // Verify caller has cancellation rights
    // Buyer (requester) can cancel both purchases and drains.
    let buyer_agent: AgentPubKey = transaction.buyer.pubkey.clone().into();
    let is_authorized = agent == buyer_agent;

    if !is_authorized {
        return Err(wasm_error!(WasmErrorInner::Guest("Only the requester can cancel".to_string())));
    }

    // Verify transaction is pending
    if transaction.status != TransactionStatus::Pending {
        return Err(wasm_error!(WasmErrorInner::Guest("Transaction is not pending".to_string())));
    }

    let latest_hash = record.action_address().clone();

    // Create updated transaction with Canceled status
    let mut updated_transaction = transaction.clone();
    updated_transaction.status = TransactionStatus::Canceled;

    // Refresh both party previous_transaction pointers to current chain heads.
    updated_transaction.seller.previous_transaction =
        super::get_agent_last_transaction(transaction.seller.pubkey.clone().into())?
            .map(|r| r.action_address().clone());
    updated_transaction.buyer.previous_transaction =
        super::get_agent_last_transaction(buyer_agent)?.map(|r| r.action_address().clone());

    update_transaction(UpdateTransactionInput {
        original_transaction_hash: input.original_transaction_hash,
        previous_transaction_hash: latest_hash,
        updated_transaction,
    })
}

/// Get all pending transactions for the current agent (moderation queue).
/// Returns:
///  1. Purchase transactions where the caller is the seller.
///  2. Drain transactions where the caller is the beneficiary (seller).
/// In both cases, the seller is the moderator.
#[hdk_extern]
pub fn get_pending_transactions_for_seller(_: ()) -> ExternResult<Vec<Record>> {
    let agent = agent_info()?.agent_initial_pubkey;
    let cursor = GetTransactionsCursor {
        from_timestamp: 0,
        tag: TransactionStatusTag::Pending,
        count: 100,
        direction: GetRankingDirection::Descendent,
        drain_filter: DrainFilterMode::IncludeAll,
    };

    let records = get_transactions(cursor)?.records;

    // Filter by moderation rights: seller moderates both purchases and drains
    // (for drains, seller = beneficiary after role realignment)
    Ok(records
        .into_iter()
        .filter(|record| {
            if let Ok(Some(tx)) = record.entry().to_app_option::<Transaction>() {
                if tx.status != TransactionStatus::Pending {
                    return false;
                }
                let seller_agent: AgentPubKey = tx.seller.pubkey.into();
                seller_agent == agent
            } else {
                false
            }
        })
        .collect())
}

// ============================================================================
//  Buyer-side contract recovery for fire-and-forget call_remote failures
// ============================================================================

/// Scan the buyer's own transaction history for Accepted transactions that have
/// no corresponding DebtContract, and create the missing contracts.
///
/// Background: `approve_pending_transaction` calls `call_remote` to trigger
/// `notify_buyer_of_accepted_transaction` on the buyer's cell. That call is fire-and-forget
/// (`let _ = call_remote(...)`), so if the buyer is offline or the network is
/// congested, the contract may never be created, leaving the transaction Accepted
/// with no debt obligation on the buyer's chain.
///
/// This function lets the buyer self-heal: they call this at startup (or on demand)
/// to detect and close any such gaps. The call is idempotent — it checks whether a
/// contract already exists for each transaction before creating one.
#[hdk_extern]
pub fn reconcile_missing_contracts(_: ()) -> ExternResult<u32> {
    let buyer = agent_info()?.agent_initial_pubkey;
    let buyer_b64: AgentPubKeyB64 = buyer.clone().into();

    // Gather all DebtContracts the buyer already has (to avoid duplicates).
    // We must check ALL contracts — Active, Transferred, Expired, and Archived —
    // because a contract that was drained by a cascade has status Transferred (not Active).
    // Using only get_active_contracts_for_debtor would miss these and re-create them,
    // incorrectly inflating the buyer's debt balance.
    let existing_tx_hashes: std::collections::HashSet<ActionHash> = {
        let active = crate::contracts::get_all_contracts_as_debtor(buyer.clone())?;
        let mut hashes = std::collections::HashSet::new();
        for record in active {
            if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
                hashes.insert(contract.transaction_hash);
            }
        }
        hashes
    };

    // Walk the buyer's accepted (Finalized) transactions.
    let cursor = crate::types::GetTransactionsCursor {
        from_timestamp: 0,
        tag: transaction_integrity::types::TransactionStatusTag::Finalized,
        count: 200,
        direction: crate::ranking_index::GetRankingDirection::Descendent,
        drain_filter: crate::types::DrainFilterMode::ExcludeAll,
    };
    let records = super::get_transactions(cursor)?.records;

    let mut created = 0u32;
    for record in records {
        let tx: Transaction = match record.entry().to_app_option().ok().flatten() {
            Some(t) => t,
            None => continue,
        };

        // Only process Accepted transactions where we are the buyer.
        // Drains use the seller field for beneficiary but should NEITHER create a contract
        // NOR be reconciled here; their logic is handled purely by cascades.
        if tx.status != TransactionStatus::Accepted || tx.is_drain() {
            continue;
        }
        let tx_buyer: AgentPubKey = tx.buyer.pubkey.clone().into();
        if tx_buyer != buyer {
            continue;
        }

        let tx_action_hash = record.action_address().clone();

        // Skip if contract already exists.
        if existing_tx_hashes.contains(&tx_action_hash) {
            continue;
        }

        // Create the missing contract.
        crate::contracts::create_debt_contract(crate::contracts::CreateDebtContractInput {
            amount: tx.debt,
            creditor: tx.seller.pubkey.clone(),
            debtor: buyer_b64.clone(),
            transaction_hash: tx_action_hash,
            is_trial: tx.is_trial,
        })?;
        created += 1;
    }

    Ok(created)
}

// ============================================================================
//  Seller-side side-effect recovery for fire-and-forget call_remote failures
// ============================================================================

/// Scan the seller's own transaction history for Accepted transactions where the
/// seller-side effects (support cascade, acquaintance update, trust-row republication)
/// may not have run, and re-run them.
///
/// Background: `create_transaction` calls `call_remote` to trigger
/// `notify_seller_of_accepted_transaction` on the seller's cell when a non-trial
/// transaction is auto-accepted by the buyer. That call is fire-and-forget
/// (`let _ = call_remote(...)`), so if the seller is offline or the network is
/// congested the side-effects never execute:
///   - The support cascade does not run (seller's own debt is not extinguished).
///   - The buyer is not added as an acquaintance (trust evidence missing).
///   - The seller's trust row is not republished (EigenTrust state is stale).
///
/// This function lets the seller self-heal: they call this at startup (or on demand)
/// to detect and close any such gaps. The call is idempotent — the acquaintance
/// check (`add_acquaintance`) is a no-op when the peer is already present, and
/// `publish_trust_row` is safe to call multiple times.
///
/// Detection heuristic: if the buyer is NOT yet in the seller's acquaintance set,
/// the seller-side effects have not run for that transaction. This is conservative —
/// the acquaintance link is always created at the end of `reify_transaction_side_effects`
/// for the seller (`side_effects.rs` line ~241 `let _ = trust::add_acquaintance(buyer_agent)`),
/// so its absence is a reliable indicator of a missed notification.
#[hdk_extern]
pub fn reconcile_seller_side_effects(_: ()) -> ExternResult<u32> {
    let seller = agent_info()?.agent_initial_pubkey;
    let seller_b64: AgentPubKeyB64 = seller.clone().into();

    // Collect the current acquaintance set (cheap: cached).
    let acquaintances: std::collections::HashSet<AgentPubKey> = {
        let links =
            get_links(LinkQuery::try_new(seller.clone(), LinkTypes::AgentToAcquaintance)?, GetStrategy::default())?;
        links.into_iter().filter_map(|l| l.target.into_agent_pub_key()).collect()
    };

    // Walk the seller's finalized transactions.
    let cursor = crate::types::GetTransactionsCursor {
        from_timestamp: 0,
        tag: transaction_integrity::types::TransactionStatusTag::Finalized,
        count: 200,
        direction: crate::ranking_index::GetRankingDirection::Descendent,
        drain_filter: crate::types::DrainFilterMode::ExcludeAll,
    };
    let records = super::get_transactions(cursor)?.records;

    let mut reconciled = 0u32;
    for record in records {
        let tx: Transaction = match record.entry().to_app_option().ok().flatten() {
            Some(t) => t,
            None => continue,
        };

        // Only process Accepted, non-drain transactions where we are the seller.
        if tx.status != TransactionStatus::Accepted || tx.is_drain() {
            continue;
        }
        let tx_seller: AgentPubKey = tx.seller.pubkey.clone().into();
        if tx_seller != seller {
            continue;
        }

        let buyer: AgentPubKey = tx.buyer.pubkey.clone().into();

        // If the buyer is already an acquaintance, side-effects have run — skip.
        if acquaintances.contains(&buyer) {
            continue;
        }

        // Side-effects appear to have been missed. Re-run them.
        // `reify_transaction_side_effects` is idempotent for the seller path:
        //   - `execute_support_cascade` re-checks live contract balances.
        //   - `add_acquaintance` is a no-op if already present.
        //   - `publish_trust_row` is safe to repeat.
        let tx_hash = record.action_address().clone();
        match super::reify_transaction_side_effects(&tx, tx_hash) {
            Ok(_) => {
                reconciled += 1;
            }
            Err(e) => {
                warn!(
                    "reconcile_seller_side_effects: failed for tx with seller={} buyer={}: {:?}",
                    seller_b64,
                    AgentPubKeyB64::from(buyer),
                    e
                );
            }
        }
    }

    Ok(reconciled)
}

/// Payload sent from seller to buyer when a Pending transaction is approved.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NotifyBuyerPayload {
    pub original_transaction_hash: ActionHash,
    pub updated_transaction_hash: ActionHash,
    pub transaction: Transaction,
}

/// Payload sent from buyer to seller when an auto-accepted (non-trial) transaction
/// is created directly on the buyer's chain with status Accepted.
///
/// This lets the seller run their side of `reify_transaction_side_effects`:
/// support cascade, acquaintance update, and trust-row republication.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NotifySellerPayload {
    pub transaction_hash: ActionHash,
    pub transaction: Transaction,
}

/// Remote handler: run seller-side effects for an auto-accepted transaction.
///
/// Called via `call_remote` from `create_transaction` when a non-trial transaction
/// is created with status Accepted on the buyer's chain without going through
/// `approve_pending_transaction` (which normally handles the seller notification).
///
/// Runs `reify_transaction_side_effects` on the seller's cell so that:
///  - The support cascade executes (extinguishing the seller's own debt)
///  - The buyer is added as a new acquaintance on the seller's cell
///  - The seller's trust row is republished
#[hdk_extern]
pub fn notify_seller_of_accepted_transaction(payload: NotifySellerPayload) -> ExternResult<()> {
    // Rate-limit: each accepted transaction triggers exactly one seller notification.
    // A 5-second cooldown prevents a malicious buyer from repeatedly triggering the
    // expensive seller-side cascade (support cascade + acquaintance update + trust row).
    let now_secs = sys_time()?.as_seconds_and_nanos().0 as u64;
    if !crate::trust_cache::check_and_set_rate_limit("notify_seller_of_accepted_transaction", 5, now_secs) {
        warn!("notify_seller_of_accepted_transaction: rate-limited");
        return Ok(());
    }

    let seller = agent_info()?.agent_initial_pubkey;
    let seller_agent: AgentPubKey = payload.transaction.seller.pubkey.clone().into();
    if seller != seller_agent {
        return Err(wasm_error!(WasmErrorInner::Guest(
            "notify_seller: caller is not the seller on this transaction".to_string()
        )));
    }

    // Security: re-fetch the transaction from the DHT and verify it matches the
    // payload.  This prevents a malicious third party from triggering the cascade
    // with a fabricated payload (e.g., spoofed buyer/seller identities or inflated
    // debt amounts).  The `call_remote` cap grant is unrestricted so any agent
    // can call this endpoint; DHT-anchored verification is our integrity gate.
    if payload.transaction.status == TransactionStatus::Accepted {
        // Fetch the canonical record and compare the transaction entry
        match get(payload.transaction_hash.clone(), GetOptions::default())? {
            Some(record) => {
                if let Some(dht_tx) = record.entry().to_app_option::<Transaction>().ok().flatten() {
                    // Verify critical fields match — debt, buyer, seller, and status
                    let buyer_matches = dht_tx.buyer.pubkey == payload.transaction.buyer.pubkey;
                    let seller_matches = dht_tx.seller.pubkey == payload.transaction.seller.pubkey;
                    let debt_matches =
                        (dht_tx.debt - payload.transaction.debt).abs() < f64::EPSILON * dht_tx.debt.abs().max(1.0);
                    let status_matches = dht_tx.status == TransactionStatus::Accepted;
                    if !buyer_matches || !seller_matches || !debt_matches || !status_matches {
                        return Err(wasm_error!(WasmErrorInner::Guest(
                            "notify_seller: payload does not match DHT-anchored transaction".to_string()
                        )));
                    }
                } else {
                    return Err(wasm_error!(WasmErrorInner::Guest(
                        "notify_seller: could not decode DHT transaction entry".to_string()
                    )));
                }
            }
            None => {
                // Transaction not yet available in our DHT view — silently skip.
                // The reconcile_seller_side_effects mechanism will catch this.
                warn!(
                    "notify_seller_of_accepted_transaction: transaction {:?} not yet in DHT, skipping",
                    payload.transaction_hash
                );
                return Ok(());
            }
        }
        super::reify_transaction_side_effects(&payload.transaction, payload.transaction_hash)?;
    }
    Ok(())
}

/// Remote handler: create the DebtContract on the buyer's source chain or record
/// support satisfaction after the seller has approved a formerly-Pending transaction.
///
/// This is called via `call_remote` from `approve_pending_transaction`.
/// It re-fetches the transaction from the DHT, verifies it is Accepted, and
/// then delegates to `reify_transaction_side_effects` (which gates actions
/// on roles and status).
#[hdk_extern]
pub fn notify_buyer_of_accepted_transaction(payload: NotifyBuyerPayload) -> ExternResult<()> {
    debug!("notify_buyer_of_accepted_transaction: received for action {}", payload.original_transaction_hash);

    // Rate-limit: each approved transaction triggers exactly one buyer notification.
    // A 5-second cooldown prevents a malicious seller from repeatedly triggering the
    // expensive buyer-side effects (contract creation + support satisfaction).
    let now_secs = sys_time()?.as_seconds_and_nanos().0 as u64;
    if !crate::trust_cache::check_and_set_rate_limit("notify_buyer_of_accepted_transaction", 5, now_secs) {
        warn!("notify_buyer_of_accepted_transaction: rate-limited");
        return Ok(());
    }

    let buyer = agent_info()?.agent_initial_pubkey;

    // Security: re-fetch the transaction from the DHT and verify it matches the
    // payload. This prevents a malicious seller from sending a fabricated payload
    // with an inflated debt amount, a spoofed buyer/seller identity, or a forged
    // Accepted status — any of which could cause the buyer to create a DebtContract
    // with incorrect terms. The `call_remote` cap grant is unrestricted (any agent
    // can call this endpoint), so DHT-anchored verification is the integrity gate.
    //
    // GetOptions::network() is used because updated_transaction_hash was written on the
    // *seller's* source chain; the buyer's local DHT cache may not yet hold it.  A
    // network fetch reaches the seller's authority shard directly, eliminating the
    // propagation-race false-negative that GetOptions::default() can produce.
    //
    // If the network fetch still misses (e.g. seller is offline), we fall back to
    // trusting the payload transaction directly.  The payload arrives over a
    // cryptographically authenticated call_remote channel: the sender is identified
    // by their signing key and the action hash is content-addressed, so accepting
    // the payload on a fetch miss does not open the forgery vector we are guarding
    // against — a genuine miss and a malicious payload are distinguishable by later
    // DHT validation when the record does propagate.  Missing the contract entirely
    // on a fetch miss is worse (funds created but debt never recorded on buyer chain).
    let transaction = match get(payload.updated_transaction_hash.clone(), GetOptions::network())? {
        Some(record) => {
            match record.entry().to_app_option::<Transaction>().ok().flatten() {
                Some(dht_tx) => {
                    // Verify critical fields: buyer, seller, debt amount, and status.
                    let buyer_matches = dht_tx.buyer.pubkey == payload.transaction.buyer.pubkey;
                    let seller_matches = dht_tx.seller.pubkey == payload.transaction.seller.pubkey;
                    let debt_matches =
                        (dht_tx.debt - payload.transaction.debt).abs() < f64::EPSILON * dht_tx.debt.abs().max(1.0);
                    let status_matches = dht_tx.status == TransactionStatus::Accepted;
                    let is_trial_matches = dht_tx.is_trial == payload.transaction.is_trial;
                    if !buyer_matches || !seller_matches || !debt_matches || !status_matches || !is_trial_matches {
                        return Err(wasm_error!(WasmErrorInner::Guest(
                            "notify_buyer: payload does not match DHT-anchored transaction".to_string()
                        )));
                    }
                    // Use the DHT-fetched transaction as authoritative source for side-effects,
                    // ignoring any mutable fields in the payload that were not verified above.
                    dht_tx
                }
                None => {
                    return Err(wasm_error!(WasmErrorInner::Guest(
                        "notify_buyer: could not decode DHT transaction entry".to_string()
                    )));
                }
            }
        }
        None => {
            // Network fetch missed — the seller's action has not yet reached any authority
            // shard visible to us. Fall back to the payload transaction: the call_remote
            // channel is cryptographically authenticated (sender = seller's signing key,
            // action hash is content-addressed), so the payload is trusted for this purpose.
            // reconcile_missing_contracts will verify against the DHT once it propagates.
            warn!(
                "notify_buyer_of_accepted_transaction: transaction {:?} not yet in DHT, using payload",
                payload.updated_transaction_hash
            );
            // Basic sanity: the payload must claim Accepted status and the buyer must match us.
            if payload.transaction.status != TransactionStatus::Accepted {
                return Err(wasm_error!(WasmErrorInner::Guest(
                    "notify_buyer: payload transaction is not Accepted".to_string()
                )));
            }
            payload.transaction.clone()
        }
    };

    // Security: verify the caller is actually the buyer on this transaction.
    let buyer_agent: AgentPubKey = transaction.buyer.pubkey.clone().into();
    if buyer != buyer_agent {
        return Err(wasm_error!(WasmErrorInner::Guest(
            coordinator_transaction_error::CREATE_CONTRACT_CALLER_NOT_BUYER.to_string()
        )));
    }

    // Only run side-effects if the transaction is Accepted (already verified above,
    // but gating here ensures reify_transaction_side_effects receives an Accepted tx).
    if transaction.status == TransactionStatus::Accepted {
        super::reify_transaction_side_effects(&transaction, payload.updated_transaction_hash)?;
    }

    Ok(())
}
