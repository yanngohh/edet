use hdk::prelude::*;
use transaction_integrity::debt_contract::DebtContract;
use transaction_integrity::types::{timestamp_to_epoch, DebtBalanceTag};
use transaction_integrity::*;

use super::get_active_contracts_for_debtor;

/// Get total outstanding debt for an agent (sum of active contract amounts as debtor).
///
/// When the caller IS the agent (i.e. reading own debt), reads the current
/// `AgentToDebtBalance` link from the **local DHT store** using `GetStrategy::Local`.
/// This avoids a DHT network round-trip that can return stale data during gossip
/// propagation windows — which in turn caused `rebuild_debt_balance` to overwrite
/// a correct (cascade-written) balance with a stale value.
///
/// For cross-agent queries (reading someone else's debt), the DHT `get_links`
/// network path is still used because the target agent's DHT shard is remote.
///
/// Falls back to `rebuild_debt_balance` only when no balance link has ever been
/// written (first access / migration).
#[hdk_extern]
pub fn get_total_debt(agent: AgentPubKey) -> ExternResult<f64> {
    let my_agent = agent_info()?.agent_initial_pubkey;

    if agent == my_agent {
        // ── LOCAL PATH: read from local DHT store (authoritative, no network) ──
        let links = get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToDebtBalance)?, GetStrategy::Local)?;

        if let Some(link) = links.first() {
            let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.clone().into_inner()));
            if let Ok(balance_tag) = DebtBalanceTag::try_from(tag_bytes) {
                return Ok(balance_tag.total_debt);
            }
        }

        // No balance link on chain yet → first access, initialize via scan
        return rebuild_debt_balance(agent, None);
    }

    // ── REMOTE PATH: read from DHT network (best-effort, eventual consistency) ──
    let links = get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToDebtBalance)?, GetStrategy::Network)?;

    if let Some(link) = links.first() {
        let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.clone().into_inner()));
        if let Ok(balance_tag) = DebtBalanceTag::try_from(tag_bytes) {
            return Ok(balance_tag.total_debt);
        }
    }

    // Fallback for foreign agent: scan their active contracts directly without
    // writing a balance link (we cannot write a link with a foreign agent as base
    // on our own chain — that would fail integrity validation).
    let records = get_active_contracts_for_debtor(agent)?;
    let total: f64 = records
        .iter()
        .filter_map(|r| r.entry().to_app_option::<DebtContract>().ok().flatten())
        .map(|c| c.amount)
        .sum();
    Ok(total)
}

/// Update the running debt balance by a delta amount.
///
/// Reads the current AgentToDebtBalance link, deletes it, and creates a new one
/// with the updated total. This is O(1) per call.
///
/// `delta` is positive when debt increases (contract created), negative when
/// debt decreases (transfer/expiration).
/// `contract_count_delta` is +1 for new contract, -1 for fully resolved, 0 for partial.
///
/// When `agent` is the calling agent (always true in practice — all callers run
/// on the agent's own cell), reads and deletes balance links from the **local DHT
/// store** (`GetStrategy::Local`) rather than the gossip network.  The local DHT
/// store is authoritative for self-links (the same node both writes and is authority
/// for agent-keyed DHT locations), so deletes written in the same zome call are
/// immediately visible on the next `GetStrategy::Local` read.
///
/// The DHT path is kept as an unreachable fallback for cross-agent calls (which
/// should never happen but are type-permitted by the `AgentPubKey` argument).
///
/// Concurrent-write safety: the `sequence` field in `DebtBalanceTag` is
/// incremented on every write.  After deleting the old link, if a second
/// concurrent write has already created a new link with a higher sequence
/// (only possible in theory since Holochain source chains are serialised per
/// agent), a full `rebuild_debt_balance` is triggered.  The rebuild carries
/// forward the highest observed sequence so that the resulting link's sequence
/// is strictly greater than any concurrent writer's sequence — preventing the
/// concurrent-write detector from firing spuriously on the next call.
pub fn update_debt_balance(agent: AgentPubKey, delta: f64, contract_count_delta: i64) -> ExternResult<f64> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    let my_agent = agent_info()?.agent_initial_pubkey;

    if agent == my_agent {
        // ── LOCAL PATH: read + delete from local DHT store (authoritative) ───────
        let links = get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToDebtBalance)?, GetStrategy::Local)?;

        let (current_debt, current_count, current_seq) = if let Some(link) = links.first() {
            let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.clone().into_inner()));
            if let Ok(bt) = DebtBalanceTag::try_from(tag_bytes) {
                (bt.total_debt, bt.contract_count, bt.sequence)
            } else {
                (0.0, 0u64, 0u64)
            }
        } else {
            (0.0, 0u64, 0u64)
        };

        // Delete all existing balance links (should be at most one)
        for link in &links {
            delete_link(link.create_link_hash.clone(), GetOptions::default())?;
        }

        // After deleting, re-check for a concurrent write: if another link already
        // exists with a higher sequence, we lost the race and must rebuild from the
        // canonical contract list to avoid applying a stale delta.
        // Pass the concurrent link's sequence to rebuild_debt_balance so the
        // rebuilt link gets a sequence strictly above the concurrent writer's,
        // preventing a false-positive concurrent-write detection on the next call.
        let links_after =
            get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToDebtBalance)?, GetStrategy::Local)?;
        if let Some(concurrent_link) = links_after.first() {
            let tag_bytes = SerializedBytes::from(UnsafeBytes::from(concurrent_link.tag.clone().into_inner()));
            if let Ok(bt) = DebtBalanceTag::try_from(tag_bytes) {
                if bt.sequence > current_seq {
                    // Concurrent write detected — rebuild from authoritative source,
                    // starting sequence above the concurrent writer's to avoid a
                    // false-positive detection loop.
                    warn!(
                        "update_debt_balance: concurrent write detected (seq {} > {}), rebuilding from seq {}",
                        bt.sequence, current_seq, bt.sequence
                    );
                    return rebuild_debt_balance(agent, Some(bt.sequence));
                }
            }
        }

        let new_debt = (current_debt + delta).max(0.0);
        let new_count = (current_count as i64 + contract_count_delta).max(0) as u64;
        let new_seq = current_seq.saturating_add(1);

        let tag =
            DebtBalanceTag { total_debt: new_debt, epoch: current_epoch, contract_count: new_count, sequence: new_seq };
        let tag_bytes = SerializedBytes::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;

        // Self-link: agent -> agent with balance in tag
        create_link(agent.clone(), agent, LinkTypes::AgentToDebtBalance, LinkTag(tag_bytes.bytes().clone()))?;

        return Ok(new_debt);
    }

    // ── DHT FALLBACK: cross-agent update (should not occur in practice) ───────
    let links = get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToDebtBalance)?, GetStrategy::Network)?;

    let (current_debt, current_count, current_seq) = if let Some(link) = links.first() {
        let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.clone().into_inner()));
        if let Ok(balance_tag) = DebtBalanceTag::try_from(tag_bytes) {
            (balance_tag.total_debt, balance_tag.contract_count, balance_tag.sequence)
        } else {
            (0.0, 0u64, 0u64)
        }
    } else {
        (0.0, 0u64, 0u64)
    };

    for link in &links {
        delete_link(link.create_link_hash.clone(), GetOptions::default())?;
    }

    let new_debt = (current_debt + delta).max(0.0);
    let new_count = (current_count as i64 + contract_count_delta).max(0) as u64;
    let new_seq = current_seq.saturating_add(1);

    let tag =
        DebtBalanceTag { total_debt: new_debt, epoch: current_epoch, contract_count: new_count, sequence: new_seq };
    let tag_bytes = SerializedBytes::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;

    create_link(agent.clone(), agent, LinkTypes::AgentToDebtBalance, LinkTag(tag_bytes.bytes().clone()))?;

    Ok(new_debt)
}

/// Rebuild the debt balance from scratch by scanning all active contracts.
///
/// Called on first access (when no AgentToDebtBalance link exists) or
/// for recovery/consistency checks after a concurrent-write is detected.
///
/// `min_sequence`: when Some(n), the rebuilt link's sequence will be n+1 rather
/// than 0.  This is used by `update_debt_balance` when it detects a concurrent
/// write — the rebuild must produce a sequence strictly above the concurrent
/// writer's sequence so that the next caller of `update_debt_balance` does not
/// incorrectly interpret the rebuilt link as a new concurrent write.
///
/// When called from `get_total_debt` on first access, `min_sequence` is None
/// and the sequence starts at 1 (so that any future update can distinguish a
/// freshly-built link from an absent one).
pub fn rebuild_debt_balance(agent: AgentPubKey, min_sequence: Option<u64>) -> ExternResult<f64> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    let records = get_active_contracts_for_debtor(agent.clone())?;
    let mut total_debt = 0.0f64;
    let mut contract_count = 0u64;

    for record in &records {
        if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
            total_debt += contract.amount;
            contract_count += 1;
        }
    }

    // Delete any existing balance links (local store — authoritative for self-links)
    let existing_links =
        get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToDebtBalance)?, GetStrategy::Local)?;
    for link in &existing_links {
        delete_link(link.create_link_hash.clone(), GetOptions::default())?;
    }

    // Compute the sequence for the rebuilt link:
    //   - If min_sequence is provided (concurrent-write recovery), use min_sequence + 1
    //     so that the rebuilt link's sequence is strictly above the concurrent writer's.
    //   - Otherwise, start at 1 (distinguishable from absent; first update goes to 2).
    let sequence = match min_sequence {
        Some(s) => s.saturating_add(1),
        None => 1,
    };

    // Create the balance link
    let tag = DebtBalanceTag { total_debt, epoch: current_epoch, contract_count, sequence };
    let tag_bytes = SerializedBytes::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    create_link(agent.clone(), agent, LinkTypes::AgentToDebtBalance, LinkTag(tag_bytes.bytes().clone()))?;

    Ok(total_debt)
}
