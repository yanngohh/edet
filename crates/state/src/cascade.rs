//! The sale composite: mutual netting, then the waterfilling support cascade.
//!
//! A sale of size δ from seller to buyer discharges existing obligations
//! before creating genesis debt. First, NETTING: whatever the seller
//! already owes the buyer settles oldest-first (expired contracts cure) —
//! mutual obligations extinguish rather than route, and only the
//! remainder is capacity-gated and cascades. This is how settlement
//! happens in the wallet: to pay a debt back you sell to your creditor;
//! no standalone settle round-trip. Then the seller's listing IS the breakdown,
//! self included (the v0.5 shape): a self entry is the seller's own share
//! of the waterfill and clears their own contracts (oldest first, capped
//! by their outstanding debt); listed-and-approved beneficiaries drain by
//! their weights, each edge capped at `ν` times what the pair has staked in
//! each other, recursing into beneficiaries' own listings up to the depth
//! ceiling. A seller with NO listing at all keeps the simple default: the
//! whole sale clears their own debts. Whatever the cascade cannot absorb
//! becomes genesis debt owed by the buyer to the seller, and the buyer's
//! total new obligation is exactly δ.
//!
//! Nothing here is capacity-GATED. The successor obligations reserve flow if
//! it is there and are uninsured if it is not, exactly as `Accept` is —
//! capacity bounds what the community underwrites, never what a member may
//! choose to risk.

use std::collections::{BTreeMap, BTreeSet};

use edet_kernel::cascade::waterfill_minor;
use edet_kernel::constants as k;

use crate::apply::{
    book, discharge_credit, member, member_mut, require_party, require_signed, resolve, seat_pair, Resolved,
};
use crate::errors::*;
use crate::state::State;
use crate::types::*;

/// Successor obligations the buyer will assume, keyed by creditor and
/// maturity: the amount, and **the reservation already taken for it**.
///
/// Not by creditor alone. A successor is a debtor swap and inherits the
/// earlier of the original's date and the sale's (`clear_member_debts` says
/// why), so two originals from one creditor on different dates are two rows.
/// `(creditor, the original was insured)` is not in the key: the bound below
/// moves every original only as far as the buyer can carry it INSURED,
/// whatever it was before, so every successor is insured and the flag is one.
/// The third field is the EARLIEST acceptance among the originals the row
/// joins: a successor is a debtor swap and keeps its original's horizon base,
/// and where several join one row the base is the one that binds first.
type Assumed = BTreeMap<(MemberId, u64), (u64, edet_kernel::flow::Held, u64)>;

/// Add one cleared original's reservation to the successor row it joins.
///
/// A `Held` is a flow rather than a list of numbers, and the union of two
/// flows into the same debtor is a flow — which is exactly the case here, since
/// both were augmented into the buyer against the same residual, one after the
/// other. Summing per arc is therefore the right merge, and the ordering
/// invariants (`edges` in `Edges` key order, `supply` in underwriter order)
/// come free from the `BTreeMap`.
fn merge_held(into: &mut edet_kernel::flow::Held, add: edet_kernel::flow::Held) {
    let mut edges: BTreeMap<(usize, usize), u64> = into.edges.iter().copied().collect();
    for (k, v) in add.edges {
        *edges.entry(k).or_insert(0) += v;
    }
    into.edges = edges.into_iter().collect();
    let mut supply: BTreeMap<usize, u64> = into.supply.iter().copied().collect();
    for (k, v) in add.supply {
        *supply.entry(k).or_insert(0) += v;
    }
    into.supply = supply.into_iter().collect();
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn sale(
    state: &mut State,
    seller: Party,
    buyer: Party,
    amount: f64,
    maturity_epochs: u64,
    signers: &[Key],
    sponsor: Option<MemberId>,
) -> Res<()> {
    let seller = resolve(state, &seller, signers)?;
    let buyer = resolve(state, &buyer, signers)?;
    if seller == buyer {
        return Err(Error(ET_CTR_SELF_DEAL));
    }
    // The seller's status gate is not a plain `require_active`: suspension
    // leaves discharge open (what the two statuses actually revoke),
    // and netting IS discharge — mutual
    // obligations extinguish rather than route — so a Suspended seller is
    // screened here only far enough to admit that later, once `nettable` is
    // known (below). Exited sellers get no such carve-out and are refused
    // right here, exactly as `require_active` always refused them. A seller
    // being seated by this very sale is Active by construction.
    let seller_status = match seller {
        Resolved::Existing(id) => {
            require_signed(state, id, signers)?;
            member(state, id)?.status
        }
        Resolved::Fresh(_) => MemberStatus::Active,
    };
    if !matches!(seller_status, MemberStatus::Active | MemberStatus::Suspended) {
        return Err(Error(ET_MEM_NOT_ACTIVE));
    }
    require_party(state, buyer, signers)?;
    if amount <= state.params.dust || !State::amount_representable(amount) {
        return Err(Error(ET_CTR_BAD_AMOUNT));
    }
    // THE BOUNDARY. Every figure below — the netting budget, the cascade's
    // drain shares, the remainder — is minor units, so the sale's arithmetic
    // closes exactly: `netted + spent + remaining == amount`, with no epsilon
    // anywhere and no share that the book cannot hold.
    let amount = State::to_minor(amount);
    if maturity_epochs < state.params.min_maturity_epochs {
        return Err(Error(ET_CTR_MATURITY_TOO_SHORT));
    }
    if maturity_epochs > k::MAX_HORIZON_EPOCHS {
        return Err(Error(ET_CTR_MATURITY_TOO_LONG));
    }
    // Mutual netting first: obligations the seller already owes the buyer
    // extinguish rather than route. Dry-computed, because `dispatch` has no
    // rollback and the suspension check below has to see the true remainder
    // before a single obligation is extinguished.
    //
    // A party this sale is about to seat has no contract book at all, so the
    // candidate set is empty by construction rather than by search — which is
    // also why the suspended branch below can only ever be reached with two
    // existing members: it needs `nettable` to cover the whole amount.
    let net_candidates: Vec<ContractId> = match (seller, buyer) {
        (Resolved::Existing(s), Resolved::Existing(b)) => state
            .contracts
            .iter()
            .filter(|(_, c)| {
                matches!(c.status, ContractStatus::Active | ContractStatus::Expired)
                    && c.debtor == s
                    && c.creditor == b
                    && c.outstanding > state.params.dust_minor()
            })
            .map(|(&id, _)| id)
            .collect(), // BTreeMap order = ascending id = oldest first.
        _ => Vec::new(),
    };
    let mut nettable: u64 = 0;
    for cid in &net_candidates {
        let left = amount.saturating_sub(nettable);
        if left <= state.params.dust_minor() {
            break;
        }
        if let Some(c) = state.contracts.get(cid) {
            nettable += c.outstanding.min(left);
        }
    }

    // Suspension leaves discharge open but revokes origination, and a sale's
    // netting step is discharge while everything past it — the cascade and
    // any genesis remainder toward the seller — is new credit the seller
    // would be originating. So a Suspended seller is admitted here exactly
    // as far as the dry pass above already shows will net: `amount <=
    // nettable + dust` means the post-netting remainder is the same
    // economically-zero dust the Active path below drops on the floor
    // anyway (see the `budget > dust` / `remaining > dust` gates), so there
    // is nothing left over to route or to mint as genesis debt. Any larger
    // request is refused outright — no partial sale that nets what it can
    // and originates the rest, because that would be exactly the "sell
    // through the cascade" origination suspension exists to stop. Nothing
    // has been mutated yet (the loop above only reads `state.contracts`),
    // so refusing here leaves state untouched.
    //
    // WRINKLE: `Sale` still carries bond multiple 1.0 like any other sale
    // (see `bond.rs::bond_multiple`) — a suspended seller's capacity is 0,
    // so this draws on the bonded party's free allowance rather than a
    // zero-bond path; the strictly-free discharge route remains the
    // standalone `Settle`/`Cure`. That's acceptable rather than a gap: the
    // free allowance (`bond_free_allowance`, 32/epoch) is sized for exactly
    // this volume of ordinary netting traffic, and `bond::due` — which must
    // stay a cheap, mutation-free read shared by the ingress screen and the
    // authoritative gate — cannot see whether a `Sale` will net fully
    // without redoing this same dry pass, ahead of the block that would
    // actually execute it.
    if seller_status == MemberStatus::Suspended && amount > nettable + state.params.dust_minor() {
        return Err(Error(ET_MEM_SUSPENDED_NO_ORIGINATION));
    }

    // Past every refusal, so this is where a row may appear — the same rule
    // `accept` follows, and the reason the dry pass above had to be dry.
    let (seller, buyer) = seat_pair(state, seller, buyer, sponsor)?;

    if seller_status == MemberStatus::Suspended {
        net_mutual(state, seller, buyer, amount, &net_candidates)?;
        return Ok(());
    }

    // No capacity GATE on the remainder, and none belongs here. Capacity
    // bounds what the community underwrites, not what a member may risk: the
    // successor obligations below reserve flow if it is there and are
    // uninsured if it is not, exactly as `Accept` is. A sale refused for want
    // of headroom would make capacity a permission again, and the bootstrap
    // it deadlocks is the same one §Recourse dissolves.
    let netted = net_mutual(state, seller, buyer, amount, &net_candidates)?;

    let mut visited: BTreeSet<MemberId> = BTreeSet::new();
    visited.insert(buyer);
    // Assumed obligations aggregate per original creditor and date.
    let mut assumed: Assumed = BTreeMap::new();
    // The sale's own date: what the genesis remainder matures at, and the
    // LATEST a routed claim may be re-dated to.
    let sale_maturity = crate::apply::horizon(state.epoch, maturity_epochs);

    let budget = amount - netted;
    let mut remaining = budget;
    if budget > state.params.dust_minor() {
        if member(state, seller)?.beneficiaries.is_empty() {
            // No listing: the whole sale clears the seller's own obligations,
            // oldest first.
            visited.insert(seller);
            let spent_own = clear_member_debts(state, seller, buyer, budget, sale_maturity, &mut assumed)?;
            remaining -= spent_own;
        } else {
            // The listing is the breakdown, self entry included: one waterfill
            // over {self share, beneficiaries}, recursing.
            let spent = drain_level(state, seller, buyer, budget, sale_maturity, 1, &mut visited, &mut assumed)?;
            remaining -= spent;
        }
    }

    // Book the successor obligations on the buyer, **against the reservations
    // the clearing loop already took for them**. No ordering rule is needed
    // any more: each row carries its own flow, taken at the moment its bound
    // was measured, so nothing booked here can consume what another was
    // promised. The insured-originals-first pass this replaces was the right
    // instinct applied one step too late — it ordered the bookings while the
    // seller's own re-hold had already run between the measurement and them.
    let maturity_epoch = sale_maturity;
    let slots: Vec<(MemberId, u64)> = assumed.keys().copied().collect();
    for (creditor, due) in slots {
        let (amt, held, accepted) = assumed.remove(&(creditor, due)).expect("key came from this map");
        if amt > state.params.dust_minor() {
            crate::apply::open_obligation(state, buyer, creditor, amt, due, None, Some(held), accepted)?;
        } else {
            // Nothing books, so nothing should hold flow for it.
            state.release_capacity(&held);
        }
    }
    // Genesis remainder toward the seller.
    if remaining > state.params.dust_minor() {
        book(state, buyer, seller, remaining, maturity_epoch, None)?;
    }
    Ok(())
}

/// Settle (or, for expired contracts, cure) the seller's obligations toward
/// the buyer, oldest first, up to `budget` — the netting step of a sale.
/// Mirrors the standalone Settle/Cure transitions: settlement evidence via
/// `discharge_credit`, open-default drains on cure, no transfer edge (this
/// is an extinguishment, not a routing). Returns the amount extinguished.
pub(crate) fn net_mutual(
    state: &mut State,
    seller: MemberId,
    buyer: MemberId,
    budget: u64,
    candidates: &[ContractId],
) -> Res<u64> {
    // The parties are the contracts' own — `candidates` is exactly the
    // seller's obligations toward the buyer, so `discharge_hop` reads them off
    // each row rather than being told.
    debug_assert!(candidates.iter().all(|cid| state
        .contracts
        .get(cid)
        .is_none_or(|c| c.debtor == seller && c.creditor == buyer)));
    let dust = state.params.dust_minor();
    let mut netted: u64 = 0;
    for &cid in candidates {
        let left = budget.saturating_sub(netted);
        if left <= dust {
            break;
        }
        netted += discharge_hop(state, cid, left)?;
    }
    Ok(netted)
}

/// **One hop of a netting, discharged by its own debtor toward its own
/// creditor.** The body both netting acts share.
///
/// Bilateral netting inside a sale walks the seller's obligations toward the
/// buyer; the epoch sweep walks a RING of defaults. The two differ in which
/// contracts they choose and in nothing else — each is an ordinary discharge:
/// the book moves, the row closes at dust, the reservation follows the debt
/// down by `rehold` rather than by a share guessed at from the arcs incident
/// to the debtor, and then the stake is written (`discharge_credit`, capped by
/// what the creditor may confer on the residual the payment leaves).
///
/// Returns what was actually taken, which is zero for a row already at dust.
pub(crate) fn discharge_hop(state: &mut State, cid: ContractId, want: u64) -> Res<u64> {
    let dust = state.params.dust_minor();
    let c = state.contracts.get(&cid).ok_or(Error(ET_CTR_UNKNOWN))?.clone();
    let take = want.min(c.outstanding);
    if take <= dust {
        return Ok(0);
    }
    let expired = c.status == ContractStatus::Expired;
    // The BOOK move, which is `take` plus whatever dust the close forgave —
    // `apply::settle` says why the cache may never follow the payment.
    let cleared = {
        let cm = state.contracts.get_mut(&cid).ok_or(Error(ET_CTR_UNKNOWN))?;
        cm.outstanding = cm.outstanding.saturating_sub(take);
        if cm.outstanding <= dust {
            cm.status = if expired { ContractStatus::Cured } else { ContractStatus::Settled };
            cm.outstanding = 0;
        }
        c.outstanding - cm.outstanding
    };
    {
        let d = member_mut(state, c.debtor)?;
        d.debt_out = d.debt_out.saturating_sub(cleared);
        d.rep.d_out += State::from_minor(take);
        if expired {
            d.rep.open_default = d.rep.open_default.saturating_sub(cleared);
        }
    }
    // The reservation follows the debt down before the stake is written —
    // `discharge_credit` says why the order is load-bearing — and the stake
    // reads the obligation's cumulative repayment, as every discharge does.
    crate::apply::rehold_public(state, cid)?;
    discharge_credit(state, c.debtor, c.creditor, c.original.saturating_sub(c.outstanding - cleared))?;
    Ok(take)
}

/// Clear up to `budget` of `debtor`'s Active obligations (oldest first,
/// partial allowed), assigning the assumed amounts to the buyer per original
/// creditor. Returns the amount cleared.
///
/// **This relieves; it does not rebuild, and that is the whole content of the
/// beneficiary's approval**. There is no `discharge_credit` here and
/// there must not be: every row this touches closes as `Transferred`, which is
/// a debtor swap, and **a stake is only ever placed by a transition the
/// creditor signed**. So the member whose debts are cleared ends the
/// sale owing less and backed by exactly as much as before, where honouring the
/// same obligation would have written the creditor's stake in full — measured,
/// 0 against 80, and capacity 200 against 380 over three rounds
/// (`tests/cascade.rs`). The seller's own share of their own cascade runs
/// through this same function and earns them nothing either, while
/// `net_mutual` — the identical debt discharged toward the buyer, who signed —
/// earns all of it. The paper said the opposite until it was measured.
fn clear_member_debts(
    state: &mut State,
    debtor: MemberId,
    buyer: MemberId,
    budget: u64,
    sale_maturity: u64,
    assumed: &mut Assumed,
) -> Res<u64> {
    let dust = state.params.dust_minor();
    let candidates: Vec<ContractId> = state
        .contracts
        .iter()
        .filter(|(_, c)| {
            c.status == ContractStatus::Active && c.debtor == debtor && c.creditor != buyer && c.outstanding > dust
        })
        .map(|(&id, _)| id)
        .collect(); // BTreeMap order = ascending id = oldest first.
    let mut spent: u64 = 0;
    for cid in candidates {
        let left = budget.saturating_sub(spent);
        if left <= dust {
            break;
        }
        let c = state.contracts.get(&cid).ok_or(Error(ET_CTR_UNKNOWN))?.clone();
        let mut take = c.outstanding.min(left);
        if take <= dust {
            continue;
        }
        // **A routed claim inherits the earlier of its own date and the
        // sale's** (below) AND its original's acceptance as horizon base, so
        // it can be carried insured only where that date is still inside the
        // horizon measured from that base. An original past it stays with its
        // debtor: it cannot move onto the buyer insured for a term its
        // underwriters were never asked for, nor be upgraded into one.
        let due = c.maturity_epoch.min(sale_maturity);
        if due > crate::apply::horizon(c.accepted_epoch, state.params.insured_horizon_epochs()) {
            continue;
        }
        // **A claim moves only as far as the buyer can carry it INSURED**, and
        // that is now the rule for every original rather than only the insured
        // ones. Clearing an obligation here is a debtor swap — the row closes
        // as `Transferred` — so it
        // owes the rule `Transfer` already states.
        //
        // The earlier reading let an UNINSURED original route freely, "since
        // its creditor never had recourse to lose, which is `Transfer`'s own
        // test". It is not `Transfer`'s test. `transfer` demands the creditor's
        // signature whenever the SUCCESSOR would be uninsured, whatever the
        // original was — one economic act, two code paths, opposite consent
        // rules, one tier further down. Measured: C lends S 300 uninsured,
        // `Transfer` to a fresh key B is refused without C (`ET-MEM-003`), and
        // `Sale S→B 300` signed by S and B alone was accepted — C left holding
        // 300 against an account with capacity 0 while S walked away with
        // `debt_out` 300 → 0.
        //
        // Bounding every original by the same measure answers both. Where the
        // buyer can carry it, the creditor is weakly UPGRADED — an uninsured
        // claim becoming insured is not a loss, so no signature is owed — and
        // where the buyer cannot, the claim simply does not move: the part that
        // stays keeps its original debtor, and the budget it did not absorb
        // falls through to the seller as the genesis remainder, a destination
        // the mechanism already had.
        let room = state.insurable_minor_after_release(buyer, take, &c.held);
        take = take.min(room);
        if take <= dust {
            continue;
        }
        // **Several cleared originals can join one successor row**, so its hold
        // is `Σ take_i` and so is its `outstanding` — and §Verification
        // invariant 2 compares the two exactly. They agree because both are
        // sums of the same integers: `take` is a whole number of minor units,
        // carried by the type rather than by a rounding step, and no `f64`
        // stands between the measurement and the book. With an `f64` there,
        // `to_minor(Σ x_i) != Σ to_minor(x_i)` in general — two takes of 1.005
        // hold 200 against a book of 201 — and that is a conservation failure
        // reported far from the line that caused it.
        //
        // **The measurement above is a promise, and this is where it is kept.**
        // Release the original's flow and take the buyer's reservation for
        // `take` NOW — before the seller's remainder is re-held below, and
        // before any later original in this same sale competes for the same
        // arcs. Booking the successors at the end of the sale instead left the
        // window the seller's own re-hold walked through: measured, with U2
        // (250) the buyer's ONLY source and U1 (300) behind the seller, the
        // re-held remainder took U2 first — Dinic scans underwriters in id
        // order — and the successor booked UNINSURED against a residual of
        // 200, downgrading the very claim the bound had just protected.
        //
        // Taking it here also retires the running `protected_total` that used
        // to thread through this loop: the reservations are real as they are
        // promised, so the next `insurable_after_release` reads a true residual
        // instead of one corrected by hand.
        state.release_capacity(&c.held);
        crate::apply::contract_mut(state, cid)?.held = Default::default();
        let Some(successor_held) = state.reserve_capacity_minor(buyer, take) else {
            // Unreachable: `room` just said the residual carries `take`, and
            // the release above only enlarged it. Restore the original's hold
            // and move on rather than panic — a validator may not panic on any
            // input, and skipping one original is a liveness cost, not a
            // safety one.
            crate::apply::rehold_public(state, cid)?;
            continue;
        };
        // The BOOK move, which is `take` plus whatever dust the close forgave
        // — `apply::settle` says why the cache may never follow the payment.
        // The successor assumes `take`; the forgiven remainder is assumed by
        // nobody, which is exactly what "the book moved further than the
        // payment" means and is why the two figures are read apart here.
        let cleared = {
            let cm = state.contracts.get_mut(&cid).ok_or(Error(ET_CTR_UNKNOWN))?;
            cm.outstanding = cm.outstanding.saturating_sub(take);
            if cm.outstanding <= dust {
                cm.status = ContractStatus::Transferred;
                cm.outstanding = 0;
            }
            c.outstanding - cm.outstanding
        };
        {
            let d = member_mut(state, c.debtor)?;
            d.debt_out = d.debt_out.saturating_sub(cleared);
            d.rep.d_out += State::from_minor(take);
        }
        // The seller's remainder takes what is left, AFTER the successor. Its
        // own hold is already empty, so this reserves rather than re-reserves.
        crate::apply::rehold_public(state, cid)?;
        // **A routed claim inherits the earlier of its own date and the
        // sale's.** It is a debtor swap, and `move_debtor` refuses to re-date
        // a transfer for a reason that holds here word for word: two
        // colluding members handing a debt back and forth would push its
        // maturity out of reach for ever, insured throughout, so no default
        // ever fires and no substitution ever lands. Booked at the SALE's
        // date, a claim due in thirty epochs matured ten thousand epochs out
        // on the signatures of two other members — the creditor signs a sale
        // nowhere. The earlier date is the one under which neither party who
        // signed anything is worse off: the creditor is paid no later than
        // the original said, and the buyer no later than the sale they signed.
        // One row per creditor and date, carrying the reservation the loop took
        // for it. Every successor is insured, so there is nothing else for the
        // key to separate.
        let slot = assumed.entry((c.creditor, due)).or_insert((0, Default::default(), u64::MAX));
        slot.0 += take;
        merge_held(&mut slot.1, successor_held);
        slot.2 = slot.2.min(c.accepted_epoch);
        spent += take;
    }
    Ok(spent)
}

/// What this pair has staked in one another, either way round: the realized,
/// capped, decaying record of a trading relationship, and the only quantity
/// available here that an attacker cannot fabricate.
fn pair_stake(state: &State, a: MemberId, b: MemberId) -> u64 {
    let get = |x: MemberId, y: MemberId| state.edges.get(&(x as usize, y as usize)).copied().unwrap_or(0);
    get(a, b).saturating_add(get(b, a))
}

/// Waterfill `budget` across `lister`'s approved beneficiaries; clear their
/// debts; pass each target's unused allocation one level deeper. Returns the
/// amount actually absorbed.
#[allow(clippy::too_many_arguments)]
fn drain_level(
    state: &mut State,
    lister: MemberId,
    buyer: MemberId,
    budget: u64,
    sale_maturity: u64,
    depth: u32,
    visited: &mut BTreeSet<MemberId>,
    assumed: &mut Assumed,
) -> Res<u64> {
    if depth > k::MAX_CASCADE_DEPTH || budget <= state.params.dust_minor() {
        return Ok(0);
    }
    // Eligible targets: listed, approved, not yet drained this sale.
    let entries: Vec<(MemberId, f64)> = member(state, lister)?.beneficiaries.iter().map(|(&b, &w)| (b, w)).collect();
    let mut targets: Vec<(MemberId, u64, f64)> = Vec::new(); // (id, cap in minor, weight)
    for (b, w) in entries {
        if visited.contains(&b) || w <= 0.0 {
            continue;
        }
        if b == lister {
            // The self entry: no approval, no relationship cap — its
            // absorbency is the lister's own outstanding debt.
            let own = member(state, lister)?.debt_out;
            targets.push((b, own, w));
            continue;
        }
        let bm = match state.members.get(&b) {
            Some(m) => m,
            None => continue,
        };
        if !bm.approved_supporters.contains(&lister) {
            continue;
        }
        // Drain cap per edge: `ν` times what this pair has actually staked in
        // each other.
        //
        // The old cap read a decayed counter of settled volume between the
        // pair. Volume is free to fabricate — settlement takes two signatures
        // and no delivery — so that counter was a quantity an attacker could
        // write, and two colluding accounts could open the drain as wide as
        // they liked. The stake graph answers the same question and cannot be
        // written for free: an edge is capped by what the creditor may confer,
        // so between two accounts that may confer nothing it is exactly zero
        // however much passes between them.
        //
        // There is deliberately NO floor. A pair with no settled history
        // drains nothing, which is the same rule the rest of the model runs
        // on: standing is evidence of having owed and paid, and an approval
        // with no trade behind it is evidence of nothing. Zero is absorbing
        // here too, and it is escaped the same way — by trading.
        // ν is applied as a ratio to the stake in minor units, so the cap
        // stays on the grid the stake is already on: scaling through an `f64`
        // and back was a third crossing of the boundary the ledger crosses
        // exactly twice.
        let cap = (pair_stake(state, lister, b) as u128 * k::NU_DRAIN_NUM as u128 / k::NU_DRAIN_DEN as u128)
            .min(u64::MAX as u128) as u64;
        targets.push((b, cap, w));
    }
    if targets.is_empty() {
        return Ok(0);
    }
    let alloc = waterfill_minor(&targets.iter().map(|&(_, cap, w)| (cap, w)).collect::<Vec<_>>(), budget);
    let mut absorbed: u64 = 0;
    for (i, &(b, _, _)) in targets.iter().enumerate() {
        let share = alloc[i];
        if share <= state.params.dust_minor() {
            continue;
        }
        // A deeper recursion of an earlier target may have drained `b`
        // already; never process a member twice in one sale.
        if !visited.insert(b) {
            continue;
        }
        let cleared = clear_member_debts(state, b, buyer, share, sale_maturity, assumed)?;
        absorbed += cleared;
        // The self entry does not recurse: its unused share flows back to
        // the seller as the genesis remainder (they simply get paid).
        if b != lister {
            let leftover = share - cleared;
            if leftover > state.params.dust_minor() {
                absorbed += drain_level(state, b, buyer, leftover, sale_maturity, depth + 1, visited, assumed)?;
            }
        }
    }
    Ok(absorbed)
}
