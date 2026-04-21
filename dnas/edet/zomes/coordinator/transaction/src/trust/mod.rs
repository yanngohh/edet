pub mod acquaintances;
pub mod attenuation;
pub mod capacity;
pub mod contagion;
mod eigentrust;
pub mod reputation;
pub mod risk;
pub mod sf_counters;

use std::collections::{HashMap, HashSet, VecDeque};

use hdk::prelude::*;
use transaction_integrity::debt_contract::DebtContract;
use transaction_integrity::types::constants::*;
use transaction_integrity::types::{timestamp_to_epoch, TrustLinkTag};
use transaction_integrity::*;

use crate::contracts;
use crate::trust_cache::{
    self, cache_reputation_result, get_cached_reputation, get_cached_trust_row_for_agent, get_or_build_cached_subgraph,
    get_trust_rows_batch, CachedTrustSubgraph,
};
use std::rc::Rc;

// Re-export all public items so external code using `crate::trust::X` continues to work.
pub use crate::capacity::compute_credit_capacity;
pub use acquaintances::{add_acquaintance, get_acquaintances, remove_acquaintance, update_acquaintances_from_evidence};
pub use attenuation::{
    compute_local_trust_row, compute_local_trust_row_from_sf, compute_local_trust_row_from_sf_with_contagion,
    get_pre_trust_distribution, trust_attenuation, trust_attenuation_with_contagion,
};
pub use capacity::{compute_credit_capacity_for_agent, get_credit_capacity};
pub use contagion::{
    get_aggregate_witness_rate, get_failure_witness_count, get_failure_witnesses, get_witness_contagion_data,
    publish_failure_observation,
};
pub(crate) use eigentrust::power_iteration;
pub use reputation::{
    compute_risk_from_claim, ensure_fresh_claim, get_reputation_claim, get_reputation_claim_action_hash,
    is_claim_fresh, publish_reputation_claim, ReputationClaimLinkTag,
};
pub use risk::{compute_full_risk_score, compute_transaction_status, is_bootstrap_eligible, is_trial_transaction};
pub use sf_counters::{compute_sf_counters, SFCounters};

// =========================================================================
//  Public API: Subjective Reputation
// =========================================================================

/// Full reputation computation result.
#[derive(Serialize, Deserialize, Debug)]
pub struct ReputationResult {
    pub trust: f64,
    pub acquaintance_count: usize,
}

/// Compute the subjective reputation of a target agent from the current
/// agent's perspective. This is the main entry point for reputation queries.
///
/// Implements the Subjective Local Expansion strategy:
/// 1. Check cache for pre-computed result
/// 2. Process any pending contract expirations (lazy epoch tick)
/// 3. Build trust subgraph from acquaintance set (cached)
/// 4. Run power iteration with personalized pre-trust
/// 5. Cache and return t^(i)_target
///
/// With caching enabled, repeat queries within an epoch return instantly.
#[hdk_extern]
pub fn get_subjective_reputation(target: AgentPubKey) -> ExternResult<ReputationResult> {
    let observer = agent_info()?.agent_initial_pubkey;
    get_subjective_reputation_as_observer(target, observer)
}

/// Compute subjective reputation of a target from a specific observer's perspective.
pub fn get_subjective_reputation_as_observer(
    target: AgentPubKey,
    observer: AgentPubKey,
) -> ExternResult<ReputationResult> {
    // 1. Check cache first (cache key must include observer)
    // NOTE: For now, we assume observer is self for caching purposes to simplify.
    // If not self, we bypass cache.
    let is_self = observer == agent_info()?.agent_initial_pubkey;
    if is_self {
        if let Some((trust, acquaintance_count)) = get_cached_reputation(&target)? {
            return Ok(ReputationResult { trust, acquaintance_count });
        }
    }

    // Lazy epoch processing: expire any overdue contracts
    contracts::process_contract_expirations(())?;

    // 3. Recompute and publish our trust row if we are the observer
    if is_self {
        debug!("get_subjective_reputation: publishing trust row for self");
        publish_trust_row(())?;
    }

    // 4. Build subgraph via Subjective Local Expansion (cached if self)
    debug!("get_subjective_reputation: building subgraph");
    let subgraph =
        if is_self { build_trust_subgraph(observer.clone())? } else { build_raw_subgraph_rc(observer.clone())? };

    debug!("get_subjective_reputation: subgraph size={}", subgraph.size());

    if subgraph.size() < 2 {
        // Matches simulation (universe.py:641-644): a lone node (or empty
        // subgraph) has no meaningful trust evidence.  Self-trust on a 1-node
        // subgraph would yield trust = 1.0, inflating capacity without any
        // real economic proof.  Return trust = 0 so that capacity depends
        // solely on V_staked for nodes with no bilateral history yet.
        // Also ensure self-reputation check doesn't bypass this.
        let acquaintance_count =
            if is_self { get_acquaintances(())?.len() } else { query_acquaintances(observer.clone())?.len() };
        return Ok(ReputationResult { trust: 0.0, acquaintance_count });
    }

    // 5. Build personalized pre-trust vector p^(i) (Whitepaper Definition 3.8)
    //    Distribute pre-trust by bilateral volume (attenuated), not uniformly.
    //    The observer's volume-weighted pre-trust ensures that acquaintances with
    //    more economic history receive higher teleportation probability.
    let acquaintances = if is_self { get_acquaintances(())? } else { query_acquaintances(observer.clone())? };
    let acquaintance_count = acquaintances.len();

    // Build volume-weighted pre-trust using get_pre_trust_distribution (Def 3.8)
    let pre_trust_dist = get_pre_trust_distribution(Some(observer.clone()))?;
    debug!("observer={:?} pre_trust_dist keys={:?}", observer, pre_trust_dist.keys().collect::<Vec<_>>());

    let mut pre_trust = vec![0.0f64; subgraph.size()];
    let mut pre_trust_assigned = 0.0;

    for (agent_key, weight) in &pre_trust_dist {
        if let Some(&idx) = subgraph.agent_index.get(agent_key) {
            pre_trust[idx] = *weight;
            pre_trust_assigned += *weight;
        } else {
            debug!("agent {:?} NOT in subgraph", agent_key);
        }
    }

    // If no acquaintances mapped into the subgraph, fall back to uniform over subgraph
    if pre_trust_assigned == 0.0 {
        debug!(
            "pre_trust_assigned=0, falling back to uniform (subgraph keys={:?})",
            subgraph.agent_index.keys().collect::<Vec<_>>()
        );
        if target == observer {
            return Ok(ReputationResult { trust: 0.0, acquaintance_count });
        }
        let val = 1.0 / subgraph.size() as f64;
        for p in pre_trust.iter_mut() {
            *p = val;
        }
    } else {
        debug!("get_subjective_reputation: pre_trust_assigned={}, normalizing", pre_trust_assigned);
        // Re-normalize to sum to 1.0 (some acquaintances may not be in subgraph)
        let sum: f64 = pre_trust.iter().sum();
        if sum > 0.0 && (sum - 1.0).abs() > 1e-9 {
            for p in pre_trust.iter_mut() {
                *p /= sum;
            }
        }
    }

    // 6. Run power iteration
    debug!("get_subjective_reputation: running power_iteration");
    let reputation = power_iteration(&subgraph, &pre_trust)?;
    debug!("get_subjective_reputation: power_iteration finished");

    let target_key: AgentPubKeyB64 = target.clone().into();
    let target_trust = if let Some(&idx) = subgraph.agent_index.get(&target_key) { reputation[idx] } else { 0.0 };

    if is_self {
        cache_reputation_result(target.clone(), target_trust, acquaintance_count)?;
    }

    Ok(ReputationResult { trust: target_trust, acquaintance_count })
}

// =========================================================================
//  Local Trust Computation & DHT Publication (Whitepaper Section 7.1)
// =========================================================================

/// Publish the current agent's local trust row to the DHT as AgentToLocalTrust links.
/// Uses diff-based publishing: only deletes/creates links that have actually changed,
/// reducing source chain writes from 2*N to 2*delta in steady state.
#[hdk_extern]
pub fn publish_trust_row(_: ()) -> ExternResult<()> {
    let agent = agent_info()?.agent_initial_pubkey;
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    // Lazy epoch tick: expire any overdue contracts before computing the trust row.
    // This ensures that S/F counters computed below already reflect contracts that
    // matured since the last reputation query, so trust rows stay accurate even when
    // publish_trust_row is called independently (e.g. via notify_trust_row_refresh).
    contracts::process_contract_expirations(())?;

    // Compute current trust row with contagion.
    // F>S pruning has been removed (Change 2): acquaintance eviction is Dunbar-cap only.
    // Failure observation publishing has been moved to process_contract_expirations (Change 2).
    let sf_counters = compute_sf_counters(agent.clone())?;

    // Proactive acquaintance maintenance: update acquaintances based on economic evidence.
    // This breaks recursion because compute_sf_counters is now pure.
    for (debtor_key, counters) in &sf_counters {
        let debtor: AgentPubKey = debtor_key.clone().into();
        if debtor != agent {
            if counters.satisfaction > DUST_THRESHOLD {
                let _ = add_acquaintance(debtor);
            } else if counters.failure > DUST_THRESHOLD && counters.failure > counters.satisfaction {
                let _ = remove_acquaintance(debtor);
            }
        }
    }

    let new_trust_row = compute_local_trust_row_from_sf_with_contagion(&sf_counters)?;

    // Fetch existing trust links and deserialize their tags
    let existing_links =
        get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToLocalTrust)?, GetStrategy::default())?;

    let mut existing_map: HashMap<AgentPubKeyB64, (f64, ActionHash)> = HashMap::new();
    for link in &existing_links {
        let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.clone().into_inner()));
        if let Ok(trust_tag) = TrustLinkTag::try_from(tag_bytes) {
            if let Some(target_agent) = link.target.clone().into_agent_pub_key() {
                let target_key: AgentPubKeyB64 = target_agent.into();
                existing_map.insert(target_key, (trust_tag.trust_value, link.create_link_hash.clone()));
            }
        }
    }

    // Epsilon for considering a trust value "unchanged"
    const TRUST_DIFF_EPSILON: f64 = 0.0001;

    // Diff: find links to delete (removed or changed targets)
    for (target_key, (old_value, link_hash)) in &existing_map {
        match new_trust_row.get(target_key) {
            None => {
                // Target removed from trust row: delete link
                delete_link(link_hash.clone(), GetOptions::default())?;
            }
            Some(new_value) => {
                if (new_value - old_value).abs() > TRUST_DIFF_EPSILON {
                    // Value changed significantly: delete old link (will recreate below)
                    delete_link(link_hash.clone(), GetOptions::default())?;
                }
                // else: unchanged, keep existing link
            }
        }
    }

    // Diff: create links for new or changed targets
    for (target_key, trust_value) in &new_trust_row {
        let should_create = match existing_map.get(target_key) {
            None => true,                                                                 // New target
            Some((old_value, _)) => (trust_value - old_value).abs() > TRUST_DIFF_EPSILON, // Changed
        };

        if should_create {
            let target_agent: AgentPubKey = target_key.clone().into();
            let tag = TrustLinkTag { trust_value: *trust_value, epoch: current_epoch };
            let tag_bytes = SerializedBytes::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
            create_link(agent.clone(), target_agent, LinkTypes::AgentToLocalTrust, LinkTag(tag_bytes.bytes().clone()))?;
        }
    }

    Ok(())
}

/// Fetch an agent's published trust row from the DHT.
/// Returns { target_agent => c_ij } deserialized from link tags.
/// Uses caching for performance.
#[hdk_extern]
pub fn get_trust_row_for_agent(agent: AgentPubKey) -> ExternResult<HashMap<AgentPubKeyB64, f64>> {
    get_cached_trust_row_for_agent(agent)
}

// =========================================================================
//  EigenTrust: Subjective Local Expansion (Whitepaper Section 7.1)
//
//  1. Start from observer's acquaintance set A_i
//  2. Fetch trust rows to depth SUBGRAPH_MAX_DEPTH
//  3. Run power iteration on the subgraph
//
//  Now with caching: subgraph is cached per-epoch to avoid redundant DHT lookups.
// =========================================================================

/// Build a trust subgraph by fetching trust links from the DHT,
/// starting from the observer's acquaintance set and expanding to depth d.
/// Uses caching - the subgraph is reused within an epoch.
fn build_trust_subgraph(_observer: AgentPubKey) -> ExternResult<Rc<CachedTrustSubgraph>> {
    // Use cache-aware function that returns cached subgraph if available
    let (subgraph, _was_cached) = get_or_build_cached_subgraph(build_trust_subgraph_uncached)?;
    Ok(subgraph)
}

/// Build trust subgraph without caching (internal helper).
/// Uses batched DHT fetching for performance (Phase 4 optimization).
fn build_trust_subgraph_uncached() -> ExternResult<CachedTrustSubgraph> {
    let acquaintances = get_acquaintances(())?;
    let mut subgraph = CachedTrustSubgraph::new();
    let mut visited: HashSet<AgentPubKey> = HashSet::new();
    let mut frontier: Vec<AgentPubKey> = acquaintances;

    for _depth in 0..SUBGRAPH_MAX_DEPTH {
        // Circuit breaker: stop expanding if subgraph is already large
        if subgraph.size() >= MAX_SUBGRAPH_NODES {
            break;
        }

        // Filter out already-visited agents from the frontier
        let agents_to_fetch: Vec<AgentPubKey> = frontier.iter().filter(|a| !visited.contains(*a)).cloned().collect();

        if agents_to_fetch.is_empty() {
            break;
        }

        // Batch fetch all trust rows for this frontier level (Phase 4 optimization)
        let trust_rows_batch = get_trust_rows_batch(&agents_to_fetch)?;

        let mut next_frontier: Vec<AgentPubKey> = Vec::new();

        for agent in &agents_to_fetch {
            visited.insert(agent.clone());
            let from_idx = subgraph.get_or_insert_agent(agent);

            // Use the batched result
            if let Some(trust_row) = trust_rows_batch.get(agent) {
                for (target_key, trust_value) in trust_row {
                    // Circuit breaker per-agent: stop adding new nodes if at limit
                    if subgraph.size() >= MAX_SUBGRAPH_NODES {
                        // Still record trust for already-known agents
                        if let Some(&to_idx) = subgraph.agent_index.get(target_key) {
                            subgraph.trust_rows[from_idx].insert(to_idx, *trust_value);
                        }
                        continue;
                    }

                    let target_agent: AgentPubKey = target_key.clone().into();
                    let to_idx = subgraph.get_or_insert_agent(&target_agent);
                    subgraph.trust_rows[from_idx].insert(to_idx, *trust_value);

                    // Add to next frontier for deeper exploration
                    if !visited.contains(&target_agent) {
                        next_frontier.push(target_agent);
                    }
                }
            }
        }

        if next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
    }

    Ok(subgraph)
}

/// Check if there is bilateral history between the seller (current agent) and a buyer.
///
/// Uses a lightweight check that only queries creditor contract links and checks
/// debtor fields, with early-exit on first match. This avoids computing full
/// SF counters just to check for key existence.
#[hdk_extern]
pub fn check_bilateral_history(buyer: AgentPubKey) -> ExternResult<bool> {
    let seller = agent_info()?.agent_initial_pubkey;
    let buyer_key: AgentPubKeyB64 = buyer.into();

    // Query creditor contracts for the seller
    let links = get_links(LinkQuery::try_new(seller, LinkTypes::CreditorToContracts)?, GetStrategy::default())?;

    let get_inputs: Vec<GetInput> = links
        .into_iter()
        .filter_map(|link| {
            link.target
                .into_action_hash()
                .map(|hash| GetInput::new(hash.into(), GetOptions::default()))
        })
        .collect();

    // Batch fetch but check each record for the buyer's key — early exit on match
    let records: Vec<Option<Record>> = HDK.with(|hdk| hdk.borrow().get(get_inputs))?;

    for record in records.into_iter().flatten() {
        if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
            if contract.debtor == buyer_key {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Check if an observer has any bilateral history with a target agent.
pub fn check_bilateral_history_as_observer(target: AgentPubKey, observer: AgentPubKey) -> ExternResult<bool> {
    let sf = compute_sf_counters(observer)?;
    Ok(sf.contains_key(&target.into()))
}

/// Helper to build a subgraph for an arbitrary observer without caching, returning Rc.
fn build_raw_subgraph_rc(observer: AgentPubKey) -> ExternResult<Rc<CachedTrustSubgraph>> {
    let subgraph = build_raw_subgraph(observer)?;
    Ok(Rc::new(subgraph))
}

/// Helper to build a subgraph for an arbitrary observer without caching.
/// Uses the same MAX_SUBGRAPH_NODES circuit breaker and SUBGRAPH_MAX_DEPTH
/// limit as the primary `build_trust_subgraph_uncached` to ensure consistent
/// reputation accuracy for non-self observer queries.
///
/// Seed BFS frontier from A_observer (observer's acquaintances), not
/// from {observer} alone. Whitepaper Theorem `thm:subgraph_approx` describes
/// the subgraph as "built starting from A_observer"; seeding from {observer}
/// instead shifts the effective depth by 1 (since depth=0 expands the observer
/// rather than their direct acquaintances at depth=0). This made non-self
/// subgraphs one level shallower than self subgraphs, inconsistently applying
/// the SUBGRAPH_MAX_DEPTH limit.
fn build_raw_subgraph(observer: AgentPubKey) -> ExternResult<CachedTrustSubgraph> {
    let mut subgraph = CachedTrustSubgraph::new();
    let mut queue = VecDeque::new();

    // Seed frontier from observer's acquaintances (A_observer), matching the
    // whitepaper §5 Subjective Local Expansion start set.
    let observer_acquaintances = query_acquaintances(observer.clone())?;
    subgraph.get_or_insert_agent(&observer);
    for acq in &observer_acquaintances {
        subgraph.get_or_insert_agent(acq);
        queue.push_back((acq.clone(), 0u32)); // depth 0 = direct acquaintance
    }
    if observer_acquaintances.is_empty() {
        // Fallback: if no acquaintances are known via DHT, start from observer.
        queue.push_back((observer.clone(), 0u32));
    }

    let mut visited = HashSet::new();
    // Mark observer and direct acquaintances as visited to skip re-expansion.
    visited.insert(observer.clone());
    for acq in &observer_acquaintances {
        visited.insert(acq.clone());
    }

    // Subjective Local Expansion (BFS limited by MAX_SUBGRAPH_NODES and SUBGRAPH_MAX_DEPTH)
    while let Some((current, depth)) = queue.pop_front() {
        if subgraph.size() >= MAX_SUBGRAPH_NODES {
            break;
        }
        if depth >= SUBGRAPH_MAX_DEPTH {
            continue;
        }
        if !visited.insert(current.clone()) {
            continue;
        }

        let trust_row = get_cached_trust_row_for_agent(current.clone())?;
        let current_idx = subgraph.get_or_insert_agent(&current);

        for (peer_b64, trust_val) in trust_row {
            let peer = AgentPubKey::from(peer_b64.clone());
            // Circuit breaker: stop adding new nodes at limit, but still
            // record trust for already-known agents (same as build_trust_subgraph_uncached).
            if subgraph.size() >= MAX_SUBGRAPH_NODES {
                if let Some(&to_idx) = subgraph.agent_index.get(&peer_b64) {
                    subgraph.trust_rows[current_idx].insert(to_idx, trust_val);
                }
                continue;
            }
            let peer_idx = subgraph.get_or_insert_agent(&peer);
            subgraph.trust_rows[current_idx].insert(peer_idx, trust_val);
            if !visited.contains(&peer) {
                queue.push_back((peer, depth + 1));
            }
        }
    }
    Ok(subgraph)
}

/// Helper to query acquaintances for an arbitrary observer.
fn query_acquaintances(observer: AgentPubKey) -> ExternResult<Vec<AgentPubKey>> {
    let links =
        get_links(LinkQuery::try_new(observer.clone(), LinkTypes::AgentToAcquaintance)?, GetStrategy::default())?;
    let mut acquaintances: Vec<AgentPubKey> =
        links.into_iter().filter_map(|link| link.target.into_agent_pub_key()).collect();
    if !acquaintances.contains(&observer) {
        acquaintances.push(observer);
    }
    Ok(acquaintances)
}

// =========================================================================
//  Cache Management API
// =========================================================================

/// Get statistics about the trust cache.
/// Useful for monitoring and debugging cache behavior.
#[hdk_extern]
pub fn get_trust_cache_stats(_: ()) -> ExternResult<trust_cache::CacheStats> {
    Ok(trust_cache::get_cache_stats())
}

/// Manually invalidate all trust caches.
/// Call this after significant state changes or for testing.
#[hdk_extern]
pub fn invalidate_trust_caches(_: ()) -> ExternResult<()> {
    trust_cache::invalidate_all_caches();
    Ok(())
}

/// Remote handler: refresh trust row after drain cascade affects this agent's contracts.
///
/// Called via `call_remote` (fire-and-forget) by a beneficiary after their drain cascade
/// transfers debt from this agent's contracts. The creditor's trust row becomes stale
/// because the contract updates (Transferred status) happened on the beneficiary's cell
/// via `transfer_debt`. This handler invalidates caches, processes any pending expirations,
/// and republishes the trust row so that the bidirectional trust loop (creditor → beneficiary)
/// is established in the EigenTrust subgraph, allowing the beneficiary's self-reputation
/// to become non-zero.
///
/// Rate-limiting: A malicious peer could call this repeatedly to force expensive
/// recomputation (EigenTrust + source-chain scan + DHT writes) on the target cell.
/// We defend against this by tracking the last refresh timestamp in the trust cache
/// and ignoring calls that arrive within TRUST_CACHE_TTL_SECS of the previous one.
/// Legitimate callers (drain cascade) fire this at most once per cascade chain, so
/// legitimate requests are never dropped.
#[hdk_extern]
pub fn notify_trust_row_refresh(_: ()) -> ExternResult<()> {
    use transaction_integrity::types::constants::TRUST_CACHE_TTL_SECS;

    let now = sys_time()?;
    let now_secs = now.as_seconds_and_nanos().0 as u64;

    // Rate-limit: skip the expensive refresh if one already ran within TTL window.
    if let Some(last_refresh) = trust_cache::get_last_trust_row_refresh_secs() {
        let elapsed = now_secs.saturating_sub(last_refresh);
        if elapsed < TRUST_CACHE_TTL_SECS {
            debug!("notify_trust_row_refresh: skipping (last refresh {}s ago, TTL {}s)", elapsed, TRUST_CACHE_TTL_SECS);
            return Ok(());
        }
    }

    trust_cache::set_last_trust_row_refresh_secs(now_secs);
    trust_cache::invalidate_all_caches();
    // Process expirations here too: remote notifications can arrive at any epoch
    // boundary and the creditor may have overdue contracts that haven't been expired yet.
    contracts::process_contract_expirations(())?;
    publish_trust_row(())?;
    Ok(())
}
