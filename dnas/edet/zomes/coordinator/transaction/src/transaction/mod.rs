pub mod moderation;
pub mod side_effects;

pub use moderation::*;
pub use side_effects::*;

use std::marker::PhantomData;

use hdk::prelude::*;
use transaction_integrity::types::constants::coordinator_transaction_error;
use transaction_integrity::types::timestamp_to_epoch;
use transaction_integrity::*;
use types::TransactionStatusTag;

use crate::{
    contracts,
    ranking_index::{GetRankingCursor, GetRankingDirection, RankingIndex},
    trust,
    types::{DrainFilterMode, GetTransactionsCursor, PaginatedTransactionsResult},
    wallet,
};

pub fn get_wallet_transactions_index() -> ExternResult<RankingIndex> {
    let link_type = ScopedLinkType::try_from(LinkTypes::WalletToTransactions)
        .map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    Ok(RankingIndex::new_with_default_mod(link_type))
}

#[hdk_extern]
pub fn create_transaction(mut transaction: Transaction) -> ExternResult<Record> {
    // Early input validation — catch bad inputs before any DHT queries.

    // Debt must be finite and positive. NaN and Infinity are rejected: `NaN <= 0.0`
    // is false in Rust, so without an explicit is_finite() guard they would slip through.
    if !transaction.debt.is_finite() || transaction.debt <= 0.0 {
        return Err(wasm_error!(WasmErrorInner::Guest(
            coordinator_transaction_error::DEBT_MUST_BE_POSITIVE.to_string()
        )));
    }

    // Buyer and seller must be different agents. Self-dealing creates circular debt
    // with no economic meaning and could be used to manufacture fake S/F evidence.
    if transaction.buyer.pubkey == transaction.seller.pubkey {
        return Err(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::BUYER_IS_SELLER.to_string())));
    }

    // Publish a fresh ReputationClaim for the buyer (this agent) if one does not
    // already exist for the current epoch. This makes the claim available on the
    // DHT so that sellers who evaluate the buyer via PATH 1 (first-contact,
    // claim-based risk) can find it without a BFS traversal.
    // The call is best-effort: a failure (e.g. no trust data yet) must not abort
    // the transaction.
    let _ = trust::ensure_fresh_claim();

    let buyer_wallet_hash = wallet::get_wallet_for_agent(transaction.buyer.pubkey.to_owned().into())?
        .1
        .map(|record: Record| record.action_address().to_owned())
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::BUYER_WALLET_NOT_FOUND.to_string())))?
        .to_owned();
    let seller_wallet_record = wallet::get_wallet_for_agent(transaction.seller.pubkey.to_owned().into())?
        .1
        .ok_or(wasm_error!(WasmErrorInner::Guest(
            coordinator_transaction_error::SELLER_WALLET_NOT_FOUND.to_string()
        )))?;
    let seller_wallet_hash = seller_wallet_record.action_address().to_owned();
    let seller_wallet_entry =
        seller_wallet_record
            .entry()
            .to_app_option::<Wallet>()
            .ok()
            .flatten()
            .ok_or(wasm_error!(WasmErrorInner::Guest(
                coordinator_transaction_error::SELLER_WALLET_NOT_FOUND.to_string()
            )))?;

    let buyer_last_transaction = get_agent_last_transaction(transaction.buyer.pubkey.to_owned().into())?;
    let seller_last_transaction = get_agent_last_transaction(transaction.seller.pubkey.to_owned().into())?;

    let timestamp = match &buyer_last_transaction {
        Some(record) => record.action().timestamp(),
        None => sys_time()?,
    };

    transaction.setup(timestamp, seller_wallet_hash, buyer_wallet_hash);
    transaction.buyer.previous_transaction = buyer_last_transaction.map(|r| r.action_address().to_owned());
    transaction.seller.previous_transaction = seller_last_transaction.map(|r| r.action_address().to_owned());

    // Protocol Logic: Compute transaction status (Whitepaper Section 5.3)
    // Trial transactions are always Pending (seller must approve manually).
    // Non-trial transactions are Pending (Path 1/2) or auto-accepted/rejected
    // based on cumulative EigenTrust risk.
    let current_epoch = timestamp_to_epoch(sys_time()?);
    transaction.status = trust::compute_transaction_status(
        transaction.buyer.pubkey.clone().into(),
        transaction.debt,
        &seller_wallet_entry,
        transaction.seller.pubkey.clone().into(),
        current_epoch,
    )?;

    // Determine whether this is a trial transaction and stamp the field.
    //
    // PATH 0 requires BOTH conditions (Whitepaper §5.3, Bootstrap Eligibility):
    //   (1) amount < eta * V_base  — trial-sized amount
    //   (2) buyer is bootstrap-eligible: Cap_b == 0  OR  n_S^(b) == 0
    //       (no effective vouched capacity OR no successful debt transfers yet)
    //
    // This matches the whitepaper exactly. The per-(buyer,seller) open-trial gate
    // (checked below) prevents a buyer from opening a second trial to the SAME seller
    // while the first is still Active. Multiple simultaneous trials to DIFFERENT
    // sellers are permitted while n_S == 0 (per whitepaper, the gate is per-pair).
    //
    // A buyer who has completed at least one successful debt transfer (n_S > 0)
    // is fully graduated and goes through PATH 1/2 for all amounts.
    let buyer_agent_pk: AgentPubKey = transaction.buyer.pubkey.clone().into();
    let is_trial_amt = trust::is_trial_transaction(transaction.debt);
    let is_bootstrap = trust::is_bootstrap_eligible(buyer_agent_pk.clone())?;
    let is_trial = is_trial_amt && is_bootstrap;
    transaction.is_trial = is_trial;
    debug!(
        "create_transaction: is_trial_amt={}, is_bootstrap={}, is_trial={}, debt={}",
        is_trial_amt, is_bootstrap, is_trial, transaction.debt
    );

    if is_trial {
        // Per-(buyer, seller) open-trial gate (Whitepaper §5.3, PATH 0 / Gap 2 mitigation):
        // Only one trial slot is open at a time between a given buyer and seller.
        // The slot is released only when the DebtContract becomes Transferred (repaid).
        // Expiry/default does NOT release the slot — the buyer must repay to earn another trial.
        let buyer_agent: AgentPubKey = transaction.buyer.pubkey.clone().into();
        let seller_agent: AgentPubKey = transaction.seller.pubkey.clone().into();
        let seller_b64: AgentPubKeyB64 = seller_agent.clone().into();
        let buyer_b64: AgentPubKeyB64 = buyer_agent.clone().into();

        // Definitive check: is the (buyer, seller) pair permanently blocked due
        // to a prior trial default, or is there already an Active trial contract?
        let trial_check = contracts::check_open_trial_for_buyer(buyer_agent.clone(), seller_agent.clone())?;
        debug!("create_transaction: trial_check result = {:?}", trial_check);
        match trial_check {
            contracts::TrialCheckResult::Allowed => {
                // `is_bootstrap_eligible` confirmed the buyer has no economic footprint.
                // This branch only reaches here for a genuine first trial to this seller.
            }
            contracts::TrialCheckResult::OpenTrialExists => {
                return Err(wasm_error!(WasmErrorInner::Guest(
                    coordinator_transaction_error::OPEN_TRIAL_EXISTS.to_string()
                )));
            }
            contracts::TrialCheckResult::PermanentlyBlocked => {
                return Err(wasm_error!(WasmErrorInner::Guest(
                    coordinator_transaction_error::TRIAL_PAIR_PERMANENTLY_BLOCKED.to_string()
                )));
            }
        }

        // Pre-gate: also block if there is already a Pending trial TRANSACTION to this seller
        // (i.e. the first trial was created but not yet approved, so no DebtContract exists yet).
        // This scan is local (buyer's own WalletToTransactions index, always available).
        let pending_tag = SerializedBytes::try_from(TransactionStatusTag::Pending)
            .map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
        let pending_txs = get_wallet_transactions_index()?.get_ranking_chunk(
            GetRankingDirection::Descendent,
            50,
            Some(GetRankingCursor {
                from_ranking: sys_time()?.as_millis(),
                tag: Some(pending_tag),
                tag_type: PhantomData::<TransactionStatusTag>,
                agent_pubkey: buyer_agent.clone(),
            }),
        )?;
        {
            for hashes in pending_txs.values() {
                for hash in hashes {
                    if let Some(dht_hash) = hash.hash.clone().into_any_dht_hash() {
                        if let Some(record) = get(dht_hash, GetOptions::default())? {
                            if let Some(tx) = record.entry().to_app_option::<Transaction>().ok().flatten() {
                                if tx.is_trial && tx.buyer.pubkey == buyer_b64 && tx.seller.pubkey == seller_b64 {
                                    return Err(wasm_error!(WasmErrorInner::Guest(
                                        coordinator_transaction_error::OPEN_TRIAL_EXISTS.to_string()
                                    )));
                                }
                            }
                        }
                    }
                }
            }
        }
        // Trials must be created with status Pending so the seller can approve manually
        // (PATH 0, Whitepaper §5.3: "trial transactions are always created with status Pending").
        // compute_transaction_status already returns Pending for trials (unless the velocity
        // limit was exceeded, in which case it returns Rejected). Asserting Pending here is
        // a defence-in-depth guard that makes the invariant explicit and prevents future
        // regressions where the status computation path might change.
        if transaction.status != TransactionStatus::Rejected {
            debug_assert_eq!(
                transaction.status,
                TransactionStatus::Pending,
                "compute_transaction_status must return Pending for non-rejected trials"
            );
            transaction.status = TransactionStatus::Pending;
        }
    }

    // Capacity check (Whitepaper Section 5.1, Step 2):
    // Verify Debt(b) + delta <= Cap_b before creating the entry.
    //
    // Trial transactions (PATH 0) bypass this check — they are the bootstrap
    // mechanism for newcomers, gated only by the seller's velocity limit
    // (Whitepaper Definition 5.5, line 442: "accepted subject only to a
    // per-seller trial velocity limit").  This matches the simulation
    // (universe.py:955-964) where trials return before capacity evaluation.
    //
    // Capacity is a property of the buyer, not of the seller's view of the buyer.
    // We use the buyer's self-published ReputationClaim (signed, integrity-validated,
    // conservative lower bound) when fresh, falling back to the buyer's own
    // EigenTrust self-computation when no claim exists.
    if transaction.debt > 0.0 && !is_trial {
        let buyer_agent: AgentPubKey = transaction.buyer.pubkey.clone().into();
        let current_debt = contracts::get_total_debt(buyer_agent.clone())?;

        let capacity = match trust::get_reputation_claim(buyer_agent.clone())? {
            Some((_, claim)) if trust::is_claim_fresh(&claim)? && claim.capacity_lower_bound > 0.0 => {
                // Fresh signed claim with positive bound: use as conservative capacity floor.
                // NOTE: capacity_lower_bound=0 can occur when the claim was published before
                // the buyer's EigenTrust subgraph was fully populated (e.g. immediately after
                // a trial is approved and trust rows haven't propagated yet). In that case we
                // fall through to the live EigenTrust computation below.
                claim.capacity_lower_bound
            }
            _ => {
                // No valid claim, stale claim, or claim with zero capacity bound:
                // fall back to buyer's own full EigenTrust computation on their cell.
                trust::compute_credit_capacity_for_agent(buyer_agent.clone())?
            }
        };
        if current_debt + transaction.debt > capacity {
            return Err(wasm_error!(WasmErrorInner::Guest(
                coordinator_transaction_error::CAPACITY_EXCEEDED.to_string()
            )));
        }
    }

    let transaction_hash = create_entry(&EntryTypes::Transaction(transaction.clone()))?;
    if let Some(base) = transaction.parent.clone() {
        create_link(transaction_hash.clone(), base, LinkTypes::TransactionToParent, ())?;
    }
    let record = get(transaction_hash.clone(), GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::CREATED_TX_NOT_FOUND.to_string())))?;

    if let Some(entry_hash) = record.action().entry_hash() {
        let agents = vec![transaction.seller.pubkey.clone().into(), transaction.buyer.pubkey.clone().into()];
        let tag = match transaction.status {
            TransactionStatus::Accepted | TransactionStatus::Rejected => TransactionStatusTag::Finalized,
            _ => TransactionStatusTag::Pending,
        };
        let tag_bytes = SerializedBytes::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;

        get_wallet_transactions_index()?.create_ranking(
            entry_hash.to_owned().into(),
            record.action().timestamp().as_millis(),
            Some(tag_bytes),
            agents,
        )?;

        // Ensure the buyer commits the debt contract immediately on creation
        // when auto-accepted. For Pending transactions, the contract is deferred
        // until the seller approves (via approve_pending_transaction -> call_remote
        // -> create_buyer_debt_contract) to prevent orphaned contracts on rejection.
        reify_transaction_side_effects(&transaction, transaction_hash.clone())?;

        // For auto-accepted transactions the seller's cell never runs
        // `update_transaction`, so the seller-side effects (support cascade,
        // acquaintance update, trust-row republication) would never execute.
        // Notify the seller via call_remote so their cell runs those effects.
        // This applies to both trial and non-trial transactions: trials still
        // need the seller's cascade to run so debt transfers occur and S/F
        // counters accumulate for EigenTrust.
        // This is fire-and-forget: a transient network failure must not abort
        // the buyer's committed transaction.
        if transaction.status == TransactionStatus::Accepted {
            let seller_agent: AgentPubKey = transaction.seller.pubkey.clone().into();
            let zome_name = zome_info()?.name;
            let payload =
                NotifySellerPayload { transaction_hash: transaction_hash.clone(), transaction: transaction.clone() };
            let _ = call_remote(seller_agent, zome_name, "notify_seller_of_accepted_transaction".into(), None, payload);
        }
    }

    Ok(record)
}

#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone)]
pub struct UpdateTransactionInput {
    pub original_transaction_hash: ActionHash,
    pub previous_transaction_hash: ActionHash,
    pub updated_transaction: Transaction,
}

#[hdk_extern]
pub fn update_transaction(mut input: UpdateTransactionInput) -> ExternResult<Record> {
    match get_original_transaction(input.previous_transaction_hash.to_owned())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::ORIGINAL_TX_NOT_FOUND.to_string())))
        .map(|record| (record.action().clone(), record.entry().to_app_option::<Transaction>().ok().flatten()))?
    {
        (action, Some(original_transaction)) => {
            input.updated_transaction.id = original_transaction.id;
            input.updated_transaction.updated_action = Some(input.previous_transaction_hash.to_owned());

            let updated_transaction_hash =
                update_entry(input.original_transaction_hash.to_owned(), &input.updated_transaction)?;

            let index = get_wallet_transactions_index()?;
            let original_timestamp = action.timestamp().as_millis();
            let original_entry_hash = action.entry_hash().ok_or(wasm_error!(WasmErrorInner::Guest(
                coordinator_transaction_error::ORIGINAL_ENTRY_HASH_NOT_FOUND.to_string()
            )))?;

            // Only update links if transitioning from Pending
            if original_transaction.status == TransactionStatus::Pending {
                index.delete_ranking(original_entry_hash.to_owned().into(), original_timestamp)?;

                let finalized_tag = SerializedBytes::try_from(TransactionStatusTag::Finalized)
                    .map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
                let agents = vec![
                    input.updated_transaction.seller.pubkey.clone().into(),
                    input.updated_transaction.buyer.pubkey.clone().into(),
                ];

                let record = get(updated_transaction_hash.clone(), GetOptions::default())?.ok_or(wasm_error!(
                    WasmErrorInner::Guest(coordinator_transaction_error::UPDATED_TX_NOT_FOUND.to_string())
                ))?;
                let entry_hash = record.action().entry_hash().ok_or(wasm_error!(WasmErrorInner::Guest(
                    coordinator_transaction_error::UPDATED_ENTRY_HASH_NOT_FOUND.to_string()
                )))?;

                index.create_ranking(
                    entry_hash.to_owned().into(),
                    record.action().timestamp().as_millis(),
                    Some(finalized_tag),
                    agents,
                )?;
            }

            let record = get(updated_transaction_hash.clone(), GetOptions::default())?.ok_or(wasm_error!(
                WasmErrorInner::Guest(coordinator_transaction_error::UPDATED_TX_NOT_FOUND.to_string())
            ))?;

            // ============================================================
            // Protocol Logic: When transaction is ACCEPTED, create debt
            // contract and perform support cascade (Whitepaper Section 5)
            // ============================================================
            if input.updated_transaction.status == TransactionStatus::Accepted
                && original_transaction.status == TransactionStatus::Pending
            {
                // Increment the seller's trial velocity counter if this is a trial tx.
                // This must run on the SELLER's chain and is part of the accept path.
                if input.updated_transaction.is_trial {
                    let seller_agent: AgentPubKey = input.updated_transaction.seller.pubkey.clone().into();
                    let _ = increment_trial_velocity(seller_agent);
                }
                reify_transaction_side_effects(&input.updated_transaction, updated_transaction_hash)?;
            }

            Ok(record)
        }
        _ => Err(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::TX_CONVERSION_FAILED.to_string()))),
    }
}

#[hdk_extern]
pub fn get_transaction_status_from_simulation(
    transaction: Transaction,
) -> ExternResult<crate::types::TransactionSimulationResult> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    let seller_agent: AgentPubKey = transaction.seller.pubkey.to_owned().into();
    let buyer_agent: AgentPubKey = transaction.buyer.pubkey.to_owned().into();
    let seller_wallet_record =
        wallet::get_wallet_for_agent(seller_agent.clone())?
            .1
            .ok_or(wasm_error!(WasmErrorInner::Guest(
                coordinator_transaction_error::SELLER_WALLET_NOT_FOUND.to_string()
            )))?;
    let seller_wallet_entry =
        seller_wallet_record
            .entry()
            .to_app_option::<Wallet>()
            .ok()
            .flatten()
            .ok_or(wasm_error!(WasmErrorInner::Guest(
                coordinator_transaction_error::SELLER_WALLET_NOT_FOUND.to_string()
            )))?;

    let is_trial_amt = trust::is_trial_transaction(transaction.debt);
    let is_bootstrap = trust::is_bootstrap_eligible(buyer_agent.clone())?;
    let is_trial = is_trial_amt && is_bootstrap;

    let mut status = trust::compute_transaction_status(
        buyer_agent.clone(),
        transaction.debt,
        &seller_wallet_entry,
        seller_agent.clone(),
        current_epoch,
    )?;

    // If it's a trial, also check the per-pair gate to be accurate.
    // If the gate is closed (open trial exists or permanently blocked), the
    // transaction will be Rejected in create_transaction. We reflect that
    // rejection here so the UI can show the "Automatic Refusal" state.
    if is_trial && status != TransactionStatus::Rejected {
        let trial_check = contracts::check_open_trial_for_buyer(buyer_agent.clone(), seller_agent.clone())?;
        if trial_check != contracts::TrialCheckResult::Allowed {
            status = TransactionStatus::Rejected;
        }
    }

    Ok(crate::types::TransactionSimulationResult { status, is_trial })
}

#[hdk_extern]
pub fn get_transactions(cursor: GetTransactionsCursor) -> ExternResult<PaginatedTransactionsResult> {
    let tag =
        SerializedBytes::try_from(cursor.tag.clone()).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    let agent_info = agent_info()?;
    let my_key: AgentPubKeyB64 = agent_info.agent_initial_pubkey.clone().into();

    let desired_count = cursor.count;
    let mut collected: Vec<Record> = Vec::with_capacity(desired_count);
    let mut current_from = if cursor.from_timestamp == 0 && cursor.direction == GetRankingDirection::Descendent {
        i64::MAX
    } else {
        cursor.from_timestamp
    };

    // Over-fetch factor: request more items from the ranking index than needed
    // to compensate for drain-filtered items. Max 5 iterations to avoid runaway.
    const MAX_ITERATIONS: usize = 5;
    for _ in 0..MAX_ITERATIONS {
        let remaining = desired_count - collected.len();
        if remaining == 0 {
            break;
        }
        // Request up to 2x remaining to compensate for filter losses
        let fetch_count = remaining * 2;

        let ranking_cursor = GetRankingCursor {
            from_ranking: current_from,
            tag: Some(tag.clone()),
            tag_type: PhantomData::<TransactionStatusTag>,
            agent_pubkey: agent_info.agent_initial_pubkey.clone(),
        };
        let hashes = get_wallet_transactions_index()?.get_ranking_chunk(
            cursor.direction.to_owned(),
            fetch_count,
            Some(ranking_cursor),
        )?;

        if hashes.is_empty() {
            break; // Index exhausted
        }

        // Track the last ranking (timestamp) seen in this chunk
        let last_ranking = match cursor.direction {
            GetRankingDirection::Descendent => *hashes.keys().next().unwrap_or(&current_from),
            GetRankingDirection::Ascendent => *hashes.keys().next_back().unwrap_or(&current_from),
        };

        let chunk_records: Vec<Record> = HDK
            .with(|hdk| {
                let hdk = hdk.borrow();
                let get_inputs = hashes
                    .values()
                    .flatten()
                    .filter_map(|hash| {
                        hash.hash
                            .clone()
                            .into_any_dht_hash()
                            .map(|any_dht_hash| GetInput { any_dht_hash, get_options: GetOptions::default() })
                    })
                    .collect();
                hdk.get(get_inputs)
            })?
            .into_iter()
            .filter_map(|r| {
                r.and_then(|record| match record.entry() {
                    RecordEntry::Present(Entry::App(_)) => {
                        if cursor.drain_filter != DrainFilterMode::IncludeAll {
                            if let Ok(Some(tx)) = record.entry().to_app_option::<Transaction>() {
                                if tx.is_drain() {
                                    match cursor.drain_filter {
                                        DrainFilterMode::ExcludeAll => return None,
                                        DrainFilterMode::BeneficiaryOnly => {
                                            if tx.seller.pubkey != my_key {
                                                return None;
                                            }
                                        }
                                        DrainFilterMode::IncludeAll => {}
                                    }
                                }
                            }
                        }
                        Some(record)
                    }
                    _ => None,
                })
            })
            .collect();

        let chunk_len = chunk_records.len();
        let take = (desired_count - collected.len()).min(chunk_records.len());
        collected.extend(chunk_records.into_iter().take(take));

        // Advance cursor past the items we just fetched.
        // Use saturating arithmetic to avoid integer overflow if ranking is at i64::MIN/MAX.
        //
        // Timestamp-collision safety: `last_ranking` is the smallest (Descendent) or
        // largest (Ascendent) timestamp in the chunk.  Advancing by ±1 skips the entire
        // `last_ranking` bucket, including any entries that share that exact millisecond
        // timestamp.  Since the chunk size is `remaining * 2` (2× over-fetch), at least
        // `remaining` entries at `last_ranking` are captured before the cursor advances.
        // The only remaining edge case is >2×remaining entries sharing a single timestamp,
        // which is astronomically unlikely for real Holochain action timestamps.
        current_from = match cursor.direction {
            GetRankingDirection::Descendent => last_ranking.saturating_sub(1),
            GetRankingDirection::Ascendent => last_ranking.saturating_add(1),
        };

        // If the raw chunk returned fewer items than requested, the index is exhausted
        if chunk_len < remaining {
            break;
        }
    }

    if cursor.direction == GetRankingDirection::Descendent {
        collected.reverse();
    }

    // Compute next_cursor: if we collected exactly desired_count, there may be more
    let next_cursor = if collected.len() >= desired_count { Some(current_from) } else { None };

    Ok(PaginatedTransactionsResult { records: collected, next_cursor })
}

#[hdk_extern]
pub fn get_original_transaction(original_transaction_hash: ActionHash) -> ExternResult<Option<Record>> {
    let Some(details) = get_details(original_transaction_hash, GetOptions::default())? else {
        return Ok(None);
    };
    match details {
        Details::Record(details) => Ok(Some(details.record)),
        _ => Err(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::MALFORMED_GET_DETAILS.to_string()))),
    }
}

#[hdk_extern]
pub fn get_latest_transaction(original_transaction_hash: ActionHash) -> ExternResult<Option<Record>> {
    let Some(details) = get_details(original_transaction_hash, GetOptions::network())? else {
        return Ok(None);
    };
    let record_details = match details {
        Details::Record(details) => details,
        _ => {
            return Err(wasm_error!(WasmErrorInner::Guest(
                coordinator_transaction_error::MALFORMED_GET_DETAILS.to_string()
            )))
        }
    };

    if record_details.updates.is_empty() {
        return Ok(Some(record_details.record));
    }

    let mut latest_record = record_details.record;
    let mut current_updates = record_details.updates;

    while !current_updates.is_empty() {
        current_updates.sort_by_key(|a| a.action().timestamp());
        let Some(latest_update) = current_updates.pop() else {
            break;
        };

        if let Some(update_record) = get(latest_update.as_hash().clone(), GetOptions::network())? {
            latest_record = update_record;
            if let Some(Details::Record(update_details)) =
                get_details(latest_update.as_hash().clone(), GetOptions::network())?
            {
                current_updates = update_details.updates;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    Ok(Some(latest_record))
}

#[hdk_extern]
pub fn get_agent_last_transaction(address: AgentPubKey) -> ExternResult<Option<Record>> {
    let my_pubkey = agent_info()?.agent_initial_pubkey;

    if address == my_pubkey {
        // Use manual chain walk for local chain as it's 100% reliable and atomic,
        // bypassing any indexer lag that might affect query().
        let mut current_hash = agent_info()?.chain_head.0;
        while let Some(record) = get(current_hash.clone(), GetOptions::default())? {
            if let Ok(Some(transaction)) = record.entry().to_app_option::<Transaction>() {
                if !matches!(transaction.status, TransactionStatus::Canceled | TransactionStatus::Rejected) {
                    return Ok(Some(record));
                }
            }

            if let Some(prev) = record.action().prev_action() {
                current_hash = prev.clone();
            } else {
                break;
            }
        }
        return Ok(None);
    }

    // For remote agents, walk the ranking index in pages until we find a non-canceled,
    // non-rejected transaction authored by this agent.
    //
    // Fetching only 20 records at a time risks returning None when the most recent 20
    // entries are all Canceled/Rejected. We loop in pages of 20 (the smallest batch that
    // avoids excessive DHT round-trips) until we find a match or exhaust the index.
    // A safety cap of 10 pages (200 records) prevents runaway on adversarially-long chains
    // of rejected transactions.
    const PAGE_SIZE: usize = 20;
    const MAX_PAGES: usize = 10;
    let mut from_ranking = i64::MAX;

    for _ in 0..MAX_PAGES {
        let ranking = get_wallet_transactions_index()?.get_ranking_chunk(
            GetRankingDirection::Descendent,
            PAGE_SIZE,
            Some(GetRankingCursor {
                from_ranking,
                tag: None,
                tag_type: PhantomData::<TransactionStatusTag>,
                agent_pubkey: address.clone(),
            }),
        )?;

        if ranking.is_empty() {
            break; // Index exhausted — no more transactions for this agent
        }

        // Track cursor for next page: the smallest (oldest) ranking value in this chunk.
        let last_ranking = *ranking.keys().next().unwrap_or(&from_ranking);

        let get_inputs: Vec<GetInput> = ranking
            .values()
            .flatten()
            .filter_map(|hash| hash.hash.clone().into_any_dht_hash())
            .map(|any_dht_hash| GetInput { any_dht_hash, get_options: GetOptions::default() })
            .collect();

        if get_inputs.is_empty() {
            break;
        }

        let records: Vec<Record> = HDK
            .with(|hdk| {
                let hdk = hdk.borrow();
                hdk.get(get_inputs)
            })?
            .into_iter()
            .flatten()
            .collect();

        // Find the first valid (non-canceled, non-rejected) record where the requested
        // agent is a party (buyer or seller). We check the transaction data rather than
        // `action.author()` because update actions (approvals) are authored by the seller,
        // not the original buyer — filtering by author alone would miss approved transactions
        // where the requested agent was the buyer.
        let found = records
            .into_iter()
            .filter(|record| {
                if let Ok(Some(transaction)) = record.entry().to_app_option::<Transaction>() {
                    !matches!(transaction.status, TransactionStatus::Canceled | TransactionStatus::Rejected)
                } else {
                    false
                }
            })
            .find(|record| {
                if let Ok(Some(transaction)) = record.entry().to_app_option::<Transaction>() {
                    Into::<AgentPubKey>::into(transaction.buyer.pubkey) == address
                        || Into::<AgentPubKey>::into(transaction.seller.pubkey) == address
                } else {
                    false
                }
            });

        if let Some(record) = found {
            return Ok(match record.entry() {
                RecordEntry::Present(Entry::App(_)) => Some(record),
                _ => None,
            });
        }

        // If this page had fewer items than PAGE_SIZE, the index is exhausted.
        let page_len: usize = ranking.values().map(|v| v.len()).sum();
        if page_len < PAGE_SIZE {
            break;
        }

        // Advance cursor past this page (saturating to avoid overflow at i64::MIN).
        from_ranking = last_ranking.saturating_sub(1);
    }

    Ok(None)
}

/// Create a pending drain Transaction on this agent's cell.
///
/// Called by `support_cascade::create_drain_request` when a supporter fires a cascade
/// drain request to this beneficiary. The resulting Pending transaction appears in the
/// beneficiary's pending transaction list and can be approved/rejected normally.
///
/// On approval (`update_transaction` → Accepted), `reify_transaction_side_effects`
/// will run `transfer_debt` locally and fire sub-cascades to the beneficiary's own
/// beneficiaries. No DebtContract is created.
pub fn create_drain_transaction(input: crate::support_cascade::CreateDrainRequestInput) -> ExternResult<Record> {
    let my_agent = agent_info()?.agent_initial_pubkey;
    let my_key: AgentPubKeyB64 = my_agent.clone().into();
    let requester_agent: AgentPubKey = input.requester.clone().into();

    // Resolve wallets for both parties
    // After role realignment: seller = beneficiary (this agent), buyer = supporter (requester)
    let (_, maybe_my_wallet) = wallet::get_wallet_for_agent(my_agent.clone())?;
    let my_wallet_record = maybe_my_wallet.ok_or(wasm_error!(WasmErrorInner::Guest(
        coordinator_transaction_error::SELLER_WALLET_NOT_FOUND.to_string()
    )))?;
    let my_wallet_hash = my_wallet_record.action_address().clone();

    let (_, maybe_requester_wallet) = wallet::get_wallet_for_agent(requester_agent.clone())?;
    let requester_wallet_hash = maybe_requester_wallet
        .map(|r| r.action_address().clone())
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::BUYER_WALLET_NOT_FOUND.to_string())))?;

    // Resolve previous transactions for chain linking
    let seller_last_transaction = get_agent_last_transaction(my_agent.clone())?;
    let buyer_last_transaction = get_agent_last_transaction(requester_agent.clone())?;

    // Compute drain transaction status via risk assessment.
    // The beneficiary (seller) evaluates the SUPPORTER (buyer) using their own EigenTrust score.
    // The status should have been pre-computed by create_drain_request and provided in the input.
    let drain_amount = input.drain_metadata.allocated_amount;
    let status = input.status.unwrap_or(TransactionStatus::Pending);

    // Build the drain Transaction entry.
    // seller = beneficiary (this agent), buyer = supporter (requester).
    let drain_tx = Transaction {
        id: None,
        seller: Party {
            side: TransactionSide::Seller,
            pubkey: my_key.clone(),
            previous_transaction: seller_last_transaction.map(|r| r.action_address().clone()),
            wallet: my_wallet_hash,
        },
        buyer: Party {
            side: TransactionSide::Buyer,
            pubkey: input.requester.clone(),
            previous_transaction: buyer_last_transaction.map(|r| r.action_address().clone()),
            wallet: requester_wallet_hash,
        },
        debt: drain_amount,
        description: "".to_string(),
        status,
        parent: None,
        updated_action: None,
        is_trial: false,
        drain_metadata: Some(input.drain_metadata),
    };

    let transaction_hash = create_entry(&EntryTypes::Transaction(drain_tx.clone()))?;
    let record = get(transaction_hash.clone(), GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_transaction_error::CREATED_TX_NOT_FOUND.to_string())))?;

    if let Some(entry_hash) = record.action().entry_hash() {
        let tag = match drain_tx.status {
            TransactionStatus::Accepted | TransactionStatus::Rejected => TransactionStatusTag::Finalized,
            _ => TransactionStatusTag::Pending,
        };
        let tag_bytes = SerializedBytes::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
        // Index both the beneficiary (seller) and supporter (buyer).
        // The supporter needs to see these for reputation (sf_counters).
        let agents = vec![drain_tx.seller.pubkey.clone().into(), drain_tx.buyer.pubkey.clone().into()];

        get_wallet_transactions_index()?.create_ranking(
            entry_hash.to_owned().into(),
            record.action().timestamp().as_millis(),
            Some(tag_bytes),
            agents,
        )?;

        reify_transaction_side_effects(&drain_tx, transaction_hash)?;
    }

    Ok(record)
}
