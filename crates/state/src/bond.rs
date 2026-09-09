//! Operation bonds: the transition-class schedule and the admission gate.
//!
//! A bond is *reserved*, never collected. It encumbers the submitter's own
//! headroom for `bond_release_epochs` and then returns, so no balance moves
//! and conservation is untouched — the ledger stays feeless in the strict
//! sense that no party is ever credited for another member's traffic.
//!
//! The one asymmetry that carries the whole design: `bond_of` returns zero
//! for every *state-shrinking* class. A member at its ceiling, or one whose
//! capacity has collapsed to the staked base under an open default, must
//! always be able to settle, cure, and exit — a bond that could strand a
//! member inside their own default would turn a recoverable failure into an
//! absorbing one, which is a strictly worse system than the unbonded one this
//! replaces.

use crate::tx::Tx;
use crate::types::{Key, MemberId};

/// Bond multiple for a transition class, in units of `params.bond_unit`.
///
/// Zero means free forever, not "free for now": these classes either shrink
/// state, unblock a stuck member, or are already bounded by state the
/// attacker would have to pay for first.
///
/// **Every zero here has to survive the question "how many of these can one
/// member force in an epoch?"** — because a zero-priced transition is unlimited
/// and each accepted transaction burns a replay id, so T4's bound (allowance
/// plus headroom over the bond) does not apply to it at all. `Settle` and `Cure`
/// are bounded by an outstanding amount on a row that was bonded at creation;
/// `Transfer` and `Exit` close what they touch; the cranks are idempotent;
/// `Assent` and `ArbAttest` are one per pair. `DeclareSupply` is the one that
/// would answer "unlimited" — see below.
///
/// It takes the state because one class's answer genuinely depends on it, and
/// the alternative was two schedules that could disagree. This is the only
/// schedule; `views` serves the public quote from it and `due` gates on it.
pub fn bond_multiple(state: &crate::state::State, tx: &Tx) -> f64 {
    match tx {
        // --- state-shrinking, or the recovery path: always free -----------
        // Discharge in every form, plus the transitions that let a member
        // wind down and leave. Charging these is what would make a default
        // absorbing (see the module note).
        Tx::Settle { .. } | Tx::Cure { .. } => 0.0,
        Tx::Exit { .. } => 0.0,
        // Moving the debtor of a claim is discharge for the outgoing debtor,
        // and the outgoing debtor may be the one at their ceiling.
        Tx::Transfer { .. } => 0.0,
        // **Lowering** a supply is the underwriter accepting less liability,
        // and it is already floored at the committed flow: a member must never
        // be priced out of reducing what they stand behind. That argument is
        // about one DIRECTION, and applying it to both makes a raise free and
        // unlimited: one member can force **5,000** `DeclareSupply`
        // transitions in a single epoch, burning 5,000 replay ids, with its
        // bond encumbrance still at zero and its headroom untouched. Nothing
        // else on this free list answers "unlimited" — a raise is not
        // idempotent, does not shrink state, and is not the recovery path — so
        // the direction decides.
        Tx::DeclareSupply { member, supply } => {
            let current = state.underwriters.get(member).copied().unwrap_or(0);
            if crate::state::State::to_minor(*supply) < current {
                0.0
            } else {
                1.0
            }
        }
        // The permissionless default crank. Idempotent — it succeeds at most
        // once per contract — so successful cranks are bounded by the
        // contract book, which was itself bonded at creation. Charging it
        // would also be unenforceable: it carries no signer to bill.
        Tx::MarkExpired { .. } => 0.0,
        // Bounded by state someone already paid for: assent is idempotent per
        // (member, proposal) and proposals cost; arbiter attestations come
        // only from a pinned panel inside a closing window. Neither is an
        // unbounded channel, and billing a member for governing or for
        // discharging an arbiter's duty would price participation itself.
        Tx::Assent { .. } | Tx::ArbAttest { .. } => 0.0,
        // Defensive halves of key rotation: the veto is a guardian stopping a
        // theft, and finalize merely completes a request whose bond was
        // already paid at `RotateRequest`.
        Tx::RotateVeto { .. } | Tx::RotateFinalize { .. } => 0.0,
        // The forfeiture crank itself, for the same reason `MarkExpired` is
        // free: pricing the transition that sanctions an abuser would let the
        // abuser's own traffic raise the cost of stopping it.
        Tx::ForfeitBonds { .. } => 0.0,

        // --- permanent state growth: the expensive classes -----------------
        // A proposal is permanent and globally visible. Already gated by the
        // establishment floor — an author the community has put nothing
        // behind cannot propose at all — so this is the second lock on a door
        // that is not open to a fresh account in the first place.
        Tx::Propose { .. } => 2.0,

        // --- ordinary state-growing transitions ----------------------------
        Tx::Accept { .. } | Tx::Sale { .. } => 1.0,
        Tx::Extend { .. } => 1.0,
        Tx::RegisterGuardians { .. } | Tx::RotateRequest { .. } => 1.0,
        // A durable row on the member, and an unlimited free channel if it
        // were not priced — a key can be re-registered as often as anyone
        // likes, and each one burns a replay id.
        Tx::SetConsensusKey { .. } => 1.0,
        Tx::ListBeneficiaries { .. } | Tx::ApproveSupporter { .. } => 1.0,
    }
}

/// **Does this transition carry a signature at all?**
///
/// `MarkExpired`, `ForfeitBonds` and `RotateFinalize` are the three
/// permissionless cranks: nobody has to be authorised to submit one,
/// `bond::due` answers `Free` from the schedule before it ever asks who would
/// pay, and the first two run at every epoch boundary anyway — calling one is a
/// way to be EARLY rather than the only way it happens.
///
/// Which is why a crank that finds nothing to do must not spend a replay id.
/// There is no consent to defer (that is what the deferred-replay policy in
/// `apply` protects, and a signature is what it protects), and all three are
/// pure state checks that mutate nothing on refusal. Without this, a key
/// belonging to nobody wrote 200 of 200 ids into state the root hashes by
/// cranking contracts that do not exist — and `RotateFinalize`, admitted
/// signerless at the ingress but missing from this list, recorded fifty of
/// fifty refusals into a replay bucket the root re-encodes whole on every
/// block, at 110 ms per million ids.
///
/// **This is the ONE list.** The node's ingress asks it to decide whether an
/// empty signer set is admissible (`block::is_permissionless` delegates here),
/// so the transitions that may arrive unsigned and the transitions whose
/// refusal leaves nothing behind cannot drift apart.
pub fn is_permissionless(tx: &Tx) -> bool {
    matches!(tx, Tx::MarkExpired { .. } | Tx::ForfeitBonds { .. } | Tx::RotateFinalize { .. })
}

/// The signers that resolve to a member with a row, in ASCENDING MEMBER ID —
/// the order every billing decision is taken in.
///
/// **Order-independence is the security property.** "Whoever signed first"
/// bills by envelope order, which is chosen by whoever assembles the
/// transaction rather than by the parties: the digest every co-signer verifies
/// covers `(tx, nonce, not_after_epoch)` and deliberately NOT `signers`, so
/// that collecting signatures in a different order does not change the
/// transaction's id — and a co-signer therefore cannot see, in what they
/// signed, that they have been placed first. A counterparty assembles every
/// envelope victim-first, bills the victim for the whole exchange, and three
/// epochs of denials arm the permissionless `ForfeitBonds` crank. That every
/// signature is verified is no answer: it conflates consent to the TRANSACTION
/// with consent to bearing its bond. Canonical order is a function of WHO
/// signed rather than of how the envelope was laid out, so it cannot be steered
/// by whoever assembles it.
///
/// An empty list is a REFUSAL for any priced class (`Due::Unpayable`), never
/// "free". A trade may name a party by key, so a ring of free keys signing an
/// `Accept` between two of themselves resolves to no member at all — and "no
/// payer means free" would hand them an unbounded creation channel on the one
/// transition that creates accounts, with nobody having earned anything to
/// reach it.
///
/// The permissionless cranks are unaffected: they are priced at zero, and `due`
/// answers `Free` from the schedule before it ever asks who would pay.
pub(crate) fn payers(state: &crate::state::State, signers: &[Key]) -> Vec<MemberId> {
    signers
        .iter()
        .filter_map(|k| state.key_index.get(k).copied())
        .filter(|id| state.members.contains_key(id))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The member billed for a transition against current state, or `None` when
/// nobody present can be billed at all.
///
/// **The FIRST signer in canonical order who can pay**, allowance before bond,
/// rather than whoever holds the most headroom. What that keeps is the
/// property the rule exists for: a co-signer with headroom still pays when the
/// first has none, so a newcomer's first trade is still billed to the
/// established member who chose to make it — the same member already taking
/// the first uninsured risk, and the reason the free allowance does not carry a
/// fresh account (see `due`). What it changes is who pays when both can.
///
/// **And it is what makes the cut memoisable.** Ranking two signers by headroom
/// needs both headrooms EXACTLY; asking whether one signer can pay is a
/// threshold test, which a lower bound answers whenever it passes. `seed_reach`
/// is a pristine cut and falls only at decay, on `DeclareSupply` and on a
/// rescale, so a stored value is a valid lower bound until one of those — which
/// is what `GateCache` keeps and `due_with_cache` spends.
pub fn bonded_party(state: &crate::state::State, tx: &Tx, signers: &[Key]) -> Option<MemberId> {
    match due(state, tx, signers) {
        Due::Free => None,
        Due::Unpayable => None,
        Due::Allowance(id) | Due::Slot(id) | Due::Bond(id, _) | Due::Seat { payer: id, .. } => Some(id),
    }
}

/// **A memo of `seed_reach`, held as a LOWER BOUND.**
///
/// Both questions the write gate asks — is this member established, and does
/// their headroom cover this bond — read `seed_reach_minor`, which is a full
/// pristine max-flow cut over the whole stake graph. An ordinary bonded
/// transaction asked for one or two of them on every write.
///
/// `seed_reach` is a maximum flow, and a maximum flow is MONOTONE in every
/// capacity: it can only fall when an edge, a supply or the id space falls.
/// Exactly three things lower one — `flow::decay` at an epoch boundary,
/// `DeclareSupply` (which may only ever lower a declaration), and `rescale` —
/// so a value stored under `(epoch, params, underwriters)` stays a valid lower
/// bound until one of those three moves. `stake` only raises, and exit,
/// suspension and retirement touch status and rows rather than the graph.
///
/// A lower bound answers a THRESHOLD test whenever it passes, which is why the
/// billing rule is "the first signer who can pay" and not "the signer with the
/// most": two lower bounds cannot rank two signers, but either one settles
/// "does this cover the bond". A bound that FAILS is recomputed exactly before
/// any verdict is taken, so the cached form's answer equals `due`'s on every
/// state — never "refused on a stale bound", which would be a censorship
/// channel keyed on how long ago somebody last traded.
///
/// A restart starts cold, like the audit cache: cold is correct and simply
/// costs the first read.
#[derive(Clone, Debug, Default)]
pub struct GateCache {
    /// The state this memo is about. `None` until the first use.
    key: Option<GateKey>,
    reach: std::collections::BTreeMap<MemberId, u64>,
    /// How many cuts this cache has actually computed — what the cost probes
    /// read, and what a probe asserts does not move over an epoch of ordinary
    /// traffic.
    queries: u64,
}

/// Everything a stored `seed_reach` is only valid under.
#[derive(Clone, Debug, PartialEq)]
struct GateKey {
    epoch: u64,
    params: crate::params::Params,
    underwriters: std::collections::BTreeMap<MemberId, u64>,
}

impl GateCache {
    /// Cuts computed since this cache was created — including the ones a
    /// failed bound forced. Read by the cost probes and by
    /// `an_ordinary_bonded_transaction_computes_no_cut`.
    pub fn queries(&self) -> u64 {
        self.queries
    }

    /// Drop everything if the state this memo was about has moved.
    ///
    /// Keyed on the three things that can LOWER a cut, and on nothing else: a
    /// key that included, say, the member count would throw the memo away on
    /// every seat, which is the shape that makes a memo pay for nothing.
    fn rekey(&mut self, state: &crate::state::State) {
        let key =
            GateKey { epoch: state.epoch, params: state.params.clone(), underwriters: state.underwriters.clone() };
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.reach.clear();
        }
    }

    /// A lower bound on `seed_reach_minor(id)`, computing one if none is held.
    fn bound(&mut self, state: &crate::state::State, id: MemberId) -> u64 {
        if let Some(&v) = self.reach.get(&id) {
            return v;
        }
        self.exact(state, id)
    }

    /// The cut itself, stored as the new bound.
    fn exact(&mut self, state: &crate::state::State, id: MemberId) -> u64 {
        let v = state.seed_reach_minor(id);
        self.queries += 1;
        self.reach.insert(id, v);
        v
    }
}

/// `established`, answered by the bound where it passes and exactly where it
/// does not.
fn established_cached(state: &crate::state::State, id: MemberId, cache: &mut GateCache) -> bool {
    let Some(m) = state.members.get(&id) else { return false };
    if !matches!(m.status, crate::types::MemberStatus::Active) {
        return false;
    }
    let floor = crate::state::State::to_minor(state.params.v_base * ESTABLISHED_FRACTION);
    cache.bound(state, id) >= floor || cache.exact(state, id) >= floor
}

/// `free_remaining`, over `established_cached`.
fn free_remaining_cached(state: &crate::state::State, id: MemberId, cache: &mut GateCache) -> u32 {
    let Some(m) = state.members.get(&id) else { return 0 };
    if !established_cached(state, id, cache) || m.rep.open_default > state.params.dust_minor() {
        return 0;
    }
    state.params.bond_free_allowance.saturating_sub(m.bond_free_used)
}

/// `State::bond_headroom_minor`, over the cached `seed_reach`.
///
/// The three subtractions are exact state, so a lower bound on the cut is a
/// lower bound on the headroom — which is all a threshold test needs.
fn headroom_cached(state: &crate::state::State, id: MemberId, cache: &mut GateCache) -> u64 {
    let Some(m) = state.members.get(&id) else { return 0 };
    if !matches!(m.status, crate::types::MemberStatus::Active) {
        return 0;
    }
    let net = |reach: u64| {
        reach
            .saturating_sub(m.debt_out)
            .saturating_sub(m.bond_enc())
            .saturating_sub(state.forfeit_reserve.get(&id).copied().unwrap_or(0))
    };
    net(cache.bound(state, id))
}

/// The same, forced exact — what a failed threshold falls back to.
fn headroom_exact(state: &crate::state::State, id: MemberId, cache: &mut GateCache) -> u64 {
    let _ = cache.exact(state, id);
    state.bond_headroom_minor(id)
}

/// What a transition costs against current state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Due {
    /// Costs nothing and never can: a zero-bond class.
    Free,
    /// Covered by this member's remaining free allowance, which the gate must
    /// spend — the one part of the decision that is a mutation, and therefore
    /// the one part `due` cannot perform itself.
    Allowance(MemberId),
    /// Covered by the one self-act this member's seat paid for
    /// (`Member::seat_slot`), which the gate must spend the same way.
    Slot(MemberId),
    /// Requires this much headroom from this member, in minor units. A
    /// non-seating bonded write.
    Bond(MemberId, u64),
    /// A bonded write that also SEATS `rows` rows, and so needs two things of
    /// `payer` at once: `bond` of work headroom, and `rows` seats of reach.
    ///
    /// The two are different budgets answering different questions. Headroom
    /// is a rate — it refills every epoch, so what it buys over time is
    /// unbounded — and a row is a stock, hashed into every state root and
    /// retired by nothing. So a row is priced by a reservation on the graph
    /// that is never released, and the work bond is charged beside it exactly
    /// as `Bond` charges it.
    Seat { payer: MemberId, bond: u64, rows: u32 },
    /// A priced class that nobody present can pay for: no signer resolves to a
    /// member. Refused, because the alternative is a free write channel
    /// available to anyone holding a fresh key (see `bonded_party`).
    Unpayable,
}

/// Classify a transition's bond against current state, without mutating
/// anything.
///
/// This is the single source of truth for the decision, shared by the
/// authoritative gate in `apply` and the advisory screens at mempool
/// admission and block proposal. They must not be allowed to drift: an
/// ingress screen stricter than the gate would silently censor valid
/// traffic, and one looser than the gate would defeat the point of screening
/// at all.
pub fn due(state: &crate::state::State, tx: &Tx, signers: &[Key]) -> Due {
    due_with_cache(state, tx, signers, &mut GateCache::default())
}

/// `due`'s answer, computed against a cache of `seed_reach` LOWER BOUNDS.
///
/// **It must agree with `due` on every state**, and it does by construction:
/// every question the two ask is a THRESHOLD test, a lower bound settles one
/// whenever it passes, and a bound that fails is recomputed exactly before the
/// verdict is taken. `due` stays the definition; the harness drives both after
/// every transition in the tree and panics on a disagreement.
///
/// What the cache buys is the reason it exists: `established` and
/// `bond_headroom` both read `seed_reach_minor`, which is a full pristine
/// max-flow cut, and an ordinary bonded transaction asked for one or two of
/// them on every single write.
pub fn due_with_cache(state: &crate::state::State, tx: &Tx, signers: &[Key], cache: &mut GateCache) -> Due {
    let multiple = bond_multiple(state, tx);
    if multiple <= 0.0 {
        return Due::Free;
    }
    let ids = payers(state, signers);
    let Some(&first) = ids.first() else {
        return Due::Unpayable;
    };
    let bond = crate::state::State::to_minor(multiple * state.params.bond_unit());
    let rows = fresh_keys(state, tx);
    cache.rekey(state);
    // **The free allowance is not a per-key entitlement**, and it must not be.
    //
    // A per-key allowance is exactly the shape the free-signature bound
    // forbids, one layer down from credit: keys are free, so N keys carry N
    // allowances, and the ledger's write floor is `allowance x keys minted`
    // rather than anything an attacker had to earn. That is the same defect as
    // an unconditional starter credit, and it has the same answer — the
    // allowance goes to accounts the community has put something behind, and
    // to nobody else.
    //
    // A ring of free keys trading only with each other therefore writes
    // NOTHING: no signer has capacity, so no signer has headroom, so the gate
    // refuses. A newcomer trading with an established member writes normally,
    // billed to the member who chose to trade with them.
    //
    // **A transition that SEATS a row is bonded, never allowance-covered.**
    // The allowance is a rate: it refills every epoch, so what it can buy over
    // time is unbounded. A row is a stock: it is permanent, it is hashed into
    // the state root on every block, and nothing retires it. Letting the free
    // allowance pay for one makes the ledger's account table grow at
    // `allowance x established accounts` per epoch, and each new account is
    // itself established as soon as it settles anything — so one honest edge
    // of 100 behind one member produces 33 established accounts in the first
    // epoch and 1,089 in the second, a factor of 33 per epoch, for free.
    //
    // Bonding it does not make seating approvable and does not price a
    // newcomer out: the bond is reserved against the ESTABLISHED counterparty
    // who chose to trade with them, released on schedule, and credited to
    // nobody. What it does is make each row cost the one thing an attacker
    // cannot mint — headroom the community conferred.
    //
    // **A bond prices the WORK and not the ROW, and the two need separate
    // budgets.** Headroom refills every epoch, so what a bond buys over time is
    // unbounded; a row is permanent. Priced by headroom alone, a wash large
    // enough to leave the seated row a bond unit of its own after a tick of
    // decay makes that row a seater too and the count compounds — measured from
    // one honest edge of 500.00, rows held after six epochs: 91 for a wash of
    // 1.00, 20.00 or 40.00, all of it the operator's own linear rate, then 310
    // at 60.00 and 1,120 at 100.00, with the farm's gross cut as a SET still
    // that one edge. Nor does the establishment floor separate them: raising
    // `ESTABLISHED_FRACTION` tenfold leaves every one of those counts identical
    // and takes the established count to zero, because what it withholds is the
    // free allowance a seated row would hold and never the row.
    //
    // So the ROW carries a price of its own — `Due::Seat` below — one bond unit
    // of the sponsor's reach, reserved on the stake graph and never released,
    // which is what makes it the price of a stock rather than of a rate.
    if rows == 0 {
        // **The seat slot: one self-act the seat paid for.** Spent by the
        // row's own key, on a transition about that row and nobody else,
        // BEFORE any co-signer is billed — and only where the row has no
        // allowance of its own, so a member the community has since backed
        // spends the budget that refills and keeps the one that does not.
        // Bounded by seats, which are bounded by the seed: N rows hold N
        // slots and no more, so it is a stock and never a rate.
        if let Some(subject) = self_act_subject(tx) {
            let own = ids.contains(&subject)
                && state
                    .members
                    .get(&subject)
                    .is_some_and(|m| m.seat_slot && matches!(m.status, crate::types::MemberStatus::Active));
            if own && free_remaining_cached(state, subject, cache) == 0 {
                return Due::Slot(subject);
            }
        }
        if let Some(&id) = ids.iter().find(|&&id| free_remaining_cached(state, id, cache) > 0) {
            return Due::Allowance(id);
        }
    }
    // The first signer whose headroom covers the bond. When nobody's does, the
    // FIRST resolved member is named in the refusal — it is the one whose
    // `bond_denied_this_epoch` the gate sets, so the member the sanction
    // counter advances is a function of who signed and not of who was tried
    // last.
    let covers_bond = |id: MemberId, cache: &mut GateCache| {
        headroom_cached(state, id, cache) >= bond || headroom_exact(state, id, cache) >= bond
    };
    if rows > 0 {
        // A seating trade asks BOTH budgets of one member, and the sponsor is
        // the first signer who holds both: a payer named on work headroom alone
        // would refuse a trade a co-signer could have seated. The seat reach is
        // asked exactly — `GateCache` memoises a lower bound on `seed_reach`,
        // and a seat lowers `seat_reach` without moving any of the three things
        // that key it, so a bound held there would go stale downward, which is
        // the one direction a memo may never err in.
        let payer = ids
            .iter()
            .copied()
            .find(|&id| {
                matches!(state.members.get(&id).map(|m| m.status), Some(crate::types::MemberStatus::Active))
                    && covers_bond(id, cache)
                    && state.can_seat_minor(id, rows as u64)
            })
            .unwrap_or(first);
        return Due::Seat { payer, bond, rows };
    }
    let payer = ids.iter().copied().find(|&id| covers_bond(id, cache)).unwrap_or(first);
    Due::Bond(payer, bond)
}

/// **The one member a transition is about, and nobody else** — the row whose
/// seat slot may pay for it. `None` for a trade, a contract, a crank and a
/// guardian rotation, each of which names or is signed by somebody else, so
/// no member's slot is ever spent by another member's write.
pub(crate) fn self_act_subject(tx: &Tx) -> Option<MemberId> {
    match tx {
        Tx::RegisterGuardians { member, .. }
        | Tx::RotateVeto { member }
        | Tx::SetConsensusKey { member, .. }
        | Tx::Exit { member }
        | Tx::DeclareSupply { member, .. }
        | Tx::Assent { member, .. } => Some(*member),
        Tx::ListBeneficiaries { supporter, .. } => Some(*supporter),
        Tx::ApproveSupporter { beneficiary, .. } => Some(*beneficiary),
        Tx::ArbAttest { arbiter, .. } => Some(*arbiter),
        Tx::Propose { author, .. } => Some(*author),
        _ => None,
    }
}

/// **How many rows would this transition seat?** — in `0..=2`.
///
/// Only the two trade transitions can seat at all (`Party::Key` in `Accept` or
/// `Sale`); every other transition names members that already exist, and a key
/// that resolves to a member seats nothing.
///
/// A COUNT rather than a flag, because a trade may name two fresh keys under a
/// third established signer and each of them is a row. The work bond stays one
/// per transaction — a bond is charged per transaction and says nothing about
/// payload size — and the seat price is what bounds this payload: two rows
/// cost two seats of the sponsor's reach.
fn fresh_keys(state: &crate::state::State, tx: &Tx) -> u32 {
    let fresh = |p: &crate::types::Party| match p {
        crate::types::Party::Member(_) => 0,
        crate::types::Party::Key(k) => u32::from(state.member_of_key(k).is_none()),
    };
    match tx {
        Tx::Accept { debtor, creditor, .. } => fresh(debtor) + fresh(creditor),
        Tx::Sale { seller, buyer, .. } => fresh(seller) + fresh(buyer),
        _ => 0,
    }
}

/// **Has the community put anything behind this account?** — the qualification
/// the free allowance turns on, and the one the wallet must not re-derive from
/// a quantity beside it.
///
/// Read GROSS of live credit, and that is load-bearing: a residual reading
/// makes this a statement about how busy a member's backers are rather than
/// about whether they have any. Measured on that reading, at **20%** community
/// utilisation: one member drawing the underwriter that reaches them takes a
/// second member's allowance from 32 to 0 — a member who has borrowed nothing
/// — and a community drawn to its ceiling refuses `Accept`, `Sale` and a
/// newcomer's first trade alike with `ET-BND-001`, where a full ceiling
/// withholds INSURANCE and not trade.
///
/// Read on the SEED's reach, which includes a member's own declared supply, so
/// a founding underwriter qualifies with capacity zero — and a key nobody has
/// backed qualifies for nothing, which is the free-signature bound one layer
/// under credit.
///
/// Served on `/member` so that the client asks the ledger's own question
/// instead of comparing `conferrable` against dust itself, which is the same
/// question against the wrong reading.
pub fn established(state: &crate::state::State, id: MemberId) -> bool {
    let Some(m) = state.members.get(&id) else { return false };
    matches!(m.status, crate::types::MemberStatus::Active)
        && state.seed_reach_minor(id) >= crate::state::State::to_minor(state.params.v_base * ESTABLISHED_FRACTION)
}

/// The share of `v_base` the seed must reach an account for before its
/// allowance opens.
///
/// **A dust floor over `conferrable` qualifies a wash trade.** One settlement
/// of 1.00 between two accounts writes a stake of 1.00, which is above dust, so
/// the account it seats is established and holds a full allowance the next
/// epoch — which it spends seating more of them. The floor has to be large
/// enough that a token trade does not clear it and small enough that a real
/// newcomer does: a twentieth of the denomination is one ordinary purchase.
///
/// Measured against SEED REACH rather than `conferrable`, for the reason
/// `bond_headroom` reads the same quantity: a declaration or a stake placed by
/// somebody the seed does not reach is standing the community never conferred,
/// and a floor over it is a floor an accomplice can manufacture.
pub const ESTABLISHED_FRACTION: f64 = 0.05;

/// **How many free transitions this member actually has left this epoch** —
/// which is zero for a member the allowance does not reach at all, not the
/// whole allowance minus nothing.
///
/// Split out of `due` so that the read surface and the gate cannot disagree.
/// `allowance - used`, with no test that the member qualifies, shows a member
/// whose backing has been withdrawn **32 of 32 free actions** in their own
/// wallet while `due` grants them none and every write comes back
/// `ET-BND-001`: measured, `conferrable` 2500 → 0 on the underwriter's
/// withdrawal, `bond_headroom` 2500 → 0, `free_remaining` still 32.
///
/// That is the allowance cliff, and the client could not warn about it because
/// the figure it had said there was nothing to warn about. A view reporting a
/// quantity the ledger does not honour is read as a promise, exactly like a
/// view describing a mechanism the chain does not run.
///
/// **The qualification is `established`, which is that same reading taken
/// GROSS of live credit.** A residual one makes the free allowance a statement
/// about how busy a member's backers are: 32 → 0 at **20%** community
/// utilisation, for a member who has borrowed nothing and done nothing. The
/// cliff this function is written for is real; a second cliff under a member
/// who was never at their own limit is not.
pub fn free_remaining(state: &crate::state::State, id: MemberId) -> u32 {
    let Some(m) = state.members.get(&id) else { return 0 };
    // The default condition has to be stated, because the qualification above
    // is gross: a defaulter whose debt consumed everything behind them would
    // otherwise keep a full allowance every epoch for as long as their stakes
    // take to decay.
    // **A default consuming the defaulter's standing IS the sanction**, and the
    // recovery path does not need the allowance to stay open — `Cure`,
    // `Settle`, `Transfer` and `Exit` are priced at zero, and `due` answers
    // `Free` from the schedule before it asks who would pay.
    if !established(state, id) || m.rep.open_default > state.params.dust_minor() {
        return 0;
    }
    state.params.bond_free_allowance.saturating_sub(m.bond_free_used)
}

/// Would the gate admit this transition right now? Read-only.
///
/// Advisory wherever it is used outside `apply`: a node screens against the
/// state IT holds, and two honest nodes at different heights can legitimately
/// disagree at the margin. That is exactly why this must never become a
/// block-validity rule — nodes voting on a headroom reading would fork on a
/// disagreement that is not a fault. Screening drops a transaction from a
/// local mempool or a proposal; only `apply` decides.
pub fn admits(state: &crate::state::State, tx: &Tx, signers: &[Key]) -> bool {
    admits_with_cache(state, tx, signers, &mut GateCache::default())
}

/// `admits`, over the gate's memo — what a node's two ingress screens use, so
/// admitting a transaction costs what applying it costs rather than a fresh
/// cut per signer.
pub fn admits_with_cache(state: &crate::state::State, tx: &Tx, signers: &[Key], cache: &mut GateCache) -> bool {
    match due_with_cache(state, tx, signers, cache) {
        Due::Free | Due::Allowance(_) | Due::Slot(_) => true,
        // `due_with_cache` already found the first signer whose headroom
        // covers this, exactly; when none did it named the first, whose
        // headroom is then known not to. Asking again through the cache is
        // therefore the same question with the same answer, and never a
        // stale one.
        Due::Bond(payer, bond) => {
            bond <= headroom_cached(state, payer, cache) || bond <= headroom_exact(state, payer, cache)
        }
        // Both budgets, for the payer `due_with_cache` named. This is what the
        // ingress, the pre-vote screen and commit each ask, so the three
        // answers stay one function of committed state.
        Due::Seat { payer, bond, rows } => {
            matches!(state.members.get(&payer).map(|m| m.status), Some(crate::types::MemberStatus::Active))
                && (bond <= headroom_cached(state, payer, cache) || bond <= headroom_exact(state, payer, cache))
                && state.can_seat_minor(payer, rows as u64)
        }
        Due::Unpayable => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use crate::types::Party;

    fn every_tx() -> Vec<Tx> {
        vec![
            Tx::RegisterGuardians { member: 0, guardians: vec![], threshold: 1, veto_window_epochs: 1 },
            Tx::RotateRequest { member: 0, new_keys: vec![] },
            Tx::RotateVeto { member: 0 },
            Tx::RotateFinalize { member: 0 },
            Tx::DeclareSupply { member: 0, supply: 1.0 },
            Tx::Exit { member: 0 },
            Tx::ListBeneficiaries { supporter: 0, entries: vec![] },
            Tx::ApproveSupporter { beneficiary: 0, supporter: 1, approved: true },
            Tx::Sale { seller: Party::Member(0), buyer: Party::Member(1), amount: 1.0, maturity_epochs: 1 },
            Tx::Accept {
                debtor: Party::Member(0),
                creditor: Party::Member(1),
                amount: 1.0,
                maturity_epochs: 1,
                arb: None,
            },
            Tx::Transfer { contract: 0, new_debtor: 1 },
            Tx::Settle { contract: 0, amount: 1.0 },
            Tx::Extend { contract: 0, new_maturity_epoch: 1 },
            Tx::MarkExpired { contract: 0 },
            Tx::Cure { contract: 0, amount: 1.0 },
            Tx::ArbAttest { contract: 0, arbiter: 0, amount: 1.0 },
            Tx::Propose { author: 0, kind: crate::types::ProposalKind::Suspend { member: 1 } },
            Tx::Assent { member: 0, proposal: 0 },
            Tx::ForfeitBonds { member: 0 },
        ]
    }

    /// The schedule is total and non-negative: every variant of the alphabet
    /// is priced, so adding a transaction without pricing it cannot compile
    /// past `bond_multiple`'s exhaustive match.
    #[test]
    fn every_transition_class_is_priced_and_non_negative() {
        for tx in every_tx() {
            assert!(bond_multiple(&State::default(), &tx) >= 0.0, "negative bond for {tx:?}");
        }
    }

    /// The load-bearing exemption: the recovery path must never carry a bond,
    /// or a member whose capacity is consumed by an open default could not
    /// afford to cure it and the default would become absorbing.
    ///
    /// `DeclareSupply` belongs here for the same reason pointed the other way:
    /// an underwriter must never be priced out of reducing what they stand
    /// behind, which is already floored at the committed flow.
    #[test]
    fn the_recovery_path_is_free() {
        // A real underwriter, because "reducing what you stand behind" is a
        // statement about a direction and needs something to reduce FROM.
        let mut st = State::default();
        st.add_underwriter(vec![[1u8; 32]], 2500.0).expect("founding underwriter");
        let free = [
            Tx::Settle { contract: 0, amount: 1.0 },
            Tx::Cure { contract: 0, amount: 1.0 },
            Tx::Transfer { contract: 0, new_debtor: 1 },
            Tx::Exit { member: 0 },
            Tx::DeclareSupply { member: 0, supply: 0.0 },
            Tx::DeclareSupply { member: 0, supply: 2499.99 },
        ];
        for tx in free {
            assert_eq!(bond_multiple(&st, &tx), 0.0, "recovery transition must be free: {tx:?}");
        }
        // The other direction is not the recovery path and is priced, or it is
        // an unlimited free write channel.
        assert!(
            bond_multiple(&st, &Tx::DeclareSupply { member: 0, supply: 2500.01 }) > 0.0,
            "raising a supply must be bonded"
        );
        assert!(
            bond_multiple(&st, &Tx::DeclareSupply { member: 0, supply: 2500.0 }) > 0.0,
            "and so must re-declaring the same figure, which shrinks nothing"
        );
    }

    /// Permanent, non-reclaimable state growth costs strictly more than an
    /// ordinary transition.
    ///
    /// There is no account-creation transition to sit at the top of this
    /// ladder: a row is seated by the trade that names it, at the trade's own
    /// price, and what bounds creation is that somebody with standing had to
    /// pay for the write at all. So what is left to order here is governance
    /// against ordinary traffic.
    #[test]
    fn permanent_state_growth_costs_more_than_ordinary_traffic() {
        let ordinary = bond_multiple(
            &State::default(),
            &Tx::Accept {
                debtor: Party::Member(0),
                creditor: Party::Member(1),
                amount: 1.0,
                maturity_epochs: 1,
                arb: None,
            },
        );
        let propose = bond_multiple(
            &State::default(),
            &Tx::Propose { author: 0, kind: crate::types::ProposalKind::Suspend { member: 1 } },
        );
        assert!(propose > ordinary, "a permanent proposal must outprice a contract: {propose} vs {ordinary}");
    }

    /// The zero-amount classes are exactly the spam vector an ad-valorem fee
    /// would have missed: they must be priced by CLASS, not by amount.
    #[test]
    fn amountless_growing_transitions_still_carry_a_bond() {
        let amountless = [
            Tx::ApproveSupporter { beneficiary: 0, supporter: 1, approved: true },
            Tx::RotateRequest { member: 0, new_keys: vec![] },
            Tx::ListBeneficiaries { supporter: 0, entries: vec![] },
        ];
        for tx in amountless {
            assert!(bond_multiple(&State::default(), &tx) > 0.0, "amountless transition must still be bonded: {tx:?}");
        }
    }

    /// A community of one underwriter, one member they back, and one stranger.
    fn three() -> (State, [u8; 32], [u8; 32], [u8; 32]) {
        let (uk, bk, sk) = ([1u8; 32], [2u8; 32], [3u8; 32]);
        let mut st = State::default();
        let u = st.add_underwriter(vec![uk], 2500.0).expect("underwriter");
        let backed = st.new_account(vec![bk]);
        st.new_account(vec![sk]);
        let conf = st.conferrable_minor(u);
        st.record_stake(u, backed, crate::state::State::to_minor(2500.0), conf);
        (st, uk, bk, sk)
    }

    /// **The spam floor is zero.** A ring of free keys trading only with each
    /// other writes nothing at all: no signer has capacity, so no signer has
    /// headroom, so the gate refuses every bonded class.
    ///
    /// A per-key free allowance is the free-signature bound's own defect one
    /// layer down — keys are free, so N keys carry N allowances and the
    /// ledger's write floor is a quantity nobody had to earn.
    #[test]
    fn free_keys_get_no_free_writes() {
        let mut st = State::default();
        let (a, b) = ([9u8; 32], [8u8; 32]);
        st.new_account(vec![a]);
        st.new_account(vec![b]);
        let tx = Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 1.0,
            maturity_epochs: 30,
            arb: None,
        };
        assert!(!admits(&st, &tx, &[a, b]), "an unbacked ring must not be able to write");
        match due(&st, &tx, &[a, b]) {
            Due::Bond(_, bond) => assert!(bond > 0),
            other => panic!("a zero-capacity payer must be billed, not waved through: {other:?}"),
        }
    }

    /// And the newcomer's genuine first trade still goes through, billed to
    /// the established member who chose to trade with them. That is the same
    /// cost §Recourse already names — somebody must take a first risk on a newcomer —
    /// priced at one refundable bond instead of nothing.
    #[test]
    fn an_established_member_carries_a_newcomers_first_trade() {
        let (st, _uk, bk, sk) = three();
        let tx = Tx::Accept {
            debtor: Party::Member(2),
            creditor: Party::Member(1),
            amount: 10.0,
            maturity_epochs: 30,
            arb: None,
        };
        assert!(admits(&st, &tx, &[bk, sk]), "a backed counterparty must be able to carry the newcomer");
        assert_eq!(due(&st, &tx, &[bk, sk]), Due::Allowance(1), "and the bill lands on them, not the stranger");
    }

    /// The bill must be a function of WHO signed, never of how the envelope
    /// was laid out. Envelope order is chosen by whoever assembles the
    /// transaction, and the digest co-signers verify excludes `signers`, so an
    /// order-dependent rule let a counterparty place the victim first and bill
    /// them for the whole exchange — arming the permissionless `ForfeitBonds`
    /// crank against a member who never submitted anything.
    #[test]
    fn the_bonded_party_does_not_depend_on_signer_order() {
        let (st, uk, bk, sk) = three();
        let tx = dud();
        let expected = bonded_party(&st, &tx, &[uk, bk, sk]);
        assert!(expected.is_some());
        for order in [[sk, bk, uk], [bk, sk, uk], [uk, sk, bk], [bk, uk, sk]] {
            assert_eq!(bonded_party(&st, &tx, &order), expected, "the payer must not move with the envelope");
        }
        // An unknown key is skipped rather than billed, wherever it sits.
        assert_eq!(bonded_party(&st, &tx, &[[77u8; 32], bk]), Some(1));
        assert_eq!(bonded_party(&st, &tx, &[]), None);
        assert_eq!(bonded_party(&st, &tx, &[[77u8; 32]]), None);
    }

    /// A transition that is always priced and always fails on its own merits,
    /// for the probes that are about WHO pays rather than about what for.
    fn dud() -> Tx {
        Tx::Extend { contract: u64::MAX, new_maturity_epoch: 9_999 }
    }

    /// **The first signer in canonical order who can pay, pays.** Not whoever
    /// holds the most headroom: ranking two signers needs both headrooms
    /// exactly, and a lower bound cannot rank — which is what would put the
    /// gate's cut beyond a memo.
    #[test]
    fn the_first_signer_who_can_pay_pays() {
        let (st, uk, bk, _sk) = three();
        // Member 0 is the underwriter and member 1 the backed member; both
        // hold an allowance, and 0 is first in canonical order.
        assert_eq!(due(&st, &dud(), &[uk, bk]), Due::Allowance(0));
        assert_eq!(due(&st, &dud(), &[bk, uk]), Due::Allowance(0), "and envelope order does not move it");
    }

    /// **A co-signer with headroom still pays when the first has none**, which
    /// is the property the old max-by rule existed for: a newcomer's first
    /// trade is billed to the established member who chose to make it.
    #[test]
    fn a_co_signer_with_headroom_still_pays_when_the_first_has_none() {
        let mut st = State::default();
        let (bk, sk) = ([1u8; 32], [2u8; 32]);
        // Member 0 is the stranger — first in canonical order, and backed by
        // nobody — and member 1 the underwriter who can carry the trade.
        st.new_account(vec![sk]);
        st.add_underwriter(vec![bk], 2500.0).expect("underwriter");
        assert_eq!(free_remaining(&st, 0), 0, "the stranger has no allowance");
        assert_eq!(due(&st, &dud(), &[sk, bk]), Due::Allowance(1), "so the bill lands on the member who can pay");
    }

    /// And when NOBODY can pay, the refusal names the first — the member whose
    /// saturation counter the gate advances, so that too is a function of who
    /// signed.
    #[test]
    fn a_refusal_names_the_first_signer() {
        let mut st = State::default();
        let (a, b) = ([9u8; 32], [8u8; 32]);
        st.new_account(vec![a]);
        st.new_account(vec![b]);
        match due(&st, &dud(), &[b, a]) {
            Due::Bond(payer, _) => assert_eq!(payer, 0, "canonical order, not envelope order"),
            other => panic!("a ring with nothing behind it must be billed and refused: {other:?}"),
        }
    }

    /// **The memo never changes a verdict.** Every threshold it answers from a
    /// bound it also answers exactly when the bound fails, so the two forms
    /// agree on every state — here across an epoch boundary, which is one of
    /// the three things that can lower a cut.
    #[test]
    fn the_gate_memo_agrees_with_the_definition_across_a_decay() {
        let (mut st, uk, bk, sk) = three();
        let mut cache = GateCache::default();
        let tx = dud();
        for epoch in 0..4u64 {
            st.begin_block(epoch * edet_kernel::constants::EPOCH_SECS);
            for signers in [vec![uk], vec![bk], vec![sk], vec![uk, bk, sk]] {
                assert_eq!(
                    due_with_cache(&st, &tx, &signers, &mut cache),
                    due(&st, &tx, &signers),
                    "the memo changed a verdict at epoch {epoch}"
                );
            }
        }
    }

    /// **An ordinary bonded transaction computes no cut** once the memo is
    /// warm, which is the whole reason it exists: `established` and
    /// `bond_headroom` both read a full pristine max-flow.
    ///
    /// Mutation that bites: key the memo on something that moves with ordinary
    /// traffic — the member count, say — and every call recomputes.
    #[test]
    fn a_warm_gate_memo_computes_no_cut() {
        let (st, uk, bk, sk) = three();
        let mut cache = GateCache::default();
        let tx = dud();
        for signers in [vec![uk], vec![bk], vec![sk]] {
            let _ = due_with_cache(&st, &tx, &signers, &mut cache);
        }
        let warm = cache.queries();
        assert!(warm > 0, "the first reads must have computed something");
        for _ in 0..20 {
            let _ = due_with_cache(&st, &tx, &[uk, bk, sk], &mut cache);
        }
        assert_eq!(cache.queries(), warm, "twenty more transitions inside one epoch must compute no cut");
    }

    /// **A stale bound below the bond is recomputed, never refused.**
    ///
    /// A stake RAISES a cut, and nothing about raising one invalidates a lower
    /// bound — so the memo legitimately holds a value below the truth for as
    /// long as a member keeps trading. Refusing on that would be a censorship
    /// channel keyed on how long ago somebody last settled: the longer you
    /// trade without an epoch boundary, the more of your own headroom the node
    /// cannot see.
    ///
    /// Mutation that bites: drop the `|| headroom_exact(...)` fallback in
    /// `due_with_cache`, and this member is billed a bond their headroom
    /// covers and refused for one it does not.
    #[test]
    fn a_stale_bound_below_the_bond_is_recomputed_not_refused() {
        let mut st = State::default();
        // Member 0 underwrites; 1 and 2 both sign. Member 1 is ESTABLISHED but
        // drawn almost to their limit, which is the state that makes the
        // headroom test — rather than the establishment test — the one the
        // bound decides.
        let (uk, first, second) = ([1u8; 32], [2u8; 32], [3u8; 32]);
        st.add_underwriter(vec![uk], 2500.0).expect("underwriter");
        st.new_account(vec![first]);
        st.new_account(vec![second]);
        st.place_stake(0, 1, 100.0);
        st.place_stake(0, 2, 500.0);
        st.members.get_mut(&1).expect("member 1").debt_out = 9_000;
        st.params.bond_free_allowance = 0;

        // The memo learns member 1's reach while it is 10,000 minor against a
        // debt of 9,000 — established, and one thousand short of the bond.
        let mut cache = GateCache::default();
        let tx = dud();
        let signers = [first, second];
        assert_eq!(due_with_cache(&st, &tx, &signers, &mut cache), Due::Bond(2, 2_000));
        let stored = cache.queries();
        assert!(stored > 0);

        // Then a settlement stakes the underwriter's whole supply on member 1,
        // in the same epoch. A stake RAISES a cut, so nothing invalidates the
        // memo — and the bound it holds is now far below the truth.
        st.place_stake(0, 1, 2000.0);
        assert!(st.bond_headroom_minor(1) >= 2_000, "the stake is real");
        assert_eq!(
            due_with_cache(&st, &tx, &signers, &mut cache),
            due(&st, &tx, &signers),
            "a bound that fails must be recomputed, not skipped over: member 1 is first in canonical order and \
             can now pay, so the bill is theirs"
        );
        assert!(cache.queries() > stored, "and recomputing is what it costs");
        assert!(
            admits_with_cache(&st, &tx, &signers, &mut cache),
            "and a stale bound must never turn into a refusal — that would be a censorship channel keyed on how \
             long ago somebody last settled"
        );
    }

    /// And the three things that CAN lower a cut throw it away.
    #[test]
    fn decay_a_lowered_supply_and_a_rescale_each_invalidate_the_memo() {
        let tx = dud();
        for change in 0..3 {
            let (mut st, uk, _bk, _sk) = three();
            let mut cache = GateCache::default();
            let _ = due_with_cache(&st, &tx, &[uk], &mut cache);
            let warm = cache.queries();
            match change {
                0 => st.begin_block(edet_kernel::constants::EPOCH_SECS),
                1 => {
                    st.underwriters.insert(0, 1);
                }
                _ => st.rescale(0.5),
            }
            let _ = due_with_cache(&st, &tx, &[uk], &mut cache);
            assert!(cache.queries() > warm, "change {change} must invalidate the memo");
        }
    }
}
