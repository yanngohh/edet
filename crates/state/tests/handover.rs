//! Whether the escape a refused debtor has is itself a door.
//!
//! `Transfer` asks the creditor only where the successor would NOT be insured
//! (`apply::transfer`), so a debtor and a willing successor move a claim on two
//! signatures neither of them the creditor's. Before that is offered to a
//! member as the way past a creditor who will not sign (`tests/refusal.rs`),
//! what it does to the CREDITOR has to be measured rather than cited.
//!
//! Four questions, each asked of the party who would lose if the answer were
//! the convenient one: does the creditor's own position move; does the claim
//! survive intact; does the panel survive; and can a debtor use it to put a
//! default on somebody else's record.

mod common;

use common::{key, Chain, MATURITY, SUPPLY};
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

/// Everything the ledger measures about a member.
#[derive(Debug, PartialEq)]
struct Standing {
    capacity: u64,
    conferrable: u64,
    seed_reach: u64,
    headroom: u64,
    allowance: u32,
    debt_out: u64,
    open_default: u64,
}

fn standing(c: &Chain, id: MemberId) -> Standing {
    let m = &c.st.members[&id];
    Standing {
        capacity: c.st.capacity_of_minor(id),
        conferrable: c.st.conferrable_minor(id),
        seed_reach: c.st.seed_reach_minor(id),
        headroom: c.st.bond_headroom_minor(id),
        allowance: edet_state::bond::free_remaining(&c.st, id),
        debt_out: m.debt_out,
        open_default: m.rep.open_default,
    }
}

/// What `debtor` owes `creditor` over live claims, in denomination units.
fn owed_to(c: &Chain, debtor: MemberId, creditor: MemberId) -> f64 {
    State::from_minor(
        c.st.contracts
            .values()
            .filter(|x| x.debtor == debtor && x.creditor == creditor)
            .filter(|x| matches!(x.status, ContractStatus::Active | ContractStatus::Expired))
            .map(|x| x.outstanding)
            .sum::<u64>(),
    )
}

/// One underwriter, a creditor, a debtor and a willing successor, all backed.
fn scene() -> Chain {
    let mut c = Chain::founded(1, 3);
    c.back(0, 1, 500.0);
    c.back(0, 2, 500.0);
    c.back(0, 3, 500.0);
    c
}

// ------------------------------------------- what the creditor keeps --

/// **Nothing the ledger measures about the creditor moves.** A transfer is not
/// a discharge: it writes no stake, mints nothing, and leaves the creditor
/// holding the same amount, on the same date, under the same acceptance, still
/// insured. What changed is whose debt it is.
#[test]
fn a_handover_the_creditor_did_not_sign_moves_nothing_of_theirs() {
    let mut c = scene();
    let (cr, d, s) = (1, 2, 3);
    let cid = c.lend(cr, d, 300.0);
    assert!(c.st.contracts[&cid].insured);
    let original = c.st.contracts[&cid].clone();

    let before = standing(&c, cr);
    let successor = c.st.next_contract;
    c.ok(Tx::Transfer { contract: cid, new_debtor: s }, &[key(d as usize), key(s as usize)]);
    let after = standing(&c, cr);

    assert_eq!(
        before, after,
        "PROVEN: the creditor's capacity, conferrable, reach, headroom and allowance are untouched"
    );

    let moved = &c.st.contracts[&successor];
    assert_eq!(moved.creditor, cr, "the claim is still theirs");
    assert_eq!(moved.outstanding, original.outstanding, "for the same amount");
    assert_eq!(moved.maturity_epoch, original.maturity_epoch, "on the same date — a transfer never re-dates");
    assert_eq!(
        moved.accepted_epoch, original.accepted_epoch,
        "under the same acceptance, so the horizon does not roll"
    );
    assert!(moved.insured, "and still insured, which is the whole condition of not asking them");
    assert_eq!(owed_to(&c, s, cr), 300.0, "owed now by the successor");
    assert_eq!(owed_to(&c, d, cr), 0.0, "and no longer by the debtor");
}

/// The condition is checked, not assumed: a successor the community cannot
/// carry for the amount needs the creditor's signature, and is refused without
/// it. This is what stops the handover from being a way to strand a claim on
/// somebody nobody backs.
#[test]
fn a_successor_the_community_cannot_carry_needs_the_creditor() {
    let mut c = Chain::founded(1, 3);
    let (cr, d, s) = (1, 2, 3);
    c.back(0, cr, 500.0);
    c.back(0, d, 500.0);
    // `s` is a member with a row and nothing behind it.
    let cid = c.lend(cr, d, 300.0);
    assert_eq!(c.st.capacity_of_minor(s), 0, "nobody has backed the successor");

    c.err(Tx::Transfer { contract: cid, new_debtor: s }, &[key(d as usize), key(s as usize)], ET_MEM_NOT_SIGNER);
    assert_eq!(owed_to(&c, d, cr), 300.0, "the claim stays where it is");

    // With the creditor's own signature it moves, and the creditor has
    // consented to holding it uninsured.
    let successor = c.st.next_contract;
    c.ok(Tx::Transfer { contract: cid, new_debtor: s }, &[key(d as usize), key(s as usize), key(cr as usize)]);
    assert!(!c.st.contracts[&successor].insured, "which is what their signature was for");
}

/// **The panel does not travel, and does not have to.** An award is minted
/// between the parties the TERMS name, off the original row, which is retained
/// for at least its own window — so a handover cannot be used to shake off the
/// remedy the two of them agreed at acceptance.
#[test]
fn a_handover_does_not_shake_off_the_arbitration_the_parties_agreed() {
    let mut c = Chain::founded(1, 6);
    let (cr, d, s) = (1, 2, 3);
    let panel: Vec<MemberId> = vec![4, 5, 6];
    c.back(0, cr, 500.0);
    c.back(0, d, 500.0);
    c.back(0, s, 500.0);

    let cid = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(d),
            creditor: Party::Member(cr),
            amount: 300.0,
            maturity_epochs: MATURITY,
            arb: Some(ArbTermsWire {
                arbiters: panel.iter().copied().collect(),
                quorum: 2,
                window_epochs: 10,
                award_cap: 300.0,
            }),
        },
        &[key(cr as usize), key(d as usize)],
    );

    c.ok(Tx::Transfer { contract: cid, new_debtor: s }, &[key(d as usize), key(s as usize)]);
    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Transferred);
    let terms = c.st.contracts[&cid]
        .arb
        .clone()
        .expect("the terms stay on the row that carries them");
    assert_eq!((terms.debtor, terms.creditor), (d, cr), "and still name the parties they were agreed between");

    // The panel attests and the sweep mints at the window's close, between
    // those two and nobody else.
    for &a in &panel[..2] {
        c.ok(Tx::ArbAttest { contract: cid, arbiter: a, amount: 200.0 }, &[key(a as usize)]);
    }
    let created = c.st.contracts[&cid].created_epoch;
    c.goto(created + 11);
    assert!(c.st.contracts[&cid].arb_awarded, "the window closed and the award was decided");
    assert_eq!(owed_to(&c, cr, d), 200.0, "PROVEN: the remedy runs between the original parties, after the handover");
}

// --------------------------------------------- what a debtor CAN do with it --

/// **A debtor can move a default onto a consenting accomplice, and the
/// accomplice pays for it with standing they had to earn.**
///
/// The record moves — the successor carries the `open_default` and the closed
/// allowance, the original debtor walks away clean — so the per-member signal
/// is launderable by anyone who can find a backed volunteer. **Nothing is
/// laundered away**, which is the whole of why it is not a defect: somebody
/// consented (`require_signed(new_debtor)`), and what they spend is capacity
/// the community conferred on them by being repaid. Measured over the SET on
/// every quantity the ledger enforces — the cut, the burned capacity, the
/// closed allowance — the pair ends exactly where it would have ended had the
/// first debtor simply defaulted. What differs is WHICH of the two burned, and
/// no bound is stated over that.
#[test]
fn a_handover_launders_the_record_and_not_the_cut() {
    // Treatment: D hands the claim to S, and S defaults.
    let mut t = scene();
    let (cr, d, s) = (1, 2, 3);
    let cid = t.lend(cr, d, 300.0);
    let successor = t.st.next_contract;
    t.ok(Tx::Transfer { contract: cid, new_debtor: s }, &[key(d as usize), key(s as usize)]);
    t.default_on(successor);

    // Control: D simply defaults, aged identically.
    let mut ctl = scene();
    let cid2 = ctl.lend(cr, d, 300.0);
    ctl.default_on(cid2);

    // The record moved, in full.
    assert_eq!(t.st.members[&d].rep.open_default, 0, "the original debtor carries no default");
    assert_eq!(edet_state::bond::free_remaining(&t.st, d), t.st.params.bond_free_allowance, "nor a closed allowance");
    assert_eq!(t.st.members[&s].rep.open_default, State::to_minor(300.0), "the accomplice carries all of it");
    assert_eq!(ctl.st.members[&d].rep.open_default, State::to_minor(300.0), "where the control's debtor carries it");

    // **The individual reading moves, and that is the part to be honest
    // about.** The handover released the first debtor's reservation, so their
    // own capacity comes back where a default would have kept it consumed —
    // and capacity is what the client's advisory score reads
    // (`ui/src/lib/risk.ts`: confidence and headroom are both functions of
    // it). `open_default` is not scored, so the laundering that buys anything
    // buys it here.
    assert_eq!(ctl.st.capacity_of_minor(d), 0, "a default keeps the defaulter's flow committed");
    assert!(
        t.st.capacity_of_minor(d) > 0,
        "PROVEN: handing the claim on gives the first debtor their capacity back, which is what a score reads"
    );

    // The cut did not. Measured over the pair, which is the quantifier the
    // theorem uses.
    let pair = [d, s];
    assert_eq!(
        t.st.capacity_of_set_minor(&pair),
        ctl.st.capacity_of_set_minor(&pair),
        "PROVEN: the coalition holds exactly what it would have held had the first debtor defaulted"
    );
    // And the community paid the same, once.
    assert_eq!(owed_to(&t, 0, cr), 300.0, "the creditor is made whole by the underwriter, once");
    assert_eq!(owed_to(&ctl, 0, cr), 300.0, "in both arms");
    assert_eq!(t.st.members[&0].debt_out, ctl.st.members[&0].debt_out, "for the same amount of the seed");

    // **Somebody pays, and pays the same.** The accomplice consented to the
    // handover and spends what the first debtor would have spent: one member
    // of the pair ends with its capacity consumed and its allowance closed, in
    // each arm, and the pair holds one open allowance either way. A bound is
    // stated over the SET and this is the set reading of the sanction.
    let burned = |c: &Chain| -> (u64, u32) {
        (
            pair.iter().filter(|&&m| c.st.capacity_of_minor(m) == 0).count() as u64,
            pair.iter().map(|&m| edet_state::bond::free_remaining(&c.st, m)).sum(),
        )
    };
    eprintln!(
        "burned (capacities at zero, allowance left over the pair): treatment {:?} control {:?}",
        burned(&t),
        burned(&ctl)
    );
    assert_eq!(burned(&t), burned(&ctl), "PROVEN: the same standing is consumed either way — only whose changes");
    assert_eq!(burned(&t).0, 1, "exactly one of the pair carries the sanction");
    assert_eq!(t.st.capacity_of_minor(s), 0, "and here it is the accomplice, who signed for it");
    assert_eq!(edet_state::bond::free_remaining(&t.st, s), 0, "allowance closed, on the member who consented");
}

/// The upgrade the creditor is not asked about, and why it is not a loss: an
/// uninsured claim moving to a successor the community CAN carry becomes
/// insured. The creditor is weakly better off, which is the whole reason their
/// signature is not owed — and it draws the community's supply only for a
/// debtor the community had already backed.
#[test]
fn an_uninsured_claim_may_be_upgraded_without_the_creditor_and_never_downgraded() {
    let mut c = Chain::founded(1, 3);
    let (cr, d, s) = (1, 2, 3);
    c.back(0, cr, 500.0);
    c.back(0, s, 500.0);
    // `d` is backed by nobody, so the claim on them is uninsured.
    let cid = c.lend(cr, d, 300.0);
    assert!(!c.st.contracts[&cid].insured, "nobody stands behind this debtor");

    let successor = c.st.next_contract;
    c.ok(Tx::Transfer { contract: cid, new_debtor: s }, &[key(d as usize), key(s as usize)]);
    assert!(c.st.contracts[&successor].insured, "the successor can carry it, so the claim is upgraded");

    // The other direction is exactly what the creditor's signature guards, and
    // `a_successor_the_community_cannot_carry_needs_the_creditor` holds it.
    let _ = SUPPLY;
}
