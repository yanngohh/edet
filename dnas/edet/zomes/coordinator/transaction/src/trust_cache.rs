//! Trust Cache Module
//!
//! Implements epoch-scoped in-memory caching for trust computations to enable
//! billion-scale networks. Without caching, every reputation query triggers
//! multiple DHT round-trips; with caching, repeat queries within an epoch
//! return instantly from memory.
//!
//! Cache invalidation is epoch-based: all cached data is automatically stale
//! when the epoch changes. Within an epoch, cached data is valid for
//! TRUST_CACHE_TTL_SECS (configurable).
//!
//! # Scalability Impact
//!
//! - First-contact transactions: O(1) via ReputationClaim (unchanged)
//! - Repeat transactions within epoch: O(1) via cache hit
//! - First transaction of epoch: O(|A| × d) to rebuild cache
//!
//! This enables high-frequency trading without DHT saturation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use hdk::prelude::*;
use transaction_integrity::types::constants::*;
use transaction_integrity::types::timestamp_to_epoch;
use transaction_integrity::LinkTypes;

// =========================================================================
//  Cache Structures
// =========================================================================

/// Cached trust subgraph for a specific observer.
#[derive(Clone, Debug)]
pub struct CachedTrustSubgraph {
    /// Indexed list of agents in the subgraph.
    pub agents: Vec<AgentPubKey>,
    /// Agent index lookup.
    pub agent_index: HashMap<AgentPubKeyB64, usize>,
    /// trust_matrix[i] = { j => c_ij } (local trust from agent i to agent j)
    pub trust_rows: Vec<HashMap<usize, f64>>,
    /// Epoch when this subgraph was built.
    pub epoch: u64,
    /// Timestamp when this was cached (for TTL).
    pub cached_at: Timestamp,
}

impl Default for CachedTrustSubgraph {
    fn default() -> Self {
        Self::new()
    }
}

impl CachedTrustSubgraph {
    pub fn new() -> Self {
        CachedTrustSubgraph {
            agents: Vec::new(),
            agent_index: HashMap::new(),
            trust_rows: Vec::new(),
            epoch: 0,
            cached_at: Timestamp::from_micros(0),
        }
    }

    pub fn get_or_insert_agent(&mut self, agent: &AgentPubKey) -> usize {
        let agent_key = AgentPubKeyB64::from(agent.clone());
        if let Some(&idx) = self.agent_index.get(&agent_key) {
            idx
        } else {
            let idx = self.agents.len();
            self.agents.push(agent.clone());
            self.agent_index.insert(agent_key, idx);
            self.trust_rows.push(HashMap::new());
            idx
        }
    }

    pub fn size(&self) -> usize {
        self.agents.len()
    }
}

/// Cached local trust row (agent's normalized c_ij values).
#[derive(Clone, Debug)]
pub struct CachedTrustRow {
    pub trust_row: HashMap<AgentPubKeyB64, f64>,
    pub epoch: u64,
    pub cached_at: Timestamp,
}

/// Cached acquaintance set.
#[derive(Clone, Debug)]
pub struct CachedAcquaintances {
    pub acquaintances: Vec<AgentPubKey>,
    pub epoch: u64,
    pub cached_at: Timestamp,
}

/// Cached reputation result for a specific (observer, target) pair.
#[derive(Clone, Debug)]
pub struct CachedReputationResult {
    pub trust: f64,
    pub acquaintance_count: usize,
    pub epoch: u64,
    pub cached_at: Timestamp,
}

/// Cached SF counter results for a specific creditor.
#[derive(Clone, Debug)]
pub struct CachedSFCounters {
    /// Tuple: (satisfaction, failure, first_seen_epoch, recent_satisfaction, recent_failure)
    pub counters: HashMap<AgentPubKeyB64, (f64, f64, u64, f64, f64)>,
    pub epoch: u64,
    pub cached_at: Timestamp,
}

/// The main trust cache holding all cached data for the current agent.
#[derive(Debug)]
pub struct TrustCache {
    /// Cached trust subgraph (built from acquaintances). Wrapped in Rc to
    /// avoid deep-copying the entire subgraph on every cache read.
    pub subgraph: Option<Rc<CachedTrustSubgraph>>,
    /// Cached local trust row (our own c_ij values).
    pub local_trust_row: Option<CachedTrustRow>,
    /// Cached acquaintance set.
    pub acquaintances: Option<CachedAcquaintances>,
    /// Cached reputation results: target_agent -> CachedReputationResult.
    pub reputation_results: HashMap<AgentPubKeyB64, CachedReputationResult>,
    /// Cached trust rows fetched from DHT: agent -> CachedTrustRow.
    pub dht_trust_rows: HashMap<AgentPubKeyB64, CachedTrustRow>,
    /// Cached SF counters: creditor_agent -> CachedSFCounters.
    pub sf_counters: HashMap<AgentPubKeyB64, CachedSFCounters>,
    /// Cached witness contagion data: debtor -> (witness_count, aggregate_rate).
    /// Eliminates redundant DHT queries for the same debtor within a trust
    /// computation cycle. Cleared on epoch change and invalidate_all.
    pub witness_contagion: HashMap<AgentPubKeyB64, (u32, f64)>,
    /// Last known epoch (for bulk invalidation).
    last_epoch: u64,
    /// Unix timestamp (seconds) of the last successful `notify_trust_row_refresh` execution.
    /// Used to rate-limit remote refresh requests to at most once per TRUST_CACHE_TTL_SECS.
    /// Set to `None` on cache construction (no refresh has run yet).
    last_trust_row_refresh_secs: Option<u64>,
    /// Per-function rate-limit timestamps: function name → last call Unix timestamp (seconds).
    /// Used by `check_and_set_rate_limit` to enforce per-function cooldowns on remotely-callable
    /// extern functions that could be abused for denial-of-service attacks.
    rate_limits: HashMap<&'static str, u64>,
}

impl Default for TrustCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TrustCache {
    pub fn new() -> Self {
        TrustCache {
            subgraph: None,
            local_trust_row: None,
            acquaintances: None,
            reputation_results: HashMap::new(),
            dht_trust_rows: HashMap::new(),
            sf_counters: HashMap::new(),
            witness_contagion: HashMap::new(),
            last_epoch: 0,
            last_trust_row_refresh_secs: None,
            rate_limits: HashMap::new(),
        }
    }

    /// Check if the cache should be invalidated due to epoch change.
    /// Returns true if cache was invalidated.
    pub fn check_epoch_invalidation(&mut self, current_epoch: u64) -> bool {
        if current_epoch > self.last_epoch {
            self.invalidate_all();
            self.last_epoch = current_epoch;
            true
        } else {
            false
        }
    }

    /// Invalidate all cached data.
    pub fn invalidate_all(&mut self) {
        self.subgraph = None;
        self.local_trust_row = None;
        self.acquaintances = None;
        self.reputation_results.clear();
        self.dht_trust_rows.clear();
        self.sf_counters.clear();
        self.witness_contagion.clear();
    }

    /// Evict oldest entries from dht_trust_rows if over the size limit.
    pub fn evict_dht_trust_rows_if_needed(&mut self) {
        if self.dht_trust_rows.len() <= MAX_DHT_TRUST_ROWS_CACHED {
            return;
        }

        // Evict entries with the oldest cached_at timestamps
        let target_size = MAX_DHT_TRUST_ROWS_CACHED * 3 / 4; // Evict 25% to avoid frequent evictions
        let mut entries: Vec<(AgentPubKeyB64, i64)> = self
            .dht_trust_rows
            .iter()
            .map(|(k, v)| (k.clone(), v.cached_at.as_micros()))
            .collect();
        entries.sort_by_key(|(_, ts)| *ts);

        let to_evict = self.dht_trust_rows.len() - target_size;
        for (agent_key, _) in entries.into_iter().take(to_evict) {
            self.dht_trust_rows.remove(&agent_key);
        }
    }

    /// Invalidate only the local trust row (e.g., after contract changes).
    pub fn invalidate_local_trust(&mut self) {
        self.local_trust_row = None;
        // Also invalidate subgraph since it depends on local trust
        self.subgraph = None;
        // And reputation results that depend on the subgraph
        self.reputation_results.clear();
        // SF counters may have changed with contract changes
        self.sf_counters.clear();
        // Witness data may change after contract expirations publish new observations
        self.witness_contagion.clear();
    }

    /// Check if a cached item is still valid (within TTL).
    pub fn is_cache_entry_valid(cached_at: Timestamp, current_time: Timestamp) -> bool {
        let elapsed_micros = current_time.as_micros() - cached_at.as_micros();
        let ttl_micros = TRUST_CACHE_TTL_SECS as i64 * 1_000_000;
        elapsed_micros < ttl_micros
    }
}

// =========================================================================
//  Thread-Local Cache Instance
// =========================================================================

thread_local! {
    /// Process-wide trust cache. Each Holochain zome call runs in the same
    /// process, so this provides efficient caching across calls.
    static TRUST_CACHE: RefCell<TrustCache> = RefCell::new(TrustCache::new());
}

/// Execute a function with access to the trust cache.
pub fn with_cache<F, R>(f: F) -> R
where
    F: FnOnce(&mut TrustCache) -> R,
{
    TRUST_CACHE.with(|cache| f(&mut cache.borrow_mut()))
}

// =========================================================================
//  Cache-Aware Query Functions
// =========================================================================

/// Get the current epoch, checking for cache invalidation.
pub fn get_current_epoch_with_cache_check() -> ExternResult<u64> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    with_cache(|cache| {
        cache.check_epoch_invalidation(current_epoch);
    });

    Ok(current_epoch)
}

/// Get cached acquaintances or fetch from DHT.
pub fn get_cached_acquaintances() -> ExternResult<Vec<AgentPubKey>> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    // Check cache
    let cached = with_cache(|cache| {
        cache.check_epoch_invalidation(current_epoch);

        if let Some(ref cached_acq) = cache.acquaintances {
            if cached_acq.epoch == current_epoch && TrustCache::is_cache_entry_valid(cached_acq.cached_at, now) {
                return Some(cached_acq.acquaintances.clone());
            }
        }
        None
    });

    if let Some(acquaintances) = cached {
        return Ok(acquaintances);
    }

    // Cache miss - fetch from DHT
    let agent = agent_info()?.agent_initial_pubkey;
    let links = get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToAcquaintance)?, GetStrategy::default())?;

    let mut acquaintances: Vec<AgentPubKey> =
        links.into_iter().filter_map(|link| link.target.into_agent_pub_key()).collect();

    // Always include self
    if !acquaintances.contains(&agent) {
        acquaintances.push(agent);
    }

    // Store in cache
    with_cache(|cache| {
        cache.acquaintances =
            Some(CachedAcquaintances { acquaintances: acquaintances.clone(), epoch: current_epoch, cached_at: now });
    });

    Ok(acquaintances)
}

/// Get cached trust row for an agent or fetch from DHT.
pub fn get_cached_trust_row_for_agent(agent: AgentPubKey) -> ExternResult<HashMap<AgentPubKeyB64, f64>> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    // Check cache
    let cached = with_cache(|cache| {
        cache.check_epoch_invalidation(current_epoch);

        let agent_key = AgentPubKeyB64::from(agent.clone());
        if let Some(cached_row) = cache.dht_trust_rows.get(&agent_key) {
            if cached_row.epoch == current_epoch && TrustCache::is_cache_entry_valid(cached_row.cached_at, now) {
                return Some(cached_row.trust_row.clone());
            }
        }
        None
    });

    if let Some(trust_row) = cached {
        return Ok(trust_row);
    }

    // Cache miss - fetch from DHT
    let links = get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToLocalTrust)?, GetStrategy::default())?;

    let mut trust_row: HashMap<AgentPubKeyB64, f64> = HashMap::new();

    for link in links {
        let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.into_inner()));
        if let Ok(trust_tag) = transaction_integrity::types::TrustLinkTag::try_from(tag_bytes) {
            if let Some(target_agent) = link.target.into_agent_pub_key() {
                let target_key: AgentPubKeyB64 = target_agent.into();
                trust_row.insert(target_key, trust_tag.trust_value);
            }
        }
    }

    // Store in cache
    with_cache(|cache| {
        let agent_key = AgentPubKeyB64::from(agent.clone());
        cache
            .dht_trust_rows
            .insert(agent_key, CachedTrustRow { trust_row: trust_row.clone(), epoch: current_epoch, cached_at: now });
    });

    Ok(trust_row)
}

/// Get cached reputation result or return None if not cached.
pub fn get_cached_reputation(target: &AgentPubKey) -> ExternResult<Option<(f64, usize)>> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    let result = with_cache(|cache| {
        cache.check_epoch_invalidation(current_epoch);

        let target_key = AgentPubKeyB64::from(target.clone());
        if let Some(cached_rep) = cache.reputation_results.get(&target_key) {
            if cached_rep.epoch == current_epoch && TrustCache::is_cache_entry_valid(cached_rep.cached_at, now) {
                return Some((cached_rep.trust, cached_rep.acquaintance_count));
            }
        }
        None
    });

    Ok(result)
}

/// Store a reputation result in the cache.
pub fn cache_reputation_result(target: AgentPubKey, trust: f64, acquaintance_count: usize) -> ExternResult<()> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    with_cache(|cache| {
        let target_key = AgentPubKeyB64::from(target.clone());
        cache.reputation_results.insert(
            target_key,
            CachedReputationResult { trust, acquaintance_count, epoch: current_epoch, cached_at: now },
        );
    });

    Ok(())
}

/// Get or build the cached trust subgraph.
/// Returns (subgraph, was_cache_hit). The subgraph is wrapped in Rc to avoid
/// deep-copying the entire graph structure on every access.
pub fn get_or_build_cached_subgraph<F>(build_fn: F) -> ExternResult<(Rc<CachedTrustSubgraph>, bool)>
where
    F: FnOnce() -> ExternResult<CachedTrustSubgraph>,
{
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    // Check cache
    let cached = with_cache(|cache| {
        cache.check_epoch_invalidation(current_epoch);

        if let Some(ref subgraph) = cache.subgraph {
            if subgraph.epoch == current_epoch && TrustCache::is_cache_entry_valid(subgraph.cached_at, now) {
                return Some(Rc::clone(subgraph));
            }
        }
        None
    });

    if let Some(subgraph) = cached {
        return Ok((subgraph, true));
    }

    // Cache miss - build subgraph
    let mut subgraph = build_fn()?;
    subgraph.epoch = current_epoch;
    subgraph.cached_at = now;

    // Store in cache wrapped in Rc
    let rc_subgraph = Rc::new(subgraph);
    let result = Rc::clone(&rc_subgraph);
    with_cache(|cache| {
        cache.subgraph = Some(rc_subgraph);
    });

    Ok((result, false))
}

/// Get cached witness contagion data for a debtor, or fetch from DHT.
/// Returns `(witness_count, aggregate_rate)`.
///
/// This is the cache-aware wrapper around `contagion::get_witness_contagion_data`.
/// Within a single trust computation cycle (same epoch, within TTL), each debtor's
/// witness data is fetched from the DHT at most once — eliminating the 4× redundancy
/// where both `compute_local_trust_row_from_sf_with_contagion` and
/// `get_pre_trust_distribution` independently queried the same data.
pub fn get_cached_witness_contagion(debtor: &AgentPubKey) -> ExternResult<(u32, f64)> {
    // Check cache first
    let debtor_key = AgentPubKeyB64::from(debtor.clone());
    let cached = with_cache(|cache| cache.witness_contagion.get(&debtor_key).copied());

    if let Some(data) = cached {
        return Ok(data);
    }

    // Cache miss — fetch from DHT via the merged single-query function
    let data = crate::trust::contagion::get_witness_contagion_data(debtor)?;

    // Store in cache
    with_cache(|cache| {
        let debtor_key = AgentPubKeyB64::from(debtor.clone());
        cache.witness_contagion.insert(debtor_key, data);
    });

    Ok(data)
}

/// Invalidate local trust cache (call after contract changes).
pub fn invalidate_local_trust_cache() {
    with_cache(|cache| {
        cache.invalidate_local_trust();
    });
}

/// Invalidate all caches (call on epoch change or manual refresh).
pub fn invalidate_all_caches() {
    with_cache(|cache| {
        cache.invalidate_all();
    });
}

/// Return the Unix timestamp (seconds) of the last successful `notify_trust_row_refresh`,
/// or `None` if no refresh has run in the current WASM instance lifetime.
///
/// Used by `notify_trust_row_refresh` to enforce a per-TTL rate limit against spam callers.
pub fn get_last_trust_row_refresh_secs() -> Option<u64> {
    with_cache(|cache| cache.last_trust_row_refresh_secs)
}

/// Record the Unix timestamp (seconds) of a successful `notify_trust_row_refresh` execution.
/// Must be called *before* the expensive work (cache invalidation + trust republication)
/// so that even a partially-completed refresh is rate-limited.
pub fn set_last_trust_row_refresh_secs(secs: u64) {
    with_cache(|cache| {
        cache.last_trust_row_refresh_secs = Some(secs);
    });
}

// =========================================================================
//  Generalised per-function rate limiting
// =========================================================================

/// Check whether a remotely-callable function is within its cooldown window and,
/// if not, record the current timestamp so future calls are rate-limited.
///
/// Returns `true` when the call **should proceed** (cooldown has elapsed or this
/// is the first call).  Returns `false` when the function was called too recently
/// and the caller should return early without doing expensive work.
///
/// The rate limit is stored in the agent-local `thread_local!` TrustCache, so
/// it survives across back-to-back zome calls in the same WASM instance lifetime
/// but is reset on conductor restart (acceptable: worst case is one extra call).
///
/// # Parameters
/// - `function_name`: a `'static` str key identifying the function (use the
///   function name as a string literal, e.g. `"create_drain_request"`).
/// - `cooldown_secs`: minimum seconds between successive invocations.
pub fn check_and_set_rate_limit(function_name: &'static str, cooldown_secs: u64, now_secs: u64) -> bool {
    with_cache(|cache| {
        if let Some(&last) = cache.rate_limits.get(function_name) {
            if now_secs.saturating_sub(last) < cooldown_secs {
                return false; // still within cooldown — reject
            }
        }
        cache.rate_limits.insert(function_name, now_secs);
        true // allowed — timestamp recorded
    })
}

// =========================================================================
//  Batched Trust Row Fetching (Phase 4: Scalability)
// =========================================================================

/// Batch fetch trust rows for multiple agents in parallel.
/// This is the core optimization for Phase 4 - instead of sequential DHT lookups,
/// we fetch all needed trust rows in a single batched operation.
///
/// Returns a map of agent -> trust_row for all requested agents.
pub fn get_trust_rows_batch(
    agents: &[AgentPubKey],
) -> ExternResult<HashMap<AgentPubKey, HashMap<AgentPubKeyB64, f64>>> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    // Check cache for each agent, collect uncached agents
    let mut results: HashMap<AgentPubKey, HashMap<AgentPubKeyB64, f64>> = HashMap::new();
    let mut uncached_agents: Vec<AgentPubKey> = Vec::new();

    with_cache(|cache| {
        cache.check_epoch_invalidation(current_epoch);

        for agent in agents {
            if let Some(cached_row) = cache.dht_trust_rows.get(&AgentPubKeyB64::from(agent.clone())) {
                if cached_row.epoch == current_epoch && TrustCache::is_cache_entry_valid(cached_row.cached_at, now) {
                    results.insert(agent.clone(), cached_row.trust_row.clone());
                    continue;
                }
            }
            uncached_agents.push(agent.clone());
        }
    });

    // If all agents were cached, return early
    if uncached_agents.is_empty() {
        return Ok(results);
    }

    // Batch fetch links for all uncached agents
    // Create GetLinksInput for each agent
    let link_queries: Vec<LinkQuery> = uncached_agents
        .iter()
        .filter_map(|agent| LinkQuery::try_new(agent.clone(), LinkTypes::AgentToLocalTrust).ok())
        .collect();

    // Batch get_links using HDK's batch capability
    let all_links: Vec<Vec<Link>> = HDK.with(|hdk| {
        hdk.borrow().get_links(
            link_queries
                .into_iter()
                .map(|q| GetLinksInput::from_query(q, GetOptions::default()))
                .collect(),
        )
    })?;

    // Process results for each agent
    for (agent, links) in uncached_agents.iter().zip(all_links.into_iter()) {
        let mut trust_row: HashMap<AgentPubKeyB64, f64> = HashMap::new();

        for link in links {
            let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.into_inner()));
            if let Ok(trust_tag) = transaction_integrity::types::TrustLinkTag::try_from(tag_bytes) {
                if let Some(target_agent) = link.target.into_agent_pub_key() {
                    let target_key: AgentPubKeyB64 = target_agent.into();
                    trust_row.insert(target_key, trust_tag.trust_value);
                }
            }
        }

        // Store in cache
        with_cache(|cache| {
            let agent_key = AgentPubKeyB64::from(agent.clone());
            cache.dht_trust_rows.insert(
                agent_key,
                CachedTrustRow { trust_row: trust_row.clone(), epoch: current_epoch, cached_at: now },
            );
        });

        results.insert(agent.clone(), trust_row);
    }

    // Evict oldest entries if cache is over the size limit
    with_cache(|cache| {
        cache.evict_dht_trust_rows_if_needed();
    });

    Ok(results)
}

// =========================================================================
//  Cache Statistics (for debugging/monitoring)
// =========================================================================

/// Statistics about cache usage.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CacheStats {
    pub has_subgraph: bool,
    pub has_local_trust_row: bool,
    pub has_acquaintances: bool,
    pub num_cached_reputations: usize,
    pub num_cached_dht_trust_rows: usize,
    pub num_cached_witness_contagion: usize,
    pub last_epoch: u64,
}

/// Get current cache statistics.
pub fn get_cache_stats() -> CacheStats {
    with_cache(|cache| CacheStats {
        has_subgraph: cache.subgraph.is_some(),
        has_local_trust_row: cache.local_trust_row.is_some(),
        has_acquaintances: cache.acquaintances.is_some(),
        num_cached_reputations: cache.reputation_results.len(),
        num_cached_dht_trust_rows: cache.dht_trust_rows.len(),
        num_cached_witness_contagion: cache.witness_contagion.len(),
        last_epoch: cache.last_epoch,
    })
}

pub type SFCounters = HashMap<AgentPubKeyB64, (f64, f64, u64, f64, f64)>;

/// Get cached SF counters for a creditor, or return None if not cached.
pub fn get_cached_sf_counters(creditor: &AgentPubKey) -> ExternResult<Option<SFCounters>> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    let result = with_cache(|cache| {
        cache.check_epoch_invalidation(current_epoch);

        let creditor_key = AgentPubKeyB64::from(creditor.clone());
        if let Some(cached) = cache.sf_counters.get(&creditor_key) {
            if cached.epoch == current_epoch && TrustCache::is_cache_entry_valid(cached.cached_at, now) {
                return Some(cached.counters.clone());
            }
        }
        None
    });

    Ok(result)
}

/// Store SF counters in the cache.
pub fn cache_sf_counters(
    creditor: AgentPubKey,
    counters: HashMap<AgentPubKeyB64, (f64, f64, u64, f64, f64)>,
) -> ExternResult<()> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    with_cache(|cache| {
        let creditor_key = AgentPubKeyB64::from(creditor.clone());
        cache
            .sf_counters
            .insert(creditor_key, CachedSFCounters { counters, epoch: current_epoch, cached_at: now });
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_invalidation_on_epoch_change() {
        let mut cache = TrustCache::new();

        // Set some cached data
        cache.last_epoch = 100;
        cache.acquaintances =
            Some(CachedAcquaintances { acquaintances: vec![], epoch: 100, cached_at: Timestamp::from_micros(0) });

        // Check that epoch change invalidates
        assert!(cache.check_epoch_invalidation(101));
        assert!(cache.acquaintances.is_none());
        assert_eq!(cache.last_epoch, 101);

        // Check that same epoch doesn't invalidate
        cache.acquaintances =
            Some(CachedAcquaintances { acquaintances: vec![], epoch: 101, cached_at: Timestamp::from_micros(0) });
        assert!(!cache.check_epoch_invalidation(101));
        assert!(cache.acquaintances.is_some());
    }

    #[test]
    fn test_ttl_validation() {
        let base = Timestamp::from_micros(1_000_000_000);
        let within_ttl = Timestamp::from_micros(1_000_000_000 + (TRUST_CACHE_TTL_SECS as i64 - 1) * 1_000_000);
        let past_ttl = Timestamp::from_micros(1_000_000_000 + (TRUST_CACHE_TTL_SECS as i64 + 1) * 1_000_000);

        assert!(TrustCache::is_cache_entry_valid(base, within_ttl));
        assert!(!TrustCache::is_cache_entry_valid(base, past_ttl));
    }

    #[test]
    fn test_local_trust_invalidation() {
        let mut cache = TrustCache::new();
        cache.last_epoch = 100;

        // Populate cache
        cache.local_trust_row =
            Some(CachedTrustRow { trust_row: HashMap::new(), epoch: 100, cached_at: Timestamp::from_micros(0) });
        cache.subgraph = Some(Rc::new(CachedTrustSubgraph::new()));
        cache.reputation_results.insert(
            AgentPubKeyB64::from(AgentPubKey::from_raw_36(vec![0u8; 36])),
            CachedReputationResult {
                trust: 0.5,
                acquaintance_count: 10,
                epoch: 100,
                cached_at: Timestamp::from_micros(0),
            },
        );

        // Invalidate local trust
        cache.invalidate_local_trust();

        // Verify local trust and dependent caches are cleared
        assert!(cache.local_trust_row.is_none());
        assert!(cache.subgraph.is_none());
        assert!(cache.reputation_results.is_empty());

        // Verify unrelated caches are preserved
        cache.dht_trust_rows.insert(
            AgentPubKeyB64::from(AgentPubKey::from_raw_36(vec![1u8; 36])),
            CachedTrustRow { trust_row: HashMap::new(), epoch: 100, cached_at: Timestamp::from_micros(0) },
        );
        cache.invalidate_local_trust();
        assert!(!cache.dht_trust_rows.is_empty());
    }

    #[test]
    fn test_invalidate_all() {
        let mut cache = TrustCache::new();
        cache.last_epoch = 100;

        // Populate all caches
        cache.local_trust_row =
            Some(CachedTrustRow { trust_row: HashMap::new(), epoch: 100, cached_at: Timestamp::from_micros(0) });
        cache.subgraph = Some(Rc::new(CachedTrustSubgraph::new()));
        cache.acquaintances =
            Some(CachedAcquaintances { acquaintances: vec![], epoch: 100, cached_at: Timestamp::from_micros(0) });
        cache.reputation_results.insert(
            AgentPubKeyB64::from(AgentPubKey::from_raw_36(vec![0u8; 36])),
            CachedReputationResult {
                trust: 0.5,
                acquaintance_count: 10,
                epoch: 100,
                cached_at: Timestamp::from_micros(0),
            },
        );
        cache.dht_trust_rows.insert(
            AgentPubKeyB64::from(AgentPubKey::from_raw_36(vec![1u8; 36])),
            CachedTrustRow { trust_row: HashMap::new(), epoch: 100, cached_at: Timestamp::from_micros(0) },
        );

        // Invalidate all
        cache.invalidate_all();

        // Verify everything is cleared
        assert!(cache.local_trust_row.is_none());
        assert!(cache.subgraph.is_none());
        assert!(cache.acquaintances.is_none());
        assert!(cache.reputation_results.is_empty());
        assert!(cache.dht_trust_rows.is_empty());
    }

    #[test]
    fn test_cached_trust_subgraph_operations() {
        let mut subgraph = CachedTrustSubgraph::new();

        // Test empty subgraph
        assert_eq!(subgraph.size(), 0);

        // Add some agents
        let agent1 = AgentPubKey::from_raw_36(vec![1u8; 36]);
        let agent2 = AgentPubKey::from_raw_36(vec![2u8; 36]);
        let agent3 = AgentPubKey::from_raw_36(vec![3u8; 36]);

        let idx1 = subgraph.get_or_insert_agent(&agent1);
        let idx2 = subgraph.get_or_insert_agent(&agent2);
        let idx3 = subgraph.get_or_insert_agent(&agent3);

        // Verify indices are sequential
        assert_eq!(idx1, 0);
        assert_eq!(idx2, 1);
        assert_eq!(idx3, 2);
        assert_eq!(subgraph.size(), 3);

        // Verify duplicate agent returns same index
        let idx1_dup = subgraph.get_or_insert_agent(&agent1);
        assert_eq!(idx1_dup, idx1);
        assert_eq!(subgraph.size(), 3);

        // Verify agent lookup works
        assert_eq!(subgraph.agent_index.get(&AgentPubKeyB64::from(agent1)), Some(&0));
        assert_eq!(subgraph.agent_index.get(&AgentPubKeyB64::from(agent2)), Some(&1));
        assert_eq!(subgraph.agent_index.get(&AgentPubKeyB64::from(agent3)), Some(&2));

        // Add trust relationships
        subgraph.trust_rows[idx1].insert(idx2, 0.5);
        subgraph.trust_rows[idx1].insert(idx3, 0.3);
        subgraph.trust_rows[idx2].insert(idx3, 0.7);

        // Verify trust matrix
        assert_eq!(subgraph.trust_rows[idx1].get(&idx2), Some(&0.5));
        assert_eq!(subgraph.trust_rows[idx1].get(&idx3), Some(&0.3));
        assert_eq!(subgraph.trust_rows[idx2].get(&idx3), Some(&0.7));
        assert_eq!(subgraph.trust_rows[idx3].len(), 0); // No outgoing trust
    }

    #[test]
    fn test_cache_stats() {
        let cache = TrustCache::new();
        let stats = CacheStats {
            has_subgraph: cache.subgraph.is_some(),
            has_local_trust_row: cache.local_trust_row.is_some(),
            has_acquaintances: cache.acquaintances.is_some(),
            num_cached_reputations: cache.reputation_results.len(),
            num_cached_dht_trust_rows: cache.dht_trust_rows.len(),
            num_cached_witness_contagion: cache.witness_contagion.len(),
            last_epoch: cache.last_epoch,
        };

        assert!(!stats.has_subgraph);
        assert!(!stats.has_local_trust_row);
        assert!(!stats.has_acquaintances);
        assert_eq!(stats.num_cached_reputations, 0);
        assert_eq!(stats.num_cached_dht_trust_rows, 0);
        assert_eq!(stats.last_epoch, 0);
    }
}
