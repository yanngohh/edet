//! Whole-state audit: the paper's invariants (the paper's §Verification) as a
//! check that runs after every transition.
//!
//! Run on the COMMIT path (`Replica::commit_block_unchecked`), not only in
//! tests. An audit with no caller on that path lets a break in the transition
//! function be committed, rooted and certified in silence.

use crate::state::State;
use crate::types::{ContractStatus, MemberId, MemberStatus};

#[derive(Debug)]
pub struct Violation(pub String);

/// What the last audit verified, and the cut inputs it verified them at.
///
/// **This never changes a verdict.** It decides only whether a dominated
/// computation is performed, which is what makes it safe to be
/// history-dependent: determinism is a claim about the DECISION and not about
/// the work, so a node resuming from a snapshot starts cold, does more of it,
/// and halts on exactly the states a warm node halts on. `audit` itself stays
/// the definition and takes no cache; `audit_with_cache` must agree with it on
/// every state, which is a probe rather than an argument
/// (`tests/model.rs::the_cache_never_changes_a_verdict`).
///
/// The claim it rests on is that a cut is MONOTONE in the three things it
/// reads. `gross_capacity_of_set` passes pristine reservations and pristine
/// committed, so a cut is a pure function of `(edges, supplies, id space)` —
/// it reads nothing else, and raising an edge, raising a supply or admitting
/// more id space can only raise a maximum flow. So a verified cut stays a
/// valid LOWER BOUND for as long as nothing decreased, and `drawn(S) <=
/// cut_cached(S)` proves invariant 1 with no query at all. Only a failed bound
/// costs a real one.
///
/// **What may decrease is a short and structural list**, not an empirical
/// guess: `flow::stake` is a peak rule and only ever raises, so ordinary
/// settlement traffic cannot lower a cut. The whole of the other direction is
/// `flow::decay` (the epoch boundary, which already walks every edge),
/// `redenominate` (governed, behind a cooldown) and a `DeclareSupply` that
/// reduces. Everything else in the alphabet leaves all three inputs alone.
///
/// **The witness is the data and never a flag a transition sets.** A defective
/// transition that lowered an edge is precisely what this audit exists to
/// catch, so it must not be able to announce that it lowered nothing: the
/// check below is an element-wise comparison against a stored snapshot. A
/// digest would answer a different question — equality, not `>=` — and would
/// throw the cache away on the settlements that raise stakes, which is most of
/// what a live community does.
#[derive(Default, Clone)]
pub struct AuditCache {
    /// The cut last verified for each set, keyed by the set's own contents.
    cuts: std::collections::BTreeMap<Vec<MemberId>, u64>,
    /// The cut inputs as of the previous audit.
    edges: edet_kernel::flow::Edges,
    supplies: std::collections::BTreeMap<MemberId, u64>,
    flow_n: usize,
    /// Invariant 6 reads live reservations, so its skip needs them too.
    reserved: edet_kernel::flow::Reservations,
    committed: edet_kernel::flow::Committed,
    warm: bool,
    /// Full cuts actually computed, cumulative. The point of the cache is a
    /// cost, and a cost claim needs a measurement that a probe can read — this
    /// is what makes "a block that changed nothing costs nothing" a gate
    /// rather than a sentence (`tests/model.rs`).
    queries: u64,
}

impl AuditCache {
    /// Full max-flow queries this cache has computed since it was created.
    pub fn queries(&self) -> u64 {
        self.queries
    }

    /// Whether every cut input is at least what it was at the previous audit.
    ///
    /// The comparison is against the PREVIOUS audit rather than against the
    /// last full recompute, and that is what makes the cached values sound by
    /// transitivity: every entry was computed at some audit, and every step
    /// since has been checked non-decreasing, so the current cut is at least
    /// every cached one. The snapshot therefore advances on every call — see
    /// `refresh`.
    fn nothing_decreased(&self, state: &State) -> bool {
        if !self.warm || state.flow_n() < self.flow_n {
            return false;
        }
        for (key, &was) in &self.edges {
            if state.edges.get(key).copied().unwrap_or(0) < was {
                return false;
            }
        }
        for (id, &was) in &self.supplies {
            if state.underwriters.get(id).copied().unwrap_or(0) < was {
                return false;
            }
        }
        true
    }

    /// Whether the graph invariant 6 re-queries is bit-for-bit what it was.
    ///
    /// Invariant 6 asks whether capacity is a function of the ledger rather
    /// than of insertion order, by running one account's query over the edge
    /// map forwards and reversed. It reads live reservations and committed
    /// flow as well as the edges, so it is skippable only when all of them are
    /// unchanged — which is exactly the block the cost complaint is
    /// about, the one that "contained nothing".
    fn graph_unchanged(&self, state: &State) -> bool {
        self.warm
            && self.flow_n == state.flow_n()
            && self.edges == *state.edges
            && self.reserved == *state.reserved
            && self.committed == state.committed
            && self.supplies == state.underwriters
    }

    fn refresh(&mut self, state: &State) {
        self.edges = (*state.edges).clone();
        self.supplies = state.underwriters.clone();
        self.reserved = (*state.reserved).clone();
        self.committed = state.committed.clone();
        self.flow_n = state.flow_n();
        self.warm = true;
    }
}

/// Check the auditable invariants of the state.
///
/// The cold form, and the definition: it consults nothing and computes every
/// cut. `audit_with_cache` is the same audit with dominated work skipped.
pub fn audit(state: &State) -> Result<(), Violation> {
    audit_inner(state, None)
}

/// The audit, skipping cuts a monotone argument already proves.
///
/// Identical in verdict to [`audit`] on every state; see [`AuditCache`].
pub fn audit_with_cache(state: &State, cache: &mut AuditCache) -> Result<(), Violation> {
    audit_inner(state, Some(cache))
}

fn audit_inner(state: &State, mut cache: Option<&mut AuditCache>) -> Result<(), Violation> {
    // Conservation: cached per-member debt equals the contract book, and the
    // debtor-side and creditor-side sums agree.
    // **Summed in `u128`, because the sum of a set of amounts is not an
    // amount.** Every row is bounded by `MAX_AMOUNT_MINOR` at the ingress that
    // wrote it, and nothing bounds how MANY of them a book carries — 16,384
    // ceiling-sized claims across two debtors is a legal book that overflows a
    // `u64` total. In a build with overflow checks that add is a panic inside
    // `commit_block_unchecked`, on every honest node, at the same height; in
    // one without, both sides of the comparison below wrap together and the
    // audit reports agreement it did not check. Widened, an overflow anywhere
    // under this can only ever be a `Violation` — the fail-stop the engine
    // already ends its loop on, with the reason named.
    let mut by_debtor: std::collections::BTreeMap<u64, u128> = Default::default();
    let mut total_outstanding: u128 = 0;
    for c in state.contracts.values() {
        // `accept` refuses a self-dealing contract, so one existing means some
        // MINTING path built what the entry path forbids. It is not free
        // money, but it puts a diagonal entry in the contagion operator and
        // tightens the community brake for everyone.
        if c.debtor == c.creditor {
            return Err(Violation(format!("contract {} has member {} on both sides", c.id, c.debtor)));
        }
        match c.status {
            ContractStatus::Active | ContractStatus::Expired => {
                *by_debtor.entry(c.debtor).or_insert(0) += c.outstanding as u128;
                total_outstanding += c.outstanding as u128;
            }
            _ => {
                if c.outstanding != 0 {
                    return Err(Violation(format!("closed contract {} carries outstanding {}", c.id, c.outstanding)));
                }
            }
        }
    }
    // Amounts are minor units, so this is EXACT equality and not a tolerance.
    // A tolerance here is a claim that the two sides may legitimately differ
    // by a little, and they may not: `debt_out` is a cache of the book and the
    // book is the definition. With `f64` on both sides and a `1e-6` slack, the
    // audit's verdict would be a function of how many payments an account had
    // taken — a busy account accumulates representation error until it crosses
    // the tolerance and every honest node halts on a ledger nothing is wrong
    // with (2,000 claims of a million and 200,000 partial discharges drift
    // 4e-5 from the recount, forty times the slack). Integers have no such
    // drift and need no such slack.
    let mut total_cached: u128 = 0;
    for (id, m) in &state.members {
        let book = by_debtor.get(id).copied().unwrap_or(0);
        if m.debt_out as u128 != book {
            return Err(Violation(format!("member {id}: cached debt {} vs book {}", m.debt_out, book)));
        }
        total_cached += m.debt_out as u128;
    }
    if total_cached != total_outstanding {
        return Err(Violation(format!("conservation: cached {total_cached} vs book {total_outstanding}")));
    }
    // Operation bonds: encumbrance is a reservation, never a balance, so it
    // is deliberately ABSENT from the conservation sum above — a bond that
    // showed up in `total_cached` would be indistinguishable from debt and
    // would mean the ledger had quietly charged a fee. What must hold is that
    // every forfeited amount still belongs to a real member — it is a future
    // obligation of that member, and an entry naming nobody could never be
    // minted against anyone. Non-negativity is not checked here and needs no
    // check: every stored amount is a `u64`, so the type carries the floor
    // that a comparison used to.
    // `bond_free_used` is deliberately NOT audited against the allowance.
    // The counter records what was spent under the allowance in force at the
    // time, and the allowance can legitimately fall below it — a governance
    // amendment mid-epoch, or a charter change — without anything being
    // wrong. It is compared at the gate and reset at every boundary; there is
    // no invariant relating the two in stored state.
    for id in state.forfeit_reserve.keys() {
        if !state.members.contains_key(id) {
            return Err(Violation(format!("forfeit reserve for unknown member {id}")));
        }
    }

    // Cascade listing edges are symmetric in both directions.
    for (id, m) in &state.members {
        for b in m.beneficiaries.keys() {
            // The self entry is a share, not a support relationship: it has
            // no reverse edge by design.
            if b == id {
                continue;
            }
            let ok = state.members.get(b).map(|bm| bm.supporters_of.contains(id)).unwrap_or(false);
            if !ok {
                return Err(Violation(format!("listing edge {id}->{b} missing reverse")));
            }
        }
        for s in &m.supporters_of {
            let ok = state
                .members
                .get(s)
                .map(|sm| sm.beneficiaries.contains_key(id))
                .unwrap_or(false);
            if !ok {
                return Err(Violation(format!("reverse edge {s}->{id} missing listing")));
            }
        }
    }
    // Every declared supply belongs to a member the ledger knows.
    //
    // There is no second map recording which part of a supply is external, and
    // so nothing to hold against this one: a declaration cannot be raised at
    // all, so every unit of every supply was seated by a ceremony and
    // `underwriters` IS the external record. A fact stored twice is a fact that
    // can drift.
    for id in state.underwriters.keys() {
        if !state.members.contains_key(id) {
            return Err(Violation(format!("underwriter {id} is not a member")));
        }
    }
    // Validators are active members holding a consensus key the set can
    // resolve. The key half fails CLOSED at `EdetValidatorSet::build`, which
    // is a halt rather than a smaller quorum — so the ledger must never reach
    // a state where a validator has none.
    for v in state.validators.keys() {
        let Some(m) = state.members.get(v) else {
            return Err(Violation(format!("validator {v} not an active member")));
        };
        if !matches!(m.status, crate::types::MemberStatus::Active) {
            return Err(Violation(format!("validator {v} not an active member")));
        }
        if m.consensus_key.is_none() {
            return Err(Violation(format!("validator {v} has no consensus key")));
        }
    }
    // Arbitration attestations only from the pinned panel.
    for c in state.contracts.values() {
        match &c.arb {
            Some(t) => {
                for a in c.arb_attestations.keys() {
                    if !t.arbiters.contains(a) {
                        return Err(Violation(format!("contract {}: non-panel attestation", c.id)));
                    }
                }
            }
            None => {
                if !c.arb_attestations.is_empty() {
                    return Err(Violation(format!("contract {}: attestations without terms", c.id)));
                }
            }
        }
    }
    capacity_invariants(state, cache.as_deref_mut())?;
    // Key index round-trips, in BOTH directions.
    //
    // The forward direction alone (index -> member holds it) was blind to the
    // failure that matters: `rotate_finalize` overwriting an index entry left
    // one key in two members' `keys` while the index named only one of them,
    // and the other member became unresolvable by `member_of_key` for the rest
    // of its life — invisible to `whois`, to viewer auth and to the pending
    // pool, while `require_signed` (which reads `member.keys`, not the index)
    // still accepted its key. One key, one member, checked from both ends.
    for (key, id) in &state.key_index {
        let ok = state.members.get(id).map(|m| m.has_key(key)).unwrap_or(false);
        if !ok {
            return Err(Violation(format!("key index stale for member {id}")));
        }
    }
    for (id, m) in &state.members {
        if m.status == MemberStatus::Exited {
            continue;
        }
        for key in &m.keys {
            match state.key_index.get(key) {
                Some(owner) if owner == id => {}
                Some(owner) => return Err(Violation(format!("key held by member {id} is indexed to member {owner}"))),
                None => return Err(Violation(format!("member {id} holds a key absent from the index"))),
            }
        }
    }
    // A consensus key is never a member key, in either direction. One key as
    // both is a validator host holding an economic signature, which is the
    // separation the field exists to keep; `set_consensus_key` refuses a
    // member key and `key_is_claimed` refuses a consensus key, and this is
    // what says the two refusals were enough.
    for (id, m) in &state.members {
        if let Some(ck) = m.consensus_key {
            if let Some(owner) = state.key_index.get(&ck) {
                return Err(Violation(format!("validator {id}'s consensus key is member {owner}'s member key")));
            }
        }
    }
    // The snapshot advances on every clean audit, not only when something was
    // recomputed. That is what makes the cached cuts sound by transitivity:
    // each was verified at some audit, and every step from there to here has
    // been checked non-decreasing, so the present cut is at least each of
    // them. Refreshing only on recompute would leave later entries resting on
    // an older snapshot that does not dominate the state they were measured at.
    if let Some(c) = cache {
        c.refresh(state);
    }
    Ok(())
}

/// The seven invariants of the paper's §Verification — the capacity model's own audit.
///
/// **Invariant 1 is evaluated over SETS, not singletons**, and that is not a
/// refinement. Every defect this model has carried was a bound asserted over a
/// set and tested over a singleton: one supply drawn by several debtors, a
/// reservation held along several hops, a supply withdrawn while several
/// obligations stood on it. A per-account check passes in every one of them —
/// one underwriter of 2500 backing a hundred debtors lent 250,000 against a
/// true cut of 2500, and each debtor individually looked perfectly legal.
/// Test at the quantifier the theorem uses.
/// Is every outstanding hold a flow in its own right?
///
/// **This is what lets invariant 1 skip the family.** The clause asks
/// `drawn_into(S) <= GrossCapa(S)`, and `GrossCapa` is a MAXIMUM flow, so a
/// feasible flow of that value settles it without computing anything. Every
/// obligation already stores one. Feasibility needs arc usage inside the stake
/// (invariant 4), supply usage inside the declaration (invariant 5), and
/// conservation, which is this.
///
/// **Per hold, not in aggregate, and the difference is load-bearing.** A
/// balanced aggregate does not attribute routes to obligations: two holds could
/// each mis-state which underwriter reaches them and cancel each other out,
/// and `drawn_into` reads the per-obligation claim. Checked per hold, each
/// one's supply entries really are the sources of its own routes, so the
/// sub-flow sourced outside any `S` has exactly the value `drawn_into(S)`
/// reports.
///
/// Cheap: one pass over the arcs each obligation holds, which is the same data
/// invariant 2 has already walked. No query, and nothing that grows with `U`.
fn holds_are_flows(live: &[(MemberId, &edet_kernel::flow::Held)]) -> bool {
    use std::collections::BTreeMap;
    for (debtor, h) in live {
        // Inflow positive, outflow negative; the debtor absorbs the whole
        // amount. A node left non-zero is flow that arrived and never left, or
        // left without arriving.
        let mut bal: BTreeMap<usize, i128> = BTreeMap::new();
        for &(u, a) in &h.supply {
            *bal.entry(u).or_insert(0) += a as i128;
        }
        for &((c, d), a) in &h.edges {
            *bal.entry(c).or_insert(0) -= a as i128;
            *bal.entry(d).or_insert(0) += a as i128;
        }
        *bal.entry(*debtor as usize).or_insert(0) -= h.amount() as i128;
        if bal.values().any(|&v| v != 0) {
            return false;
        }
    }
    true
}

fn capacity_invariants(state: &State, mut cache: Option<&mut AuditCache>) -> Result<(), Violation> {
    use edet_kernel::flow;
    use std::collections::{BTreeMap, BTreeSet};

    // Every live insured obligation, and exactly what it holds. Expired ones
    // count: a default does not release the flow it committed, which is the
    // whole of why stealing through one costs 1.00x and cannot be repeated.
    let live: Vec<(MemberId, &flow::Held)> = state
        .contracts
        .values()
        .filter(|c| c.insured && matches!(c.status, ContractStatus::Active | ContractStatus::Expired))
        .map(|c| (c.debtor, &c.held))
        .collect();

    // --- 2. Reservation conservation -------------------------------------
    // The two views of one fact: `reserved` and `committed` are a cache of
    // every outstanding `Held`, and nothing else may write them. Exact
    // equality, in integer minor units — not "close", because the moment they
    // are allowed to drift the capacity query stops describing the ledger.
    let mut want_reserved: flow::Reservations = Default::default();
    let mut want_committed: flow::Committed = Default::default();
    for (_, held) in &live {
        for &(key, amount) in &held.edges {
            *want_reserved.entry(key).or_insert(0) += amount;
        }
        for &(u, amount) in &held.supply {
            *want_committed.entry(u).or_insert(0) += amount;
        }
    }
    want_reserved.retain(|_, v| *v > 0);
    want_committed.retain(|_, v| *v > 0);
    if want_reserved != *state.reserved {
        return Err(Violation(format!(
            "invariant 2: reserved is {} arcs against {} the obligations hold",
            state.reserved.len(),
            want_reserved.len()
        )));
    }
    if want_committed != state.committed {
        return Err(Violation(format!(
            "invariant 2: committed {} against {} the obligations hold",
            flow::committed_total(&state.committed),
            flow::committed_total(&want_committed)
        )));
    }
    // Every insured unit crosses exactly one supply arc, so the supply side IS
    // the community's outstanding insured credit.
    let booked: u64 = state
        .contracts
        .values()
        .filter(|c| c.insured && matches!(c.status, ContractStatus::Active | ContractStatus::Expired))
        .map(|c| c.outstanding)
        .sum();
    let committed_total = flow::committed_total(&state.committed);
    if committed_total != booked {
        return Err(Violation(format!(
            "invariant 2: {committed_total} committed against {booked} outstanding insured"
        )));
    }

    // --- 3. Stake bound ---------------------------------------------------
    // The height-pinned form — no edge above the staking creditor's capacity
    // AT THE ACCEPTING HEIGHT — is enforced where it is written (`flow::stake`
    // caps by `conferrable`) and cannot be re-derived here: a creditor's own
    // backing legitimately moves after they place a stake, so an edge sitting
    // above their present capacity is ordinary rather than wrong. What stored
    // state can be held to is that every edge names real accounts and carries
    // something, which is what the flow builder assumes.
    for (&(c, d), &w) in &state.edges {
        if w == 0 {
            return Err(Violation(format!("invariant 3: empty stake edge {c}->{d}")));
        }
        if c == d {
            return Err(Violation(format!("invariant 3: self-stake at {c}")));
        }
        for x in [c, d] {
            if !state.members.contains_key(&(x as MemberId)) {
                return Err(Violation(format!("invariant 3: stake edge {c}->{d} names unknown account {x}")));
            }
        }
    }

    // --- 4. Decay floor ---------------------------------------------------
    // No edge below its live reservation. The debt did not shrink because the
    // evidence aged, so the collateral behind it may not either — without this
    // decay silently releases collateral from under outstanding obligations,
    // and free capacity reappears that nothing is behind.
    for (key, &r) in &state.reserved {
        let stake = state.edges.get(key).copied().unwrap_or(0);
        if r > stake {
            return Err(Violation(format!(
                "invariant 4: {}->{} reserves {r} against a stake of {stake}",
                key.0, key.1
            )));
        }
    }

    // --- 5. Supply floor --------------------------------------------------
    // No underwriter's supply below the flow committed through it (§Stability). A
    // withdrawal is decay applied to a source arc and takes the same floor.
    for (&u, &drawn) in &state.committed {
        let supply = state.underwriters.get(&(u as MemberId)).copied().unwrap_or(0);
        if drawn > supply {
            return Err(Violation(format!(
                "invariant 5: {drawn} committed through underwriter {u} against a supply of {supply}"
            )));
        }
    }

    // --- 7. Seat conservation ---------------------------------------------
    // The seat maps are a cache of what the live seats hold, exactly as
    // `reserved` and `committed` are a cache of what the obligations hold, and
    // nothing else may write them. Every seat is a flow from the seed to its
    // sponsor, and a sponsor is a member — which the sweep keeps true by never
    // retiring a row that sponsors a live seat (`apply::retire_empty_rows`).
    //
    // **Three things are deliberately NOT claimed, and each is a load-bearing
    // absence.** `seat_reserved(e) <= edges(e)` is FALSE by design: `decay`
    // floors an edge at its CREDIT reservation and at nothing else, so the
    // stake under a seat fades while the seat stays spent — and flooring it
    // here would hand a sponsor 500.00 of permanent capacity for 25 seats,
    // which is the exploit rather than the fix. `seat_committed(u) <=
    // supply(u)` is absent for the same reason pointed at the source arc: an
    // underwriter must never be priced out of lowering what they stand behind,
    // and a seat is a spent budget rather than a liability, so a lowered
    // declaration leaves the rows it already seated exactly where they are.
    //
    // And a seat's SIZE is not compared to anything here, in either direction.
    // Both bounds are properties of the write: `reserve_seat` takes exactly one
    // bond unit at the unit in force, and no transition gives one back. Two
    // lawful acts then move a stored seat away from the live unit — a
    // re-denomination floors every route, so a seat can hold a minor unit less
    // than it took, and `BondFraction` moves the unit itself, so seats taken
    // under different units coexist for the life of their rows. A clause
    // comparing a stored seat against the live price halted every node on the
    // first downward amendment of the one dial the design offers for the seat
    // count. `tests/seats.rs` measures the count the rule allows and holds the
    // audit clean across both acts.
    let seats: Vec<(MemberId, &flow::Held)> = state
        .members
        .values()
        .filter_map(|m| m.seat.as_ref().map(|s| (s.sponsor, &s.held)))
        .collect();
    for (sponsor, _) in &seats {
        if !state.members.contains_key(sponsor) {
            return Err(Violation(format!("invariant 7: a seat names sponsor {sponsor}, who is not a member")));
        }
    }
    if !holds_are_flows(&seats) {
        return Err(Violation("invariant 7: a seat's hold is not a flow to its sponsor".into()));
    }
    let mut want_seat_reserved: flow::Reservations = Default::default();
    let mut want_seat_committed: flow::Committed = Default::default();
    for (_, held) in &seats {
        for &(key, amount) in &held.edges {
            *want_seat_reserved.entry(key).or_insert(0) += amount;
        }
        for &(u, amount) in &held.supply {
            *want_seat_committed.entry(u).or_insert(0) += amount;
        }
    }
    want_seat_reserved.retain(|_, v| *v > 0);
    want_seat_committed.retain(|_, v| *v > 0);
    if want_seat_reserved != *state.seat_reserved {
        return Err(Violation(format!(
            "invariant 7: seat_reserved is {} arcs against {} the seats hold",
            state.seat_reserved.len(),
            want_seat_reserved.len()
        )));
    }
    if want_seat_committed != state.seat_committed {
        return Err(Violation(format!(
            "invariant 7: seat_committed {} against {} the seats hold",
            flow::committed_total(&state.seat_committed),
            flow::committed_total(&want_seat_committed)
        )));
    }

    // --- 1. Cut bound, over sets ------------------------------------------
    // For every set checked: the flow committed INTO that set, drawn from
    // underwriters outside it, never exceeds that set's capacity measured on a
    // pristine residual.
    //
    // Which sets. Checking every subset is exponential, so the audit checks
    // the ones the theorem is actually about: all insured debtors together,
    // and — for each underwriter — everyone drawing through them. That second
    // family is precisely the scene the singleton check was blind to, and it
    // costs one flow query per underwriter.
    //
    // **Those sets are small and that is not why this is affordable.** "A set
    // small by design" reads as a statement about the price and is a statement
    // about the sets: a cut is a max-flow over the WHOLE edge map into the set,
    // so the query is a function of `E` and not of `|S|`. Measured at 20,000 accounts (`just cost`), one
    // cut costs 22.8 / 33.2 / 26.5 / 26.3 / 21.2 ms for sets of 1 / 10 / 100 /
    // 1,000 / 10,000 — flat, and cheaper for the larger ones, because more
    // sinks make the level graph shallower. What this family costs is `U`
    // queries of `O(E)` each, per committed block, on every validator.
    //
    // The book is indexed ONCE rather than re-scanned per set. `drawn` for a
    // set is a sum over the obligations of the accounts inside it, and the
    // obvious shape re-walks every live obligation and every one of its
    // supply arcs for each of the `U + 1` sets — `O(U x live)` passes for an
    // answer that touches only the set's own rows. That is invisible next to
    // `U` max-flows and it is the part that stays hot if those are ever made
    // cheaper. The remaining bound is honest rather than free:
    // where every debtor draws through every underwriter the two shapes meet,
    // because then each set really is the whole book.
    let mut by_debtor: BTreeMap<MemberId, Vec<&flow::Held>> = BTreeMap::new();
    let mut by_underwriter: BTreeMap<usize, BTreeSet<MemberId>> = BTreeMap::new();
    for (debtor, held) in &live {
        by_debtor.entry(*debtor).or_default().push(held);
        for &(u, _) in &held.supply {
            by_underwriter.entry(u).or_default().insert(*debtor);
        }
    }

    // What a set owes that the community underwrites — counting only the
    // supply reaching it from OUTSIDE, since a cut over the set does not
    // measure underwriters within it. Anything else would compare two
    // different quantities and pass by accident.
    let drawn_into = |inside: &BTreeSet<MemberId>| -> u64 {
        inside
            .iter()
            .filter_map(|d| by_debtor.get(d))
            .flat_map(|held| held.iter())
            .flat_map(|held| held.supply.iter())
            .filter(|(u, _)| !inside.contains(&(*u as MemberId)))
            .map(|&(_, a)| a)
            .sum()
    };

    // Decided before invariant 1 borrows the cache, and read against the
    // PREVIOUS audit's snapshot — `refresh` runs after this whole function.
    let graph_unchanged = cache.as_deref().map(|c| c.graph_unchanged(state)).unwrap_or(false);

    // Whether the witness is available at all. It is deliberately NOT offered
    // to the cold form: `audit` is the definition and goes on computing every
    // cut, while this is `audit_with_cache` declining work a stored proof
    // already settles — the same bargain the memo makes, gated the same way,
    // with the harness driving both after every transition and holding them to
    // one verdict.
    let witness_available = cache.is_some();

    let mut sets: Vec<BTreeSet<MemberId>> = Vec::new();
    let all: BTreeSet<MemberId> = live.iter().map(|(d, _)| *d).collect();
    if !all.is_empty() {
        sets.push(all);
    }
    sets.extend(by_underwriter.into_values());

    // A cached cut is usable only while nothing this query reads has fallen
    // since the audit that computed it (see `AuditCache`). Anything else and
    // the cached values go: they are lower bounds on a graph that no longer
    // dominates the one they were measured on.
    // Cuts actually computed in this call, banked into the cache at the end —
    // a plain local rather than a second borrow of it, since `memo` holds one.
    let mut computed: u64 = 0;
    let mut memo: Option<&mut std::collections::BTreeMap<Vec<MemberId>, u64>> = match cache {
        Some(ref mut c) => {
            if !c.nothing_decreased(state) {
                c.cuts.clear();
            }
            Some(&mut c.cuts)
        }
        None => None,
    };

    // Every set the family holds THIS time, so the ones it no longer holds can
    // be dropped below. Without that the map keeps one entry per distinct set
    // ever seen, and the family's membership turns over constantly — the
    // all-insured-debtors set changes the moment any new debtor is insured —
    // so a validator running for a year would carry a year of dead sets.
    let mut seen: BTreeSet<Vec<MemberId>> = BTreeSet::new();
    let mut violation: Option<Violation> = None;
    // Computed at most once per audit, and only if some set actually reaches
    // the query below.
    let mut witness: Option<bool> = None;

    for inside in sets {
        let drawn = drawn_into(&inside);
        // Ascending ledger order, because a BTreeSet iterates that way: the
        // answer must be a function of WHICH accounts are in the set and never
        // of the order anything was inserted (invariant 6).
        let set: Vec<MemberId> = inside.iter().copied().collect();
        seen.insert(set.clone());
        // The cheap proof first, and it is a proof rather than a shortcut: the
        // cached value is a cut this audit already verified on a graph the one
        // below it dominates, so `drawn <= cached <= cut` closes the clause.
        // A set whose CONTENTS changed is a different set and is not looked up
        // — Cap is not monotone in the set, since an underwriter falling
        // inside it loses its source arc to that very measurement.
        if let Some(m) = memo.as_ref() {
            if let Some(&floor) = m.get(&set) {
                if drawn <= floor {
                    continue;
                }
            }
        }
        // The memo missed, so this set is about to cost a real query. Ask the
        // stored witness first — but only HERE, and at most once per audit.
        //
        // **Order is the whole of it.** Evaluated eagerly, the witness runs on
        // every audit including the ones the memo answers for nothing, and an
        // ordinary block pays for a pass it did not need: measured at 20,000
        // accounts, 27 ms became 67 ms. Evaluated here it is free on exactly
        // those blocks, because a memo hit never reaches this line.
        //
        // Where it pays is the epoch boundary, where the memo is worth nothing:
        // decay lowers every edge free to fall, `nothing_decreased` throws the
        // cached cuts away, and the block pays a full cold audit — 201 cuts
        // after decay against the 201 a cold audit computes at `U = 200`, which
        // is the cut-bound family (`1 + U`) and not invariant 6's determinism
        // pair, the two queries neither the memo nor the witness touches.
        // **Decay cannot touch the witness**, because invariant 4 floors every
        // edge at its live reservation, so the arcs the holds occupy are still
        // there whatever the epoch did.
        if witness_available && *witness.get_or_insert_with(|| holds_are_flows(&live)) {
            // Bank what the witness proved. It settles `cut >= drawn`
            // without producing `cut`, so `drawn` is the strongest floor
            // available — and it is a real one, verified this audit on this
            // state, which is all the memo below ever asks of an entry.
            //
            // Without this the witness proves the set and banks nothing, so
            // the memo stays empty and every later audit re-derives the
            // same proof: measured at 20,000 accounts, an unchanged block
            // paying 49 ms for a pass that had already been done. With it
            // the second audit of an unchanged state is a memo hit and the
            // witness is never reached.
            if let Some(m) = memo.as_mut() {
                m.insert(set, drawn);
            }
            continue;
        }
        // Measured on a PRISTINE residual. `capacity_of_set` nets out what is
        // already drawn, so comparing outstanding credit against it compares a
        // number with itself subtracted out — and reads zero at exactly the
        // moment the ceiling is fully and legitimately drawn.
        let cut = State::to_minor(state.gross_capacity_of_set(&set));
        computed += 1;
        if drawn > cut {
            // Recorded rather than returned, so that the work this call did is
            // banked below on the failing path too. A count that only ever
            // rises on success would make the cost probes read a cache that
            // looked cheaper than it was every time it caught something.
            violation = Some(Violation(format!(
                "invariant 1: a set of {} accounts holds {drawn} against a cut of {cut}",
                set.len()
            )));
            break;
        }
        if let Some(m) = memo.as_mut() {
            m.insert(set, cut);
        }
    }
    if violation.is_none() {
        if let Some(m) = memo.as_mut() {
            m.retain(|k, _| seen.contains(k));
        }
    }
    if let Some(v) = violation {
        if let Some(c) = cache.as_deref_mut() {
            c.queries += computed;
        }
        return Err(v);
    }

    // --- 6. Determinism ---------------------------------------------------
    // Capacity recomputed from committed state must reproduce the value the
    // block was validated against, bit for bit. What stored state can check on
    // its own is the property that claim rests on: the answer is a function of
    // the ledger, not of the order records happened to be inserted. Integer
    // Dinic over ordered maps makes that true; this catches the day it stops
    // being true, on one set, for the price of one extra query.
    // Skippable only when every input it reads is bit-for-bit unchanged — the
    // edges, the supplies, and the live reservations, since the query runs on
    // the residual. That is the block the cost complaint is about: the
    // one that "contained nothing" and paid two full queries for it.
    if graph_unchanged {
        if let Some(c) = cache.as_deref_mut() {
            c.queries += computed;
        }
        return Ok(());
    }
    if let Some(&(c, _)) = state.edges.keys().next() {
        let target = c as MemberId;
        let mut reversed: flow::Edges = Default::default();
        for (&k, &v) in state.edges.iter().rev() {
            reversed.insert(k, v);
        }
        let uw: Vec<(usize, u64)> = state.underwriters.iter().map(|(&id, &s)| (id as usize, s)).collect();
        let n = state.next_member as usize;
        let a = flow::capacity(&state.edges, &state.reserved, &state.committed, &uw, &[c], n, u64::MAX);
        let b = flow::capacity(&reversed, &state.reserved, &state.committed, &uw, &[c], n, u64::MAX);
        computed += 2;
        if a != b {
            return Err(Violation(format!("invariant 6: capacity of {target} is {a} one way and {b} the other")));
        }
    }
    if let Some(c) = cache {
        c.queries += computed;
    }
    Ok(())
}
