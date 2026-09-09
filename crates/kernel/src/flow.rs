//! Capacity as a cut.
//!
//! An account's capacity is the maximum flow into it from the community's
//! underwriters, across directed stakes of what creditors have placed, over a
//! residual that already has outstanding credit reserved out of it. Admission,
//! reputation, credit limits and Sybil resistance are all this one quantity.
//!
//! An underwriter is any member who has declared a SUPPLY: an accepted
//! liability, capped by their own capacity. Supply bounds not only what they
//! may confer on one member but the total across everyone they back, so an
//! underwriter's worst case — folly, malice, or backing a coalition of
//! identities they control — is losing exactly what they declared.
//!
//! Everything here is **integer**, in minor units. Max-flow over floating
//! point puts branch decisions behind epsilon comparisons: two replicas can
//! agree on the value and still take different augmenting paths, which is a
//! consensus fault rather than a rounding one. Iteration order comes from
//! ordered maps, so the flow network is a function of state and not of the
//! order records happened to be inserted.
//!
//! # What an outstanding obligation holds
//!
//! An augmenting path runs from an underwriter's SUPPLY ARC, through stake
//! edges, to the debtor. **Both must be charged**, and the obligation must
//! remember exactly what it took.
//!
//! Charging only the stake edges leaves every supply arc at its full declared
//! value on the next query, so two debtors backed by the same underwriter each
//! draw the whole supply and the overshoot is the number of debtors that
//! underwriter backs — measured, before this was fixed, at 250,000 against a
//! true cut of 2,500 with a hundred of them.
//!
//! Releasing anything less than what was taken is the same error pointing the
//! other way. A release that walked only the edges incident to the debtor left
//! every upstream edge of a multi-hop path reserved forever, so settling an
//! obligation destroyed capacity that nothing was behind.
//!
//! [`Held`] is the fix for both: [`reserve`] returns precisely the arcs it
//! consumed and [`release`] gives precisely those back. It is exact rather than
//! proportional, so no standing is created or destroyed and the release is the
//! inverse of the reservation rather than an approximation of it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Directed stake, keyed `(creditor, debtor)`: "this creditor backs this
/// debtor, to this much".
///
/// Direction is load-bearing rather than bookkeeping. Backing is not a
/// symmetric relation: that A is willing to carry B says nothing about
/// whether anyone is willing to carry A. Held symmetrically, extending credit
/// would raise the LENDER's own limit, so a member could grow their capacity
/// by lending — which is backwards, and which a symmetric graph cannot
/// express its way out of.
pub type Edges = BTreeMap<(usize, usize), u64>;

/// Flow currently held against outstanding credit, keyed like [`Edges`].
pub type Reservations = BTreeMap<(usize, usize), u64>;

/// What each edge holds, read in ONE pass beside the edge map.
///
/// Both maps are ordered on the same key and every reader here walks `edges`
/// in key order, so the reservation on an edge is the next entry of
/// `reserved` at or past it: one sequential walk over both maps, where a
/// lookup per edge descends the whole tree every time. The answer is the same
/// to the bit — a walk and a lookup read the same entry — and what changes is
/// that the build stops growing with how much of the graph is reserved.
/// Measured A/B on one box in one sitting, conditions interleaved and the
/// least of five rounds kept: with every edge reserved the build reads 81 ms
/// by lookup and 44 ms by this cursor at 100,000 accounts, 20 ms and 5.6 ms
/// at 20,000, against 33 ms and 4.4 ms with nothing reserved. Keys must be
/// asked in ascending order, which `debug_assert` holds.
struct ReservedCursor<'a> {
    next: std::iter::Peekable<std::collections::btree_map::Iter<'a, (usize, usize), u64>>,
    #[cfg(debug_assertions)]
    last: Option<(usize, usize)>,
}

impl<'a> ReservedCursor<'a> {
    fn new(reserved: &'a Reservations) -> Self {
        Self {
            next: reserved.iter().peekable(),
            #[cfg(debug_assertions)]
            last: None,
        }
    }

    /// The reservation on `key`, or zero. Advances past every entry below it.
    fn at(&mut self, key: (usize, usize)) -> u64 {
        #[cfg(debug_assertions)]
        {
            debug_assert!(self.last.is_none_or(|l| l < key), "reservation cursor asked out of order");
            self.last = Some(key);
        }
        while let Some(&(&k, &v)) = self.next.peek() {
            if k < key {
                self.next.next();
            } else {
                return if k == key { v } else { 0 };
            }
        }
        0
    }
}

/// Flow currently drawn through each underwriter's supply arc.
///
/// The supply arc is a real arc of the network and it is shared by every
/// debtor that underwriter backs. Without this, reservations recorded against
/// distinct stake edges never see each other and one declared supply is lent
/// once per debtor.
pub type Committed = BTreeMap<usize, u64>;

/// Exactly what one outstanding obligation holds: which stake edges, which
/// supply arcs, and how much on each.
///
/// Stored with the obligation, and handed back verbatim at settlement. The
/// alternative — releasing proportionally across arcs incident to the debtor —
/// is not the inverse of anything and was measurably wrong in both directions.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Held {
    /// `((creditor, debtor), amount)`, in [`Edges`] key order.
    pub edges: Vec<((usize, usize), u64)>,
    /// `(underwriter, amount)`, in underwriter order.
    pub supply: Vec<(usize, u64)>,
}

impl Held {
    /// The obligation this covers. Every unit crosses exactly one supply arc,
    /// so the supply side is the total.
    pub fn amount(&self) -> u64 {
        self.supply.iter().map(|&(_, a)| a).sum()
    }
}

/// One source-to-debtor route of a [`Held`], and what it carries.
///
/// A `Held` is stored as arc totals because that is what `reserved` and
/// `committed` cache. It is a FLOW, though, and the difference shows the
/// moment anything divides one: an arc total can be split proportionally and
/// the pieces are no longer routes, while a path can be split at any value and
/// every piece still is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    /// The underwriter whose supply arc this route leaves.
    pub source: usize,
    /// Stake arcs in order, source side first.
    pub arcs: Vec<(usize, usize)>,
    /// Minor units carried, the same on every arc of the route.
    pub value: u64,
}

/// Decompose a [`Held`] into the routes it is made of, or say it is not a flow.
///
/// **This is the conservation check, and it is the same operation as the
/// decomposition.** A conserved flow always decomposes: at any node that is
/// not the debtor, whatever arrived has to leave, so a forward walk can never
/// be stranded. One that is not conserved strands the walk, and that is the
/// `Err`. Nothing else has to compare inflow against outflow separately.
///
/// Deterministic: sources are taken in underwriter order and each step takes
/// the lowest-numbered arc still carrying something, so the decomposition is a
/// function of the `Held` and never of how it was built. Every value is an
/// exact integer partition of what the arcs already carried, so the routes sum
/// back to the input arc for arc with no rounding anywhere.
pub fn decompose(held: &Held, debtor: usize) -> Result<Vec<Path>, String> {
    let mut edge_left: BTreeMap<(usize, usize), u64> = held.edges.iter().copied().collect();
    let mut supply_left: BTreeMap<usize, u64> = held.supply.iter().copied().collect();
    let mut out: Vec<Path> = Vec::new();

    // Bounded by the arc count: every completed route empties at least one arc
    // or one supply entry, and neither is ever refilled.
    let budget = held.edges.len() + held.supply.len() + 1;
    for _ in 0..budget {
        let Some((&src, _)) = supply_left.iter().find(|(_, &v)| v > 0) else { break };
        let mut node = src;
        let mut arcs: Vec<(usize, usize)> = Vec::new();
        let mut value = supply_left[&src];
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        seen.insert(node);
        while node != debtor {
            let Some((&(a, b), &w)) = edge_left.range((node, 0)..(node + 1, 0)).find(|(_, &w)| w > 0) else {
                return Err(format!("held is not a flow: route from {src} stranded at {node} with {value} to place"));
            };
            debug_assert_eq!(a, node);
            value = value.min(w);
            arcs.push((a, b));
            node = b;
            if !seen.insert(node) {
                return Err(format!("held is not a flow: route from {src} cycles at {node}"));
            }
        }
        *supply_left.get_mut(&src).expect("source was just read") -= value;
        for key in &arcs {
            *edge_left.get_mut(key).expect("arc was just walked") -= value;
        }
        out.push(Path { source: src, arcs, value });
    }

    if supply_left.values().any(|&v| v > 0) {
        return Err("held is not a flow: supply left unplaced".into());
    }
    if edge_left.values().any(|&v| v > 0) {
        return Err("held is not a flow: arc flow with no source behind it".into());
    }
    Ok(out)
}

/// Rebuild a [`Held`] from routes: arc totals are the sum of what crosses them.
///
/// Exact by construction, which is the point of doing every division at the
/// route level. `reserved` and `committed` are compared to these sums for
/// EQUALITY (§Verification invariant 2), and a sum of integers cannot round.
pub fn from_paths(paths: &[Path]) -> Held {
    let mut edges: BTreeMap<(usize, usize), u64> = BTreeMap::new();
    let mut supply: BTreeMap<usize, u64> = BTreeMap::new();
    for p in paths {
        if p.value == 0 {
            continue;
        }
        *supply.entry(p.source).or_insert(0) += p.value;
        for &key in &p.arcs {
            *edges.entry(key).or_insert(0) += p.value;
        }
    }
    Held { edges: edges.into_iter().collect(), supply: supply.into_iter().collect() }
}

/// Stand-in for an unbounded arc — used for sink arcs only, never for supply.
/// Not `u64::MAX`: arcs are summed during augmentation, and a true maximum
/// would overflow the moment two of them meet.
const UNBOUNDED: u64 = u64::MAX / 4;

/// An underwriter and the supply they declared, in minor units.
pub type Underwriters = [(usize, u64)];

// ---------------------------------------------------------------- network --

/// Residual flow network. Built fresh per query — it holds no state that
/// outlives the answer, so nothing here can drift from the ledger.
///
/// Its nodes are the accounts that actually TAKE PART, not every account the
/// ledger has ever issued an id to, and the difference is a liveness property
/// rather than a tidiness one: an account with no incident stake, no declared
/// supply and no place in the measured set cannot lie on any augmenting path —
/// it has no arcs — so it contributes nothing to any cut. Sizing the graph by
/// `next_member` nonetheless makes every dead account cost a node in every
/// query on every validator, permanently, and account creation is unbillable by
/// construction. A community carrying 10⁵ abandoned rows would pay for them on
/// every acceptance for the rest of its life.
///
/// The compact index is rebuilt per query in ascending ledger-id order, so it
/// is a function of state and not of history — the same discipline the arc
/// order already follows, and the reason two replicas still build
/// byte-identical networks. What it costs is the old "ids ARE indices, so no
/// side table can drift out of step" property; what replaces it is a table
/// that cannot drift because it does not outlive the answer.
///
/// **The BUILD is what an ACCEPTANCE costs**, not the search: measured at
/// 100,000 accounts and 800k edges, 25 ms of a 34 ms early-exit query, and the
/// share grows with the graph — two thirds at 1,000 accounts, nine tenths at
/// 100,000 — because an early exit stops long before the search has seen the
/// network the build had to write. A full cut is different: the same probe
/// puts the build at a third of a maximum flow, and a full cut is what the
/// audit, `seed_reach`, `conferrable` and every served capacity compute.
///
/// Keeping the adjacency BESIDE the edge map instead of rebuilding it is not
/// taken, and not for determinism: held in ascending key order, the order a
/// fresh build produces, a patched adjacency and a rebuilt one are the same
/// bytes, and a probe can hold them equal. It is a second structure that every
/// write to the edge map has to keep equal to the first, and what it would buy
/// is the build's share of an early-exit query on a term that bounds nothing:
/// an acceptance is paid per transaction, and a block carries at most
/// `MAX_TXS_PER_BLOCK` of them, where the state root is paid on every block
/// whatever it held and is linear in the ledger; the full cuts, which gain a
/// third at most, are the ones the memo already keeps off the block. What is
/// taken is the constant factors that need no second structure: the CSR
/// adjacency and the mark-and-rank index below, a 3x on the same term, and
/// [`ReservedCursor`], which reads the reservation map in the edge walk's own
/// order rather than once per edge — without it the build grows with how much
/// of the graph is reserved, an axis the kernel cost table holds at zero and
/// the probe beside it varies.
struct Network {
    to: Vec<usize>,
    cap: Vec<u64>,
    /// The tail of each arc, in arc-id order — the input to the CSR fill
    /// below, and dropped once `head_start`/`head_arcs` exist.
    tail: Vec<u32>,
    /// **Adjacency in CSR form**: `head_arcs[head_start[u]..head_start[u+1]]`
    /// is the arc list of node `u`, in the order the arcs were created.
    ///
    /// The order is load-bearing — `flow`'s `next_arc` walks it, so it chooses
    /// the augmenting paths — and a counting sort by tail is stable, so it is
    /// exactly the order a `Vec` per node would have held. What it removes is
    /// one heap allocation per NODE per query, plus every regrowth. Measured
    /// A/B on one quiet box, three runs each, with the mark-and-rank slot pass
    /// below — the two together are what these figures cover, since neither
    /// changes what the search does:
    ///
    /// | | build | one acceptance |
    /// |---|---|---|
    /// | 20,000 accounts, before | 16.5 ms | 20.1 ms |
    /// | 20,000 accounts, after | 4.7 ms | 6.2 ms |
    /// | 100,000 accounts, before | 117.4 ms | 128.8 ms |
    /// | 100,000 accounts, after | 25.4 ms | 34.4 ms |
    ///
    /// Read them as ratios. The same probe on the same tree read 81.7 ms for a
    /// full 20,000-account query while the machine was busy and 25.2 ms while
    /// it was not, which is why every figure here is a pair.
    head_start: Vec<u32>,
    head_arcs: Vec<u32>,
    /// Arc id of the forward direction of each ledger edge, in `Edges` order.
    /// Used to read back which edges an augmentation consumed.
    ledger_arc: Vec<((usize, usize), usize)>,
    /// Arc id of each underwriter's supply arc, in underwriter order. Read
    /// back exactly as `ledger_arc` is.
    supply_arc: Vec<(usize, usize)>,
    src: usize,
    snk: usize,
}

impl Network {
    /// `n` bounds which ledger ids are valid; it no longer sizes the graph.
    ///
    /// Arcs are added in `Edges` order, then supply arcs in underwriter order,
    /// then target arcs in `targets` order — fixed, and over a node index
    /// built in ascending id order, so two replicas build byte-identical
    /// networks from equal state.
    ///
    /// `own_supply` says whether a target's OWN supply arc takes part. `false`
    /// is the credit reading: the exclusion below is the no-self-underwriting
    /// rule. `true` is the write layer's, where a member's own declaration is
    /// already part of what they may spend.
    fn build(
        edges: &Edges,
        reserved: &Reservations,
        committed: &Committed,
        uw: &Underwriters,
        targets: &[usize],
        n: usize,
        own_supply: bool,
    ) -> Self {
        // Exactly the accounts that will receive an arc below, marked under
        // exactly the conditions those loops apply. An account absent here has
        // no arcs, so no path crosses it and no cut is changed by leaving it
        // out — which is what makes this a compaction rather than a heuristic.
        //
        // **Counted, not sorted.** The slots are ranks in ascending ledger-id
        // order either way, so the mapping is still a function of WHICH
        // accounts take part and never of insertion history — but a
        // mark-and-rank pass is O(n + E) where collecting `2E` ids and sorting
        // them is O(E log E), and E is the term that grows.
        //
        // What it costs is one `u32` per ledger id per query, which is the
        // allocation proportional to the ID SPACE the compaction removed from
        // the SEARCH. It is not back in the search: `m` is still the
        // participant count, so the level array, the arc cursors and the
        // adjacency are all sized by who takes part. A community carrying 10^5
        // abandoned rows pays 400 KB and a memset here, and nothing per query
        // anywhere else.
        let mut mark = vec![false; n];
        for &(c, d) in edges.keys() {
            if c < n && d < n {
                mark[c] = true;
                mark[d] = true;
            }
        }
        for &(u, _) in uw {
            if u < n {
                mark[u] = true;
            }
        }
        for &t in targets {
            if t < n {
                mark[t] = true;
            }
        }
        let mut slot_of = vec![u32::MAX; n];
        let mut m = 0usize;
        for (id, &here) in mark.iter().enumerate() {
            if here {
                slot_of[id] = m as u32;
                m += 1;
            }
        }
        let slot = |id: usize| (id < n && slot_of[id] != u32::MAX).then(|| slot_of[id] as usize);

        let arcs = 2 * (edges.len() + uw.len() + targets.len());
        let mut net = Network {
            to: Vec::with_capacity(arcs),
            cap: Vec::with_capacity(arcs),
            tail: Vec::with_capacity(arcs),
            head_start: Vec::new(),
            head_arcs: Vec::new(),
            ledger_arc: Vec::with_capacity(edges.len()),
            supply_arc: Vec::with_capacity(uw.len()),
            src: m,
            snk: m + 1,
        };
        let mut held = ReservedCursor::new(reserved);
        for (&(c, d), &w) in edges {
            let (Some(ci), Some(di)) = (slot(c), slot(d)) else {
                continue;
            };
            // Free capacity only, and one arc only: standing flows from the
            // creditor toward the debtor they back, never the other way. The
            // residual reverse arc `arc` creates carries zero capacity, so it
            // can only ever undo flow this search itself pushed.
            let free = w.saturating_sub(held.at((c, d)));
            let arc = net.arc(ci, di, free);
            // Keyed by LEDGER id, not by slot: `reserve` hands these back to
            // the caller, who knows nothing about this query's index.
            net.ledger_arc.push(((c, d), arc));
        }
        let tset: std::collections::BTreeSet<usize> = targets.iter().copied().collect();
        for &(u, supply) in uw {
            // Underwriters INSIDE the measured set supply it nothing. This one
            // clause is what stops a coalition underwriting itself: appointing
            // your own accomplices adds nothing to what your coalition can
            // borrow, because the measurement excludes them along with it.
            //
            // It is a CREDIT rule, and `own_supply` is the one reading that
            // does not want it: the write layer already counts a member's own
            // declaration in their floor, and a founding underwriter holds
            // supply and no in-stakes, so excluding it would leave the first
            // members of a community with nobody able to bring them in.
            if let (Some(ui), false) = (slot(u), !own_supply && tset.contains(&u)) {
                // Uncommitted supply only — the arc is shared by every debtor
                // this underwriter backs, and each of them has already drawn
                // through it.
                let free = supply.saturating_sub(committed.get(&u).copied().unwrap_or(0));
                let arc = net.arc(net.src, ui, free);
                net.supply_arc.push((u, arc));
            }
        }
        for &t in targets {
            if let Some(ti) = slot(t) {
                net.arc(ti, net.snk, UNBOUNDED);
            }
        }
        net.index(m + 2);
        net
    }

    fn arc(&mut self, u: usize, v: usize, c: u64) -> usize {
        let id = self.to.len();
        self.to.push(v);
        self.cap.push(c);
        self.tail.push(u as u32);
        self.to.push(u);
        self.cap.push(0);
        self.tail.push(v as u32);
        id
    }

    /// Turn `tail` into the CSR adjacency, by a stable counting sort.
    ///
    /// Stable is the whole requirement: `flow` walks `head_arcs` in order and
    /// that choice decides which augmenting paths it takes, so this has to
    /// reproduce the order a `Vec` per node would have been pushed in, arc for
    /// arc, or two replicas holding equal state take different paths.
    fn index(&mut self, nodes: usize) {
        let mut start = vec![0u32; nodes + 1];
        for &t in &self.tail {
            start[t as usize + 1] += 1;
        }
        for i in 0..nodes {
            start[i + 1] += start[i];
        }
        let mut cursor = start.clone();
        let mut arcs = vec![0u32; self.tail.len()];
        for (id, &t) in self.tail.iter().enumerate() {
            arcs[cursor[t as usize] as usize] = id as u32;
            cursor[t as usize] += 1;
        }
        self.head_start = start;
        self.head_arcs = arcs;
    }

    /// The arcs leaving `u`, in creation order.
    #[inline]
    fn out(&self, u: usize) -> &[u32] {
        &self.head_arcs[self.head_start[u] as usize..self.head_start[u + 1] as usize]
    }

    /// Level graph by breadth-first search over arcs with residual capacity.
    fn levels(&self) -> Option<Vec<u32>> {
        let nodes = self.head_start.len() - 1;
        let mut level = vec![u32::MAX; nodes];
        level[self.src] = 0;
        let mut q = VecDeque::from([self.src]);
        while let Some(u) = q.pop_front() {
            for &a in self.out(u) {
                let a = a as usize;
                if self.cap[a] > 0 && level[self.to[a]] == u32::MAX {
                    level[self.to[a]] = level[u] + 1;
                    q.push_back(self.to[a]);
                }
            }
        }
        (level[self.snk] != u32::MAX).then_some(level)
    }

    /// Dinic, stopping as soon as `limit` units are found.
    ///
    /// The inner search is **iterative**. A recursive augmenting-path walk is
    /// bounded only by the level-graph depth, which is bounded only by the
    /// account count — so a large community could overflow the stack, and a
    /// stack overflow inside block validation is a liveness fault.
    fn flow(&mut self, limit: u64) -> u64 {
        let mut total = 0u64;
        while total < limit {
            let Some(level) = self.levels() else { break };
            let mut next_arc = vec![0usize; self.head_start.len() - 1];
            let mut path: Vec<usize> = Vec::new();
            let mut u = self.src;
            loop {
                if total >= limit {
                    break;
                }
                if u == self.snk {
                    // Augment by the bottleneck, then retreat to the first
                    // arc this saturated so the walk resumes there.
                    let want = limit - total;
                    let push = path.iter().map(|&a| self.cap[a]).min().unwrap_or(0).min(want);
                    let mut cut = path.len();
                    for (i, &a) in path.iter().enumerate() {
                        self.cap[a] -= push;
                        self.cap[a ^ 1] += push;
                        if self.cap[a] == 0 && i < cut {
                            cut = i;
                        }
                    }
                    total += push;
                    path.truncate(cut);
                    u = if cut == 0 { self.src } else { self.to[path[cut - 1]] };
                    continue;
                }
                // Advance along the next usable arc out of `u`.
                let mut advanced = false;
                while next_arc[u] < self.out(u).len() {
                    let a = self.out(u)[next_arc[u]] as usize;
                    let v = self.to[a];
                    if self.cap[a] > 0 && level[v] == level[u] + 1 {
                        path.push(a);
                        u = v;
                        advanced = true;
                        break;
                    }
                    next_arc[u] += 1;
                }
                if advanced {
                    continue;
                }
                // Dead end: this node cannot reach the sink in this level
                // graph. Retreat, and never try the arc that led here again.
                if let Some(a) = path.pop() {
                    u = self.to[a ^ 1];
                    next_arc[u] += 1;
                } else {
                    break;
                }
            }
        }
        total
    }
}

// ---------------------------------------------------------------- queries --

/// Capacity of an account set: the maximum flow the underwriters can push into
/// it, over what is left after outstanding credit.
///
/// The set form is the security statement. Because this is a cut, a coalition
/// gains nothing by splitting across identities — edges internal to the set
/// never cross its boundary, so wash trading between members of the set is
/// arithmetically worthless rather than merely detectable.
///
/// `limit` stops the search early; pass `u64::MAX` for the true maximum. A
/// transaction only ever needs to know whether room exists for one amount.
pub fn capacity(
    edges: &Edges,
    reserved: &Reservations,
    committed: &Committed,
    uw: &Underwriters,
    targets: &[usize],
    n: usize,
    limit: u64,
) -> u64 {
    capacity_inner(edges, reserved, committed, uw, targets, n, limit, false)
}

/// [`capacity`] with the target's own supply arc in the network.
///
/// The write layer's reading. Credit must keep the exclusion — a member may
/// not underwrite their own borrowing — but a member's own declaration is
/// already part of what they may SPEND, and a founding underwriter with supply
/// and no in-stakes has to be able to bring the first members in.
pub fn capacity_with_own_supply(
    edges: &Edges,
    reserved: &Reservations,
    committed: &Committed,
    uw: &Underwriters,
    targets: &[usize],
    n: usize,
    limit: u64,
) -> u64 {
    capacity_inner(edges, reserved, committed, uw, targets, n, limit, true)
}

#[allow(clippy::too_many_arguments)]
fn capacity_inner(
    edges: &Edges,
    reserved: &Reservations,
    committed: &Committed,
    uw: &Underwriters,
    targets: &[usize],
    n: usize,
    limit: u64,
    own_supply: bool,
) -> u64 {
    if targets.is_empty() || n == 0 {
        return 0;
    }
    Network::build(edges, reserved, committed, uw, targets, n, own_supply).flow(limit)
}

/// What a member may confer on somebody else: their declared supply if they
/// are an underwriter, otherwise their own capacity.
///
/// The two are different quantities and must not be conflated. Capacity is how
/// much the community will carry YOU; supply is how much you have promised to
/// carry others. Supply is not re-derived at read time — it was capped by
/// capacity when declared and stands until changed, because a promise that
/// silently shrank would not be one. Nor is it netted against what the
/// underwriter has already committed: what they may confer is a limit, and
/// limits propagate freely. Only simultaneous use is bounded.
pub fn conferrable(
    edges: &Edges,
    reserved: &Reservations,
    committed: &Committed,
    uw: &Underwriters,
    member: usize,
    n: usize,
) -> u64 {
    if let Some(&(_, supply)) = uw.iter().find(|&&(u, _)| u == member) {
        return supply;
    }
    capacity(edges, reserved, committed, uw, &[member], n, u64::MAX)
}

/// Capacity of one account.
pub fn capacity_of(
    edges: &Edges,
    reserved: &Reservations,
    committed: &Committed,
    uw: &Underwriters,
    account: usize,
    n: usize,
) -> u64 {
    capacity(edges, reserved, committed, uw, &[account], n, u64::MAX)
}

/// Reserve `amount` of flow against `debtor`, returning exactly the arcs the
/// augmentation consumed. Returns `None` and changes nothing if the residual
/// cannot carry it.
///
/// Reservation is what holds the bound across *independent* creditors. Without
/// it each creditor evaluates a pristine graph and lends against the same
/// edges, and the community's total exposure overshoots by the number of
/// creditors — not by a little.
///
/// Both halves of the path are charged. The supply arcs are not bookkeeping:
/// they are the arcs every debtor of one underwriter shares, and leaving them
/// uncharged is the difference between a bound and a suggestion.
pub fn reserve(
    edges: &Edges,
    reserved: &mut Reservations,
    committed: &mut Committed,
    uw: &Underwriters,
    debtor: usize,
    n: usize,
    amount: u64,
) -> Option<Held> {
    reserve_inner(edges, reserved, committed, uw, debtor, n, amount, false)
}

/// [`reserve`] with the debtor's own supply arc in the network.
///
/// The write layer's reading, paired with [`capacity_with_own_supply`]. The
/// `Held` it returns may name the debtor's own supply arc; [`release`] needs no
/// change for it.
pub fn reserve_with_own_supply(
    edges: &Edges,
    reserved: &mut Reservations,
    committed: &mut Committed,
    uw: &Underwriters,
    debtor: usize,
    n: usize,
    amount: u64,
) -> Option<Held> {
    reserve_inner(edges, reserved, committed, uw, debtor, n, amount, true)
}

#[allow(clippy::too_many_arguments)]
fn reserve_inner(
    edges: &Edges,
    reserved: &mut Reservations,
    committed: &mut Committed,
    uw: &Underwriters,
    debtor: usize,
    n: usize,
    amount: u64,
    own_supply: bool,
) -> Option<Held> {
    if amount == 0 {
        return Some(Held::default());
    }
    let mut net = Network::build(edges, reserved, committed, uw, &[debtor], n, own_supply);
    let before_edges: Vec<u64> = net.ledger_arc.iter().map(|&(_, a)| net.cap[a]).collect();
    let before_supply: Vec<u64> = net.supply_arc.iter().map(|&(_, a)| net.cap[a]).collect();
    if net.flow(amount) < amount {
        return None;
    }
    let mut held = Held::default();
    for (i, &(key, arc)) in net.ledger_arc.iter().enumerate() {
        let used = before_edges[i].saturating_sub(net.cap[arc]);
        if used > 0 {
            *reserved.entry(key).or_insert(0) += used;
            held.edges.push((key, used));
        }
    }
    for (i, &(u, arc)) in net.supply_arc.iter().enumerate() {
        let used = before_supply[i].saturating_sub(net.cap[arc]);
        if used > 0 {
            *committed.entry(u).or_insert(0) += used;
            held.supply.push((u, used));
        }
    }
    Some(held)
}

/// Give back exactly what [`reserve`] took.
///
/// Exact rather than proportional, and that is the whole point: the flow an
/// obligation held ran along specific arcs, and every other rule for choosing
/// which arcs to credit is wrong somewhere. Crediting only the arcs incident to
/// the debtor stranded the upstream half of every multi-hop path.
pub fn release(reserved: &mut Reservations, committed: &mut Committed, held: &Held) {
    for &(key, amount) in &held.edges {
        if let Some(r) = reserved.get_mut(&key) {
            *r = r.saturating_sub(amount);
        }
    }
    for &(u, amount) in &held.supply {
        if let Some(c) = committed.get_mut(&u) {
            *c = c.saturating_sub(amount);
        }
    }
    reserved.retain(|_, v| *v > 0);
    committed.retain(|_, v| *v > 0);
}

/// The least an underwriter may reduce their supply to: the flow currently
/// drawn through them.
///
/// A withdrawal is decay applied to a source arc, and it takes the same floor
/// stakes take — the debt did not shrink because the underwriter changed their
/// mind. Without it, withdrawing while credit stands on you breaks the cut
/// bound directly.
pub fn supply_floor(committed: &Committed, underwriter: usize) -> u64 {
    committed.get(&underwriter).copied().unwrap_or(0)
}

/// Record a settled obligation as stake.
///
/// `edge = max(edge, min(amount, creditor_capacity))`. Two properties carry
/// the security, and both are in this one line:
///
/// - **A peak, not a sum.** Repeating a cycle between the same pair raises
///   nothing, so a wash loop cannot accumulate however often it runs.
/// - **Capped by the creditor's own capacity.** A lender cannot confer more
///   standing than they hold. Between two accounts of zero capacity every
///   settlement stakes zero, so no amount of traffic between them creates an
///   edge.
///
/// This is why the measure is stake rather than volume: settlement takes two
/// signatures and no delivery, so volume is free to fabricate, while a stake
/// is capped by a quantity the fabricator had to earn.
///
/// Note the direction, and what it means: the edge runs creditor to debtor, so
/// standing is evidence of having OWED and paid. Selling earns none of it.
pub fn stake(edges: &mut Edges, creditor: usize, debtor: usize, amount: u64, conferrable: u64) {
    if creditor == debtor {
        return;
    }
    let staked = amount.min(conferrable);
    if staked == 0 {
        return;
    }
    let e = edges.entry((creditor, debtor)).or_insert(0);
    *e = (*e).max(staked);
}

/// Decay every edge by `num/den`, but never below its live reservation.
///
/// Standing should reflect present backing rather than history. The floor is
/// not a refinement: without it, decay silently releases collateral from under
/// obligations that are still outstanding, and free capacity reappears that
/// nothing is behind.
///
/// Declared supply does not decay. It is a standing promise rather than
/// evidence of a past trade, and it changes only when its underwriter changes
/// it — down to [`supply_floor`].
pub fn decay(edges: &mut Edges, reserved: &Reservations, num: u64, den: u64) {
    if den == 0 || num >= den {
        return;
    }
    let mut held = ReservedCursor::new(reserved);
    for (k, w) in edges.iter_mut() {
        let floor = held.at(*k);
        // u128 so a large stake times the numerator cannot wrap.
        let decayed = ((*w as u128 * num as u128) / den as u128) as u64;
        *w = decayed.max(floor);
    }
    edges.retain(|_, w| *w > 0);
}

/// Total reservation held, for the conservation invariant.
pub fn reserved_total(reserved: &Reservations) -> u64 {
    reserved.values().sum()
}

/// Total flow drawn through the underwriters, which is the community's
/// outstanding insured credit. Every insured unit crosses exactly one supply
/// arc, so this is the figure invariant 2 conserves.
pub fn committed_total(committed: &Committed) -> u64 {
    committed.values().sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPLY: u64 = 2500;

    /// `k` founding underwriters, each declaring `SUPPLY`. No stakes yet: a
    /// community begins with people willing to stand behind it, not with a
    /// graph.
    fn founded(k: usize) -> (Edges, Vec<(usize, u64)>) {
        (Edges::new(), (0..k).map(|i| (i, SUPPLY)).collect())
    }

    /// `creditor` backs `debtor` for everything they may confer.
    fn back(e: &mut Edges, uw: &Underwriters, creditor: usize, debtor: usize, n: usize) {
        let c = conferrable(e, &Reservations::new(), &Committed::new(), uw, creditor, n);
        stake(e, creditor, debtor, u64::MAX, c);
    }

    fn cap(e: &Edges, uw: &Underwriters, x: usize, n: usize) -> u64 {
        capacity_of(e, &Reservations::new(), &Committed::new(), uw, x, n)
    }

    /// Capacity of a set on a pristine residual — the gross cut, which is what
    /// invariant 1 compares outstanding credit against.
    fn cut(e: &Edges, uw: &Underwriters, set: &[usize], n: usize) -> u64 {
        capacity(e, &Reservations::new(), &Committed::new(), uw, set, n, u64::MAX)
    }

    #[test]
    fn a_fresh_key_is_worth_nothing() {
        let (e, uw) = founded(6);
        assert_eq!(cap(&e, &uw, 42, 60), 0);
    }

    #[test]
    fn a_newcomer_earns_capacity_by_being_backed() {
        let (mut e, uw) = founded(6);
        let n = 60;
        assert_eq!(cap(&e, &uw, 10, n), 0);
        back(&mut e, &uw, 0, 10, n);
        assert_eq!(cap(&e, &uw, 10, n), SUPPLY);
    }

    #[test]
    fn wash_trading_creates_nothing() {
        // Two accounts nobody backs, settling enormous amounts forever.
        let (mut e, uw) = founded(6);
        let n = 60;
        for _ in 0..2000 {
            back(&mut e, &uw, 40, 41, n);
            back(&mut e, &uw, 41, 40, n);
        }
        assert_eq!(cut(&e, &uw, &[40, 41], n), 0);
    }

    #[test]
    fn selling_confers_nothing_on_the_seller() {
        // Standing runs creditor -> debtor, so it is evidence of having owed
        // and paid. A member who only ever delivers accumulates none of it,
        // however much they deliver and to however many people who have it.
        let (mut e, uw) = founded(6);
        let n = 120;
        let buyers: Vec<usize> = (10..30).collect();
        for (i, &b) in buyers.iter().enumerate() {
            back(&mut e, &uw, i % 6, b, n);
        }
        let seller = 90;
        for _ in 0..100 {
            for &b in &buyers {
                back(&mut e, &uw, seller, b, n);
            }
        }
        assert_eq!(cap(&e, &uw, seller, n), 0, "2000 settled sales are worth nothing");
        // One honoured debt, and the picture changes at once.
        back(&mut e, &uw, buyers[0], seller, n);
        assert_eq!(cap(&e, &uw, seller, n), SUPPLY);
    }

    #[test]
    fn conferring_more_than_you_hold_is_a_no_op() {
        // A flow network holds source arcs and edge weights. An edge carries
        // only what reaches its tail, so raising what a member may confer above
        // their own inflow changes nothing at all.
        let (mut e, uw) = founded(1);
        let n = 60;
        back(&mut e, &uw, 0, 10, n);
        for granted in [SUPPLY, 50_000, 1_000_000] {
            stake(&mut e, 10, 20, u64::MAX, granted);
            assert_eq!(e[&(10, 20)], granted, "the edge really is that wide");
            assert_eq!(cap(&e, &uw, 20, n), SUPPLY, "and it carries only what reaches it");
        }
        // The same from an account with no inflow whatsoever.
        let mut e2 = Edges::new();
        for p in 30..50 {
            stake(&mut e2, 91, p, u64::MAX, 1_000_000);
        }
        assert_eq!(cut(&e2, &uw, &(30..50).collect::<Vec<_>>(), n), 0);
    }

    #[test]
    fn lending_does_not_raise_the_lenders_own_limit() {
        // Why backing is directed: capacity reflects who will carry YOU, and
        // carrying somebody else is not evidence of that.
        let (mut e, uw) = founded(6);
        let n = 60;
        back(&mut e, &uw, 0, 10, n);
        let before = cap(&e, &uw, 10, n);
        for d in 20..40 {
            back(&mut e, &uw, 10, d, n);
        }
        assert_eq!(cap(&e, &uw, 10, n), before, "a lender gains nothing by lending");
        assert_eq!(cap(&e, &uw, 20, n), before, "and confers it whole");
    }

    #[test]
    fn capacity_propagates_undiminished_but_use_is_shared() {
        let (mut e, uw) = founded(6);
        let n = 60;
        back(&mut e, &uw, 0, 10, n);
        for hop in 10..15 {
            back(&mut e, &uw, hop, hop + 1, n);
        }
        for hop in 10..16 {
            assert_eq!(cap(&e, &uw, hop, n), SUPPLY, "limits propagate along the chain");
        }
        let chain: Vec<usize> = (10..16).collect();
        assert_eq!(
            cut(&e, &uw, &chain, n),
            SUPPLY,
            "but the chain as a whole can only owe what one underwriter supplied"
        );
    }

    #[test]
    fn an_underwriters_total_exposure_is_their_supply() {
        // The property that makes the role safe to open to anyone: backing
        // twenty accounts does not multiply what the underwriter risks.
        let (mut e, uw) = founded(6);
        let n = 80;
        let backed: Vec<usize> = (10..31).collect();
        for &d in &backed {
            back(&mut e, &uw, 0, d, n);
        }
        for &d in &backed {
            assert_eq!(cap(&e, &uw, d, n), SUPPLY);
        }
        assert_eq!(cut(&e, &uw, &backed, n), SUPPLY);
    }

    #[test]
    fn one_supply_cannot_be_lent_twice() {
        // The smallest statement of what the supply arc is for. Both borrowers
        // are backed by the same underwriter and by nobody else, so their
        // reservations land on different edges and would never meet if the arc
        // they share were not charged.
        let uw = vec![(0usize, SUPPLY)];
        let mut e = Edges::new();
        stake(&mut e, 0, 1, u64::MAX, SUPPLY);
        stake(&mut e, 0, 2, u64::MAX, SUPPLY);
        let n = 8;
        assert_eq!(cap(&e, &uw, 1, n), SUPPLY);
        assert_eq!(cap(&e, &uw, 2, n), SUPPLY);
        assert_eq!(cut(&e, &uw, &[1, 2], n), SUPPLY, "but together, one supply");

        let (mut r, mut c) = (Reservations::new(), Committed::new());
        assert!(reserve(&e, &mut r, &mut c, &uw, 1, n, SUPPLY).is_some());
        assert!(
            reserve(&e, &mut r, &mut c, &uw, 2, n, SUPPLY).is_none(),
            "the second draw is on a supply that is already spent"
        );
        assert_eq!(committed_total(&c), SUPPLY);
        assert!(committed_total(&c) <= cut(&e, &uw, &[1, 2], n), "invariant 1");
    }

    #[test]
    fn an_underwriter_is_bounded_across_every_debtor_they_back() {
        // The same thing at scale, and asserted over the SET. A per-account
        // check passes in every scene this catches.
        for k in [2usize, 5, 20, 100] {
            let uw = vec![(0usize, SUPPLY)];
            let mut e = Edges::new();
            for d in 1..=k {
                stake(&mut e, 0, d, u64::MAX, SUPPLY);
            }
            let n = k + 2;
            let set: Vec<usize> = (1..=k).collect();
            let (mut r, mut c) = (Reservations::new(), Committed::new());
            let mut lent = 0;
            for d in 1..=k {
                if reserve(&e, &mut r, &mut c, &uw, d, n, SUPPLY).is_some() {
                    lent += SUPPLY;
                }
            }
            assert_eq!(lent, SUPPLY, "k={k}: one supply, lent once");
            assert!(lent <= cut(&e, &uw, &set, n), "k={k}: invariant 1");
        }
    }

    #[test]
    fn a_malicious_underwriter_loses_only_what_they_declared() {
        let (mut e, uw) = founded(6);
        let n = 80;
        let sybils: Vec<usize> = (50..70).collect();
        for _ in 0..50 {
            for &s in &sybils {
                back(&mut e, &uw, 0, s, n);
                back(&mut e, &uw, s, 0, n); // back-staking
            }
        }
        assert_eq!(cut(&e, &uw, &sybils, n), SUPPLY);
    }

    #[test]
    fn a_coalition_cannot_underwrite_itself() {
        // Any member may declare a supply. A coalition declaring its own
        // members underwriters gains nothing, because the measurement of the
        // coalition excludes underwriters inside it.
        let (mut e, mut uw) = founded(6);
        let n = 80;
        back(&mut e, &uw, 0, 50, n); // one colluder, honestly backed
        let external = cap(&e, &uw, 50, n);
        let coalition: Vec<usize> = std::iter::once(50).chain(60..75).collect();

        for _ in 0..8 {
            for &m in &coalition {
                let c = cap(&e, &uw, m, n);
                if c > 0 && !uw.iter().any(|&(x, _)| x == m) {
                    uw.push((m, c));
                }
            }
            for &m in &coalition {
                for &s in &coalition {
                    back(&mut e, &uw, m, s, n);
                }
            }
            let outside: Vec<(usize, u64)> = uw.iter().filter(|(x, _)| !coalition.contains(x)).copied().collect();
            assert_eq!(
                cut(&e, &outside, &coalition, n),
                external,
                "a coalition is bounded by what backs it from outside"
            );
        }
    }

    #[test]
    fn admitting_an_underwriter_raises_what_everyone_else_may_owe() {
        // The set is open, so a community keeps allocating credit as founders
        // age out. Note carefully what this does NOT say: the rise is real for
        // a set that EXCLUDES the new underwriters, who are genuinely outside
        // it and have genuinely accepted liability. It is not new independent
        // backing, and the closing assertion is the one that says so — measured
        // together with the people they back, the total is the founders' again.
        // A declared total is an upper bound, never a measure.
        let (mut e, mut uw) = founded(2);
        let n = 80;
        for d in 10..14 {
            back(&mut e, &uw, 0, d, n);
            back(&mut e, &uw, 1, d, n);
        }
        let newcomers: Vec<usize> = (20..40).collect();
        for &d in &newcomers {
            back(&mut e, &uw, 0, d, n);
            back(&mut e, &uw, 1, d, n);
        }
        assert_eq!(cut(&e, &uw, &newcomers, n), 2 * SUPPLY, "two founders, so two supplies");

        // Two members who were themselves backed now declare a supply and put
        // it behind the same newcomers.
        uw.push((10, SUPPLY));
        uw.push((11, SUPPLY));
        for &d in &newcomers {
            back(&mut e, &uw, 10, d, n);
            back(&mut e, &uw, 11, d, n);
        }
        assert_eq!(cut(&e, &uw, &newcomers, n), 4 * SUPPLY, "four supplies stand outside this set");

        // The same question asked of the set that CONTAINS the new
        // underwriters. They can only pay from standing the founders gave
        // them, so nothing was added to what the community can carry.
        let with_them: Vec<usize> = newcomers.iter().copied().chain([10, 11]).collect();
        assert_eq!(
            cut(&e, &uw, &with_them, n),
            2 * SUPPLY,
            "declaring against standing the community conferred adds nothing in aggregate"
        );
    }

    #[test]
    fn reservation_stops_independent_creditors_double_drawing() {
        let (mut e, uw) = founded(6);
        let (mut r, mut c) = (Reservations::new(), Committed::new());
        let (n, debtor) = (60, 10);
        back(&mut e, &uw, 0, debtor, n);
        let bound = capacity_of(&e, &r, &c, &uw, debtor, n);
        assert_eq!(bound, SUPPLY);
        let mut lent = 0;
        for _ in 0..50 {
            if reserve(&e, &mut r, &mut c, &uw, debtor, n, 100).is_some() {
                lent += 100;
            }
        }
        assert_eq!(lent, bound, "total lent equals the cut, not a multiple of it");
        assert_eq!(capacity_of(&e, &r, &c, &uw, debtor, n), 0);
    }

    #[test]
    fn releasing_gives_back_the_whole_path() {
        // A multi-hop path reserves the upstream edges too. Crediting only the
        // arcs incident to the debtor stranded them forever, so settling an
        // obligation destroyed capacity nothing was behind.
        let uw = vec![(0usize, SUPPLY)];
        let mut e = Edges::new();
        stake(&mut e, 0, 1, u64::MAX, SUPPLY);
        stake(&mut e, 1, 2, u64::MAX, SUPPLY);
        let n = 8;
        let (mut r, mut c) = (Reservations::new(), Committed::new());
        let held = reserve(&e, &mut r, &mut c, &uw, 2, n, SUPPLY).expect("the chain carries it");
        assert_eq!(r.len(), 2, "both hops are held");
        assert_eq!(held.amount(), SUPPLY);
        release(&mut r, &mut c, &held);
        assert_eq!(reserved_total(&r), 0, "and both hops come back");
        assert_eq!(committed_total(&c), 0);
        assert_eq!(cap(&e, &uw, 2, n), SUPPLY);
    }

    #[test]
    fn a_reserved_hold_decomposes_into_routes_and_back() {
        // A two-hop chain: the hold has to come apart into one route
        // 0 -> 1 -> 2 and go back together arc for arc.
        let uw = vec![(0usize, SUPPLY)];
        let mut e = Edges::new();
        stake(&mut e, 0, 1, u64::MAX, SUPPLY);
        stake(&mut e, 1, 2, u64::MAX, SUPPLY);
        let n = 8;
        let (mut r, mut c) = (Reservations::new(), Committed::new());
        let held = reserve(&e, &mut r, &mut c, &uw, 2, n, SUPPLY).expect("the chain carries it");

        let paths = decompose(&held, 2).expect("a reserved hold is a flow");
        assert_eq!(paths.len(), 1, "one source, one route");
        assert_eq!(paths[0].source, 0);
        assert_eq!(paths[0].arcs, vec![(0, 1), (1, 2)], "source side first");
        assert_eq!(paths[0].value, SUPPLY);
        assert_eq!(from_paths(&paths), held, "and the routes rebuild it exactly");
    }

    #[test]
    fn two_underwriters_decompose_into_one_route_each() {
        let uw = vec![(0usize, 100), (1usize, 100)];
        let mut e = Edges::new();
        stake(&mut e, 0, 5, u64::MAX, 100);
        stake(&mut e, 1, 5, u64::MAX, 100);
        let n = 8;
        let (mut r, mut c) = (Reservations::new(), Committed::new());
        let held = reserve(&e, &mut r, &mut c, &uw, 5, n, 200).expect("both arcs carry it");

        let paths = decompose(&held, 5).expect("a reserved hold is a flow");
        assert_eq!(paths.len(), 2);
        // Each route leaves ONE supply arc, which is what a proportional split
        // of the arcs cannot say: there, each piece holds a share of both.
        assert_eq!(paths.iter().map(|p| p.value).sum::<u64>(), 200);
        for pth in &paths {
            assert_eq!(pth.arcs.len(), 1);
            assert_eq!(pth.arcs[0].0, pth.source, "a route starts at its own underwriter");
        }
        assert_eq!(from_paths(&paths), held);
    }

    /// The probe that fails without the check. A hold carrying flow on an arc
    /// no source reaches is exactly what a proportional split produces once one
    /// piece has been cured, and it is what `decompose` has to refuse.
    #[test]
    fn a_hold_that_is_not_a_flow_is_refused() {
        // Arc 7 -> 8 carries 50 with nothing arriving at 7.
        let orphan = Held { edges: vec![((7, 8), 50)], supply: vec![] };
        assert!(decompose(&orphan, 8).is_err(), "arc flow with no source behind it");

        // Supply that never reaches the debtor: 0 sources 50, the only arc
        // leaves 0 for 1, and the debtor is 9.
        let stranded = Held { edges: vec![((0, 1), 50)], supply: vec![(0, 50)] };
        assert!(decompose(&stranded, 9).is_err(), "the route strands at 1");

        // Conserved in total but not at the node: 0 sources 50 and 60 leaves 1.
        let broken = Held { edges: vec![((0, 1), 50), ((1, 2), 60)], supply: vec![(0, 50)] };
        assert!(decompose(&broken, 2).is_err(), "10 leaves 1 that never arrived");
    }

    #[test]
    fn decomposition_is_a_function_of_the_hold() {
        let uw = vec![(0usize, 100), (1usize, 100)];
        let mut e = Edges::new();
        stake(&mut e, 0, 5, u64::MAX, 100);
        stake(&mut e, 1, 5, u64::MAX, 100);
        let n = 8;
        let (mut r, mut c) = (Reservations::new(), Committed::new());
        let held = reserve(&e, &mut r, &mut c, &uw, 5, n, 200).expect("both arcs carry it");
        // Same input, same routes — the walk reads ordered maps, never history.
        assert_eq!(decompose(&held, 5).unwrap(), decompose(&held, 5).unwrap());
    }

    #[test]
    fn releasing_restores_exactly_what_was_held() {
        let (mut e, uw) = founded(6);
        let (mut r, mut c) = (Reservations::new(), Committed::new());
        let (n, debtor) = (60, 10);
        back(&mut e, &uw, 0, debtor, n);
        let held = reserve(&e, &mut r, &mut c, &uw, debtor, n, 2000).expect("within the cut");
        assert_eq!(reserved_total(&r), 2000);
        assert_eq!(committed_total(&c), 2000);
        assert_eq!(capacity_of(&e, &r, &c, &uw, debtor, n), 500);
        release(&mut r, &mut c, &held);
        assert_eq!(reserved_total(&r), 0);
        assert_eq!(capacity_of(&e, &r, &c, &uw, debtor, n), SUPPLY);
    }

    #[test]
    fn reserving_more_than_the_cut_changes_nothing() {
        let (mut e, uw) = founded(6);
        let (mut r, mut c) = (Reservations::new(), Committed::new());
        let (n, debtor) = (60, 10);
        back(&mut e, &uw, 0, debtor, n);
        assert!(reserve(&e, &mut r, &mut c, &uw, debtor, n, SUPPLY + 1).is_none());
        assert_eq!(reserved_total(&r), 0, "a refused reservation must not half-apply");
        assert_eq!(committed_total(&c), 0);
    }

    #[test]
    fn a_supply_may_not_be_withdrawn_below_its_committed_flow() {
        // A withdrawal is decay applied to a source arc, and it takes the same
        // floor. Without it, leaving while credit stands on you breaks the cut
        // bound outright.
        let (mut e, uw) = founded(6);
        let (mut r, mut c) = (Reservations::new(), Committed::new());
        let n = 80;
        let borrowers: Vec<usize> = (10..30).collect();
        for (i, &b) in borrowers.iter().enumerate() {
            back(&mut e, &uw, i % 6, b, n);
        }
        for &b in &borrowers {
            let room = capacity_of(&e, &r, &c, &uw, b, n);
            if room > 0 {
                reserve(&e, &mut r, &mut c, &uw, b, n, room);
            }
        }
        let outstanding = committed_total(&c);
        assert_eq!(outstanding, 6 * SUPPLY, "the ceiling is drawn");
        assert_eq!(supply_floor(&c, 0), SUPPLY, "underwriter 0 may not go below this");

        // Withdrawing to the floor is safe; the bound still holds.
        let floored: Vec<(usize, u64)> = uw
            .iter()
            .map(|&(u, s)| (u, if u == 0 { supply_floor(&c, 0) } else { s }))
            .collect();
        assert!(outstanding <= cut(&e, &floored, &borrowers, n), "invariant 1 survives a legal exit");

        // Withdrawing below it does not.
        let illegal: Vec<(usize, u64)> = uw.iter().filter(|&&(u, _)| u != 0).copied().collect();
        assert!(outstanding > cut(&e, &illegal, &borrowers, n), "which is exactly why the floor is not optional");
    }

    #[test]
    fn decay_never_releases_live_collateral() {
        let (mut e, uw) = founded(6);
        let (mut r, mut c) = (Reservations::new(), Committed::new());
        let (n, debtor) = (60, 10);
        back(&mut e, &uw, 0, debtor, n);
        let held = reserve(&e, &mut r, &mut c, &uw, debtor, n, 2000).expect("within the cut");
        let mut free_before = capacity_of(&e, &r, &c, &uw, debtor, n);
        assert_eq!(free_before, 500);
        for _ in 0..24 {
            decay(&mut e, &r, 977, 1000);
            let free = capacity_of(&e, &r, &c, &uw, debtor, n);
            assert_eq!(reserved_total(&r), 2000, "an outstanding obligation keeps its collateral");
            assert!(free <= free_before, "free capacity may only shrink under decay");
            assert_eq!(e[&(0, debtor)], free + 2000, "the stake is its reservation plus what is free");
            free_before = free;
        }
        assert_eq!(e[&(0, debtor)], 2000);
        release(&mut r, &mut c, &held);
        assert_eq!(capacity_of(&e, &r, &c, &uw, debtor, n), 2000);
    }

    #[test]
    fn a_fully_reserved_stake_does_not_decay_at_all() {
        let (mut e, uw) = founded(6);
        let (mut r, mut c) = (Reservations::new(), Committed::new());
        let (n, debtor) = (60, 10);
        back(&mut e, &uw, 0, debtor, n);
        assert!(reserve(&e, &mut r, &mut c, &uw, debtor, n, SUPPLY).is_some());
        for _ in 0..50 {
            decay(&mut e, &r, 977, 1000);
        }
        assert_eq!(e[&(0, debtor)], SUPPLY);
        assert_eq!(capacity_of(&e, &r, &c, &uw, debtor, n), 0);
    }

    #[test]
    fn capacity_is_invariant_under_insertion_order() {
        let (mut e, uw) = founded(6);
        let n = 40;
        for (i, d) in (10..30usize).enumerate() {
            e.insert((i % 6, d), 40 + i as u64);
        }
        let first = cap(&e, &uw, 12, n);
        let mut reversed = Edges::new();
        for (&k, &v) in e.iter().rev() {
            reversed.insert(k, v);
        }
        assert_eq!(reversed, e);
        assert_eq!(cap(&reversed, &uw, 12, n), first);
    }

    /// **Dead accounts cost nothing and change nothing.**
    ///
    /// An account with no incident stake and no declared supply cannot lie on
    /// any augmenting path, so it belongs in no cut — the network is built over
    /// the accounts that take part rather than over every id the ledger has
    /// issued. This matters because account creation is unbillable by
    /// construction: sizing the graph by the id counter made every abandoned
    /// row cost a node in every query on every validator, for ever.
    ///
    /// The assertion is equality across four orders of magnitude of `n`. If
    /// the answer moved with the id space at all, the compaction would be
    /// changing the model rather than the cost of computing it.
    #[test]
    fn an_id_space_full_of_dead_accounts_changes_no_answer() {
        let (mut e, uw) = founded(6);
        back(&mut e, &uw, 0, 10, 20);
        back(&mut e, &uw, 10, 11, 20);
        let set = [10usize, 11];

        let baseline = cut(&e, &uw, &set, 20);
        assert_eq!(baseline, SUPPLY);
        for n in [20usize, 200, 20_000, 1_000_000] {
            assert_eq!(cap(&e, &uw, 10, n), SUPPLY, "n={n}: one account");
            assert_eq!(cut(&e, &uw, &set, n), baseline, "n={n}: and the set");
            // A reservation must walk the same arcs, whatever the id space.
            let (mut r, mut c) = (Reservations::new(), Committed::new());
            let held = reserve(&e, &mut r, &mut c, &uw, 11, n, SUPPLY).expect("the chain carries it");
            assert_eq!(held.edges.len(), 2, "n={n}: both hops, keyed by LEDGER id");
            assert_eq!(held.amount(), SUPPLY);
            release(&mut r, &mut c, &held);
            assert_eq!(reserved_total(&r), 0);
        }
    }

    /// And the cost of a query really does follow the participants rather than
    /// the id counter. Timing is not asserted — a wall-clock bound in a unit
    /// test is a flake waiting to happen — but a graph that scaled with `n`
    /// would not return at all at this id space, which is the point.
    #[test]
    fn a_vast_id_space_is_not_a_vast_graph() {
        let (mut e, uw) = founded(6);
        for d in 10..30 {
            back(&mut e, &uw, d % 6, d, 30);
        }
        let n = 50_000_000; // far past anything that could be allocated per node
        assert_eq!(cap(&e, &uw, 10, n), SUPPLY);
        assert_eq!(cut(&e, &uw, &(10..30).collect::<Vec<_>>(), n), 6 * SUPPLY);
    }

    /// The cursor reads what a lookup reads, across gaps on both sides: edges
    /// with no reservation, and reservations on keys that are not edges.
    /// Mutation that bites: `at` compares the peeked key for equality without
    /// advancing past the lesser ones, and a stray reservation just below an
    /// edge hides the edge's own.
    #[test]
    fn a_reservation_cursor_reads_what_a_lookup_reads() {
        let mut edges = Edges::new();
        let mut reserved = Reservations::new();
        for c in 0..40usize {
            for d in 0..40usize {
                if (c * 7 + d * 3) % 5 == 0 && c != d {
                    edges.insert((c, d), 100 + (c * d) as u64);
                }
                if (c * 11 + d) % 4 == 0 {
                    reserved.insert((c, d), (c + d) as u64 + 1);
                }
            }
        }
        assert!(reserved.keys().any(|k| !edges.contains_key(k)), "reservations off any edge exist");
        assert!(edges.keys().any(|k| !reserved.contains_key(k)), "unreserved edges exist");
        assert!(edges.keys().any(|k| reserved.contains_key(k)), "reserved edges exist");
        let mut cursor = ReservedCursor::new(&reserved);
        for &k in edges.keys() {
            assert_eq!(cursor.at(k), reserved.get(&k).copied().unwrap_or(0), "at {k:?}");
        }
    }

    #[test]
    fn early_exit_agrees_with_the_full_answer() {
        let (mut e, uw) = founded(6);
        let (r, c) = (Reservations::new(), Committed::new());
        let (n, d) = (60, 10);
        back(&mut e, &uw, 0, d, n);
        let full = cap(&e, &uw, d, n);
        for want in [1u64, 999, SUPPLY] {
            assert_eq!(capacity(&e, &r, &c, &uw, &[d], n, want), want.min(full));
        }
        assert_eq!(capacity(&e, &r, &c, &uw, &[d], n, 10 * SUPPLY), full);
    }

    #[test]
    fn a_members_own_supply_is_outside_their_credit_and_inside_their_write_reach() {
        let (e, uw) = founded(2);
        let (r, mut c) = (Reservations::new(), Committed::new());
        let n = 20;
        assert_eq!(capacity(&e, &r, &c, &uw, &[0], n, u64::MAX), 0, "nobody underwrites their own borrowing");
        assert_eq!(capacity_with_own_supply(&e, &r, &c, &uw, &[0], n, u64::MAX), SUPPLY);

        let mut res = Reservations::new();
        let held = reserve_with_own_supply(&e, &mut res, &mut c, &uw, 0, n, 2_000).expect("its own supply carries it");
        assert_eq!(held.supply, vec![(0, 2_000)]);
        assert!(held.edges.is_empty(), "no stake arc is on the path");
        assert_eq!(c.get(&0).copied(), Some(2_000));
        assert_eq!(c.get(&1).copied(), None, "a second underwriter's arc is untouched");
        assert_eq!(capacity_with_own_supply(&e, &res, &c, &uw, &[0], n, u64::MAX), SUPPLY - 2_000);
        assert_eq!(capacity_with_own_supply(&e, &res, &c, &uw, &[1], n, u64::MAX), SUPPLY);
    }
}
