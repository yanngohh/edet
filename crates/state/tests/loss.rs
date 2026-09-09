//! What an underwriter actually pays (the paper's §Recourse).
//!
//! The bound on an underwriter's loss was measured long before this suite; the
//! settlement of it is what these probe. In a closed system the only way
//! anyone can pay is to become a debtor, so a default splits into two moves
//! that are one event: the creditor's claim lands on the underwriters
//! (**substitution**), and the defaulter's debt moves to them instead of to
//! the creditor (**subrogation**).
//!
//! **Tested at the quantifier the theorem uses.** Every defect this model has
//! carried was a bound asserted over a set and tested over a singleton, and
//! the shape here is exactly the one that invites it: one supply arc, many
//! debtors, all defaulting. A per-default check passes in every one of them.

mod common;

use common::{key, Chain, MATURITY, SUPPLY};
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

/// What `debtor` owes `creditor` over live claims.
fn owed_to(c: &Chain, debtor: MemberId, creditor: MemberId) -> f64 {
    c.st.contracts
        .values()
        .filter(|x| x.debtor == debtor && x.creditor == creditor)
        .filter(|x| matches!(x.status, ContractStatus::Active | ContractStatus::Expired))
        .map(|x| x.outstanding)
        .sum::<u64>() as f64
        / 100.0
}

/// One underwriter, a debtor and a creditor it has backed, and a defaulted
/// insured obligation of `amount` between them. Returns the chain and the
/// original contract id.
fn defaulted(amount: f64) -> (Chain, ContractId) {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let cid = c.lend(2, 1, amount);
    assert!(c.st.contracts[&cid].insured, "the fixture is about the INSURED tier");
    c.default_on(cid);
    (c, cid)
}

// ------------------------------------------------------- the two movements --

/// The creditor is made whole, and by somebody concrete. Their claim on the
/// defaulter is gone; in its place they hold an ordinary claim on the
/// underwriter, who now owes them goods and services — which is the only thing
/// "you lose 1000" can mean in a system with no external redemption.
#[test]
fn a_default_lands_the_creditors_claim_on_the_underwriter() {
    let (c, _cid) = defaulted(1000.0);
    assert_eq!(owed_to(&c, 1, 2), 0.0, "the creditor no longer holds a claim on the defaulter");
    assert!((owed_to(&c, 0, 2) - 1000.0).abs() < 1e-9, "they hold one on the underwriter instead");
    assert!(c.st.members[&0].debt_out == State::to_minor(1000.0), "which is a real debt, cached like any other");
}

/// The defaulter's debt does not vanish; it runs to the underwriter. This is
/// classical subrogation arrived at from inside the model, and it is why
/// `Cure` needs no special case — only a new creditor.
#[test]
fn the_defaulters_debt_subrogates_to_the_underwriter() {
    let (c, cid) = defaulted(1000.0);
    assert!((owed_to(&c, 1, 0) - 1000.0).abs() < 1e-9, "the defaulter owes the underwriter now");
    assert_eq!(c.st.contracts[&cid].creditor, 0, "and it is the same claim, not a new one");
    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Expired, "still in default");
    assert!(c.st.members[&1].rep.open_default == State::to_minor(1000.0), "and still on the record");
}

/// Curing pays the underwriter, who is the party now at risk — and the
/// creditor, already whole, is not paid twice.
#[test]
fn curing_after_substitution_pays_the_underwriter() {
    let (mut c, cid) = defaulted(1000.0);
    let creditor_holds = owed_to(&c, 0, 2);

    c.ok(Tx::Cure { contract: cid, amount: 1000.0 }, &[key(1), key(0)]);
    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Cured);
    assert_eq!(owed_to(&c, 1, 0), 0.0, "the underwriter has recovered");
    assert!(
        (owed_to(&c, 0, 2) - creditor_holds).abs() < 1e-9,
        "and the creditor's position is untouched: they were made whole once"
    );
    assert!(c.st.edges.contains_key(&(0, 1)), "the cure writes a stake, like any other discharge");
}

/// The creditor cannot sign the cure of a claim that is no longer theirs.
/// Every discharge is authorised by the party who loses if it is wrong, and
/// after substitution that party is the underwriter.
#[test]
fn the_old_creditor_can_no_longer_discharge_the_claim() {
    let (mut c, cid) = defaulted(1000.0);
    c.err(Tx::Cure { contract: cid, amount: 1000.0 }, &[key(1), key(2)], ET_MEM_NOT_SIGNER);
    assert!((owed_to(&c, 1, 0) - 1000.0).abs() < 1e-9, "nothing moved");
}

// ------------------------------------------------------------ the sanction --

/// **The defaulter's standing stays consumed.** Substitution must not release
/// the flow the default committed, or the community paying a loss would
/// restore the member who caused it — and stealing through a default would
/// stop costing anything.
#[test]
fn substitution_does_not_give_the_defaulter_their_standing_back() {
    // Two underwriters, backing different people: the creditor draws on the
    // second, so it keeps the headroom to keep writing after the first one's
    // supply is wholly absorbed.
    let mut c = Chain::founded(2, 2);
    let (debtor, creditor) = (2u64, 3u64);
    c.back(0, debtor, SUPPLY);
    c.back(1, creditor, SUPPLY);
    let cid = c.lend(creditor, debtor, SUPPLY);
    c.default_on(cid);

    assert_eq!(c.cap(debtor), 0.0, "their capacity does not come back");
    assert_eq!(c.st.committed_total(), SUPPLY, "the flow they took stays taken");

    // And they cannot do it again: the supply behind them is wholly absorbed.
    let second = c.lend(creditor, debtor, 100.0);
    assert!(!c.st.contracts[&second].insured, "a second obligation falls to the uninsured tier");

    // Only paying gives it back.
    c.ok(Tx::Cure { contract: cid, amount: SUPPLY }, &[key(debtor as usize), key(0)]);
    assert_eq!(c.st.committed_total(), 0.0);
    assert_eq!(c.cap(debtor), SUPPLY);
}

/// **The underwriter's supply stays drawn** — unpaid absorbed losses throttle
/// the declaration automatically, and §Stability's floor stops a withdrawal out from
/// under it. This is the clause that stops an underwriter absorbing losses,
/// never paying them, and insuring the same amount over again.
#[test]
fn an_absorbed_loss_throttles_the_underwriters_declaration() {
    let (mut c, _cid) = defaulted(SUPPLY);
    assert_eq!(c.st.supply_floor(0), edet_state::state::State::to_minor(SUPPLY));
    c.err(Tx::DeclareSupply { member: 0, supply: SUPPLY - 1.0 }, &[key(0)], ET_UWR_BELOW_COMMITTED);

    // Even paying the creditor does not free it. The loss is real and
    // unrecovered until the defaulter makes them good, and the supply behind
    // it is not available to insure anybody else in the meantime.
    let leg =
        c.st.contracts
            .values()
            .find(|x| x.debtor == 0 && x.creditor == 2 && x.status == ContractStatus::Active)
            .expect("the substitution leg")
            .id;
    c.ok(Tx::Settle { contract: leg, amount: SUPPLY }, &[key(0), key(2)]);
    assert_eq!(c.st.supply_floor(0), edet_state::state::State::to_minor(SUPPLY), "still drawn");
    assert_eq!(c.cap(2), 0.0, "and there is nothing left to insure anybody with");
}

/// **The substitution leg is uninsured, deliberately.** An insured one would
/// draw on what OTHERS have put behind this underwriter — the loss walking a
/// hop further out through the graph, which is the path contagion the model
/// refuses. Losses land on source arcs, not on paths.
#[test]
fn the_substitution_leg_does_not_cascade_to_the_next_ring() {
    let mut c = Chain::founded(2, 2);
    // Underwriter 1 has backed underwriter 0, so if the leg were insured
    // there would be somewhere for the loss to go.
    c.back(1, 0, SUPPLY);
    c.back(0, 2, SUPPLY);
    c.back(0, 3, SUPPLY);
    let cid = c.lend(3, 2, 1000.0);
    let drawn_before = c.st.committed_total();

    c.default_on(cid);

    let leg =
        c.st.contracts
            .values()
            .find(|x| x.debtor == 0 && x.creditor == 3)
            .expect("the substitution leg");
    assert!(!leg.insured, "the underwriter's own liability is not underwritten by anybody else");
    assert_eq!(
        c.st.committed_total(),
        drawn_before,
        "and no further supply is drawn: the loss stops at the source arc it came from"
    );
}

/// **The bottom of the waterfall.** If the underwriter defaults on the
/// substituted obligation too, the creditor finally bears the loss — visibly
/// and attributably, because that second default is an ordinary expiry on an
/// uninsured claim and substitutes nothing. Without an external asset the
/// system cannot do better and must not pretend to.
#[test]
fn an_underwriter_who_defaults_too_leaves_the_creditor_bearing_it() {
    let (mut c, _cid) = defaulted(1000.0);
    let leg =
        c.st.contracts
            .values()
            .find(|x| x.debtor == 0 && x.creditor == 2 && x.status == ContractStatus::Active)
            .expect("the substitution leg")
            .id;
    let contracts_before = c.st.contracts.len();

    c.default_on(leg);

    assert_eq!(c.st.contracts[&leg].status, ContractStatus::Expired);
    assert_eq!(c.st.contracts.len(), contracts_before, "nothing further is minted — there is nobody left to bill");
    assert!(c.st.members[&0].rep.open_default == State::to_minor(1000.0), "and the underwriter's failure is on record");
}

// ---------------------------------------------------- who is substituted --

/// A creditor who is themselves the only underwriter behind the claim insured
/// it themselves, and there is nothing to move. The same clause that stops a
/// coalition underwriting itself, one level down.
#[test]
fn a_creditor_who_underwrote_their_own_claim_simply_bears_it() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    let cid = c.lend(0, 1, 1000.0);
    let contracts_before = c.st.contracts.len();

    c.default_on(cid);

    assert_eq!(c.st.contracts.len(), contracts_before, "no leg is minted toward oneself");
    assert_eq!(c.st.contracts[&cid].creditor, 0, "and the claim does not move");
    assert_eq!(c.st.committed_total(), 1000.0, "the flow stays committed, as a default always leaves it");
}

/// An uninsured default substitutes nothing: nobody stood behind it, so the
/// creditor bears it alone — which is exactly what §Recourse promised when they chose
/// to lend beyond the debtor's capacity.
#[test]
fn an_uninsured_default_substitutes_nothing() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, 10.0);
    c.back(0, 2, SUPPLY);
    let cid = c.lend(2, 1, 400.0);
    assert!(!c.st.contracts[&cid].insured);
    let contracts_before = c.st.contracts.len();

    c.default_on(cid);
    assert_eq!(c.st.contracts.len(), contracts_before);
    assert_eq!(c.st.contracts[&cid].creditor, 2, "the creditor keeps a claim nobody will make good");
}

/// The crank is idempotent, so two nodes running it in the same block cannot
/// substitute twice. The second call finds a claim that is no longer Active
/// and refuses.
#[test]
fn a_second_crank_cannot_substitute_the_same_loss_again() {
    let (mut c, cid) = defaulted(1000.0);
    let contracts_before = c.st.contracts.len();
    let debt_before = c.total_debt();

    for _ in 0..3 {
        c.err(Tx::MarkExpired { contract: cid }, &[], ET_CTR_BAD_STATE);
    }
    assert_eq!(c.st.contracts.len(), contracts_before);
    assert!((c.total_debt() - debt_before).abs() < 1e-9);
}

// ---------------------------------------------------------- over the set --

/// **One supply, lent once, and absorbed once.** k debtors all backed by the
/// same underwriter, all defaulting: the total the underwriter is substituted
/// into is bounded by what they declared, however many defaults there are.
///
/// This is the exact scene a per-default check passes in every time — each
/// individual substitution is plainly within the supply, and the question is
/// whether the SUM is.
#[test]
fn many_defaults_racing_one_supply_arc_absorb_one_supply_in_total() {
    for k in [2usize, 5, 20] {
        // Underwriter 0 backs every debtor; underwriter 1 backs the creditor,
        // so the creditor keeps the headroom to keep writing while the first
        // supply is absorbed out from under everybody else.
        let mut c = Chain::founded(2, k + 1);
        let debtors: Vec<MemberId> = (2..2 + k as MemberId).collect();
        let creditor = 2 + k as MemberId;
        for &d in &debtors {
            c.back(0, d, SUPPLY);
        }
        c.back(1, creditor, SUPPLY);
        // Each of them looks fully backed on its own; together they share one
        // supply, and here they draw exactly all of it.
        let share = SUPPLY / k as f64;
        let ids: Vec<ContractId> = debtors.iter().map(|&d| c.lend(creditor, d, share)).collect();
        assert!((c.st.committed_total() - SUPPLY).abs() < 0.011, "k={k}: one supply, lent once");
        for &cid in &ids {
            assert!(c.st.contracts[&cid].insured, "k={k}: every one of them is insured");
        }

        // One boundary, every default at once — which is the racing case: the
        // sweep substitutes k losses into one supply arc inside a single
        // `begin_block`, with no ordering anybody chose.
        c.goto(MATURITY + 2);
        for &cid in &ids {
            assert_eq!(c.st.contracts[&cid].status, ContractStatus::Expired, "k={k}: all of them, in one sweep");
        }

        let absorbed: u64 =
            c.st.contracts
                .values()
                .filter(|x| x.debtor == 0 && x.creditor == creditor)
                .filter(|x| matches!(x.status, ContractStatus::Active | ContractStatus::Expired))
                .map(|x| x.outstanding)
                .sum();
        assert!(
            absorbed <= State::to_minor(SUPPLY),
            "k={k}: the underwriter is substituted into {absorbed} against a declared supply of {SUPPLY}"
        );
        // Exact, not within a hundredth: the split partitions integers that
        // are already on the ledger's grid, so the bound is tight by
        // construction rather than to a tolerance.
        assert_eq!(absorbed, State::to_minor(SUPPLY), "k={k}: and the bound must be tight, not vacuous");
        assert!((c.st.committed_total() - SUPPLY).abs() < 0.011, "k={k}: every unit of it still drawn");
    }
}

/// A loss split across several underwriters lands on each in proportion to
/// what their arc actually carried — a lookup, not an estimate, because the
/// obligation already remembers which supply arcs it drew and how much on
/// each.
#[test]
fn a_loss_splits_across_underwriters_by_what_each_arc_carried() {
    let mut c = Chain::founded_with(&[2500.0, 1000.0], 2);
    let (debtor, creditor) = (2u64, 3u64);
    c.back(0, debtor, 2500.0);
    c.back(1, debtor, 1000.0);
    c.back(0, creditor, 100.0);
    assert_eq!(c.cap(debtor), 3500.0, "backed by both");

    let cid = c.lend(creditor, debtor, 3500.0);
    let drawn: Vec<(usize, u64)> = c.st.contracts[&cid].held.supply.clone();
    assert_eq!(drawn.len(), 2, "both arcs carried some of it");

    c.default_on(cid);

    for (u, f) in drawn {
        let owed = owed_to(&c, u as MemberId, creditor);
        let want = edet_state::state::State::from_minor(f);
        assert!(
            (owed - want).abs() < 0.011,
            "underwriter {u} carried {want} and must be substituted into exactly that, not {owed}"
        );
    }
    let total: f64 = (0..2).map(|u| owed_to(&c, u, creditor)).sum();
    assert!((total - 3500.0).abs() < 0.011, "and together, the whole loss: {total}");
    assert!((c.total_debt() - 7000.0).abs() < 0.011, "the defaulter still owes it too, now to the underwriters");
}

/// The split must lose nothing. Every unit of the defaulter's debt lands on
/// some underwriter's subrogated claim, and every unit of the flow the
/// obligation held stays exactly where the caches say it is — §Verification invariant 2
/// asks for equality, not closeness, and the audit after every transition is
/// what enforces it.
#[test]
fn the_split_conserves_the_debt_and_the_flow_it_was_holding() {
    let mut c = Chain::founded_with(&[900.0, 700.0, 500.0], 2);
    let (debtor, creditor) = (3u64, 4u64);
    for u in 0..3 {
        c.back(u, debtor, 900.0);
    }
    c.back(0, creditor, 100.0);
    let cid = c.lend(creditor, debtor, 2100.0);
    let held_before = c.st.contracts[&cid].held.clone();
    let committed_before = c.st.committed_total();
    let reserved_before = c.st.reserved_total();

    c.default_on(cid);

    assert_eq!(c.st.committed_total(), committed_before, "no supply released, none conjured");
    assert_eq!(c.st.reserved_total(), reserved_before, "and the defaulter's edges are exactly where they were");
    let subrogated: u64 =
        c.st.contracts
            .values()
            .filter(|x| x.debtor == debtor && matches!(x.status, ContractStatus::Expired))
            .map(|x| x.outstanding)
            .sum::<u64>();
    assert_eq!(subrogated, State::to_minor(2100.0), "the defaulter owes exactly what they owed");

    // Every piece's held sums back to the original, arc for arc.
    let mut edges: std::collections::BTreeMap<(usize, usize), u64> = Default::default();
    let mut supply: std::collections::BTreeMap<usize, u64> = Default::default();
    for x in c.st.contracts.values().filter(|x| x.debtor == debtor) {
        for &(k, a) in &x.held.edges {
            *edges.entry(k).or_default() += a;
        }
        for &(u, a) in &x.held.supply {
            *supply.entry(u).or_default() += a;
        }
    }
    for &(k, a) in &held_before.edges {
        assert_eq!(edges.get(&k).copied().unwrap_or(0), a, "arc {k:?} lost or gained in the split");
    }
    for &(u, a) in &held_before.supply {
        assert_eq!(supply.get(&u).copied().unwrap_or(0), a, "supply arc {u} lost or gained in the split");
    }
}

/// A defaulter who cures every subrogated piece pays each underwriter what
/// that underwriter was substituted into, and gets the whole of their standing
/// back — nothing is stranded by the split.
#[test]
fn curing_every_piece_makes_each_underwriter_whole_and_frees_the_flow() {
    let mut c = Chain::founded_with(&[2500.0, 1000.0], 2);
    let (debtor, creditor) = (2u64, 3u64);
    c.back(0, debtor, 2500.0);
    c.back(1, debtor, 1000.0);
    c.back(0, creditor, 100.0);
    let cid = c.lend(creditor, debtor, 3500.0);
    c.default_on(cid);

    let pieces: Vec<(ContractId, MemberId, f64)> =
        c.st.contracts
            .values()
            .filter(|x| x.debtor == debtor && x.status == ContractStatus::Expired)
            .map(|x| (x.id, x.creditor, State::from_minor(x.outstanding)))
            .collect();
    assert_eq!(pieces.len(), 2);
    for (id, underwriter, amount) in pieces {
        c.ok(Tx::Cure { contract: id, amount }, &[key(debtor as usize), key(underwriter as usize)]);
    }

    assert_eq!(c.st.committed_total(), 0.0, "every supply arc is free again");
    assert!(c.st.members[&debtor].rep.open_default <= c.st.params.dust_minor(), "and the default is fully cured");
    assert_eq!(c.cap(debtor), 3500.0, "with the whole of their standing back");
}

/// A defaulter is not released by the community having paid. Substitution
/// moves who is owed; it does not shrink what is owed, and the audit's
/// conservation check is what says so after every step.
#[test]
fn substitution_moves_the_claim_without_shrinking_it() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let cid = c.lend(2, 1, 1000.0);
    let debt_before = c.total_debt();

    c.default_on(cid);

    assert!(
        (c.total_debt() - debt_before - 1000.0).abs() < 1e-9,
        "total obligation grows by exactly the loss: the underwriter really did accept a liability"
    );
    assert!((owed_to(&c, 1, 0) - 1000.0).abs() < 1e-9, "the defaulter's half");
    assert!((owed_to(&c, 0, 2) - 1000.0).abs() < 1e-9, "and the underwriter's");
}

// ----------------------------------------- the tier is the whole of the rule --

/// **The self-insured insured default, which a loss pool would pay** —
/// the construction, and the reason there is no pool (§Recourse) rather
/// than patched.
///
/// `substitute` leaves through an early return when every supply arc behind a
/// claim is the creditor's own, and a `pool_covered = false` written after
/// the loop sits *after* that return. So a claim could be insured AND
/// pool-covered at once, and `PoolClaim` then wrote the loss off the defaulted
/// row, re-held it, and minted restitution from the covenanters. Measured
/// with such a layer, on this exact scene: **D's outstanding 300 → 0, D's capacity
/// restored to 300, the honest covenanter H drained 500 → 200.60, and D left
/// owing 0.60** — its own 1/501 restitution share. The defaulter walked and the
/// members who paid held nothing.
///
/// What this asserts is the model with no pool in it: the loss stays exactly
/// where §Recourse and §Recourse put it. A tier cannot be enforced by a flag cleared inside
/// one branch of something else, so the fix is at the tier.
#[test]
fn a_self_insured_default_releases_nobody_and_drains_nobody() {
    let mut c = Chain::founded(1, 2);
    // D (1) earns standing from U (0) alone, so U is the only underwriter
    // behind anything D borrows.
    c.back(0, 1, 500.0);
    // H (2) is the honest third party a pool would conscript.
    c.back(0, 2, SUPPLY);

    let cid = c.lend(0, 1, 300.0);
    assert_eq!(c.st.contracts[&cid].held.supply, vec![(0, 30_000)], "insured by its own creditor and nobody else");

    let (contracts_before, debt_before) = (c.st.contracts.len(), c.total_debt());
    let h_debt_before = c.st.members[&2].debt_out;
    c.default_on(cid);

    assert!((c.outstanding(cid) - 300.0).abs() < 1e-9, "the loss is not written off: nothing pays it");
    assert_eq!(c.st.contracts[&cid].creditor, 0, "and it does not move — there is nobody to move it to");
    assert!(c.st.members[&1].debt_out == State::to_minor(300.0), "the defaulter still owes the whole of it");
    assert!(c.st.members[&1].rep.open_default == State::to_minor(300.0), "and is on record for it");
    assert_eq!(c.st.committed_total(), 300.0, "their flow stays consumed, which is the sanction");
    assert_eq!(c.cap(1), 0.0, "so no standing comes back");
    assert_eq!(c.st.contracts.len(), contracts_before, "no leg, no restitution, no second row anywhere");
    assert!((c.total_debt() - debt_before).abs() < 1e-9, "and total obligation is unmoved by a loss nobody moved");
    assert_eq!(c.st.members[&2].debt_out, h_debt_before, "H signed nothing and owes nothing");
}

/// **A signature manufactures no community backing.**
///
/// A pool commitment was uncapped and unreserved, so coverage was a free
/// signal: measured, a member holding **1.0** of standing committed **1e9**,
/// was recorded as 1,000,000,501 of "balance", and minted **399.9998** of a 400
/// claim while an honest member's 500 minted **nothing** — an uninsured
/// obligation the attacker never honours. A debtor self-covered with 1.0 to
/// show "pool covered" to every creditor, because coverage was keyed on the
/// debtor's own covenant.
///
/// After §Recourse there is exactly one quantity that says the community stands
/// behind a claim, and it is the cut. This is that assertion at the quantifier
/// the free-signature bound uses: whatever A signs, A confers what A holds.
#[test]
fn no_signature_manufactures_community_backing() {
    let mut c = Chain::founded(1, 4);
    // A (1) holds one honoured purchase and nothing more — enough to write
    // with, and nowhere near enough to carry anybody.
    c.back(0, 1, 50.0);
    c.back(0, 2, SUPPLY); // H, honest and well backed
    c.back(0, 3, 10.0); // D, the thin debtor
    c.back(0, 4, SUPPLY); // C, who lends

    assert!((c.st.conferrable(1) - 50.0).abs() < 1e-9, "A confers exactly what A holds");
    assert!(!c.st.fits_capacity(3, 400.0), "and D cannot carry 400 insured");

    let cid = c.lend(4, 3, 400.0);
    assert!(!c.st.contracts[&cid].insured, "so the claim sits in the uninsured tier, where §Recourse leaves it");
    assert_eq!(c.st.contracts[&cid].held.supply, vec![], "backed by nothing, which is what uninsured means");

    // A signs everything A can, on its own behalf and D's. None of it is a
    // declaration of coverage, because there is no such transition — and none
    // of it moves the tier.
    //
    // The 1e9 is the point of comparison. `PoolCommit` took it: a commitment
    // was checked only against the member's own self-declared ceiling, so 1.0
    // of standing pledged a billion. The nearest thing the alphabet still has
    // is `DeclareSupply`, and it is refused outright — a declaration is capped
    // by what the community has actually put behind the declarer (plus their
    // external seed), which is the cut. That cap is what a covenant would need and what
    // a covenant could never carry, because a pledge reserves nothing at the
    // moment it matters.
    c.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(3, 1.0)] }, &[key(1)]);
    c.err(Tx::DeclareSupply { member: 1, supply: 1e9 }, &[key(1)], ET_UWR_ABOVE_CAPACITY);
    assert!((c.st.conferrable(1) - 50.0).abs() < 1e-9, "A confers what A held before signing all of it");
    assert!(!c.st.fits_capacity(3, 400.0), "and D still cannot carry 400");
    assert!(!c.st.contracts[&cid].insured, "the live claim is not upgraded by anybody's signature");

    // And the default lands where it was always going to land.
    let h_debt_before = c.st.members[&2].debt_out;
    let contracts_before = c.st.contracts.len();
    c.default_on(cid);
    assert!((c.outstanding(cid) - 400.0).abs() < 1e-9, "the creditor bears it, alone and in full");
    assert_eq!(c.st.contracts.len(), contracts_before, "nothing is minted from anybody who did not sign");
    assert_eq!(c.st.members[&2].debt_out, h_debt_before, "least of all from H");
}

// ------------------------------------------------------------- the throttle --

/// A defaulted insured claim subrogated to `U_b` (id 1), with `U_a` (id 0) also
/// reaching the debtor through one honoured uninsured purchase. Returns the
/// chain and the subrogated claim.
///
/// The honoured purchase is the whole point of the fixture: it is what gives
/// `flow::reserve` a second, cheaper arc to find, and `U_a` is id 0 so Dinic
/// finds it FIRST.
fn subrogated_with_a_second_arc() -> (Chain, ContractId) {
    let mut c = Chain::founded(2, 2);
    c.back(1, 2, 300.0);
    let cid = c.lend(3, 2, 300.0);
    assert_eq!(c.st.contracts[&cid].held.supply, vec![(1, 30_000)], "insured by U_b alone");
    c.default_on(cid);
    assert_eq!(c.st.contracts[&cid].creditor, 1, "and subrogated to U_b");
    c.back(0, 2, 300.0);
    (c, cid)
}

/// **The throttle stays on the underwriter that was substituted** —
/// construction, and §Recourse's load-bearing line made true.
///
/// "The supply arc stays committed until the defaulter repays the underwriter"
/// assumed the hold left after a partial repayment sits where the old one sat.
/// `rehold` released the whole thing and re-solved, so the defaulter's own
/// honoured trade with a third party handed the solver a cheaper arc. Measured
/// on a re-solving cure, on this fixture: a cure of 100 moves **committed `{U_b: 300}`
/// → `{U_a: 200}`** — U_b, who bore a loss it has not paid, free to insure 300
/// again, and U_a's supply frozen indefinitely by a claim owed to somebody else.
#[test]
fn a_partial_cure_leaves_the_throttle_on_the_underwriter_that_was_substituted() {
    let (mut c, cid) = subrogated_with_a_second_arc();

    c.ok(Tx::Cure { contract: cid, amount: 100.0 }, &[key(2), key(1)]);

    let held = &c.st.contracts[&cid].held;
    assert_eq!(held.supply, vec![(1, 20_000)], "the piece still draws U_b, and only U_b");
    assert_eq!(held.edges, vec![((1, 2), 20_000)], "on the arc that was holding it");
    assert_eq!(c.st.supply_floor(1), 20_000, "U_b stays throttled by what it has not been repaid");
    assert_eq!(c.st.supply_floor(0), 0, "and U_a, who was never substituted, is free");
    assert!((c.outstanding(cid) - 200.0).abs() < 1e-9);
}

/// **The same repayment through a sale, which the audit did not name.**
/// `net_mutual` partially discharges Expired claims too — a subrogated claim
/// runs debtor → underwriter, so the defaulter delivering goods to the
/// underwriter it owes nets against it — and it reaches the same `rehold`.
/// Measured with a re-solve on this fixture: `committed {U_b: 300}` →
/// **`{U_a: 200}`**, identically. One code path, two economic acts, and the
/// audit had named one of them.
#[test]
fn netting_a_subrogated_claim_repays_the_same_underwriter() {
    let (mut c, cid) = subrogated_with_a_second_arc();

    c.ok(
        Tx::Sale { seller: Party::Member(2), buyer: Party::Member(1), amount: 100.0, maturity_epochs: MATURITY },
        &[key(2), key(1)],
    );

    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Expired, "a partial net leaves it defaulted");
    assert_eq!(c.st.contracts[&cid].held.supply, vec![(1, 20_000)], "and repays U_b, not U_a");
    assert_eq!(c.st.supply_floor(0), 0, "U_a is untouched");
}

/// Repaying every unit frees every unit, and the supply lands exactly on zero
/// rather than near it.
#[test]
fn curing_a_piece_to_the_end_frees_exactly_what_it_held() {
    let (mut c, cid) = subrogated_with_a_second_arc();
    // A third of what is left each time, never below the installment floor,
    // and the last payment whole.
    let floor = State::from_minor(c.st.contracts[&cid].original.div_ceil(edet_kernel::constants::MAX_INSTALLMENTS));
    for _ in 0..40 {
        let left = c.outstanding(cid);
        if left <= c.st.params.dust {
            break;
        }
        let amount = if left <= 2.0 * floor { left } else { (left / 3.0).max(floor) };
        c.ok(Tx::Cure { contract: cid, amount }, &[key(2), key(1)]);
        let ct = c.st.contracts[&cid].clone();
        if ct.status == ContractStatus::Expired {
            assert_eq!(ct.held.amount(), ct.outstanding, "hold vs book");
            assert_eq!(ct.held.supply.iter().map(|&(u, _)| u).collect::<Vec<_>>(), vec![1]);
        }
    }
    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Cured);
    assert_eq!(c.st.committed_total(), 0.0, "every unit of supply is free again");
    assert_eq!(c.st.reserved_total(), 0.0, "and so is every arc");
}

/// A re-denomination and THEN a partial cure. The old path re-reserved and could
/// come up short — the fallback its own comment calls unreachable; a shrink
/// cannot, because it hands back a share of what it already holds. Stronger by
/// construction rather than by luck, at every factor the boundary suite uses.
#[test]
fn a_cure_after_a_re_denomination_keeps_its_insurance_and_its_underwriter() {
    for (num, den) in [(2u64, 3u64), (3, 2), (5, 7), (7, 5)] {
        let (mut c, cid) = subrogated_with_a_second_arc();
        c.st.rescale(num as f64 / den as f64);
        edet_state::invariants::audit(&c.st).expect("the re-denomination itself must hold");
        let before = c.st.contracts[&cid].clone();
        // The smallest partial the installment floor admits, and never one
        // that closes the row: a downward re-denomination can take `dust`
        // below the finest unit the ledger holds, and an amount that rounds to
        // nothing is refused rather than booked.
        let pay = before.original.div_ceil(edet_kernel::constants::MAX_INSTALLMENTS).max(1);
        if !before.insured || before.outstanding <= pay + 2 {
            continue;
        }

        c.ok(Tx::Cure { contract: cid, amount: State::from_minor(pay) }, &[key(2), key(1)]);

        let after = c.st.contracts[&cid].clone();
        assert!(after.insured, "a cure after {num}/{den} lost the insurance the fallback was meant to keep");
        assert_eq!(after.held.supply.iter().map(|&(u, _)| u).collect::<Vec<_>>(), vec![1], "at {num}/{den}");
        assert_eq!(after.held.amount(), after.outstanding, "at {num}/{den}");
    }
}

/// The shrink is per-arc and the supply side is allocated globally, so the two
/// could in principle disagree — A3's shape, one transition over: a hold whose
/// supply claims more than its own kept arcs can deliver. Probed on the topology
/// that has an intermediary in it, one underwriter reaching the debtor down TWO
/// two-hop paths, at every cure amount from one minor unit to four hundred.
#[test]
fn a_shrunk_hold_never_claims_more_supply_than_its_arcs_deliver() {
    let mut checked = 0;
    for step in 0..400u64 {
        let mut c = Chain::founded(1, 4);
        c.back(0, 1, 1000.0);
        c.back(0, 2, 1001.0);
        c.back(1, 3, 1000.0);
        c.back(2, 3, 1001.0);
        let room = c.cap(3);
        let cid = c.lend(4, 3, room);
        if !c.st.contracts[&cid].insured {
            continue;
        }
        c.default_on(cid);
        // Four hundred cure amounts, one minor unit apart, starting at the
        // smallest partial the installment floor admits: this probe is about
        // the shrink and not the floor.
        let cure_minor = c.st.contracts[&cid].original.div_ceil(edet_kernel::constants::MAX_INSTALLMENTS) + step;
        let amount = cure_minor as f64 / 100.0;
        if amount >= c.outstanding(cid) {
            continue;
        }
        let creditor = c.st.contracts[&cid].creditor;
        c.ok(Tx::Cure { contract: cid, amount }, &[key(3), key(creditor as usize)]);

        let ct = &c.st.contracts[&cid];
        let mut arcs: std::collections::BTreeMap<(usize, usize), u64> = Default::default();
        for &(k, a) in &ct.held.edges {
            *arcs.entry(k).or_insert(0) += a;
        }
        let claimed = ct.held.amount();
        let deliverable = edet_kernel::flow::capacity(
            &arcs,
            &Default::default(),
            &Default::default(),
            &ct.held.supply,
            &[ct.debtor as usize],
            c.st.next_member as usize,
            claimed,
        );
        assert_eq!(deliverable, claimed, "cure of {amount}: supply claims {claimed}, arcs deliver {deliverable}");
        checked += 1;
    }
    assert!(checked > 300, "only {checked} cures actually landed — the sweep is vacuous");
}

/// Randomised: many defaulted scenes, each with an arc the solver would have
/// preferred, and a partial cure of every piece. The audit runs after every
/// transition; what this adds is that no piece's throttle ever wanders and the
/// hold never disagrees with the book.
#[test]
fn randomised_partial_cures_never_move_a_throttle() {
    struct R(u64);
    impl R {
        fn pick(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }
    let mut rng = R(0x7E11_7A1E_5EED_0001);
    let (mut scenes, mut cures) = (0, 0);
    for _ in 0..120 {
        let k = 2 + rng.pick(3) as usize;
        let supplies: Vec<f64> = (0..k)
            .map(|_| 200.0 + rng.pick(2000) as f64 + rng.pick(100) as f64 / 100.0)
            .collect();
        let mut c = Chain::founded_with(&supplies, 3);
        let (d, cr) = (k as u64, k as u64 + 1);
        let mut backed = 0;
        for u in 0..k as u64 {
            if rng.pick(3) > 0 {
                c.back(u, d, 100.0 + rng.pick(500) as f64 + rng.pick(100) as f64 / 100.0);
                backed += 1;
            }
        }
        if backed == 0 {
            continue;
        }
        let room = c.cap(d);
        if room <= 1.0 {
            continue;
        }
        let cid = c.lend(cr, d, (room * (50 + rng.pick(51)) as f64 / 100.0).max(1.0));
        if !c.st.contracts[&cid].insured {
            continue;
        }
        c.default_on(cid);
        scenes += 1;
        // The defaulter honours an uninsured purchase from an underwriter that
        // did NOT back it — the arc a re-solve walks onto.
        for u in 0..k as u64 {
            if !c.st.edges.contains_key(&(u as usize, d as usize)) {
                c.back(u, d, 100.0 + rng.pick(300) as f64);
                break;
            }
        }
        let pieces: Vec<(ContractId, MemberId)> =
            c.st.contracts
                .values()
                .filter(|x| x.debtor == d && x.status == ContractStatus::Expired && x.insured)
                .map(|x| (x.id, x.creditor))
                .collect();
        for (id, uw) in pieces {
            let left = c.outstanding(id);
            let pay = (left * (1 + rng.pick(90)) as f64 / 100.0).max(0.01);
            if left <= 3.0 * c.st.params.dust || pay >= left {
                continue;
            }
            c.ok(Tx::Cure { contract: id, amount: pay }, &[key(d as usize), key(uw as usize)]);
            cures += 1;
            let ct = c.st.contracts[&id].clone();
            if ct.status == ContractStatus::Expired {
                assert_eq!(
                    ct.held.supply.iter().map(|&(u, _)| u as MemberId).collect::<Vec<_>>(),
                    vec![uw],
                    "piece {id} moved its throttle off underwriter {uw}"
                );
                assert_eq!(ct.held.amount(), ct.outstanding, "piece {id}");
            }
        }
    }
    assert!(scenes > 40 && cures > 40, "not enough scenes ({scenes}) or cures ({cures}) — the sweep is vacuous");
    // Printed rather than pinned: the floor above is the gate, and a document
    // quoting "114 scenes, 204 cures" should be able to read the figure off a
    // run rather than off a written claim.
    println!("randomised partial cures: {scenes} scenes, {cures} cures");
}

/// **An underwriter's LOSS is bounded; their DURATION is not** (decided
/// rather than fixed).
///
/// Committed flow never releases until the defaulter repays — that is §Recourse's
/// sanction, and it is exact — so an underwriter who has honoured the
/// substitution leg IN FULL is still floored at the committed flow: cannot
/// withdraw, cannot `Exit`, cannot leave the role, for as long as the defaulter
/// chooses. Measured here rather than argued, because the alternative is worse
/// and this gate is what stops it: releasing committed flow on a timer would let
/// an underwriter absorb a loss, wait, and insure the same amount again, which is
/// exactly what E's fix and §Recourse's throttle exist to prevent.
///
/// So this is a consequence to state, not a defect to close. "An underwriter's
/// failure is bounded" is true of loss and false of time.
#[test]
fn an_underwriter_who_has_paid_in_full_is_still_floored() {
    let mut c = Chain::founded(2, 2);
    let (d, cr) = (2u64, 3u64);
    c.back(0, d, 500.0);
    c.back(1, cr, SUPPLY);
    let cid = c.lend(cr, d, 500.0);
    assert!(c.st.contracts[&cid].insured);
    c.default_on(cid);

    // The underwriter pays the creditor everything the substitution leg owes.
    let leg =
        c.st.contracts
            .values()
            .find(|x| x.debtor == 0 && x.creditor == cr && x.status == ContractStatus::Active)
            .expect("the substitution leg")
            .id;
    let owed = c.outstanding(leg);
    c.ok(Tx::Settle { contract: leg, amount: owed }, &[key(0), key(cr as usize)]);
    assert!(owed > 0.0);

    // And is still held, by the defaulter's choice alone.
    assert_eq!(c.st.supply_floor(0), edet_state::state::State::to_minor(500.0), "the flow stays committed");
    c.err(Tx::DeclareSupply { member: 0, supply: 0.0 }, &[key(0)], ET_UWR_BELOW_COMMITTED);
    c.err(Tx::Exit { member: 0 }, &[key(0)], ET_UWR_STILL_DECLARED);
    assert!((c.outstanding(cid) - 500.0).abs() < 1e-9, "and only the defaulter can end it");

    // Curing is the one thing that frees them, and it is not theirs to do.
    c.ok(Tx::Cure { contract: cid, amount: 500.0 }, &[key(d as usize), key(0)]);
    assert_eq!(c.st.supply_floor(0), 0, "the defaulter repaying is what releases the supply");
    c.ok(Tx::DeclareSupply { member: 0, supply: 0.0 }, &[key(0)]);
}

/// The same lock with **no default at all** is bounded in TIME by the insured
/// horizon, measured from the ACCEPTANCE: debtor and creditor together can
/// hold an underwriter's supply for `Params::insured_horizon_epochs` past the
/// acceptance — a year at genesis — and the extension that crosses it drops
/// the insurance and returns the flow, on the creditor's own signature. The
/// underwriter is not asked and need not be: the horizon is the electorate's
/// standing answer, and keeping a claim insured longer is a settle and a
/// re-acceptance against the current cut, which is where the underwriters are
/// asked again. Asking them inside the transition would discover them there
/// and hand each a veto over ordinary trade, which is the shape the cascade
/// refuses too.
#[test]
fn extend_holds_a_supply_only_to_the_insured_horizon_from_acceptance() {
    let mut c = Chain::founded(2, 2);
    let (d, cr) = (2u64, 3u64);
    c.back(0, d, 500.0);
    c.back(1, cr, SUPPLY);
    let cid = c.lend(cr, d, 500.0);
    let h = c.st.params.insured_horizon_epochs();
    let edge = c.st.epoch + h;

    c.ok(Tx::Extend { contract: cid, new_maturity_epoch: edge }, &[key(d as usize), key(cr as usize)]);
    assert_eq!(c.st.contracts[&cid].maturity_epoch, edge);
    assert_eq!(c.st.supply_floor(0), edet_state::state::State::to_minor(500.0));
    c.err(Tx::DeclareSupply { member: 0, supply: 0.0 }, &[key(0)], ET_UWR_BELOW_COMMITTED);

    c.ok(Tx::Extend { contract: cid, new_maturity_epoch: edge + 1 }, &[key(d as usize), key(cr as usize)]);
    assert!(!c.st.contracts[&cid].insured);
    assert_eq!(c.st.supply_floor(0), 0, "the flow returned: the underwriter may leave");
    c.ok(Tx::DeclareSupply { member: 0, supply: 0.0 }, &[key(0)]);
    // The one lever is the dial, and its genesis value.
    assert_eq!(h, edet_kernel::constants::INSURED_HORIZON_EPOCHS);
    assert_eq!(edet_kernel::constants::INSURED_HORIZON_EPOCHS, 365);
    assert_eq!(edet_kernel::constants::EPOCH_SECS, 86_400);
}

// ------------------------------------------------------- the review's probes --

mod final_review_panel {
    use super::*;
    use std::collections::BTreeSet;

    /// **The panel survives substitution, and the award runs between the
    /// parties it was consented by.** The row becomes the underwriter's claim
    /// at a default, and dropping the panel with the change of creditor closed
    /// the buyer's remedy for non-delivery in the one case a buyer withholds
    /// payment. The terms name the parties they bind, so the award is minted
    /// from the original creditor to the original debtor, bounded by the
    /// original amount, and never against the underwriter.
    ///
    /// Mutation that bites: clear `arb` in `substitute`, or read the award's
    /// parties off the row.
    #[test]
    fn the_panel_survives_substitution_and_the_award_runs_between_the_original_parties() {
        let mut c = Chain::founded(1, 3);
        let (u, d, cr, arb) = (0u64, 1u64, 2u64, 3u64);
        c.back(u, d, 500.0);
        let cid = c.st.next_contract;
        c.ok(
            Tx::Accept {
                debtor: Party::Member(d),
                creditor: Party::Member(cr),
                amount: 300.0,
                maturity_epochs: 30,
                arb: Some(ArbTermsWire {
                    arbiters: BTreeSet::from([arb]),
                    quorum: 1,
                    window_epochs: 90,
                    award_cap: 300.0,
                }),
            },
            &[key(d as usize), key(cr as usize)],
        );
        assert!(c.st.contracts[&cid].insured);
        c.default_on(cid);
        let row = c.st.contracts[&cid].clone();
        assert_eq!(row.creditor, u, "the row is the underwriter's claim now");
        let terms = row.arb.expect("the panel survives the change of creditor");
        assert_eq!((terms.debtor, terms.creditor, terms.amount), (d, cr, State::to_minor(300.0)));

        c.ok(Tx::ArbAttest { contract: cid, arbiter: arb, amount: 300.0 }, &[key(arb as usize)]);
        c.goto(92);
        let award =
            c.st.contracts
                .values()
                .find(|x| x.debtor == cr && x.creditor == d)
                .expect("minted at the window's close, from the seller to the buyer");
        assert_eq!(award.outstanding, State::to_minor(300.0));
        assert!(!award.insured);
        assert!(
            c.st.contracts.values().all(|x| !(x.debtor == u && x.creditor == d)),
            "nothing is minted against the underwriter, who was never asked"
        );
    }
}
