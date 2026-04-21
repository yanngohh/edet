use hdk::prelude::*;
use transaction_integrity::types::constants::DUST_THRESHOLD;
use transaction_integrity::types::{timestamp_to_epoch, SupportSatisfactionTag};
use transaction_integrity::*;

use crate::{
    contracts::{self, CreateDebtContractInput},
    support_cascade, trust, wallet,
};

/// Increment the seller's trial velocity counter when a trial transaction is accepted.
///
/// Updates `trial_tx_count` and `last_trial_epoch` in the seller's Wallet entry.
/// If the stored epoch differs from the current one, the counter resets to 1.
///
/// This is called on the SELLER's chain (inside update_transaction) after a
/// trial transaction transitions from Pending → Accepted.
pub fn increment_trial_velocity(seller_agent: AgentPubKey) -> ExternResult<()> {
    let (maybe_original_hash, maybe_record) = wallet::get_wallet_for_agent(seller_agent.clone())?;
    // Capture both the original create hash (needed by update_wallet for the
    // `original_wallet_hash` field) and the latest action hash from the record
    // (needed for `previous_wallet_hash`). Using the original hash as
    // `previous_wallet_hash` is incorrect when the wallet has been updated at
    // least once — it would fork the update chain and link from the original
    // create action instead of the most recent update.
    let (original_wallet_hash, latest_wallet_hash, mut wallet_entry) = match (
        maybe_original_hash,
        maybe_record.and_then(|r| {
            let latest_action_hash = r.action_address().to_owned();
            r.entry()
                .to_app_option::<Wallet>()
                .ok()
                .flatten()
                .map(|w| (latest_action_hash, w))
        }),
    ) {
        (Some(orig), Some((latest, w))) => (orig, latest, w),
        _ => return Ok(()), // No wallet yet, skip
    };

    let current_epoch = timestamp_to_epoch(sys_time()?);
    if wallet_entry.last_trial_epoch == current_epoch {
        wallet_entry.trial_tx_count = wallet_entry.trial_tx_count.saturating_add(1);
    } else {
        wallet_entry.trial_tx_count = 1;
        wallet_entry.last_trial_epoch = current_epoch;
    }

    wallet::update_wallet(wallet::UpdateWalletInput {
        original_wallet_hash,
        previous_wallet_hash: latest_wallet_hash,
        updated_wallet: wallet_entry,
    })?;

    Ok(())
}

/// Execute protocol side-effects for a transaction (Whitepaper Section 5).
///
/// This is called during create_transaction and update_transaction.
/// Side-effects are execution-conditional based on the caller's role (author).
pub fn reify_transaction_side_effects(transaction: &Transaction, transaction_hash: ActionHash) -> ExternResult<()> {
    if transaction.debt <= 0.0 {
        return Ok(());
    }

    let author = agent_info()?.agent_initial_pubkey;
    let seller_agent: AgentPubKey = transaction.seller.pubkey.clone().into();
    let buyer_agent: AgentPubKey = transaction.buyer.pubkey.clone().into();

    let my_agent = agent_info()?.agent_initial_pubkey;
    let is_drain = transaction.drain_metadata.is_some();
    let is_seller = Into::<AgentPubKey>::into(transaction.seller.pubkey.clone()) == my_agent;
    let is_buyer = Into::<AgentPubKey>::into(transaction.buyer.pubkey.clone()) == my_agent;
    let is_active_party = is_seller || is_buyer;

    debug!(
        "reify_side_effects: agent={}, is_buyer={}, is_seller={}, status={:?}",
        my_agent, is_buyer, is_seller, transaction.status
    );

    // 1. BUYER SIDE: Create debt contract
    // Only for PURCHASE transactions (not drains) and only when Accepted.
    if is_buyer && transaction.status == TransactionStatus::Accepted && !is_drain {
        contracts::create_debt_contract(CreateDebtContractInput {
            amount: transaction.debt,
            creditor: transaction.seller.pubkey.clone(),
            debtor: transaction.buyer.pubkey.clone(),
            transaction_hash: transaction_hash.clone(),
            is_trial: transaction.is_trial,
        })?;
    }

    // 2. ACTIVE SIDE: Execute protocol transitions when the transaction is Accepted.
    // (Wait: we also accept 'Initial' status for cell bootstrapping)
    if is_active_party
        && (transaction.status == TransactionStatus::Accepted || transaction.status == TransactionStatus::Initial)
    {
        if let Some(ref drain_meta) = transaction.drain_metadata {
            // ── DRAIN ACCEPTANCE ────────────────────────────────────────────
            // This transaction is a cascade drain request.

            if is_seller {
                // CASE 1: The Beneficiary (seller) approves the drain.
                // - Drains own active contracts (extinguishing their own debt).
                // - Fires sub-cascades to their own beneficiaries.
                debug!(
                    "reify_side_effects: DRAIN ACCEPTANCE (BENEFICIARY) — beneficiary={:?}, amount={}, depth={}",
                    author, drain_meta.allocated_amount, drain_meta.cascade_depth
                );

                let visited = drain_meta.visited.clone();
                let cascade_result = support_cascade::execute_support_cascade(
                    author.clone(), // Always drain the current cell's agent
                    drain_meta.allocated_amount,
                    visited,
                    drain_meta.parent_tx.clone(),
                    drain_meta.cascade_depth + 1,
                )?;

                debug!(
                    "reify_side_effects: DRAIN cascade result — own={}, requests_sent={}",
                    cascade_result.own_transferred, cascade_result.beneficiary_requests_sent
                );

                // Update acquaintances and trust for own transfers
                if !cascade_result.own_creditor_transfers.is_empty() {
                    trust::update_acquaintances_from_evidence(&cascade_result.own_creditor_transfers, &[])?;
                }

                // Record support satisfaction evidence for reputation from the supporter
                if cascade_result.own_transferred > DUST_THRESHOLD {
                    let supporter_key: AgentPubKeyB64 = transaction.buyer.pubkey.clone();
                    let current_epoch = timestamp_to_epoch(sys_time()?);
                    let tag = SupportSatisfactionTag {
                        supporter: supporter_key,
                        amount: cascade_result.own_transferred,
                        epoch: current_epoch,
                    };
                    let tag_bytes =
                        SerializedBytes::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
                    create_link(
                        author.clone(),
                        transaction_hash.clone(),
                        LinkTypes::AgentToSupportSatisfaction,
                        LinkTag(tag_bytes.bytes().clone()),
                    )?;

                    // Add supporter as acquaintance so they appear in pre-trust vector
                    let supporter_agent: AgentPubKey = transaction.buyer.pubkey.clone().into();
                    let _ = trust::add_acquaintance(supporter_agent);

                    debug!(
                        "reify_side_effects: beneficiary recorded support satisfaction — supporter={}, amount={}, epoch={}",
                        transaction.buyer.pubkey, cascade_result.own_transferred, current_epoch
                    );

                    // Invalidate trust caches so next reputation query picks up new evidence
                    crate::trust_cache::invalidate_all_caches();

                    // Publish updated trust row
                    let _ = trust::publish_trust_row(());
                }

                // Notify affected creditors to republish their trust rows
                if !cascade_result.own_creditor_transfers.is_empty() {
                    let zome_name = zome_info()?.name;
                    for (creditor_key, _amt) in &cascade_result.own_creditor_transfers {
                        let creditor_agent: AgentPubKey = creditor_key.clone().into();
                        if creditor_agent != author {
                            let _ = call_remote(
                                creditor_agent,
                                zome_name.clone(),
                                "notify_trust_row_refresh".into(),
                                None,
                                (),
                            );
                        }
                    }
                }
            }

            if is_buyer {
                // CASE 2: The Supporter (buyer) observes the drain being accepted.
                // This allows the supporter to trust the beneficiary when the beneficiary utilizes
                // the allocated support. This completes the trust loop in the EigenTrust graph.
                debug!(
                    "reify_side_effects: DRAIN OBSERVED (SUPPORTER) — supporter={:?}, total_allocated={}",
                    author, transaction.debt
                );

                if transaction.debt > DUST_THRESHOLD {
                    let beneficiary_key: AgentPubKeyB64 = transaction.seller.pubkey.clone();
                    let current_epoch = timestamp_to_epoch(sys_time()?);

                    // Record satisfaction for the beneficiary for the debt reduced.
                    // We use the same SupportSatisfactionTag mechanism as the beneficiary.
                    let tag = SupportSatisfactionTag {
                        supporter: beneficiary_key.clone(), // The beneficiary "satisfied" the supporter's trust
                        amount: transaction.debt,
                        epoch: current_epoch,
                    };
                    let tag_bytes =
                        SerializedBytes::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
                    create_link(
                        author.clone(),
                        transaction_hash.clone(),
                        LinkTypes::AgentToSupportSatisfaction,
                        LinkTag(tag_bytes.bytes().clone()),
                    )?;

                    // Add beneficiary as acquaintance so they appear in reputation subgraph
                    let beneficiary_agent: AgentPubKey = beneficiary_key.into();
                    let _ = trust::add_acquaintance(beneficiary_agent);

                    debug!(
                        "reify_side_effects: supporter recorded beneficiary satisfaction — beneficiary={}, amount={}, epoch={}",
                        transaction.seller.pubkey, transaction.debt, current_epoch
                    );

                    crate::trust_cache::invalidate_all_caches();

                    let _ = trust::publish_trust_row(());
                }
            }
        } else if is_seller {
            // ── PURCHASE ACCEPTANCE ─────────────────────────────────────────
            // Standard sale: drain seller's own debt + fire-and-forget drain
            // requests to each beneficiary in the breakdown.
            debug!(
                "reify_side_effects: PURCHASE ACCEPTANCE (SELLER) — author={:?}, seller={:?}, status={:?}, debt={}, invoking cascade",
                author, seller_agent, transaction.status, transaction.debt
            );
            let visited = vec![buyer_agent.clone().into()];
            let cascade_result = support_cascade::execute_support_cascade(
                author.clone(), // Always drain the current cell's agent
                transaction.debt,
                visited,
                transaction_hash.clone(),
                0,
            )?;

            debug!(
                "reify_side_effects: PURCHASE ACCEPTANCE (SELLER) — cascade result own={}. \
                  Acquaintance NOT added here: per Whitepaper §3.2 acquaintances are \
                  established strictly upon successful debt *transfer*, not on acceptance. \
                  The add_acquaintance call happens in contracts/transfer.rs \
                  via update_acquaintances_from_evidence after a successful transfer_debt.",
                cascade_result.own_transferred
            );
            // Do NOT call trust::add_acquaintance(buyer_agent) here.
            // Whitepaper §3.2 states: "An acquaintance relationship is established
            // strictly upon successful debt transfer (S_ij > 0), and explicitly NOT
            // upon transaction acceptance or proposal." Adding the buyer as an
            // acquaintance on acceptance (before any debt is transferred) opens a
            // Sybil-trial-inflation vector: an unvouched identity can raise the
            // seller's |A| count just by having a trial accepted, increasing the
            // integrity-zome capacity ceiling before repaying anything.
            // The correct site is contracts/transfer.rs::update_acquaintances_from_evidence.
            let _ = trust::publish_trust_row(());
        }
    } else {
        debug!("reify_side_effects: NO-OP for status={:?} or not active party author={:?}", transaction.status, author);
    }

    Ok(())
}
