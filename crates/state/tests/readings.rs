//! Readings the paper cites, measured: how insured capacity grows, what a
//! suspension can and cannot freeze, what a seat costs, and what decay measures
//! — each the sentence a claim rests on,
//! against the real transition function, audited after every call.

mod common;

use common::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

fn k(id: MemberId) -> Key {
    key(id as usize)
}

/// The claim "every increase in insured credit is preceded by an uninsured
/// loan of the same size". Measured across THREE creditors the seed reaches.
#[test]
fn insured_capacity_grows_through_insured_loans_alone() {
    let mut c = Chain::founded_with(&[10_000.0], 4);
    let (u, s, t, w, n) = (0, 1, 2, 3, 4);
    c.back(u, s, 1000.0);
    c.back(u, t, 1000.0);
    c.back(u, w, 1000.0);
    assert_eq!(c.cap(n), 0.0);

    // The one uninsured loan: the first.
    let first = c.lend(s, n, 50.0);
    assert!(!c.st.contracts[&first].insured, "nobody has staked on N yet");
    c.settle(first, 50.0);
    assert_eq!(c.cap(n), 50.0);

    // From here every loan is inside N's capacity, insured, and each
    // settlement writes a NEW creditor's edge or raises an old one.
    let mut uninsured = 50.0;
    let mut insured = 0.0;
    let plan = [(t, 50.0), (w, 100.0), (s, 200.0), (t, 350.0), (w, 650.0), (s, 1200.0), (t, 2000.0), (w, 2650.0)];
    let mut caps = vec![c.cap(n)];
    for &(cred, amt) in &plan {
        assert!(amt <= c.cap(n), "the plan lends inside capacity: {amt} vs {}", c.cap(n));
        let id = c.lend(cred, n, amt);
        assert!(c.st.contracts[&id].insured, "a loan inside capacity holds a reservation");
        insured += amt;
        c.settle(id, amt);
        caps.push(c.cap(n));
    }
    eprintln!("capacity after each insured round: {caps:?}");
    eprintln!("uninsured lent {uninsured:.2}, insured lent {insured:.2}, final capacity {:.2}", c.cap(n));
    uninsured += 0.0;
    assert_eq!(c.cap(n), 3000.0, "the whole reach of the three creditors, all of it earned on insured loans");
    assert!(c.cap(n) >= 60.0 * uninsured);
}

/// The honest half of the same claim: within ONE creditor the stake is a peak, so
/// growth needs a loan above the peak, and reservation is all-or-nothing, so
/// that loan is uninsured in full — not just its increment.
#[test]
fn within_one_creditor_growth_is_uninsured_and_whole() {
    let mut c = Chain::founded_with(&[10_000.0], 2);
    let (u, s, n) = (0, 1, 2);
    c.back(u, s, 1000.0);
    c.back(s, n, 50.0);
    assert_eq!(c.cap(n), 50.0);

    let same = c.lend(s, n, 50.0);
    assert!(c.st.contracts[&same].insured);
    c.settle(same, 50.0);
    assert_eq!(c.cap(n), 50.0, "a repeated cycle at the peak grows nothing (the wash rule)");

    let bigger = c.lend(s, n, 60.0);
    assert!(!c.st.contracts[&bigger].insured, "60 does not fit 50, and reserve is all-or-nothing");
    c.settle(bigger, 60.0);
    assert_eq!(c.cap(n), 60.0);
    eprintln!("one creditor: 50 insured repeats hold 50; the 60 that raises it is uninsured in full");
}

/// The claim "every member costs the community real liability". A seat is a
/// reservation on a layer credit never reads.
#[test]
fn a_seat_consumes_no_insurable_capacity_and_no_supply() {
    let mut c = Chain::founded_with(&[10_000.0], 1);
    let (u, s) = (0, 1);
    c.back(u, s, 1000.0);
    let (cap_before, seat_before, seed_before, committed_before) =
        (c.cap(s), c.st.seat_reach(s), c.st.external_seed(), c.st.committed_total());
    let newcomer = stranger_key(1);
    c.ok(
        Tx::Accept {
            debtor: Party::Key(newcomer),
            creditor: Party::Member(s),
            amount: 10.0,
            maturity_epochs: 30,
            arb: None,
        },
        &[k(s), newcomer],
    );
    let seated = c.st.member_of_key(&newcomer).expect("the trade seated the row");
    eprintln!(
        "seating one row: credit capacity of the sponsor {cap_before:.2} → {:.2}, seat reach {seat_before:.2} → {:.2}, seed {seed_before:.2} → {:.2}, committed {committed_before:.2} → {:.2}",
        c.cap(s), c.st.seat_reach(s), c.st.external_seed(), c.st.committed_total()
    );
    assert_eq!(c.cap(s), cap_before, "credit capacity never reads the seat layer");
    assert_eq!(c.st.seat_reach(s), seat_before - 20.0, "one bond unit of SEAT reach, at the genesis unit");
    assert_eq!(c.st.external_seed(), seed_before, "no supply moved");
    assert_eq!(c.st.committed_total(), committed_before, "and nothing was drawn: the newcomer's loan is uninsured");
    assert_eq!(c.cap(seated), 0.0, "and the row itself has nothing");
}

/// The claim "the sanction can freeze governance". The bar is half the seed
/// with the suspended weight still in the denominator, so no sequence of
/// suspensions takes the live electorate below half.
#[test]
fn a_suspension_cannot_take_the_live_electorate_below_half() {
    let mut c = Chain::founded_with(&[3000.0, 3000.0, 4000.0], 1);
    let (a, b, cc) = (0, 1, 2);
    c.ok(Tx::Propose { author: a, kind: ProposalKind::Suspend { member: b } }, &[k(a)]);
    let pid = *c.st.proposals.keys().max().unwrap();
    c.ok(Tx::Assent { member: a, proposal: pid }, &[k(a)]);
    assert!(!c.st.proposals[&pid].enacted, "0.3 of the seed is under the bar");
    c.ok(Tx::Assent { member: cc, proposal: pid }, &[k(cc)]);
    assert!(c.st.proposals[&pid].enacted, "0.7 of the seed enacts");
    assert_eq!(c.status(b), MemberStatus::Suspended);

    // Now C alone (0.4) tries to suspend A (0.3): the live non-target mass is
    // 0.4 < 0.5, and B may vote on nothing but reinstatement.
    c.ok(Tx::Propose { author: cc, kind: ProposalKind::Suspend { member: a } }, &[k(cc)]);
    let pid2 = *c.st.proposals.keys().max().unwrap();
    c.ok(Tx::Assent { member: cc, proposal: pid2 }, &[k(cc)]);
    assert!(!c.st.proposals[&pid2].enacted);
    c.err(Tx::Assent { member: b, proposal: pid2 }, &[k(b)], edet_state::errors::ET_MEM_NOT_ACTIVE);
    assert_eq!(c.status(a), MemberStatus::Active);

    // And the suspended 0.3 needs only 0.2 more to come back: A alone suffices.
    // A suspended member has no write budget of its own; A co-signs and pays.
    c.ok(Tx::Propose { author: b, kind: ProposalKind::Unsuspend { member: b } }, &[k(a), k(b)]);
    let pid3 = *c.st.proposals.keys().max().unwrap();
    c.ok(Tx::Assent { member: b, proposal: pid3 }, &[k(a), k(b)]);
    assert!(!c.st.proposals[&pid3].enacted, "a minority cannot reinstate itself");
    c.ok(Tx::Assent { member: a, proposal: pid3 }, &[k(a)]);
    assert!(c.st.proposals[&pid3].enacted);
    assert_eq!(c.status(b), MemberStatus::Active);
    eprintln!("30/30/40: B suspended by A+C; C alone cannot suspend A; B+A reinstate B");
}

/// The seasonal producer. Three members with the same 500 of
/// evidence, aged one season (180 epochs): one repaid on day 0 and went quiet
/// at the genesis ratio, one the same at the top of the governed range, and
/// one who carried the debt to harvest and repaid then.
#[test]
fn the_decay_dial_and_the_maturity_are_what_a_season_is_sized_with() {
    let season = 180;
    let mut quiet = Chain::founded_with(&[10_000.0], 2);
    let (u, s, n) = (0, 1, 2);
    quiet.back(u, s, 1000.0);
    quiet.back(s, n, 500.0);
    let mut slow = quiet.clone_for_control();
    slow.st.params.stake_decay = 999.0;
    let mut carried = quiet.clone_for_control();
    // The producer borrows for the season, insured on the 500 they earned.
    carried.ok(
        Tx::Accept {
            debtor: Party::Member(n),
            creditor: Party::Member(s),
            amount: 500.0,
            maturity_epochs: season,
            arb: None,
        },
        &[k(s), k(n)],
    );
    let loan = *carried.st.contracts.keys().max().unwrap();
    assert!(carried.st.contracts[&loan].insured);

    quiet.goto(season - 1);
    slow.goto(season - 1);
    carried.goto(season - 1);
    let carried_gross = State::from_minor(carried.st.gross_capacity_of_set_minor(&[n]));
    carried.settle(loan, 500.0);
    eprintln!(
        "after {season} epochs from 500: quiet at 977/1000 → {:.2}; quiet at 999/1000 → {:.2}; carried to harvest: gross {carried_gross:.2} through the season, {:.2} after repaying",
        quiet.cap(n), slow.cap(n), carried.cap(n)
    );
    assert!(quiet.cap(n) < 10.0);
    assert!(slow.cap(n) > 400.0);
    assert_eq!(carried.cap(n), 500.0);
}
