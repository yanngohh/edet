//! Operation bonds: the admission gate, the release cycle, the recovery
//! exemption, and the forfeiture crank.
//!
//! A bond is *reserved*, never collected. Nothing here may end with a party
//! better off for another member's traffic, or the mechanism has become a fee
//! — which is the one thing it must never be. The state is audited after every
//! transition, so conservation is re-checked implicitly by every case below.

mod common;

use common::{bonded_dud, key, stranger_key, Chain, MATURITY, SUPPLY};

/// A key belonging to no account yet — what a newcomer is named by.
fn sk(n: u64) -> Key {
    let mut k = [0xA0u8; 32];
    k[8..16].copy_from_slice(&n.to_be_bytes());
    k
}
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

// ---------------------------------------------------------------- the gate --

/// The headline property: ordinary honest traffic never encumbers anything,
/// because it never leaves the free allowance. If this fails, the mechanism
/// has become a fee on normal use.
#[test]
fn honest_traffic_never_touches_a_bond() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    let headroom = c.st.bond_headroom(1);
    for _ in 0..c.st.params.bond_free_allowance {
        let _ = c.apply(bonded_dud(), &[key(1)]);
    }
    assert_eq!(c.st.members[&1].bond_enc(), 0, "traffic inside the allowance must encumber nothing");
    assert_eq!(c.st.bond_headroom(1), headroom, "headroom must be untouched");
}

/// A transaction that fails on its own merits must STILL pay its bond, and the
/// gate must eventually deny. Without the first half, designed-to-fail traffic
/// is free and the whole mechanism is bypassed by submitting garbage: a
/// consensus round and a durable write per message, priced at nothing.
#[test]
fn a_failing_transition_still_pays_its_bond_and_the_gate_eventually_denies() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.tighten();
    assert!(c.st.bond_headroom(1) > 0.0, "the fixture needs real headroom to spend");

    let admitted = c.exhaust(key(1));
    assert!(admitted > 0, "some duds must be admitted before the gate closes");
    assert!(c.st.members[&1].bond_enc() > 0, "failed transactions must still encumber");
    assert!(
        c.st.bond_headroom(1) < c.st.params.bond_unit(),
        "headroom must be spent down below one further bond, not merely reduced"
    );
}

/// The denial is the EXISTING capacity ceiling, not a second independent
/// limit: outstanding debt and encumbrance compete for the same headroom, so a
/// member carrying debt is denied strictly sooner.
#[test]
fn debt_and_encumbrance_share_one_ceiling() {
    let free_run = {
        let mut c = Chain::founded(1, 1);
        c.back(0, 1, SUPPLY);
        c.tighten();
        c.exhaust(key(1))
    };

    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.lend(0, 1, 1000.0);
    c.tighten();
    let indebted_run = c.exhaust(key(1));

    assert!(
        indebted_run < free_run,
        "debt must consume bond headroom: {indebted_run} admitted with debt against {free_run} without"
    );
}

/// The allowance is what stops the gate becoming a wall around the very
/// transitions that would clear a member's own encumbrance. A member with
/// standing but no headroom left — debt has eaten all of it — still writes,
/// up to `A` times per epoch, and then stops.
#[test]
fn the_allowance_carries_a_member_whose_headroom_is_spent() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    // The WHOLE of what the seed reaches them for. A residual reading takes 2000 of debt
    // to spend 2500 of headroom, because the residual reach fell by the amount
    // and then `- debt_out` took the same amount again; the scene was written at
    // the point where that did not show.
    c.lend(0, 1, SUPPLY);
    assert!(c.st.seed_reach(1) > c.st.params.dust, "they still have standing — the seed still reaches them");
    assert_eq!(c.st.conferrable(1), 0.0, "and none of it is free credit");
    assert_eq!(c.st.bond_headroom(1), 0.0, "and none of it free to encumber");

    assert_eq!(
        c.apply(bonded_dud(), &[key(1)]),
        Err(Error(ET_CTR_UNKNOWN)),
        "admitted by the allowance, then failed on its own merits"
    );
    assert_eq!(c.st.members[&1].bond_enc(), 0, "allowance traffic encumbers nothing");

    c.st.members.get_mut(&1).unwrap().bond_free_used = c.st.params.bond_free_allowance;
    assert_eq!(c.apply(bonded_dud(), &[key(1)]), Err(Error(ET_BOND_EXHAUSTED)), "and the allowance is finite");
}

/// **The allowance is not a per-key entitlement.** A key nobody has backed has
/// no standing, so it is not established, so it gets no allowance at all — a
/// per-key one would be the free-signature bound's own defect a layer down,
/// since keys are free and N of them would carry N allowances.
#[test]
fn a_key_nobody_backed_gets_no_allowance() {
    let mut c = Chain::founded(1, 1);
    assert_eq!(c.st.conferrable(1), 0.0);
    assert_eq!(c.apply(bonded_dud(), &[key(1)]), Err(Error(ET_BOND_EXHAUSTED)));
    assert_eq!(c.st.members[&1].bond_free_used, 0, "a refusal must not spend an allowance slot either");
}

/// Bonds are reserved, not collected: they must come back on schedule, and the
/// member must be able to act again once they do.
#[test]
fn bonds_release_and_restore_headroom() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.tighten();
    c.st.params.bond_release_epochs = 2;

    let headroom0 = c.st.bond_headroom(1);
    c.exhaust(key(1));
    assert!(c.st.members[&1].bond_enc() > 0);

    c.goto(1);
    assert!(c.st.members[&1].bond_enc() > 0, "a bond must not release early with T_b = 2");

    c.goto(2);
    assert_eq!(c.st.members[&1].bond_enc(), 0, "every bond must return once its window elapses");
    // Compared against PRESENT standing, not against `headroom0`. Two epochs
    // of decay have passed and the stake behind this member is legitimately
    // smaller than it was; what the release must restore is the whole of the
    // encumbrance, which is exactly "headroom is once again all of what they
    // may confer". Pinning the old figure would make this a decay test that
    // fails for the wrong reason.
    assert!(headroom0 > c.st.bond_headroom(1), "decay has moved standing, so the absolute figure is not the claim");
    assert!(
        (c.st.bond_headroom(1) - c.st.conferrable(1)).abs() < 1e-9,
        "nothing may stay encumbered: {} against a conferrable of {}",
        c.st.bond_headroom(1),
        c.st.conferrable(1)
    );
    assert_eq!(c.apply(bonded_dud(), &[key(1)]), Err(Error(ET_CTR_UNKNOWN)), "and the member writes again");
}

/// The exemption the whole design rests on. A member whose headroom is gone
/// must still be able to discharge and leave, or a bond could strand a member
/// inside their own default and turn a recoverable failure into an absorbing
/// one — strictly worse than the unbonded system this replaces.
#[test]
fn the_recovery_path_stays_open_at_full_saturation() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let cid = c.lend(2, 1, 200.0);
    c.tighten();

    c.exhaust(key(1));
    assert_eq!(c.apply(bonded_dud(), &[key(1)]), Err(Error(ET_BOND_EXHAUSTED)), "past the ceiling for bonded classes");

    c.settle(cid, 200.0);
    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Settled, "settlement is free, and works");

    c.st.members.get_mut(&1).unwrap().bonds.clear();
    c.ok(Tx::Exit { member: 1 }, &[key(1)]);
    assert_eq!(c.status(1), MemberStatus::Exited, "so is leaving");
}

/// A default consumes the defaulter's whole standing — that is the sanction —
/// so if `Cure` were bonded it would be unaffordable exactly when it matters
/// most, and the one transition out of the hole would be priced by the hole.
#[test]
fn an_open_default_never_locks_a_member_out_of_curing_it() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    let cid = c.lend(0, 1, SUPPLY);
    c.default_on(cid);

    assert!(c.st.members[&1].rep.open_default > 0, "the fixture needs a live default");
    assert_eq!(c.cap(1), 0.0, "a default does not release the flow it committed");
    assert_eq!(c.st.bond_headroom(1), 0.0, "so there is no headroom to bond with at all");
    assert_eq!(c.apply(bonded_dud(), &[key(1)]), Err(Error(ET_BOND_EXHAUSTED)), "every bonded class is refused");

    c.ok(Tx::Cure { contract: cid, amount: SUPPLY }, &[key(0), key(1)]);
    assert!(c.st.members[&1].rep.open_default <= c.st.params.dust_minor(), "but the cure lands");
    assert_eq!(c.cap(1), SUPPLY, "and the standing comes back with the debt it was behind");
}

// -------------------------------------------------------------- forfeiture --

/// Forfeiture is a pure state check on sustained exhaustion — it cannot be
/// forged against a member that merely had a busy afternoon, which is what
/// makes a permissionless crank safe to leave permissionless and safe to run
/// automatically.
#[test]
fn forfeiture_requires_sustained_exhaustion() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.tighten();

    c.exhaust(key(1));
    c.err(Tx::ForfeitBonds { member: 1 }, &[], ET_BOND_NOT_SATURATED);
    for e in 1..c.st.params.bond_forfeit_epochs {
        c.goto(e);
        let _ = c.apply(bonded_dud(), &[key(1)]);
        c.err(Tx::ForfeitBonds { member: 1 }, &[], ET_BOND_NOT_SATURATED);
        assert!(c.st.forfeit_reserve.is_empty(), "and the sweep must not fire early either");
    }
}

/// An honest burst — throttled in one epoch, clear the next — must never
/// accumulate toward a sanction. The reset at the boundary is what makes the
/// counter measure *sustained* exhaustion rather than lifetime busyness.
#[test]
fn an_intermittent_burst_never_reaches_forfeiture() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.tighten();
    c.st.params.bond_release_epochs = 1; // bonds return each epoch

    for e in 0..8u64 {
        c.goto(e);
        if e % 2 == 0 {
            c.exhaust(key(1));
        }
        assert!(
            c.st.members[&1].bond_saturated_epochs < c.st.params.bond_forfeit_epochs,
            "an alternating pattern must never accumulate: epoch {e} reached {}",
            c.st.members[&1].bond_saturated_epochs
        );
    }
    c.goto(8);
    assert!(c.st.forfeit_reserve.is_empty(), "so nothing is ever forfeited");
    c.err(Tx::ForfeitBonds { member: 1 }, &[], ET_BOND_NOT_SATURATED);
}

/// **The epoch sweep fires the sanction, and fires it once.** Nobody has to
/// notice sustained exhaustion and nobody gets to choose when it counts — the
/// counter reaching the window IS the event, and the crank that follows resets
/// it, so the sanction is once per episode rather than once per block for as
/// long as the episode lasts.
#[test]
fn sustained_exhaustion_forfeits_the_encumbrance_once() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.tighten();
    let window = c.st.params.bond_forfeit_epochs;

    c.exhaust(key(1));
    for e in 1..window {
        c.goto(e);
        assert_eq!(c.apply(bonded_dud(), &[key(1)]), Err(Error(ET_BOND_EXHAUSTED)));
    }
    let held = c.st.members[&1].bond_enc();
    assert!(held > 0, "the abuser must be holding encumbrance to forfeit");

    c.goto(window);
    assert_eq!(c.st.members[&1].bond_enc(), 0, "the sweep takes it, unasked");
    assert!(
        c.st.forfeit_reserve.get(&1).copied().unwrap_or(0) == held,
        "and the forfeited amount lands in the reserve intact"
    );
    c.err(Tx::ForfeitBonds { member: 1 }, &[], ET_BOND_NOT_SATURATED);
    let reserved = c.st.forfeit_reserve[&1];
    c.goto(window + 1);
    assert_eq!(c.st.forfeit_reserve[&1], reserved, "and a later boundary does not take it again");
}

/// Forfeiture credits nobody at the moment it happens: no balance moves, no
/// contract is minted, total outstanding debt is unchanged. This is the whole
/// reason it is not a fee — no position anywhere improves.
#[test]
fn forfeiture_itself_credits_no_one() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.tighten();
    let window = c.st.params.bond_forfeit_epochs;
    c.exhaust(key(1));
    for e in 1..window {
        c.goto(e);
        assert_eq!(c.apply(bonded_dud(), &[key(1)]), Err(Error(ET_BOND_EXHAUSTED)));
    }
    let (debt_before, contracts_before) = (c.total_debt(), c.st.contracts.len());

    c.goto(window);
    assert!(!c.st.forfeit_reserve.is_empty(), "the fixture must actually have forfeited something");

    assert!((c.total_debt() - debt_before).abs() < 1e-9, "forfeiture must not mint debt");
    assert_eq!(c.st.contracts.len(), contracts_before, "forfeiture must not mint a contract");
}

/// A crank against a member holding nothing is refused rather than recorded:
/// the sanction is the loss of an encumbrance, and there is no such thing as
/// forfeiting zero. Reachable only by hand now — the sweep never leaves a
/// saturated member holding anything to take.
#[test]
fn forfeiting_nothing_is_refused() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    let window = c.st.params.bond_forfeit_epochs;
    c.st.members.get_mut(&1).unwrap().bond_saturated_epochs = window;
    c.err(Tx::ForfeitBonds { member: 1 }, &[], ET_BOND_NOTHING_HELD);
}

// ------------------------------------------------------------- forfeiture --

/// **Forfeiture fires at GENESIS parameters.**
///
/// `release_bonds` ran inside `begin_block`'s epoch loop while the sweep ran
/// after it, so by the time `ForfeitBonds` looked there was nothing held — and at
/// the genesis release period of one epoch that made the sanction unreachable
/// outright. Measured with the release inside the loop: a member saturated for **seven**
/// consecutive epochs, re-exhausting its headroom in every one of them, with
/// `forfeit_reserve` empty throughout and 21 to 24 writes admitted per epoch for
/// ever. It had been said that "the sweep never forfeits at genesis parameters"
/// and this is that, at the quantifier — the counter climbs, and nothing happens.
#[test]
fn forfeiture_fires_at_the_genesis_release_period() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    // Genesis bond parameters, except the allowance: with 32 free writes an
    // epoch the gate never engages inside a short test.
    c.st.params.bond_free_allowance = 0;
    c.st.params.bond_fraction = 0.10;
    assert_eq!(c.st.params.bond_release_epochs, 1, "the fixture is about the GENESIS release period");
    let threshold = c.st.params.bond_forfeit_epochs;

    c.exhaust(key(1));
    for e in 1..threshold {
        c.goto(e);
        c.exhaust(key(1));
        assert!(c.st.forfeit_reserve.is_empty(), "nothing is forfeited before the threshold (epoch {e})");
    }
    c.goto(threshold);

    let forfeited =
        c.st.forfeit_reserve
            .get(&1)
            .copied()
            .expect("sustained saturation must forfeit");
    // The headroom does not come back: what is left is whatever the member's
    // (decayed) standing exceeds the forfeit by, which is a sliver rather than
    // the whole encumbrance the old code handed over.
    assert!(
        c.st.bond_headroom_minor(1) + forfeited <= c.st.conferrable_minor(1),
        "the forfeited amount must be netted out of the headroom: {} + {} against {}",
        c.st.bond_headroom(1),
        forfeited,
        c.st.conferrable(1)
    );
    let after = c.exhaust(key(1));
    assert!(after <= 1, "the abuser's write rate collapses instead of resuming: {after} admitted");
}

/// **A forfeiture is a reservation that never returns.** It must not call
/// `m.bonds.clear()` and hand the headroom straight back — measured, an abuser
/// forfeited 2500, its headroom went 0 → 2231 and it immediately wrote 22 more
/// transitions. `bond_headroom` now nets `forfeit_reserve` out, which is also
/// what gives that field a consumer again: it was the retired loss pool's
/// first-loss layer (§Recourse) and nothing read it in between.
///
/// Still no rent: nothing is minted, nobody is owed it, and total debt does not
/// move — which is the property that separates this from a fee.
#[test]
fn a_forfeiture_takes_the_headroom_rather_than_returning_it() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.tighten();
    c.exhaust(key(1));
    let (debt_before, contracts_before) = (c.total_debt(), c.st.contracts.len());

    for e in 1..=c.st.params.bond_forfeit_epochs {
        c.goto(e);
        let _ = c.apply(bonded_dud(), &[key(1)]);
    }

    let forfeited = c.st.forfeit_reserve.get(&1).copied().expect("the fixture must have forfeited");
    assert!(forfeited > 0);
    assert_eq!(c.st.bond_headroom(1), 0.0, "the forfeited encumbrance is not handed back");
    assert!((c.total_debt() - debt_before).abs() < 1e-9, "and it mints nothing");
    assert_eq!(c.st.contracts.len(), contracts_before, "no obligation, to anybody");
}

/// **The way back is the model's own: outgrow the forfeit.** The penalty is an
/// absolute amount of encumbrance that never returns, so a member recovers a
/// write channel only by earning standing beyond it — and decay works against
/// them while they do. What keeps that from being absorbing is the pair of
/// exemptions either side of it: the free allowance still applies (a forfeited
/// member's `conferrable` is above dust, so it has something to lose), and the
/// recovery path is priced at zero.
///
/// Backed by a SECOND underwriter here, because a member already drawing its
/// first underwriter's whole supply has nothing left to outgrow it with —
/// `record_stake` keeps the maximum, it does not add.
#[test]
fn a_member_who_outgrows_its_forfeit_writes_again() {
    let mut c = Chain::founded(2, 1);
    // Backed by U0 alone, and thinly — but above the bond unit, or the member
    // could never encumber anything to forfeit in the first place.
    c.back(0, 2, 400.0);
    c.tighten();
    assert!(c.st.params.bond_unit() < 400.0, "the fixture must be able to take a bond at all");
    c.exhaust(key(2));
    for e in 1..=c.st.params.bond_forfeit_epochs {
        c.goto(e);
        let _ = c.apply(bonded_dud(), &[key(2)]);
    }
    let forfeited = c.st.forfeit_reserve.get(&2).copied().expect("must have forfeited");
    assert_eq!(c.st.bond_headroom(2), 0.0, "throttled to nothing by its own abuse");

    // U1 backs it for far more than was taken.
    c.back(1, 2, SUPPLY);
    assert!(
        c.st.bond_headroom(2) > 0.0,
        "a member whose standing grows past the forfeit ({forfeited}) writes again: headroom {}",
        c.st.bond_headroom(2)
    );
    assert!(
        c.st.forfeit_reserve.get(&2).copied().unwrap_or(0) >= forfeited,
        "and the forfeit itself is not forgiven by the growth"
    );
}

/// The recovery path stays free for a forfeited member, and that matters more
/// now than it did: with the headroom gone for good, a priced `Cure` or `Settle`
/// would make the sanction absorbing — a member unable to afford to discharge
/// what it owes. `bond::due` prices those at zero and this is the end-to-end
/// check of it.
#[test]
fn a_forfeited_member_can_still_discharge_what_it_owes() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let owed = c.lend(2, 1, 100.0);
    c.tighten();
    c.exhaust(key(1));
    for e in 1..=c.st.params.bond_forfeit_epochs {
        c.goto(e);
        let _ = c.apply(bonded_dud(), &[key(1)]);
    }
    assert!(c.st.forfeit_reserve.contains_key(&1));
    assert_eq!(c.st.bond_headroom(1), 0.0);

    c.ok(Tx::Settle { contract: owed, amount: 100.0 }, &[key(1), key(2)]);
    assert_eq!(c.st.contracts[&owed].status, ContractStatus::Settled, "discharge is free even with no headroom");
}

/// An honest burst never forfeits, however often it happens. The saturation
/// counter resets outright in any epoch the member was not denied, so the
/// sanction reaches only sustained, deliberate exhaustion — throttled one epoch
/// and clear the next, for ever, is not an abuse.
#[test]
fn an_honest_burst_never_forfeits() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.st.params.bond_free_allowance = 0;
    c.st.params.bond_fraction = 0.10;
    for e in 0..4 * c.st.params.bond_forfeit_epochs {
        c.goto(e);
        if e % 2 == 0 {
            c.exhaust(key(1)); // a burst that hits the gate
        }
        // and an epoch that does not
    }
    assert!(c.st.forfeit_reserve.is_empty(), "an alternating burst must never reach the sanction");
}

/// Moving `release_bonds` out of the epoch loop must not change WHICH bonds
/// release. A block that closes several epochs at once has to hand back
/// everything due along the way, not just what fell due in the last one — the
/// same property the sweep's own once-after-the-loop placement rests on.
#[test]
fn one_release_pass_hands_back_every_epoch_the_block_closed() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.st.params.bond_free_allowance = 0;
    c.st.params.bond_fraction = 0.001; // small bonds, so many are admitted
    c.st.params.bond_release_epochs = 3;
    let _ = c.apply(bonded_dud(), &[key(1)]);
    assert!(c.st.members[&1].bond_enc() > 0, "the fixture must hold a bond");

    // One block, five epochs closed at once.
    c.goto(5);
    assert_eq!(c.st.members[&1].bond_enc(), 0, "a bond due three epochs ago is released by the single pass");
    assert!(c.st.forfeit_reserve.is_empty(), "and nothing was forfeited on the way: it was never denied");
}

// ------------------------------------------------------- governance & scale --

/// Re-denomination must carry encumbrance and the reserve, or a bond would
/// silently change size in real terms at every rescale — and the bond unit
/// must track `v_base` through the fraction, without a rescale entry of its
/// own.
#[test]
fn redenomination_carries_bonds_and_the_reserve() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.tighten();
    c.exhaust(key(1));
    c.st.forfeit_reserve.insert(2, State::to_minor(40.0));

    let enc_before = c.st.members[&1].bond_enc();
    let unit_before = c.st.params.bond_unit();
    c.st.rescale(2.0);

    assert!(c.st.members[&1].bond_enc() == 2 * enc_before, "encumbrance must rescale");
    assert!(c.st.forfeit_reserve[&2] == State::to_minor(80.0), "the reserve must rescale");
    assert!((c.st.params.bond_unit() - 2.0 * unit_before).abs() < 1e-9, "and so must the bond unit");
}

/// The traffic bound, measured rather than asserted: what a
/// member can force through in one epoch is capped by its allowance plus its
/// own headroom divided by the bond — and a key with no earned standing
/// contributes only the allowance, which for an unbacked key is zero.
#[test]
fn forced_traffic_is_bounded_by_allowance_plus_earned_headroom() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.tighten();

    let headroom = c.st.bond_headroom(1);
    let bond = c.st.params.bond_unit();
    let admitted = c.exhaust(key(1)) as f64;
    let bound = (headroom / bond).floor();
    assert!(admitted <= bound + 1e-9, "admitted {admitted} must not exceed the bound {bound}");
    assert!(admitted >= bound - 1.0, "and the bound must be tight, not vacuous: {admitted} against {bound}");

    // The second term is zero for a key nobody has backed, and so is the
    // first: a Sybil contributes nothing at all to a coalition's total.
    assert_eq!(c.st.bond_headroom(2), 0.0);
    assert_eq!(c.apply(bonded_dud(), &[key(2)]), Err(Error(ET_BOND_EXHAUSTED)));
}

// ------------------------------------------------------------ the write floor --

/// The construction: one honest round-trip seeds the head of a chain,
/// and every accomplice after it is fake-backed by ALL the previous ones and then
/// tries to declare the maximum its own capacity would carry. Nothing in it is
/// forged — every settlement takes the two signatures it is supposed to take.
/// Returns the chain in order.
///
/// **Every declaration in it is refused**, because a supply is seated by a
/// ceremony and by nothing else. The helper asserts the refusal rather than
/// skipping the call: the coalition's whole leverage is that the call SUCCEEDS,
/// so a version that simply stopped declaring would stay green if the door
/// opened.
fn chain_of_accomplices(c: &mut Chain, founders: usize, honest: f64, links: usize) -> Vec<MemberId> {
    let mut js = Vec::new();
    let j1 = founders as MemberId;
    c.back(0, j1, honest);
    js.push(j1);
    c.err(Tx::DeclareSupply { member: j1, supply: c.cap(j1) }, &[key(j1 as usize)], ET_UWR_ABOVE_CAPACITY);
    for n in 1..links {
        let j = (founders + n) as MemberId;
        for &prev in &js {
            c.back(prev, j, honest * 4096.0);
        }
        let cap = c.cap(j);
        c.err(Tx::DeclareSupply { member: j, supply: cap }, &[key(j as usize)], ET_UWR_ABOVE_CAPACITY);
        js.push(j);
    }
    js
}

/// **The write floor is measured against the seed's reach, and the reason is a
/// growth rate.**
///
/// `bond_headroom` read `conferrable`, which for an underwriter is the DECLARED
/// supply — a promise made on the strength of another promise. Each declaration
/// becomes a source arc feeding the next member's capacity, which becomes their
/// declaration, so the coalition's write headroom **doubled with every
/// accomplice**: measured on that reading, 2,400 at four links, 19,200 at seven,
/// 153,600 at ten and **614,400 at twelve**, with the real cut over the whole set
/// pinned at **300** throughout. Accounts are free, so that was not a 64×
/// amplification — it is an unbounded write channel, and it makes "traffic
/// is bounded by earned standing" vacuous. The earlier pass had measured the single
/// point at seven links and called it a decision.
///
/// Now every member of the chain holds exactly the 300 the honest trade reaches
/// them for, so the total is linear in the number of accounts rather than
/// exponential.
///
/// **There are two refusals in the way, and this probe holds both.**
/// The declarations that fed the exponential are refused outright, so the
/// coalition's `conferrable` is its real capacity and nothing more. And the
/// write floor still reads the seed's reach rather than the declaration — which
/// is what has to keep being true if a declaration ever becomes raisable again
/// by some route nobody has thought of yet.
#[test]
fn the_write_floor_does_not_follow_a_chain_of_declarations() {
    for links in [4usize, 7, 10, 12] {
        let mut c = Chain::founded(6, 24);
        let js = chain_of_accomplices(&mut c, 6, 300.0, links);
        c.st.params.bond_free_allowance = 0;
        c.st.params.bond_fraction = 0.10;

        // The construction, pinned: the chain declares nothing at all now, and
        // what each member may confer is the one honest trade.
        let declared: f64 = js.iter().map(|&j| c.st.conferrable(j)).sum();
        assert_eq!(
            declared,
            300.0 * links as f64,
            "at {links} links the coalition may confer 300 apiece, not 2,400 at four and 614,400 at twelve"
        );
        assert_eq!(c.st.capacity_of_set(&js), 300.0, "one honest trade is the whole coalition's real capacity");

        let headroom: f64 = js.iter().map(|&j| c.st.bond_headroom(j)).sum();
        assert_eq!(
            headroom,
            300.0 * links as f64,
            "at {links} links the write floor must be 300 per member, not {declared} of declared supply"
        );
        for &j in &js {
            assert_eq!(c.st.bond_headroom(j), 300.0, "member {j} writes against its real reach");
        }
    }
}

/// And the two members whose write channel must NOT move: a founding underwriter,
/// whose capacity is zero and who could not write at all if this read capacity;
/// and an honest member, who keeps exactly what the community put behind them.
/// This is the standing objection to moving the write floor to the seed,
/// answered — the quantity is the max-flow OVER the seeded arcs, not the seed.
#[test]
fn the_seed_reach_leaves_a_founder_and_an_honest_member_where_they_were() {
    let mut c = Chain::founded(6, 4);
    let honest = 7u64;
    c.back(0, honest, 300.0);

    assert_eq!(c.cap(0), 0.0, "a founding underwriter's capacity is zero — the objection to reading it");
    assert_eq!(c.st.seed_reach(0), SUPPLY, "and its seed reach is the whole of what the ceremony seated");
    assert_eq!(c.st.bond_headroom(0), SUPPLY, "so a founder writes exactly as before");

    assert_eq!(c.st.conferrable(honest), 300.0);
    assert_eq!(c.st.seed_reach(honest), 300.0, "an honest member's reach IS their capacity");
    assert_eq!(c.st.bond_headroom(honest), 300.0);
}

/// **1.00×, at the set quantifier.** k accomplices reached through one honest edge
/// must write exactly what k honestly-backed members write — which is the
/// standard the rest of the model holds itself to, and the reason the residual
/// per-member over-count is inherent rather than a leftover: the honest case
/// over-counts identically.
#[test]
fn a_coalition_writes_exactly_what_the_same_number_of_honest_members_writes() {
    for k in [3usize, 7, 12] {
        let coalition: f64 = {
            let mut c = Chain::founded(6, 24);
            let js = chain_of_accomplices(&mut c, 6, 300.0, k);
            js.iter().map(|&j| c.st.bond_headroom(j)).sum()
        };
        let honest: f64 = {
            let mut c = Chain::founded(6, 24);
            let hs: Vec<MemberId> = (6..6 + k as u64).collect();
            for &h in &hs {
                c.back(0, h, 300.0);
            }
            hs.iter().map(|&h| c.st.bond_headroom(h)).sum()
        };
        assert_eq!(coalition, honest, "at k = {k}: {coalition} against {honest}");
    }
}

/// **No capacity above the seed's reach exists to buy writes with any more**,
/// and this is the probe that measures the gap is closed.
///
/// A member backed generously by an accomplice whose own supply was a promise on
/// promises held a real capacity of **1,200** while the seed reached them for
/// **300**, and the write floor reads the 300. The gap is closed where it opens:
/// the accomplice can declare nothing, so the stake it places is capped by its
/// own capacity, and the member behind it is at 300 by every reading. The probe
/// keeps both — the figure, so the regression is legible, and the inequality
/// `capacity <= seed_reach` over every member of the construction, which is the
/// general statement and the one that would catch a new route in.
#[test]
fn capacity_above_the_seeds_reach_buys_no_writes() {
    let mut c = Chain::founded(6, 12);
    let js = chain_of_accomplices(&mut c, 6, 300.0, 4);
    let h = 15u64;
    c.back(*js.last().unwrap(), h, 5000.0);

    assert_eq!(c.cap(h), 300.0, "their capacity was 1,200 of promises on promises; it is the honest trade now");
    assert_eq!(c.st.conferrable(h), 300.0, "and so is what they may confer");
    assert_eq!(c.st.seed_reach(h), 300.0, "which is exactly what the seed reaches them for");
    assert_eq!(c.st.bond_headroom(h), 300.0);
    for &m in js.iter().chain([&h]) {
        assert!(
            c.cap(m) <= c.st.seed_reach(m) + 1e-9,
            "member {m} holds capacity {} above a seed reach of {}",
            c.cap(m),
            c.st.seed_reach(m)
        );
    }
}

/// **The floor takes a member's own debt once.** `prop:write-floor` is
/// "the seed's reach, net of what they owe and have already encumbered", and
/// and a reach that was itself a max-flow over the RESIDUAL graph would mean
/// so an obligation reserved the arcs into its debtor, the reach fell by the
/// amount, and `- debt_out` then took the same amount again.
///
/// Measured on that reading: reach 500 → 300 on a loan of 200, headroom **100**
/// where the paper says 300. The first version of this reading mirrored
/// `conferrable`'s netting deliberately, so that swapping the basis would
/// change only the basis; `conferrable` is a limit, where netting is the point,
/// and a write floor is what a member has to LOSE.
#[test]
fn the_write_floor_takes_a_members_own_debt_once() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=2 {
        c.back(0, m, 500.0);
    }
    assert_eq!(c.st.seed_reach(1), 500.0);
    assert_eq!(c.st.bond_headroom(1), 500.0);

    let id = c.lend(2, 1, 200.0);
    assert!(c.st.contracts[&id].insured, "the fixture needs the reservation a residual reading would double-count");
    assert_eq!(c.st.seed_reach(1), 500.0, "the reach is what the seed puts behind them, drawn or not");
    assert_eq!(c.st.members[&1].debt_out, State::to_minor(200.0));
    assert_eq!(c.st.bond_headroom(1), 300.0, "500 - 200, once");
}

/// **Somebody else's credit is not a deduction from your write floor**, and it
/// was: both readings the gate makes were residual, so an obligation belonging
/// to another member removed your allowance and your headroom alike.
///
/// The scene is deliberately far from any ceiling — community utilisation
/// **20%** — because the defect does not need one. One member draws the
/// underwriter that reaches a second member, and the second member has borrowed
/// nothing, defaulted on nothing, and done nothing at all.
#[test]
fn another_members_credit_is_not_a_deduction_from_your_write_floor() {
    let mut c = Chain::founded_with(&[500.0, 2000.0], 6);
    c.back(0, 2, 500.0);
    c.back(0, 3, 500.0);
    c.back(1, 6, 500.0);
    let before = edet_state::bond::free_remaining(&c.st, 3);
    assert_eq!(c.st.bond_headroom(3), 500.0);
    assert_eq!(before, c.st.params.bond_free_allowance, "a full allowance, unspent");

    // Member 2 draws the whole of the 500 underwriter. Member 3 is not party
    // to it and does not share a debt with anyone.
    let id = c.lend(6, 2, 500.0);
    assert!(c.st.contracts[&id].insured);
    assert!(c.st.utilisation() < 0.25, "nowhere near a ceiling: {}", c.st.utilisation());

    assert_eq!(c.cap(3), 0.0, "their CREDIT is gone, which is the cut doing its job");
    assert_eq!(c.st.bond_headroom(3), 500.0, "their write floor is what the community put behind them");
    assert_eq!(edet_state::bond::free_remaining(&c.st, 3), before, "and the allowance is untouched");
    assert!(edet_state::bond::established(&c.st, 3), "the community has put something behind them either way");
}

/// **A binding ceiling withholds insurance, not trade.** §Standing is explicit that a
/// reservation which cannot be found is not an error — the obligation is simply
/// uninsured and the creditor bears it alone — and the client says so on the
/// network page. The gate above it said otherwise: at 100% utilisation every
/// ordinary member's headroom and allowance were zero, so `Accept`, `Sale` and
/// a newcomer's first trade all came back `ET-BND-001`, and the one path still
/// open was a trade with an underwriter.
///
/// Which also means the bootstrap that dissolves the deadlock (§Recourse: first trades
/// are uninsured, they settle, they create stakes) was shut exactly when the
/// community had drawn everything it had.
#[test]
fn a_community_at_its_ceiling_still_trades_and_still_refuses_insurance() {
    let mut c = Chain::founded(1, 8);
    for m in 1..=7 {
        c.back(0, m, 500.0);
    }
    for m in 1..=5 {
        let id = c.lend(7, m, 500.0);
        assert!(c.st.contracts[&id].insured);
    }
    assert_eq!(c.st.utilisation(), 1.0, "the ceiling binds");
    assert_eq!(c.cap(6), 0.0, "and there is no insured capacity left for anybody");

    let late = c.lend(7, 6, 500.0);
    assert!(!c.st.contracts[&late].insured, "so the trade is uninsured, which is what the ceiling means");

    c.ok(
        Tx::Sale { seller: Party::Member(6), buyer: Party::Member(7), amount: 25.0, maturity_epochs: MATURITY },
        &[key(6), key(7)],
    );

    let newcomer = stranger_key(3);
    c.ok(
        Tx::Accept {
            debtor: Party::Key(newcomer),
            creditor: Party::Member(7),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(7), newcomer],
    );
    assert!(c.st.key_index.contains_key(&newcomer), "and the first trade still seats an account");
}

/// **A default consumes the defaulter's standing, and that is the sanction** —
/// stated here rather than inherited. Under a residual qualification it falls
/// out for free: a defaulter's debt holds every arc into them, so their
/// `conferrable` is zero and the allowance goes with it. Gross, it does not,
/// and 32 free writes an epoch would survive defaulting on everything.
///
/// The recovery path does not need the allowance: the free classes are free
/// before the gate asks who would pay.
#[test]
fn a_defaulter_keeps_no_allowance_and_can_still_cure() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    let cid = c.lend(0, 1, SUPPLY);
    c.default_on(cid);

    assert!(c.st.members[&1].rep.open_default > 0);
    assert!(edet_state::bond::established(&c.st, 1), "the seed still reaches them — this is not a Sybil test");
    assert_eq!(edet_state::bond::free_remaining(&c.st, 1), 0, "but the allowance is part of what a default costs");
    assert_eq!(c.apply(bonded_dud(), &[key(1)]), Err(Error(ET_BOND_EXHAUSTED)), "so every bonded class is refused");

    c.ok(Tx::Cure { contract: cid, amount: SUPPLY }, &[key(0), key(1)]);
    assert!(c.st.members[&1].rep.open_default <= c.st.params.dust_minor());
    assert_eq!(
        edet_state::bond::free_remaining(&c.st, 1),
        c.st.params.bond_free_allowance,
        "and the way back is the model's own"
    );
}

/// **A zero-priced transition is unlimited, so every zero has to survive "how
/// many can one member force in an epoch?"** `DeclareSupply` was the one that
/// answered "as many as it likes": the free list justified it with an argument
/// about LOWERING a supply — a member must never be priced out of reducing what
/// they stand behind — and applying it to both directions is measured at
/// fix: one member forced **5,000** raises in a single epoch, burning 5,000
/// replay ids, with its bond encumbrance still zero and its headroom untouched.
///
/// **A raise is refused outright, and the pricing still has to be
/// right**, because a REFUSAL is a write too. `apply` refunds exactly one
/// class of failure — an envelope that authorised nothing — and this is not it,
/// so a refused raise pays its bond and the channel stays bounded. A version of
/// this fix that made the refusal free would have replaced an unbounded channel
/// of accepted raises with an unbounded channel of rejected ones.
#[test]
fn a_refused_raise_is_bonded_and_lowering_is_not() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    c.st.params.bond_free_allowance = 0;
    c.st.params.bond_fraction = 0.10;

    // A raise is refused, and it is priced, so it is bounded by headroom like
    // any other write.
    let before = c.st.members[&1].bond_enc();
    c.err(Tx::DeclareSupply { member: 1, supply: c.cap(1) }, &[key(1)], ET_UWR_ABOVE_CAPACITY);
    assert!(c.st.members[&1].bond_enc() > before, "a raise must encumber a bond even when it is refused");
    assert_eq!(c.st.conferrable(1), 500.0, "and it must leave the declaration where it was");

    // Lowering is free, always: the recovery path is what keeps a default from
    // being absorbing, and reducing a liability belongs to it. Member 0 is the
    // founder, and the one member here with something to lower.
    let held = c.st.members[&0].bond_enc();
    c.ok(Tx::DeclareSupply { member: 0, supply: 1.0 }, &[key(0)]);
    assert_eq!(c.st.members[&0].bond_enc(), held, "lowering must stay free");

    // And the channel is bounded: raises stop when the headroom does.
    let mut attempts = 0;
    for _ in 0..10_000 {
        match c.apply(Tx::DeclareSupply { member: 1, supply: 1e9 }, &[key(1)]) {
            Err(Error(ET_UWR_ABOVE_CAPACITY)) => attempts += 1,
            Err(Error(ET_BOND_EXHAUSTED)) => break,
            other => panic!("unexpected outcome from a raise: {other:?}"),
        }
    }
    assert!(attempts < 100, "a member forced {attempts} raises in one epoch — the channel is unbounded again");
}

/// **A write budget bounds a RATE, and what a raise buys is a STOCK.**
///
/// The schedule's question is "how many of these can one member force in an
/// epoch?", and every entry on it answers about WRITES: a durable row, a replay
/// id, a contract. `DeclareSupply`'s raise is priced and bounded like the rest
/// (above). What nothing on the schedule asks is what a write costs *after* it
/// lands, and this is the one transition that buys a recurring cost: an
/// underwriter carrying live insured debt is a set in invariant 1's family, so
/// it adds **one full max-flow query to every committed block on every
/// validator, for as long as the debt lives** — measured at 24.6–36.1 ms per
/// set at 20,000 accounts, and flat in the size of the set.
///
/// **What that sentence COSTS and what it MEASURES are different questions.**
/// The cut is memoised (§Implementation, §Verification), so an ordinary
/// block pays only for the sets whose own inputs moved and a seat nobody is
/// drawing through is free to carry. The stock is still a stock: the set is
/// still in the family, the epoch boundary still recomputes all of it, and
/// nothing but settlement retires one. So this probe measures the SET COUNT
/// rather than a wall-clock, which is the quantity the decision did not move
/// and the one a rate limit still cannot bound.
///
/// The two quantities do not compose. A bond is *reserved and released*, so the
/// budget is a per-epoch flow; the audit's cost is a stock that only ever
/// accumulates. A rate limit cannot bound a stock, and this measures the gap:
/// the same sponsor seats a fresh batch every epoch, every bond hands itself
/// back at the boundary, every allowance resets — and the per-block cost of the
/// ledger is strictly higher than it was, permanently.
///
/// The first seat is the sharpest form of it. On a fresh community it costs
/// **nothing at all** — it fits inside the free allowance, encumbers 0.00, and
/// still puts a query on every future block.
///
/// Found by asking what a committed block may cost, one level in: the answer had
/// established that the cost is linear in `U` and that "nothing bounds it"
/// (§Standing leaves the underwriter role open on purpose). What it had not asked is
/// what raising `U` by one COSTS the member who raises it.
#[test]
fn a_write_budget_bounds_a_rate_and_an_underwriter_seat_is_a_stock() {
    // Two accounts per seat: one who will declare, one who will draw through
    // them. Six a side leaves the sponsor's free allowance unspent in both
    // epochs, so nothing here is bounded by the budget under test.
    let seats_per_epoch = 6usize;
    // A seed large enough that the amendment rate bound is not what limits the
    // seats: this probe is about the write budget against the audit's stock,
    // and a second bound in the frame would measure the wrong thing.
    let mut c = Chain::founded_with(&[100_000.0], 4 * seats_per_epoch + 4);

    // The first seat, on a community with nothing insured in it yet.
    assert_eq!(audited_sets(&c.st), 0, "an empty book audits no sets");
    seat_an_underwriter(&mut c, 1, 2);
    assert_eq!(audited_sets(&c.st), 2, "one seated underwriter is one set of its own, plus the all-debtors set");
    // WHICH party and WHICH quantity: not just the new underwriter's own
    // encumbrance but the whole ledger's, because the sponsor signs two of the
    // four writes and asserting one side would leave the claim half-measured.
    assert_eq!(
        State::from_minor(c.st.members.values().map(|m| m.bond_enc()).sum::<u64>()),
        0.0,
        "the seat that buys a query on every future block encumbered nothing, anywhere"
    );

    // Fill out the epoch, then close it.
    for i in 1..seats_per_epoch {
        seat_an_underwriter(&mut c, (1 + 2 * i) as MemberId, (2 + 2 * i) as MemberId);
    }
    let after_first = audited_sets(&c.st);
    assert_eq!(after_first, seats_per_epoch + 1, "every seat is a set");

    c.goto(c.st.epoch + 1);

    // The budget is back and the cost is not.
    assert_eq!(
        State::from_minor(c.st.members.values().map(|m| m.bond_enc()).sum::<u64>()),
        0.0,
        "a bond is reserved and released, so the epoch hands the whole budget back"
    );
    assert_eq!(
        edet_state::bond::free_remaining(&c.st, 0),
        c.st.params.bond_free_allowance,
        "and the free allowance resets with it"
    );
    assert_eq!(audited_sets(&c.st), after_first, "while the per-block cost the last epoch bought is still there");

    // So the second epoch buys more of it, from the same budget.
    let base = 2 * seats_per_epoch;
    for i in 0..seats_per_epoch {
        seat_an_underwriter(&mut c, (base + 1 + 2 * i) as MemberId, (base + 2 + 2 * i) as MemberId);
    }
    assert_eq!(
        audited_sets(&c.st),
        2 * seats_per_epoch + 1,
        "the stock grew again, out of a budget that had reset to exactly what it was"
    );
}

/// **And nothing takes a seat back.** Attacking the fixture above: its story
/// says the query lives "as long as the debt lives", so what if the debt never
/// dies?
///
/// The audit counts `Active | Expired`, and a default does not release the flow
/// it committed — deliberately, because that is what makes stealing through an
/// underwriter cost 1.00x and be unrepeatable (§Recourse, §Recourse). So an obligation the
/// debtor simply never settles holds its arc for ever: measured, the same
/// `held.supply` and the same audited set **two hundred epochs** past maturity,
/// with nothing encumbered and nobody able to act.
///
/// Which makes the per-block cost a **ratchet**, not just a stock. Settlement is
/// the only thing that removes a set, and settlement is the one act the party
/// who raised the cost is under no obligation to perform. The rule this measures
/// is right and is not the defect; what it means for the block-cost question is that "how
/// many underwriters may carry live insured debt" has no decay term in it.
#[test]
fn a_seat_the_debtor_never_settles_is_permanent() {
    let mut c = Chain::founded_with(&[100_000.0], 4);
    c.back(0, 1, 200.0);
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 1, kind: ProposalKind::SeedAmendment { amount: 200.0 } }, &[key(1), key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert!(c.st.proposals[&pid].enacted, "the ceremony seats the underwriter");
    c.back(1, 2, 100.0);
    let cid = c.lend(0, 2, 50.0);
    let held = c.st.contracts[&cid].held.supply.clone();
    assert_eq!(audited_sets(&c.st), 2);

    c.default_on(cid);
    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Expired);
    assert_eq!(audited_sets(&c.st), 2, "a default does not release the flow, so the set stays");

    c.goto(c.st.epoch + 200);
    assert_eq!(c.st.contracts[&cid].held.supply, held, "two hundred epochs on, the hold is exactly what it was");
    assert_eq!(audited_sets(&c.st), 2, "and the query is still on every block");
    assert_eq!(
        State::from_minor(c.st.members.values().map(|m| m.bond_enc()).sum::<u64>()),
        0.0,
        "with nothing encumbered against it by anybody"
    );

    // And the underwriter cannot stand down out of it either — the same
    // property from the other side. `supply_floor` is the flow committed
    // through them (§Stability), so leaving the role is refused for exactly as long as
    // the defaulted obligation holds their arc: whoever wanted the cost gone
    // has no transition that removes it, and neither does the member paying
    // for it with their own supply.
    c.err(Tx::DeclareSupply { member: 1, supply: 0.0 }, &[key(1)], ET_UWR_BELOW_COMMITTED);
    assert_eq!(audited_sets(&c.st), 2, "the refusal leaves the set exactly where it was");
}

/// The sets invariant 1 measures: every live insured debtor as one set, plus
/// one per underwriter carrying any of them (`invariants::capacity_invariants`).
/// Each is one full max-flow query on the commit path.
fn audited_sets(st: &edet_state::state::State) -> usize {
    let mut carrying: std::collections::BTreeSet<usize> = Default::default();
    let mut any = false;
    for ct in st.contracts.values() {
        if ct.insured && matches!(ct.status, ContractStatus::Active | ContractStatus::Expired) {
            any = true;
            for &(u, _) in &ct.held.supply {
                carrying.insert(u);
            }
        }
    }
    usize::from(any) + carrying.len()
}

/// Seat `u` as an underwriter carrying live insured debt, the only way there
/// is: the community endorses their external commitment through a ceremony
/// (§Governance), the sponsor backs them, they reach `d` with it, and one
/// insured obligation on `d` draws through their supply arc.
///
/// A seat cannot be taken by a `DeclareSupply` against `u`'s own capacity,
/// which raises the bar on WHO may take one without changing what a seat COSTS
/// the ledger — which is what the probes below measure. Founder 0 holds the whole seed here, so its single assent enacts.
///
/// `source -> u -> d` is shorter than `source -> sponsor -> u -> d`, so Dinic's
/// shortest augmenting path routes the draw through the new arc — which is
/// asserted rather than assumed, since it is the whole point of the fixture.
fn seat_an_underwriter(c: &mut Chain, u: MemberId, d: MemberId) {
    // Comfortably above the establishment floor (a twentieth of `v_base`), so
    // what this probe measures is the audit's set count and not the write gate.
    c.back(0, u, 200.0);
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: u, kind: ProposalKind::SeedAmendment { amount: 200.0 } }, &[key(u as usize), key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert!(c.st.proposals[&pid].enacted, "the ceremony must seat {u}");
    c.back(u, d, 100.0);
    let cid = c.lend(0, d, 50.0);
    let ct = &c.st.contracts[&cid];
    assert!(ct.insured, "the fixture needs a live INSURED obligation on {d}");
    assert!(
        ct.held.supply.iter().any(|&(x, _)| x == u as usize),
        "the draw must route through {u}'s supply arc, not the sponsor's: {:?}",
        ct.held.supply
    );
}

/// **The same theorem, on the one zero that was still answering "unlimited".**
///
/// The free list justifies `Exit` with "`Transfer` and `Exit` close what they
/// touch". True of the first; the second closes nothing, and nothing refused
/// it. Measured without the refusal: **500 of 500** repeated `Exit`s accepted on an
/// already-`Exited` row, with `bond_free_allowance` at zero and
/// `bond_headroom` at 0.00 — a member with no write budget at all growing
/// the replay cache without bound, and that cache is hashed into the state
/// root. `bond::admits` returned true for every one of them, so the mempool
/// screen was not the bound either.
///
/// Every other free class already refuses its own second call, which is why
/// this was the only one: `RotateVeto` ET-ROT-003, `RotateFinalize`
/// ET-ROT-001, `MarkExpired` ET-CTR-002, `ForfeitBonds` ET-BND-002. Found while
/// scoping the note that "`Exit` and suspension are underspecified".
#[test]
fn exit_succeeds_at_most_once_and_stays_open_under_suspension() {
    let mut c = Chain::founded(2, 2);
    c.back(0, 2, 300.0);
    c.st.params.bond_free_allowance = 0;
    c.st.params.bond_fraction = 0.10;

    c.ok(Tx::Exit { member: 2 }, &[key(2)]);
    assert_eq!(c.st.members[&2].status, MemberStatus::Exited);
    assert_eq!(c.st.bond_headroom(2), 0.0, "an exited row has no write budget");

    // The channel: with no headroom and no allowance, the repeat must cost the
    // id nothing because it must not be admitted at all.
    let ids = c.st.applied_count();
    for _ in 0..8 {
        c.err(Tx::Exit { member: 2 }, &[key(2)], ET_MEM_NOT_ACTIVE);
    }
    assert!(c.st.applied_count() - ids <= 8, "a refused exit must not be a cheaper write than a bonded one");

    // And the carve-out: winding down under sanction is the recovery path, so
    // `Suspended` is refused by nothing here. A status a member cannot leave
    // turns a recoverable failure into an absorbing one.
    let mut c = Chain::founded(2, 2);
    c.back(0, 3, 300.0);
    c.st.members.get_mut(&3).unwrap().status = MemberStatus::Suspended;
    c.ok(Tx::Exit { member: 3 }, &[key(3)]);
    assert_eq!(c.st.members[&3].status, MemberStatus::Exited);
}

/// **A budget zeroed by STATUS is not a budget that was spent**, and until this
/// probe the two shared one refusal — with a sanction attached to it.
///
/// `bond_headroom` returns 0.00 for any member that is not `Active`, so every
/// priced transition a suspended member signed for themselves arrived as
/// `ET-BND-001`: wait, your bonds will release. Nothing had been charged, so
/// nothing could release, and that branch also sets `bond_denied_this_epoch` —
/// which the epoch sweep counts toward `ForfeitBonds`. Measured on that code:
/// a suspended member holding **20.00** encumbered from before the suspension
/// lost all of it into `forfeit_reserve`, permanently, in three epochs, for
/// nothing but trying to use their own wallet. A sanction the community voted
/// for must not grow a second one nobody proposed.
///
/// The abuse detector itself is unchanged, and the second half checks that: an
/// ACTIVE member who spends their headroom still gets `ET-BND-001` and still
/// arms the counter.
#[test]
fn a_status_zeroed_budget_is_refused_as_itself_and_forfeits_nothing() {
    let mut c = Chain::founded(2, 3);
    for m in [2, 3, 4] {
        c.back(0, m, 300.0);
    }
    c.st.params.bond_free_allowance = 0;
    c.st.params.bond_release_epochs = 100; // bonds from before the suspension stay encumbered
                                           // Member 2 encumbers a bond of its own while still Active, and member 3
                                           // lists it as a beneficiary so the approval below has something to approve.
    c.ok(Tx::ListBeneficiaries { supporter: 2, entries: vec![(4, 10.0)] }, &[key(2)]);
    c.ok(Tx::ListBeneficiaries { supporter: 3, entries: vec![(2, 100.0)] }, &[key(3)]);
    let held = c.st.members[&2].bond_enc();
    assert!(held > 0, "the fixture needs bonds encumbered before the suspension");
    c.st.members.get_mut(&2).unwrap().status = MemberStatus::Suspended;

    c.err(Tx::Propose { author: 2, kind: ProposalKind::Unsuspend { member: 2 } }, &[key(2)], ET_BOND_STATUS);
    assert!(
        !c.st.members[&2].bond_denied_this_epoch,
        "a status refusal must not arm the counter the forfeiture crank reads"
    );

    // The sweep runs at every boundary, so this is the whole exposure.
    for _ in 0..=c.st.params.bond_forfeit_epochs + 1 {
        c.err(Tx::Propose { author: 2, kind: ProposalKind::Unsuspend { member: 2 } }, &[key(2)], ET_BOND_STATUS);
        c.goto(c.st.epoch + 1);
    }
    assert_eq!(c.st.members[&2].bond_saturated_epochs, 0, "no saturation episode ever began");
    assert!(
        c.st.members[&2].bond_enc() == held,
        "a suspended member lost bonds for asking: {held:.2} -> {:.2}",
        c.st.members[&2].bond_enc()
    );
    assert!(!c.st.forfeit_reserve.contains_key(&2), "nothing was forfeited");

    // What a non-Active member may still do, a co-signer with headroom pays
    // for: the bill goes to the first signer in canonical order who CAN pay,
    // and a suspended member's headroom is zero by status.
    c.ok(Tx::ApproveSupporter { beneficiary: 2, supporter: 3, approved: true }, &[key(2), key(4)]);

    // And the detector is untouched: an ACTIVE member who spends their headroom
    // gets the exhaustion code and arms the counter, exactly as before.
    let mut c = Chain::founded(2, 2);
    c.back(0, 2, 300.0);
    c.tighten();
    c.exhaust(key(2));
    assert!(c.st.members[&2].bond_denied_this_epoch, "an active member at their ceiling still arms it");
}

/// **The allowance cliff, and the figure that hides it** (the paper
/// the bond-schedule audit).
///
/// A member's standing is other members' present willingness, so it can fall
/// through no act of their own — an underwriter reduces a supply, a stake
/// decays, a creditor loses their own backing. Below `dust` of `conferrable`
/// the free allowance stops applying at all, which is deliberate: a per-key
/// allowance is the free-signature bound's own defect one layer down.
///
/// What was NOT deliberate is that `free_remaining` was `allowance - used`,
/// computed with no test that the member qualifies. So the member on the wrong
/// side of the cliff was told by their own wallet that they had the whole
/// allowance left, in the "good" tone, while every write came back
/// `ET-BND-001`. **A view reporting a quantity the ledger does not honour is
/// read as a promise**, and it is the reason the client could not warn about
/// this: the figure it had said there was nothing to warn about.
///
/// The gate and the read surface answer with one function now, which is the
/// only arrangement that cannot drift.
#[test]
fn the_allowance_a_view_reports_is_the_allowance_the_gate_grants() {
    // Two founders and one ordinary member: 0 and 1 underwrite, 2 is the
    // member whose standing is about to be withdrawn from under them. Two,
    // because the recovery at the end has to be an ordinary one — a founder who
    // withdraws to zero cannot re-declare alone, since raising is bonded
    // and their own reach went with the withdrawal, and the model's answer to
    // that is its answer to a newcomer: somebody else.
    let mut c = Chain::founded(2, 1);
    let m = 2u64;
    c.back(0, m, SUPPLY);
    let allowance = c.st.params.bond_free_allowance;
    assert_eq!(edet_state::bond::free_remaining(&c.st, m), allowance, "backed, and nothing spent yet");

    // Spending inside the allowance moves it exactly as far as it was spent.
    // `bonded_dud` fails on its own merits and pays anyway, which is the point:
    // a designed-to-fail transaction must not be a free write.
    let _ = c.apply(bonded_dud(), &[key(m as usize)]);
    assert_eq!(edet_state::bond::free_remaining(&c.st, m), allowance - 1);

    // The underwriter withdraws. Nothing this member did changed.
    c.ok(Tx::DeclareSupply { member: 0, supply: 0.0 }, &[key(0)]);
    assert_eq!(c.st.conferrable(m), 0.0, "the cliff: nothing backs them now");
    assert_eq!(c.st.bond_headroom(m), 0.0);
    assert_eq!(
        edet_state::bond::free_remaining(&c.st, m),
        0,
        "so the allowance is gone too, and the figure a member reads must say so"
    );
    assert_eq!(
        c.apply(bonded_dud(), &[key(m as usize)]),
        Err(Error(ET_BOND_EXHAUSTED)),
        "which is exactly what the gate does"
    );

    // And it is not absorbing: standing returns, and the allowance with it.
    c.back(1, m, SUPPLY);
    assert!(edet_state::bond::free_remaining(&c.st, m) > 0, "the way back is the model's own");
    assert_eq!(
        c.apply(bonded_dud(), &[key(m as usize)]),
        Err(Error(ET_CTR_UNKNOWN)),
        "and the gate admits it again — refused on its merits now, not by the bond"
    );
}

/// A suspended member is on the same cliff and for a different reason: the
/// allowance tests `Active` as well as standing. Reported the same way, because
/// the member needs to know their next write will be refused either way.
#[test]
fn a_suspended_member_has_no_allowance_and_the_figure_says_so() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    assert!(edet_state::bond::free_remaining(&c.st, 1) > 0);

    c.st.members.get_mut(&1).expect("member 1").status = MemberStatus::Suspended;
    assert_eq!(edet_state::bond::free_remaining(&c.st, 1), 0, "suspension revokes origination, allowance included");
    assert!(c.st.conferrable(1) > 0.0, "and it is the STATUS that did it, not the standing");
}

// -------------------------------------------- what a row costs, and how long --

/// **A token wash seats a bounded handful and confers nothing on what it
/// seated**, so the farm stays one member wide.
///
/// Two thresholds are at work here and they are not the same one. A seated row
/// becomes a SEATER when its own headroom reaches one bond unit, which is what
/// `bond_headroom` answers and what has no establishment gate on it; it holds a
/// free ALLOWANCE when the seed reaches it above `ESTABLISHED_FRACTION x
/// v_base`. A wash of 1.00 clears neither, which is what this holds.
///
/// Two rules are at work and this probe holds both. Seating a fresh key is
/// bonded against the established counterparty, so each row costs headroom
/// nobody can mint — where the free allowance would be a RATE that refills
/// every epoch, and letting it pay for a permanent row makes the account table
/// grow at `allowance x established accounts` per epoch at a bond spend of
/// zero. And the establishment floor is a fraction of `v_base` measured on the
/// seed's REACH, so a token settlement of 1.00 confers no allowance and no
/// headroom on the account it seated: the farm stays one member wide.
///
/// **Above the bond unit the row count compounds**, and raising the floor does
/// not touch it. Measured from one honest edge of 500.00, rows held after six
/// epochs: a wash of 1.00, 20.00 or 40.00 gives 91 — the operator's own linear
/// rate — while 60.00 gives 310 and 100.00 gives 1,120. Raising
/// `ESTABLISHED_FRACTION` tenfold leaves every one of those counts unchanged
/// and takes the established count to zero, which is the whole of the
/// separation: the floor withholds an allowance, never a row. The capacity
/// bound is untouched throughout — the farm's gross cut as a SET stays at that
/// one edge. `crates/swarm`'s `sybil-farm` corpus entry pins the growth and its
/// archetype test names the mechanisms that would close it.
#[test]
fn a_token_wash_confers_neither_a_seat_nor_an_allowance() {
    let mut c = Chain::founded_with(&[100.0], 1);
    let (f, m) = (0u64, 1u64);
    c.back(f, m, 100.0);
    assert!(edet_state::bond::established(&c.st, m), "the member behind the one honest edge is established");

    let allowance = c.st.params.bond_free_allowance;
    let mut seated: Vec<MemberId> = Vec::new();
    let mut key_of: std::collections::BTreeMap<MemberId, Key> = std::collections::BTreeMap::new();
    let mut next_key = 0u64;

    // One epoch of the parent seating everything it can afford.
    c.goto(1);
    for _ in 0..allowance * 4 {
        next_key += 1;
        let k = sk(next_key);
        let cid = c.st.next_contract;
        if c.apply(
            Tx::Accept {
                debtor: Party::Key(k),
                creditor: Party::Member(m),
                amount: 1.0,
                maturity_epochs: MATURITY,
                arb: None,
            },
            &[k, key(m as usize)],
        )
        .is_err()
        {
            break;
        }
        let id = c.st.member_of_key(&k).expect("seated");
        key_of.insert(id, k);
        c.ok(Tx::Settle { contract: cid, amount: 1.0 }, &[k, key(m as usize)]);
        seated.push(id);
    }

    // **The seats are bounded by headroom, not by the allowance**, so a
    // member with one honest edge of 100 behind it seats a handful and stops.
    assert!(!seated.is_empty(), "an established member must still be able to trade with a newcomer");
    assert!(
        (seated.len() as u32) < allowance,
        "{} rows seated in one epoch against an allowance of {allowance} — seating is not bonded",
        seated.len()
    );

    // **And none of them is established**, so next epoch multiplies nothing.
    // A settlement of 1.00 is a wash trade, and the floor is a twentieth of
    // the denomination.
    for &id in &seated {
        assert!(
            !edet_state::bond::established(&c.st, id),
            "member {id}, seated by a settlement of 1.00, must not hold an allowance of its own"
        );
    }
    c.goto(2);
    for &id in &seated {
        assert_eq!(edet_state::bond::free_remaining(&c.st, id), 0, "and not next epoch either");
    }

    // Non-vacuity: a member the community really backs IS established, so the
    // floor is a floor rather than a wall. Their key is the one that seated
    // them, not this harness's index-derived one.
    let honest = *seated.first().expect("at least one seat");
    let hk = key_of[&honest];
    let real = c.st.next_contract;
    let amount = c.st.params.v_base * 0.2;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(honest),
            creditor: Party::Member(f),
            amount,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[hk, key(f as usize)],
    );
    c.ok(Tx::Settle { contract: real, amount }, &[hk, key(f as usize)]);
    assert!(
        edet_state::bond::established(&c.st, honest),
        "a member the founder backed for a fifth of the denomination must hold an allowance"
    );
}

/// **A closed row is retired, so the book does not grow for ever.**
///
/// A settled obligation owes nothing, holds nothing, and answers only "this was
/// paid" — which the block history records and the stake the settlement
/// conferred already reflects. Kept for ever it is a permanent per-block cost
/// bought with one bond, which is a rate paying for a stock.
///
/// An `Expired` row is NOT closed: it is a default, the audit counts it, and
/// its hold is still reserved. This probe holds that distinction, because
/// pruning one would release a loss nobody paid.
#[test]
fn a_settled_row_is_retired_and_a_defaulted_one_is_not() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, 500.0);
    c.back(0, 2, 500.0);

    let settled = c.lend(2, 1, 50.0);
    c.settle(settled, 50.0);
    let defaulted = c.lend(2, 1, 50.0);
    c.default_on(defaulted);
    assert_eq!(c.st.contracts[&settled].status, ContractStatus::Settled);
    assert_eq!(c.st.contracts[&defaulted].status, ContractStatus::Expired);

    // Inside the retention window both are kept: a settled row is evidence for
    // as long as anybody is plausibly still arguing about the trade.
    c.goto(edet_kernel::constants::CLOSED_RETENTION_EPOCHS - 1);
    assert!(c.st.contracts.contains_key(&settled), "a closed row is kept for its whole retention window");

    c.goto(edet_kernel::constants::CLOSED_RETENTION_EPOCHS + 2);
    assert!(!c.st.contracts.contains_key(&settled), "and retired after it");
    assert!(
        c.st.contracts.contains_key(&defaulted),
        "a DEFAULT is live — it holds committed flow, and dropping it would release a loss nobody paid"
    );
}

/// A proposal is a permanent row too, and the same rule applies: one that
/// enacted has said everything it can say, and one nobody carried in a year is
/// not going to carry.
#[test]
fn a_proposal_is_retired_once_it_can_say_nothing_more() {
    let mut c = Chain::founded(2, 0);
    let enacted = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 0.6 } }, &[key(0)]);
    // Two founders of equal seed, so one assent is exactly Θ and carries it.
    c.ok(Tx::Assent { member: 0, proposal: enacted }, &[key(0)]);
    assert!(c.st.proposals[&enacted].enacted);

    let abandoned = c.st.next_proposal;
    c.ok(
        Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::SealAmounts, value: 0.0 } },
        &[key(0)],
    );

    c.goto(edet_kernel::constants::PROPOSAL_RETENTION_EPOCHS + 2);
    assert!(c.st.proposals.is_empty(), "both are past saying anything: {:?}", c.st.proposals.keys());
    let _ = abandoned;
}

// ------------------------------------------------- the failure path of a zero --

mod final_review_free_classes {
    use super::*;

    /// **One list of permissionless cranks, and a refused crank leaves
    /// nothing.** `RotateFinalize` arrived unsigned at the ingress and was
    /// missing from the ledger's list, so its refusal recorded a replay id —
    /// fifty of fifty, from nobody, into a bucket the root re-encodes whole.
    ///
    /// Mutation that bites: drop the variant from `bond::is_permissionless`.
    #[test]
    fn a_keyless_crank_that_finds_nothing_to_do_leaves_no_id_whatever_its_kind() {
        let mut c = Chain::founded(1, 0);
        let ids = c.st.applied_count();
        for _ in 0..64 {
            c.err(Tx::RotateFinalize { member: 0 }, &[], ET_ROT_NO_GUARDIANS);
        }
        assert_eq!(c.st.applied_count(), ids, "a refused RotateFinalize is a crank like the other two");
        for tx in [Tx::MarkExpired { contract: 0 }, Tx::ForfeitBonds { member: 0 }, Tx::RotateFinalize { member: 0 }] {
            assert!(edet_state::bond::is_permissionless(&tx), "{tx:?} is a crank");
        }
        assert!(!edet_state::bond::is_permissionless(&Tx::Exit { member: 0 }));
    }

    /// **A refused free class is priced, or it is forgotten.** `Exit` by a
    /// member who owes something fails on its merits and, until this, wrote a
    /// durable replay id for nothing, without limit. Each refusal now costs
    /// the first signer with an allowance one slot; with no slot left the id is
    /// not kept.
    ///
    /// Mutation that bites: skip `price_refusal` — the debtor writes sixteen
    /// ids past their allowance for free.
    #[test]
    fn a_refused_free_class_spends_an_allowance_slot_or_leaves_nothing() {
        let mut c = Chain::founded(1, 2);
        c.back(0, 1, 500.0);
        c.lend(2, 1, 100.0);
        let free = edet_state::bond::free_remaining(&c.st, 1);
        assert!(free > 0, "an established debtor holds an allowance");
        let ids = c.st.applied_count();
        for i in 0..free as usize {
            c.err(Tx::Exit { member: 1 }, &[key(1)], ET_LIF_OUTSTANDING_DEBT);
            assert_eq!(c.st.applied_count(), ids + i + 1, "a refusal somebody can pay for is recorded");
        }
        assert_eq!(edet_state::bond::free_remaining(&c.st, 1), 0, "and every one of them cost a slot");
        let ids = c.st.applied_count();
        for _ in 0..16 {
            c.err(Tx::Exit { member: 1 }, &[key(1)], ET_LIF_OUTSTANDING_DEBT);
        }
        assert_eq!(c.st.applied_count(), ids, "with no slot left, nothing durable is written");
        assert_eq!(c.st.members[&1].bond_enc(), 0, "and no bond is ever charged for a free class");
    }

    /// An envelope naming an id the ledger has not issued names nothing, and
    /// the ingress keeps it out of a mempool on that answer.
    #[test]
    fn an_unissued_id_is_named_as_such() {
        use edet_state::apply::names_issued_ids;
        let mut c = Chain::founded(1, 2);
        c.back(0, 1, 500.0);
        let st = &c.st;
        assert_eq!(
            names_issued_ids(st, &Tx::Settle { contract: st.next_contract, amount: 1.0 }),
            Err(Error(ET_CTR_UNKNOWN))
        );
        assert_eq!(names_issued_ids(st, &Tx::Settle { contract: 0, amount: 1.0 }), Ok(()));
        assert_eq!(
            names_issued_ids(st, &Tx::Assent { member: 0, proposal: st.next_proposal }),
            Err(Error(ET_GOV_UNKNOWN_PROPOSAL))
        );
        assert_eq!(names_issued_ids(st, &Tx::Exit { member: st.next_member }), Err(Error(ET_MEM_UNKNOWN)));
        assert_eq!(
            names_issued_ids(
                st,
                &Tx::Accept {
                    debtor: Party::Key(stranger_key(1)),
                    creditor: Party::Member(1),
                    amount: 1.0,
                    maturity_epochs: MATURITY,
                    arb: None
                }
            ),
            Ok(()),
            "a key is not an id"
        );
    }

    /// **A partial payment is at least a 128th of the original.** A settle is
    /// free and was bounded by the amount, and an uninsured amount is bounded
    /// by nothing but the ingress ceiling: one allowance slot bought 3,000
    /// settles of one minor unit with 9,997,000 more to go. The closing payment
    /// is any size.
    ///
    /// Mutation that bites: drop `below_installment_floor`.
    #[test]
    fn a_partial_payment_is_at_least_a_hundred_and_twenty_eighth_of_the_original() {
        let mut c = Chain::founded(1, 2);
        c.back(0, 1, 500.0);
        let cid = c.lend(1, 2, 100_000.0);
        assert!(!c.st.contracts[&cid].insured);
        let signers = [key(2), key(1)];
        c.err(Tx::Settle { contract: cid, amount: 0.01 }, &signers, ET_CTR_BAD_AMOUNT);
        c.err(Tx::Settle { contract: cid, amount: 781.24 }, &signers, ET_CTR_BAD_AMOUNT);
        let floor = 100_000.0 / edet_kernel::constants::MAX_INSTALLMENTS as f64;
        for _ in 0..127 {
            c.ok(Tx::Settle { contract: cid, amount: floor }, &signers);
        }
        let left = State::from_minor(c.st.contracts[&cid].outstanding);
        assert!((left - floor).abs() < 1e-9, "127 installments leave the 128th: {left}");
        // The last payment may be anything that closes the row, however small
        // the remainder — a debt is never left unpayable by its own floor.
        c.ok(Tx::Settle { contract: cid, amount: left }, &signers);
        assert_eq!(c.st.contracts[&cid].status, ContractStatus::Settled);

        // And a cure runs on the same floor.
        let cid = c.lend(1, 2, 1_280.0);
        c.default_on(cid);
        c.err(Tx::Cure { contract: cid, amount: 9.99 }, &signers, ET_CTR_BAD_AMOUNT);
        c.ok(Tx::Cure { contract: cid, amount: 10.0 }, &signers);
    }
}
