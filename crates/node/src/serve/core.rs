//! The node core: a replica, a mempool, the client-facing caches (sessions,
//! rate limiters, tx outcomes) and the address index, all behind one mutex.
//! HTTP handlers, Tauri IPC commands and the Malachite engine's own handlers
//! all operate on it.
//!
//! It holds no consensus state of its own. Blocks are decided by the engine
//! (`engine_malachite`) and enter here through the one commit path,
//! `commit_decided`. There is no second, dev-only consensus
//! (rotating proposer, one vote round, ≥2/3 commit) that committed into this
//! same core; it is gone, along with the class of bug where a rule holds on
//! the consensus we develop against and not on the one we ship.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::block::{Block, SignedTx};
use crate::mempool::Mempool;
use crate::replica::{Replica, ReplicaError, TxOutcome};

use super::pending::PendingPool;
use super::ratelimit::RateLimiter;
use super::session::SessionStore;

/// Bound on remembered per-tx commit outcomes: a client can look up
/// whether its submission landed, rejected, or is still unknown, without
/// this growing forever.
const OUTCOME_CAP: usize = 4096;

/// Token-bucket sizing for `/tx`: generous enough that a legitimate
/// burst of activity (a lively cluster, a lively wallet) never trips it, but
/// bounded so a single signer identity (or, for the permissionless cranks,
/// the shared anonymous bucket) cannot flood the mempool.
/// `pub(crate)` for the ingress probe in `serve::tests`, which asserts the
/// bucket's own guarantee — admissions never exceed capacity plus refill for
/// the time the flood took — rather than a request count that only throttles
/// on a machine fast enough.
pub(crate) const TX_BUCKET_CAPACITY: f64 = 64.0;
pub(crate) const TX_BUCKET_REFILL_PER_SEC: f64 = 16.0;
/// `/pending/sign`: a co-signer typically acts once or twice per
/// proposal, so a tighter bucket is fine.
const PENDING_BUCKET_CAPACITY: f64 = 32.0;
const PENDING_BUCKET_REFILL_PER_SEC: f64 = 8.0;
/// Token-bucket sizing for anonymous READS (`/members`, `/contracts`,
/// `/tx/check`, `/tx/digest`, and the rest of the read surface — see
/// `http::is_rate_limited_path`). Reads answer anonymous callers by design
/// at this step, so this bucket is keyed on source IP rather than a signer
/// identity. Sized generously above `TX_BUCKET_CAPACITY`/`PENDING_BUCKET_
/// CAPACITY`: a polling UI legitimately issues several reads per user
/// action (head + network + members + contracts on every refresh) from one
/// IP, and NAT/shared-office egress can put many distinct legitimate users
/// behind one address — but still bounded, so a scripted loop hammering
/// `/members` or `/contracts` (full-state serialization, cost growing with
/// community size) cannot turn one anonymous IP into a standing DoS.
const READ_BUCKET_CAPACITY: f64 = 120.0;
const READ_BUCKET_REFILL_PER_SEC: f64 = 30.0;
const RATE_LIMIT_MAX_KEYS: usize = 10_000;

/// Rate-limit key for a read request: the caller's source IP, as raw octets
///. Reads are anonymous by design, so there is no signer identity to
/// key on the way `tx_rate_key` does for `/tx` — the network-perimeter
/// identity is the only one available.
fn ip_rate_key(ip: std::net::IpAddr) -> Vec<u8> {
    match ip {
        std::net::IpAddr::V4(v4) => v4.octets().to_vec(),
        std::net::IpAddr::V6(v6) => v6.octets().to_vec(),
    }
}

/// Rate-limit key for a submitted transaction: the first signer in CANONICAL
/// order — the smallest member id among the keys that resolve, the same order
/// the bond gate bills in — or a single shared bucket for the permissionless
/// cranks (which carry none) — still bounded, just not attributable to one
/// identity.
///
/// Canonical rather than envelope order for the reason `bond::payers` gives:
/// envelope order is chosen by whoever assembles the transaction, so a bucket
/// keyed on it is a bucket the assembler chooses to drain. A signer that
/// resolves to no member falls back to the raw key, which is a bucket only its
/// holder can fill.
fn tx_rate_key(state: &edet_state::State, tx: &SignedTx) -> Vec<u8> {
    let canonical = tx
        .signers
        .iter()
        .filter_map(|k| state.member_of_key(k).map(|id| (id, k)))
        .min_by_key(|(id, _)| *id)
        .map(|(_, k)| k.to_vec());
    canonical
        .or_else(|| tx.signers.first().map(|k| k.to_vec()))
        .unwrap_or_else(|| b"__crank__".to_vec())
}

/// (the paper's §Implementation Part 2):
/// caches `views::whois`'s address→member reverse lookup so it's O(1)
/// amortized instead of an O(members) SHA-256-per-member scan on every
/// call. Keyed on `members.len()` as a cheap staleness signal: this ledger
/// only ever ADDS members (admission), never removes one, and an address is
/// immutable once derived (`views::member_address_bytes` hashes the
/// immutable admission attestation plus id, not the current key), so a
/// length change is both necessary and sufficient to detect "this cache is
/// stale, rebuild it." Node-local only — never serialized, never touches
/// the replicated `State`/snapshot format — the cheaper of the backlog's two
/// fix options.
#[derive(Default)]
struct AddressIndex {
    built_at_len: usize,
    map: HashMap<[u8; 20], u64>,
}

impl AddressIndex {
    /// Rebuild from `state` if its member count has moved since the last
    /// build, then look up `addr`. Takes `&mut self` behind the `RefCell`
    /// on `NodeCore` so a shared `&NodeCore` (every read handler's
    /// signature) can still populate/refresh it.
    fn resolve(&mut self, state: &edet_state::State, addr: &[u8; 20]) -> Option<u64> {
        if self.built_at_len != state.members.len() {
            self.map.clear();
            self.map
                .extend(state.members.values().map(|m| (super::views::member_address_bytes(m), m.id)));
            self.built_at_len = state.members.len();
        }
        self.map.get(addr).copied()
    }
}

/// Capacity answers already computed against the CURRENT cut inputs.
///
/// **A capacity query is a max-flow over the whole edge map**, and the list
/// view answers one per member it serves — up to `MAX_VIEW_ITEMS` of them.
/// Measured on a release build: 500 queries cost 0.3 s at 1,000 accounts,
/// 2.3 s at 5,000 and 5.0 s at 10,000. They ran under the node lock, which is
/// the same lock a commit needs, so one source at the allowed read rate held
/// it continuously and the validator missed its rounds; behind a NAT every
/// member shares one bucket, so it happened at scale without an attacker. Two
/// things answer it: this cache, and `CapacitySnapshot`, on which the queries
/// a cold read still needs run off the lock.
///
/// **Keyed on the cut's own inputs, never on the height.** A cut reads
/// `(edges, reserved, committed, supplies, id space)` and nothing else, so an
/// answer stays exact until one of them changes — and heights change every
/// second on an idle chain (`EMPTY_BLOCK_INTERVAL`), while an empty block moves
/// none of the five. A height key would throw the table away once a second and
/// buy nothing.
///
/// Exact equality rather than the monotone argument `invariants::AuditCache`
/// uses: that cache needs a valid LOWER BOUND and this needs the right answer.
#[derive(Default)]
struct CapacityCache {
    edges: edet_kernel::flow::Edges,
    reserved: edet_kernel::flow::Reservations,
    committed: edet_kernel::flow::Committed,
    supplies: BTreeMap<u64, u64>,
    flow_n: usize,
    warm: bool,
    answers: BTreeMap<u64, f64>,
}

impl CapacityCache {
    /// Drop everything if any cut input moved, then answer from the table.
    /// `None` when this member is not in it yet — the caller decides whether
    /// it can afford to compute one.
    fn get(&mut self, state: &edet_state::State, id: u64) -> Option<f64> {
        if !self.warm
            || self.flow_n != state.flow_n()
            || self.edges != *state.edges
            || self.reserved != *state.reserved
            || self.committed != state.committed
            || self.supplies != state.underwriters
        {
            self.edges = (*state.edges).clone();
            self.reserved = (*state.reserved).clone();
            self.committed = state.committed.clone();
            self.supplies = state.underwriters.clone();
            self.flow_n = state.flow_n();
            self.warm = true;
            self.answers.clear();
        }
        self.answers.get(&id).copied()
    }

    fn insert(&mut self, id: u64, capacity: f64) {
        self.answers.insert(id, capacity);
    }
}

/// The cut's inputs at one instant, and the two things a read computes from
/// them off the node lock: a member's capacity and how a viewer may see it.
///
/// Statuses are not in it: a non-active member's capacity is zero by rule and
/// is answered under the lock without a query, so only active members reach
/// the snapshot.
#[derive(Clone, Debug)]
pub struct CapacitySnapshot {
    edges: edet_kernel::flow::Edges,
    reserved: edet_kernel::flow::Reservations,
    committed: edet_kernel::flow::Committed,
    uw: Vec<(usize, u64)>,
    supplies: BTreeMap<u64, u64>,
    n: usize,
    seal_amounts: f64,
}

impl CapacitySnapshot {
    /// `State::capacity_of` for an active member, on the copied inputs.
    pub fn capacity_of(&self, id: u64) -> f64 {
        edet_state::State::from_minor(edet_kernel::flow::capacity(
            &self.edges,
            &self.reserved,
            &self.committed,
            &self.uw,
            &[id as usize],
            self.n,
            u64::MAX,
        ))
    }

    /// `views::visible_amount` on the policy the snapshot carries.
    pub fn visible(&self, x: f64, viewer_is_party: bool) -> f64 {
        if viewer_is_party || self.seal_amounts < 0.5 {
            x
        } else {
            edet_kernel::cascade::pow2_bucket(x)
        }
    }

    fn matches(&self, st: &edet_state::State) -> bool {
        self.n == st.flow_n()
            && self.edges == *st.edges
            && self.reserved == *st.reserved
            && self.committed == st.committed
            && self.supplies == st.underwriters
    }
}

/// The most capacities one read may COMPUTE that are not already cached.
///
/// The cache makes a repeated read free; this bounds the first read after every
/// state-changing block, which is the one an attacker can force by trading.
/// A list view serves what it has and OMITS the rest — the client reads a
/// missing field as "not known yet" and holds, which is the same rule it
/// applies to every other absent risk input, and is the honest answer rather
/// than a stale or invented number.
///
/// **Eight because the bound that matters is TIME, not a count.** One capacity
/// query costs about 10 ms at 10,000 accounts, so this is ~80 ms of the
/// handler's own thread per cold read at a size the design contemplates —
/// off the node lock, which holds only for the clone of the cut's inputs
/// (`NodeCore::capacity_snapshot`) — against the seconds an unbounded pass
/// takes. It is also the read's price in budget tokens (`http::read_cost`), so
/// a source can sustain a few of them a second and no more.
pub(crate) const MAX_COLD_CAPACITY_PER_READ: usize = 8;

pub struct NodeCore {
    pub index: usize,
    pub n: usize,
    pub replica: Replica,
    pub mempool: Mempool,
    /// Multi-party transactions collecting signatures (see pending.rs).
    pub pending: PendingPool,
    pub committed_count: u64,
    /// Per-signer token buckets guarding `/tx` and `/pending/sign`.
    tx_limiter: RateLimiter,
    pub pending_limiter: RateLimiter,
    /// Per-source-IP token bucket guarding the read surface (see
    /// `READ_BUCKET_CAPACITY`'s doc comment). Reused verbatim rather than a
    /// second implementation — only the key shape (IP octets, not a signer)
    /// differs from `tx_limiter`.
    read_limiter: RateLimiter,
    /// Commit-time outcome of every transaction this replica has applied
    /// (bounded, FIFO-evicted), so a client can learn a tx failed at commit
    /// even though ingress said "queued".
    outcomes: BTreeMap<[u8; 32], TxOutcome>,
    outcome_order: VecDeque<[u8; 32]>,
    /// Authenticated-reads §Standing: in-memory bearer session tokens, behind the
    /// same mutex as the rest of this core. Swept for TTL expiry and
    /// rotation/suspension invalidity after every committed block (see
    /// `commit_decided`).
    pub sessions: SessionStore,
    /// **Every viewer signature this node has verified inside its window**, so
    /// each is admitted exactly once (`super::replay`). Behind the same mutex
    /// as the rest of this core, which the extractor already takes.
    pub(crate) seen: super::replay::SeenSignatures,
    /// See `AddressIndex`'s doc comment. `RefCell` because `views::whois`
    /// only holds a shared `&NodeCore` (it's a read, not a write) but still
    /// needs to populate/refresh this cache.
    address_index: RefCell<AddressIndex>,
    /// See `CapacityCache`. `RefCell` for the same reason `address_index` is
    /// one: every read handler holds a shared `&NodeCore`.
    capacity_cache: RefCell<CapacityCache>,
}

impl NodeCore {
    pub fn new(index: usize, n: usize, genesis: edet_state::State) -> NodeCore {
        NodeCore::from_replica(index, n, Replica::new(genesis))
    }

    /// Build a core whose replica is durable when `data_dir` is set:
    /// recovers from an existing snapshot + WAL if the directory holds one,
    /// otherwise starts fresh from `genesis` and begins persisting. `None`
    /// keeps the old in-memory behaviour.
    pub fn open(
        index: usize,
        n: usize,
        genesis: edet_state::State,
        data_dir: Option<&str>,
        snapshot_interval: u64,
        prune_margin_blocks: u64,
    ) -> Result<NodeCore, ReplicaError> {
        let mut replica = match data_dir {
            Some(dir) => Replica::open(dir, genesis, snapshot_interval)?,
            None => Replica::new(genesis),
        };
        replica.prune_margin_blocks = prune_margin_blocks;
        Ok(NodeCore::from_replica(index, n, replica))
    }

    fn from_replica(index: usize, n: usize, replica: Replica) -> NodeCore {
        NodeCore {
            index,
            n,
            replica,
            mempool: Mempool::default(),
            pending: PendingPool::default(),
            committed_count: 0,
            tx_limiter: RateLimiter::new(TX_BUCKET_CAPACITY, TX_BUCKET_REFILL_PER_SEC, RATE_LIMIT_MAX_KEYS),
            pending_limiter: RateLimiter::new(
                PENDING_BUCKET_CAPACITY,
                PENDING_BUCKET_REFILL_PER_SEC,
                RATE_LIMIT_MAX_KEYS,
            ),
            read_limiter: RateLimiter::new(READ_BUCKET_CAPACITY, READ_BUCKET_REFILL_PER_SEC, RATE_LIMIT_MAX_KEYS),
            outcomes: BTreeMap::new(),
            outcome_order: VecDeque::new(),
            sessions: SessionStore::default(),
            seen: Default::default(),
            address_index: RefCell::new(AddressIndex::default()),
            capacity_cache: RefCell::new(CapacityCache::default()),
        }
    }

    /// Resolve a raw 20-byte wallet address to a member id via the
    /// cached index (`views::whois`'s address path), rebuilding it first if
    /// the membership count has changed since the last lookup.
    pub fn resolve_address(&self, addr: &[u8; 20]) -> Option<u64> {
        self.address_index.borrow_mut().resolve(&self.replica.state, addr)
    }

    /// This member's capacity from the cache, when the cut inputs have not
    /// moved since it was computed — and nothing else: no query runs under the
    /// node lock for a read. See `CapacityCache` for why the alternative hands
    /// one source the lock for seconds at a time, and `capacity_snapshot` for
    /// where a cold read's queries run instead.
    pub fn capacity_cached_only(&self, id: u64) -> Option<f64> {
        self.capacity_cache.borrow_mut().get(&self.replica.state, id)
    }

    /// The cut's inputs, copied out from under the lock so a cold read's
    /// queries run OFF it.
    ///
    /// A capacity is a max-flow over the whole edge map, milliseconds each at
    /// community scale, and the cache above makes a repeated read free — but a
    /// free settle moves `reserved`, so an attacker who trades once a block
    /// makes every read cold, and a few source addresses at the read budget
    /// then held the lock a commit needs for most of every second. The clone
    /// is one pass over the same maps, a small fraction of the eight queries
    /// it replaces, and the queries then cost the handler's thread and nobody
    /// else's.
    pub fn capacity_snapshot(&self) -> CapacitySnapshot {
        let st = &self.replica.state;
        CapacitySnapshot {
            edges: (*st.edges).clone(),
            reserved: (*st.reserved).clone(),
            committed: st.committed.clone(),
            uw: st.underwriters.iter().map(|(&id, &s)| (id as usize, s)).collect(),
            supplies: st.underwriters.clone(),
            n: st.flow_n(),
            seal_amounts: st.params.seal_amounts,
        }
    }

    /// Bank capacities computed off the lock on `snapshot`, if the cut inputs
    /// are still the ones it was taken from. A block between the two phases
    /// moves them, and an answer for the old inputs is then discarded rather
    /// than served for the new — the read that computed it still gets it,
    /// exactly as it would have under the lock.
    pub fn fill_capacity_cache(&self, snapshot: &CapacitySnapshot, computed: &[(u64, f64)]) {
        let st = &self.replica.state;
        if !snapshot.matches(st) {
            return;
        }
        let mut cache = self.capacity_cache.borrow_mut();
        // `get` resets the table if the inputs moved since the last read; they
        // have not, so this only ever warms it.
        let _ = cache.get(st, u64::MAX);
        for &(id, cap) in computed {
            cache.insert(id, cap);
        }
    }

    /// A transaction arrived (from a client or a peer). Queue it, and report
    /// whether it was NEWLY accepted — the caller (`serve::driver::submit`)
    /// gossips it onward exactly then, so a transaction already in this
    /// mempool never re-floods the network. Gated by a per-signer rate
    /// limit (the empty-signer/non-crank and signature checks already
    /// happened at the ingress, before this is ever called).
    pub fn submit(&mut self, tx: SignedTx) -> bool {
        // A duplicate spends no token: the mempool already holds it, and a
        // token consumed before the dedup let anyone re-post a member's own
        // envelope to drain that member's bucket.
        if tx.hash().is_ok_and(|h| self.mempool.contains(&h)) {
            return false;
        }
        if !self.tx_limiter.allow(&tx_rate_key(&self.replica.state, &tx)) {
            return false;
        }
        // An id the ledger has not issued names nothing: `apply` refuses such
        // an envelope before it records anything, and this keeps it out of the
        // mempool for the same reason the authorisation screen below does.
        if edet_state::apply::names_issued_ids(&self.replica.state, &tx.tx).is_err() {
            return false;
        }
        // The operation-bond screen. Without it the bond would be enforced
        // only at `apply` — that is, AFTER the transaction had already been
        // gossiped, proposed, ordered and committed — so an exhausted member
        // would still consume the full consensus round the bond exists to
        // protect. Dropping here, before the mempool and before the gossip
        // hop, is what makes the traffic bound a bound on consensus work
        // rather than merely on state growth.
        //
        // Advisory, never authoritative: this node screens against the state
        // IT holds, and two honest nodes at different heights can disagree at
        // the margin. So a screened-out transaction is dropped from this
        // node's mempool and not relayed; it is NOT voted down, and no block
        // is judged by it (`engine_malachite::screen` deliberately does not
        // consult this — making a headroom reading a validity rule would fork
        // the network on a disagreement that is not a fault). `apply` remains
        // the only authority, and it re-checks.
        // Over the SAME memo the commit path uses. A screen is advisory and a
        // node at another height may legitimately disagree at the margin; what
        // it may not do is cost a full cut per signer on every submission,
        // which is the read an unauthenticated flood would otherwise buy.
        if !edet_state::bond::admits_with_cache(&self.replica.state, &tx.tx, &tx.signers, &mut self.replica.gate_cache)
        {
            return false;
        }
        // **An envelope that authorises nothing never enters a mempool**, and
        // this is where that has to happen rather than only at the block rule
        // Such an envelope is refused at dispatch
        // with `ET-MEM-NOT_SIGNER`, refunded and its id forgotten — so it costs
        // its sender nothing, can be replayed without limit, and still costs
        // every node that applies it a `seed_reach` max-flow per signer.
        //
        // The block rule refuses a block carrying one. Which is exactly why
        // this screen is not optional: without it an HONEST proposer would
        // batch these out of its own mempool and build a block every other
        // validator then had to vote down — closing a DoS by opening a
        // liveness fault. Unlike the bond screen above, this one is not
        // advisory-only in character: it is the same deterministic function
        // the block rule applies, so this node and the block rule can only
        // disagree by being at different heights, which resolves the way every
        // other such disagreement does.
        if !edet_state::authorises(&self.replica.state, &tx.tx, &tx.signers) {
            return false;
        }
        self.mempool.push(tx)
    }

    /// A block batch with the transactions this node believes the bond gate
    /// would now refuse left behind.
    ///
    /// A second look rather than a redundant one: `submit` screened these on
    /// the way in, but a payer's headroom moves as blocks commit, so a
    /// transaction admitted to the mempool at height N can be unaffordable by
    /// the time this node proposes at N+k. Filtering here keeps block space
    /// for transactions that can actually execute.
    ///
    /// Best-effort by construction, in two ways that are both fine. It judges
    /// each transaction against current state rather than simulating the
    /// batch in sequence, so a block can still carry more than one payer's
    /// headroom covers; and it is this node's private view, so proposers may
    /// legitimately differ. Both resolve the same way — `apply` re-checks and
    /// refuses — which is exactly why no validity rule may be built on this.
    ///
    /// The one caller is the engine's `GetValue` handler, so a `serve`-only
    /// build (no `malachite`) has nothing but this module's own tests using
    /// it — that combination is still built and linted, hence the allow.
    #[cfg_attr(not(feature = "malachite"), allow(dead_code))]
    pub(crate) fn proposable_batch(&mut self, max: usize) -> Vec<SignedTx> {
        let batch = self.mempool.batch(max);
        let state = &self.replica.state;
        let gate = &mut self.replica.gate_cache;
        batch
            .into_iter()
            .filter(|tx| edet_state::bond::admits_with_cache(state, &tx.tx, &tx.signers, gate))
            // The second look at the authorisation rule, for the same reason
            // as the bond one: a co-signature can be rotated away between
            // admission and proposal, and a block carrying an envelope that
            // authorises nothing is one every other validator refuses.
            .filter(|tx| edet_state::authorises(state, &tx.tx, &tx.signers))
            .collect()
    }

    /// Is this source IP still under its read-rate budget? Consumes one
    /// token when it is. Called from the `read_rate_limit` middleware layer
    /// (`http.rs`), never from a handler directly — the perimeter check
    /// belongs at the layer, not scattered across individual views.
    pub fn allow_read(&mut self, ip: std::net::IpAddr) -> bool {
        self.allow_read_cost(ip, 1.0)
    }

    /// The same gate, charged for what the read may COST rather than for the
    /// fact of it. See `RateLimiter::allow_cost`, and `CapacityCache` for the
    /// read whose price is not one.
    pub fn allow_read_cost(&mut self, ip: std::net::IpAddr, cost: f64) -> bool {
        self.read_limiter.allow_cost(&ip_rate_key(ip), cost)
    }

    /// The recorded commit-time outcome of a transaction this replica has
    /// applied, if it is still within the bounded window.
    pub fn tx_outcome(&self, h: &[u8; 32]) -> Option<TxOutcome> {
        self.outcomes.get(h).copied()
    }

    /// Is this hash currently queued, uncommitted? (distinguishes
    /// "pending" from "unknown" at `/tx/outcome`.)
    pub fn tx_pending(&self, h: &[u8; 32]) -> bool {
        self.mempool.contains(h)
    }

    /// Remember each transaction's commit outcome, bounded FIFO.
    fn record_outcomes(&mut self, block: &Block, outcomes: Vec<TxOutcome>) {
        for (stx, outcome) in block.txs.iter().zip(outcomes) {
            let Ok(h) = stx.hash() else { continue };
            if self.outcomes.insert(h, outcome).is_none() {
                self.outcome_order.push_back(h);
                if self.outcome_order.len() > OUTCOME_CAP {
                    if let Some(old) = self.outcome_order.pop_front() {
                        self.outcomes.remove(&old);
                    }
                }
            }
        }
    }

    /// Commit a block the consensus engine has DECIDED, together with the
    /// commit certificate that decided it, and run every post-commit duty
    /// that goes with it.
    ///
    /// This is the only way a block enters this core, and that is the point.
    /// The duties below belong to the one commit path,
    /// which is where they were written and where they stayed: the Malachite
    /// `Decided` handler recorded outcomes and drained the mempool but did
    /// neither of the other two, so on the engine we actually ship, `/head`'s
    /// `committed` counter never moved and — the one that mattered — a
    /// session token survived the rotation or suspension that was supposed to
    /// cut it off, for up to its full TTL. Keeping the duties attached to the
    /// commit means a new driver inherits them by calling this instead of
    /// having to remember a list; `replica.commit_block*` remains reachable,
    /// but reaching for it is now visibly a decision to skip them.
    ///
    /// Verification lives in `commit_block_with_certificate` (transaction
    /// signatures, timestamp monotonicity, app-hash agreement) and, before
    /// that, in `engine_malachite::verify_decided`; nothing here re-decides
    /// anything. An `Err` means nothing was applied, so no duty runs either.
    pub fn commit_decided(&mut self, block: &Block, certificate_bytes: &[u8]) -> Result<(), ReplicaError> {
        let outcomes = self.replica.commit_block_with_certificate(block, certificate_bytes)?;
        self.record_outcomes(block, outcomes);
        self.mempool.remove_committed(block);
        self.committed_count += 1;
        // A completed key rotation or a suspension enacted in this
        // very block must cut off any session token minted under the
        // pre-rotation key or before the suspension — immediately, not left
        // to expire on TTL. Sweeping against the just-committed state (which
        // already reflects both) after every commit covers this without a
        // separate per-tx hook into `edet_state::apply`.
        let now = super::session::now_unix_secs();
        self.sessions.prune_expired(now);
        self.sessions.prune_invalid(&self.replica.state);
        // Beside the session sweep and for the same reason: a store that only
        // ever grew would hold every credential a node was ever shown. Past
        // its window a signature is refused by the skew check anyway, so
        // remembering it buys nothing.
        self.seen.prune(now);
        Ok(())
    }

    pub fn head(&self) -> u64 {
        self.replica.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{dev_seed, pubkey_of, sign_tx};
    use crate::serve::dev_genesis;
    use edet_state::tx::Tx;
    use edet_state::types::Party;

    /// A signed, well-formed transaction from a member with no bond headroom
    /// left.
    /// Two accounts nobody has backed, and their signing seeds.
    fn broke_pair(core: &mut NodeCore) -> (u64, u64) {
        let st = &mut core.replica.state;
        let a = st.new_account(vec![crate::block::pubkey_of(&dev_seed(90))]);
        let b = st.new_account(vec![crate::block::pubkey_of(&dev_seed(91))]);
        (a, b)
    }

    fn broke_tx(a: u64, b: u64, n: u64) -> SignedTx {
        sign_tx(
            crate::block::DEV_CHAIN_ID,
            Tx::Accept {
                debtor: Party::Member(a),
                creditor: Party::Member(b),
                amount: 40.0,
                maturity_epochs: 30,
                arb: None,
            },
            crate::block::counter_nonce(n),
            30,
            &[dev_seed(90), dev_seed(91)],
        )
        .expect("sign")
    }

    fn bonded_tx(n: u64) -> SignedTx {
        sign_tx(
            crate::block::DEV_CHAIN_ID,
            Tx::Accept {
                debtor: Party::Member(0),
                creditor: Party::Member(1),
                amount: 40.0,
                maturity_epochs: 30,
                arb: None,
            },
            crate::block::counter_nonce(n),
            30,
            &[dev_seed(0), dev_seed(1)],
        )
        .expect("sign")
    }

    /// The gap this screen closes: without it the bond binds only at `apply`,
    /// i.e. after the transaction has already been gossiped, proposed,
    /// ordered and committed — so an exhausted member still consumes the
    /// whole consensus round the bond exists to protect.
    #[test]
    fn the_ingress_screen_drops_a_bonded_out_transaction_before_mempool_or_gossip() {
        let mut core = NodeCore::new(0, 1, dev_genesis(5));
        // Two accounts the community has put NOTHING behind — the exhausted
        // payer this screen exists for, and the shape a spam ring actually
        // takes. Neither has capacity, so neither has headroom, so neither
        // gets the free allowance: the write floor for free keys is zero.
        let (a, b) = broke_pair(&mut core);
        core.replica.state.params.bond_free_allowance = 0;
        assert!(
            core.replica.state.bond_headroom(a) < core.replica.state.params.dust,
            "fixture must have an exhausted payer"
        );

        let queued = core.submit(broke_tx(a, b, 1));

        assert!(!queued, "a screened transaction must not be relayed to peers");
        assert_eq!(core.mempool.len(), 0, "nor queued for a block");
    }

    /// The screen must agree with the gate, not exceed it: the same
    /// transaction from a member still inside its allowance is admitted and
    /// gossiped as before.
    #[test]
    fn the_ingress_screen_admits_traffic_the_gate_would_admit() {
        let mut core = NodeCore::new(0, 1, dev_genesis(5));
        assert!(core.replica.state.params.bond_free_allowance > 0);

        let queued = core.submit(bonded_tx(2));

        assert!(queued, "an affordable transaction must still be relayed");
        assert_eq!(core.mempool.len(), 1, "and queued");
    }

    /// Screening must never touch the recovery path, or a saturated member
    /// could not get its discharge into a block at all — the ingress version
    /// of the exemption that keeps a default from becoming absorbing.
    #[test]
    fn the_ingress_screen_never_blocks_a_free_class() {
        let mut core = NodeCore::new(0, 1, dev_genesis(5));
        let (a, _b) = broke_pair(&mut core);
        core.replica.state.params.bond_free_allowance = 0;
        assert!(core.replica.state.bond_headroom(a) < core.replica.state.params.dust);

        // A free class the exhausted member signs alone. `Exit` rather than a
        // `Settle` on a contract that does not exist yet: an envelope naming an
        // unissued id is kept out of the mempool on its own account, and this
        // probe is about the bond screen.
        let exit = sign_tx(
            crate::block::DEV_CHAIN_ID,
            Tx::Exit { member: a },
            crate::block::counter_nonce(3),
            30,
            &[dev_seed(90)],
        )
        .expect("sign");
        let queued = core.submit(exit);

        assert!(queued, "a zero-bond class must pass the screen at any headroom");
        assert_eq!(core.mempool.len(), 1);
    }

    /// **A cold members read computes off the lock and banks what it
    /// computed.** Under the lock the read hands back the rows, the members
    /// still to compute and a copy of the cut's inputs; the handler computes
    /// on the copy; and the next read finds every one of them cached, so it
    /// computes nothing.
    #[test]
    fn a_cold_members_read_computes_off_the_lock_and_fills_the_cache() {
        let core = NodeCore::new(0, 1, dev_genesis(5));
        let first = super::super::views::members_read(&core, Some(0), super::super::views::Page::first());
        assert_eq!(first.pending.len(), 5, "every founder's capacity is cold on the first read");
        let snap = first.snapshot.as_ref().expect("a copy of the cut's inputs to compute on");
        let computed: Vec<(u64, f64)> = first.pending.iter().map(|p| (p.id, snap.capacity_of(p.id))).collect();
        core.fill_capacity_cache(snap, &computed);
        let value = super::super::views::members_finish(first, &computed);
        let rows = value.as_array().expect("an unpaged read is a bare array");
        assert!(rows.iter().all(|r| r.get("capacity").is_some()), "every computed capacity was patched in");

        let second = super::super::views::members_read(&core, Some(0), super::super::views::Page::first());
        assert!(second.pending.is_empty(), "the second read finds every capacity cached");
        assert!(second.snapshot.is_none(), "and copies nothing");
    }

    /// A duplicate spends no token: the limiter is keyed on the member the
    /// envelope is billed to, and a token consumed before the dedup let anyone
    /// re-post a member's own envelope to drain that member's bucket. Two
    /// hundred re-posts of one envelope, and a fresh one still queues.
    #[test]
    fn a_duplicate_submission_spends_no_token() {
        let mut core = NodeCore::new(0, 1, dev_genesis(5));
        let tx = bonded_tx(7);
        assert!(core.submit(tx.clone()));
        for _ in 0..200 {
            assert!(!core.submit(tx.clone()), "already queued");
        }
        assert!(core.submit(bonded_tx(8)), "the bucket was not drained by the duplicates");
        assert_eq!(core.mempool.len(), 2);
    }

    /// An envelope naming an id the ledger has not issued never enters the
    /// mempool: it cannot succeed now, and a block slot is what it would cost.
    #[test]
    fn an_envelope_naming_an_unissued_id_is_not_queued() {
        let mut core = NodeCore::new(0, 1, dev_genesis(5));
        let future = core.replica.state.next_contract;
        let stx = sign_tx(
            crate::block::DEV_CHAIN_ID,
            Tx::Settle { contract: future, amount: 1.0 },
            crate::block::counter_nonce(9),
            30,
            &[dev_seed(0), dev_seed(1)],
        )
        .expect("sign");
        assert!(!core.submit(stx));
        assert_eq!(core.mempool.len(), 0);
    }

    /// A transaction already in this mempool must not re-flood the network:
    /// `submit` reports "newly accepted", and that is what the gossip hop is
    /// conditioned on (`driver::submit`).
    #[test]
    fn a_resubmitted_transaction_is_not_gossiped_twice() {
        let mut core = NodeCore::new(0, 1, dev_genesis(5));
        let tx = bonded_tx(6);

        assert!(core.submit(tx.clone()), "first submission is new");
        assert!(!core.submit(tx), "the same transaction again is not");
        assert_eq!(core.mempool.len(), 1);
    }

    /// A transaction can be affordable when it enters the mempool and
    /// unaffordable by the time this node proposes, so the proposer takes a
    /// second look rather than spending block space on it.
    #[test]
    fn the_proposer_drops_transactions_that_became_unaffordable() {
        let mut core = NodeCore::new(0, 1, dev_genesis(5));
        let (a, b) = broke_pair(&mut core);
        // Backed at admission, so it is affordable when it enters.
        core.replica.state.place_stake(0, a, 5_000.0);
        core.submit(broke_tx(a, b, 4));
        assert_eq!(core.proposable_batch(512).len(), 1, "affordable at admission");

        // The standing behind the payer goes away underneath it.
        core.replica.state.edges.clear();
        core.replica.state.params.bond_free_allowance = 0;

        assert!(core.proposable_batch(512).is_empty(), "the proposer must leave it behind");
        assert_eq!(core.mempool.len(), 1, "without dropping it from the mempool, where it may recover");
    }

    /// `AddressIndex` must not get stuck at whatever membership size it
    /// was first built against — a member admitted after the cache's first
    /// build has to resolve too, the moment membership length changes.
    #[test]
    fn address_index_rebuilds_when_membership_grows() {
        let mut core = NodeCore::new(0, 1, dev_genesis(1));
        let addr0 = super::super::views::member_address_bytes(&core.replica.state.members[&0]);
        assert_eq!(core.resolve_address(&addr0), Some(0), "cache builds on first lookup");

        let new_id = core
            .replica
            .state
            .add_underwriter(vec![[77u8; 32]], 25_000.0)
            .expect("add a second member after the cache already built once");
        let addr_new = super::super::views::member_address_bytes(&core.replica.state.members[&new_id]);
        assert_eq!(
            core.resolve_address(&addr_new),
            Some(new_id),
            "a newly admitted member must resolve once the cache detects the length change, not stay stale"
        );
    }

    /// The post-commit duties must hang off the DECIDED path, not off some
    /// consensus driver that remembers to run them.
    ///
    /// This is the shape of a break that shipped: `try_commit` — the dev
    /// consensus's commit — swept sessions and counted commits, and the
    /// engine's `Decided` handler did neither, so on the binary we deploy a
    /// rotated-out device kept its read access until its token's TTL ran out.
    /// The dev consensus is gone; what this pins down is that the ONE
    /// remaining commit entry point still does both. That the ENGINE goes
    /// through it is the other half, gated in `tests/malachite_http.rs` on
    /// `/head`'s `committed` counter, which only this method advances.
    ///
    /// The rotation here is applied to state directly rather than driven
    /// through a `RotateFinalize` transaction: this test is about the commit
    /// path running the sweep, not about how a keyset comes to change
    /// (`session.rs`'s own tests cover `prune_invalid`'s rule, and
    /// `edet_state::apply` covers rotation).
    #[test]
    fn the_decided_commit_path_runs_the_post_commit_bookkeeping() {
        let mut core = NodeCore::new(0, 1, dev_genesis(5));
        let key0 = pubkey_of(&dev_seed(0));
        let (token, _expires) = core
            .sessions
            .mint(0, key0, super::super::session::now_unix_secs())
            .expect("mint a session for member 0");
        assert_eq!(
            core.sessions.resolve(&token, super::super::session::now_unix_secs()),
            Some(0),
            "the token authenticates before anything changes"
        );

        // Member 0's device key is rotated out from under the live session.
        // `key_index` moves with `keys`, as `apply::rotate_finalize` does it:
        // the commit path audits the invariants now, and a fixture that left
        // the two disagreeing would be refused as a violation rather than
        // testing what it means to.
        core.replica.state.members.get_mut(&0).expect("member 0").keys = vec![[99u8; 32]];
        core.replica.state.key_index.remove(&key0);
        core.replica.state.key_index.insert([99u8; 32], 0);

        let block = Block { height: 1, time_secs: 1_000, app_hash: core.replica.app_hash(), txs: Vec::new() };
        core.commit_decided(&block, b"certificate").expect("commit the decided block");

        assert_eq!(
            core.sessions.resolve(&token, super::super::session::now_unix_secs()),
            None,
            "a rotated-out key's session must be cut off by the commit, not left to expire on TTL"
        );
        assert_eq!(core.committed_count, 1, "and the commit must be counted (`/head`'s `committed`)");
    }
}
