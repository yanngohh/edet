//! What a creditor who will not sign a discharge costs, and to whom.
//!
//! Every discharge is authorised by the party who loses if it is wrong, so a
//! creditor's signature is the ledger's only record that value arrived. The
//! question these probe is the other side of that rule: a creditor who simply
//! does not sign forces a default the debtor was ready to cure, and the
//! community pays the claim.
//!
//! **Measured at the quantifier, and against a control aged identically.** The
//! per-default reading is uninformative — every one of them looks like an
//! ordinary default, which is the point — so what has to be compared is the
//! refuser's own position across the two arms, over a SET of debtors.

mod common;

use common::{key, Chain, SUPPLY};
use edet_state::errors::*;
use edet_state::root::state_root;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

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

/// Everything the ledger measures ABOUT a member — the figures a counterparty,
/// a view or the write gate would read off them.
#[derive(Debug, PartialEq)]
struct Standing {
    capacity: u64,
    conferrable: u64,
    seed_reach: u64,
    headroom: u64,
    allowance: u32,
    debt_out: u64,
    open_default: u64,
    status: MemberStatus,
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
        status: m.status,
    }
}

// ---------------------------------------------------- the refusal itself --

/// **A debtor has no unilateral act that discharges a debt**, and the attempt
/// leaves nothing behind.
///
/// Both routes out need the creditor: `Settle` takes their signature, and the
/// sale that nets the debt is a purchase THEY record. So a refusal is not a
/// transition anybody can point at afterwards — it is the absence of one, and
/// the state root is the proof that the ledger cannot see it.
#[test]
fn a_refused_discharge_leaves_no_trace_the_ledger_can_read() {
    let mut c = Chain::founded(1, 2);
    let (u, cr, d) = (0, 1, 2);
    c.back(u, cr, 500.0);
    c.back(u, d, 500.0);
    let cid = c.lend(cr, d, 300.0);
    assert!(c.st.contracts[&cid].insured, "the fixture is about the INSURED tier");

    let before = state_root(&c.st).expect("a finite state encodes");
    // The debtor offers the whole amount, alone. This is what "the creditor
    // will not sign" looks like from inside `apply`.
    c.err(Tx::Settle { contract: cid, amount: 300.0 }, &[key(d as usize)], ET_MEM_NOT_SIGNER);
    let after = state_root(&c.st).expect("a finite state encodes");

    assert_eq!(
        before, after,
        "PROVEN: an envelope that authorises nothing is refunded and forgotten, so an offered \
         discharge the creditor declined is indistinguishable from one that was never made"
    );
    assert_eq!(c.st.contracts[&cid].outstanding, State::to_minor(300.0), "and the debt stands");
}

/// The debtor is punished for the creditor's inaction, and every consequence
/// is one the debtor could have avoided by paying — which they offered to do.
#[test]
fn the_refusal_forces_a_default_and_the_debtor_carries_every_consequence() {
    let mut c = Chain::founded(1, 2);
    let (u, cr, d) = (0, 1, 2);
    c.back(u, cr, 500.0);
    c.back(u, d, 500.0);
    let cid = c.lend(cr, d, 300.0);

    let before = standing(&c, d);
    assert_eq!(before.allowance, c.st.params.bond_free_allowance, "a backed member holds a full allowance");

    c.err(Tx::Settle { contract: cid, amount: 300.0 }, &[key(d as usize)], ET_MEM_NOT_SIGNER);
    c.default_on(cid);

    let after = standing(&c, d);
    assert_eq!(after.open_default, State::to_minor(300.0), "the default is on the debtor's record");
    assert_eq!(after.allowance, 0, "and it closes their free allowance entirely");
    assert_eq!(after.capacity, 0, "a default releases none of the flow it committed");
    assert_eq!(after.status, MemberStatus::Active, "the ledger does not suspend them — it does not have to");

    // Nor can they wind down: leaving needs the debt gone, and the debt needs
    // a signature they cannot produce.
    c.err(Tx::Exit { member: d }, &[key(d as usize)], ET_LIF_OUTSTANDING_DEBT);

    // The creditor, meanwhile, is made whole by the community.
    assert_eq!(owed_to(&c, d, cr), 0.0, "their claim on the debtor is gone");
    assert_eq!(owed_to(&c, u, cr), 300.0, "they hold one on the underwriter instead");
    assert_eq!(owed_to(&c, d, u), 300.0, "and the debtor now owes the underwriter");
}

// ------------------------------------------------- what it costs the refuser --

/// **The reading this suite exists for.** One creditor, six debtors, two arms
/// of the same chain aged to the same epoch: in one the creditor signs every
/// discharge, in the other it signs none.
///
/// Every figure the ledger measures about the CREDITOR is identical in the two
/// arms. What differs is that the refusing arm ends holding claims on the
/// underwriter for the whole amount it lent, and the signing arm holds
/// nothing — which is what being paid looks like on a ledger with no token.
///
/// The creditor is fed by a DIFFERENT underwriter from the debtors, and that is
/// the point of the shape rather than a convenience: where they share one, the
/// refuser bears a share of the supply its own defaults drew, and the reading
/// would credit the mechanism with an externality that any second underwriter
/// removes.
#[test]
fn refusing_costs_the_refuser_nothing_the_ledger_measures() {
    const K: MemberId = 6;
    const AMOUNT: f64 = 400.0;

    // U_d (0) backs the debtors; U_c (1) backs the creditor.
    let mut t = Chain::founded(2, 1 + K as usize);
    let cr: MemberId = 2;
    let debtors: Vec<MemberId> = (3..3 + K).collect();
    t.back(1, cr, 500.0);
    for &d in &debtors {
        t.back(0, d, 500.0);
    }

    // The control diverges here, and is aged to the same epoch below.
    let mut ctl = t.clone_for_control();

    let cids: Vec<ContractId> = debtors.iter().map(|&d| t.lend(cr, d, AMOUNT)).collect();
    for &cid in &cids {
        assert!(t.st.contracts[&cid].insured, "every claim in the treatment is one the community stands behind");
    }
    let due = t.st.contracts[&cids[0]].maturity_epoch;

    // The refusal: nothing is submitted at all, and the sweep does the rest.
    t.goto(due + 1);
    for &cid in &cids {
        assert_eq!(t.st.contracts[&cid].status, ContractStatus::Expired, "the crank expires what fell due");
    }

    // The control: the same claims, every one of them honoured.
    for &d in &debtors {
        let cid = ctl.lend(cr, d, AMOUNT);
        ctl.settle(cid, AMOUNT);
    }
    ctl.goto(due + 1);

    let refusing = standing(&t, cr);
    let signing = standing(&ctl, cr);
    eprintln!("creditor after refusing {K} discharges: {refusing:?}");
    eprintln!("creditor after signing   {K} discharges: {signing:?}");
    assert_eq!(
        refusing, signing,
        "PROVEN: capacity, conferrable, seed reach, write headroom, allowance, debt and default \
         record are the same whether a creditor honours its discharges or refuses every one"
    );

    // What differs is the balance sheet, and it differs in the refuser's favour.
    assert_eq!(
        owed_to(&t, 0, cr),
        K as f64 * AMOUNT,
        "the refuser holds the whole amount as a claim on the underwriter"
    );
    assert_eq!(owed_to(&ctl, 0, cr), 0.0, "the signer holds nothing: being paid confers standing, it does not earn it");

    // And the cost is borne by parties who chose none of it.
    for &d in &debtors {
        assert_eq!(t.st.members[&d].rep.open_default, State::to_minor(AMOUNT), "each debtor carries a default");
        assert_eq!(edet_state::bond::free_remaining(&t.st, d), 0, "and a closed allowance");
        assert_eq!(ctl.st.members[&d].rep.open_default, 0, "where the control's debtors carry neither");
    }
    assert_eq!(
        t.st.members[&0].debt_out,
        State::to_minor(K as f64 * AMOUNT),
        "the underwriter's supply pays for all of it"
    );
}

/// The bound, and it is the one the community already accepted: the whole
/// scheme cannot draw more than the seed, and each use consumes a backed
/// debtor who cannot be used again.
#[test]
fn the_refusal_is_bounded_by_the_seed_and_burns_one_backed_debtor_per_use() {
    let mut c = Chain::founded(1, 2);
    let (u, cr, d) = (0, 1, 2);
    c.back(u, cr, 500.0);
    c.back(u, d, SUPPLY);

    // Everything this debtor's backing carries, in one claim.
    let cid = c.lend(cr, d, SUPPLY);
    assert!(c.st.contracts[&cid].insured);
    c.default_on(cid);
    assert_eq!(owed_to(&c, u, cr), SUPPLY, "the refuser holds the underwriter's whole supply as a claim");

    // The same debtor cannot carry a second one: the flow the default
    // committed is not released, so there is nothing left to insure.
    assert_eq!(c.st.capacity_of_minor(d), 0, "the burned debtor has nothing left");
    let second = c.lend(cr, d, 100.0);
    assert!(
        !c.st.contracts[&second].insured,
        "PROVEN: a second claim on the same debtor is uninsured, so refusing it substitutes nobody"
    );
    c.default_on(second);
    assert_eq!(owed_to(&c, u, cr), SUPPLY, "and the community pays nothing more for it");
}

// ------------------------------------------------------------ the escape --

/// **The debtor's one route past a creditor who will not sign**, and the epoch
/// it closes.
///
/// `Transfer` asks the creditor only where the successor would NOT be insured,
/// so a debtor who can find a member the community can back for the amount
/// leaves without them. It is refused the moment the claim stops being Active
/// — which is to say, at the default the refusal was driving toward.
#[test]
fn a_debtor_can_hand_the_claim_on_without_the_creditor_until_it_matures() {
    let mut c = Chain::founded(1, 3);
    let (u, cr, d, s) = (0, 1, 2, 3);
    c.back(u, cr, 500.0);
    c.back(u, d, 500.0);
    c.back(u, s, 500.0);

    let cid = c.lend(cr, d, 300.0);
    c.err(Tx::Settle { contract: cid, amount: 300.0 }, &[key(d as usize)], ET_MEM_NOT_SIGNER);

    // The successor can carry it insured, so the creditor's consent is not
    // asked for: what changed is whose debt it is, not what stands behind it.
    c.ok(Tx::Transfer { contract: cid, new_debtor: s }, &[key(d as usize), key(s as usize)]);
    assert_eq!(c.st.members[&d].debt_out, 0, "the debtor is out, on two signatures neither of them the creditor's");
    assert_eq!(c.st.members[&d].rep.open_default, 0, "and out before any default could be recorded");

    // The same route, one epoch too late.
    let mut late = Chain::founded(1, 3);
    late.back(u, cr, 500.0);
    late.back(u, d, 500.0);
    late.back(u, s, 500.0);
    let cid = late.lend(cr, d, 300.0);
    late.default_on(cid);
    late.err(Tx::Transfer { contract: cid, new_debtor: s }, &[key(d as usize), key(s as usize)], ET_CTR_BAD_STATE);
    assert_eq!(
        late.st.members[&d].rep.open_default,
        State::to_minor(300.0),
        "PROVEN: the escape closes at maturity, and what is left is a cure the creditor must also sign"
    );
}

/// After substitution the claim is the UNDERWRITER's, so a debtor whose
/// creditor went dark has a new counterparty to pay — which is the one way the
/// refusal partly undoes itself, and only for an insured claim.
#[test]
fn substitution_hands_the_debtor_a_creditor_who_can_still_be_paid() {
    let mut c = Chain::founded(1, 2);
    let (u, cr, d) = (0, 1, 2);
    c.back(u, cr, 500.0);
    c.back(u, d, 500.0);
    let cid = c.lend(cr, d, 300.0);
    c.default_on(cid);

    assert_eq!(c.st.contracts[&cid].creditor, u, "the claim is the underwriter's now");
    // And the underwriter signs the cure the original creditor would not.
    c.ok(Tx::Cure { contract: cid, amount: 300.0 }, &[key(d as usize), key(u as usize)]);
    assert_eq!(c.st.members[&d].rep.open_default, 0, "the default is repaired");
    assert_eq!(
        edet_state::bond::free_remaining(&c.st, d),
        c.st.params.bond_free_allowance,
        "and the allowance reopens"
    );
    // The record of it does not go away: a cured row is kept for its retention
    // window, and the capacity the default consumed comes back with payment.
    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Cured);
    assert!(c.st.capacity_of_minor(d) > 0, "the flow the cure released is back on the graph");
}
/// The one figure a shared supply DOES move, and the three it does not.
///
/// Where the creditor draws on the same underwriter as its debtors, the flow
/// its own defaults committed is flow that no longer reaches it: capacity and
/// conferrable fall. That is an externality the whole community bears and the
/// refuser happens to be standing in — a second underwriter removes it
/// entirely, which is why the reading above is the honest one.
///
/// **Seed reach, write headroom and the free allowance do not move in either
/// shape.** The write surface is read GROSS of live credit, so nothing a
/// refuser does to the community's insurance reaches its own ability to keep
/// doing it.
#[test]
fn a_shared_supply_costs_the_refuser_capacity_and_never_the_write_surface() {
    const K: MemberId = 6;
    const AMOUNT: f64 = 400.0;

    let mut t = Chain::founded(1, 1 + K as usize);
    let cr: MemberId = 1;
    let debtors: Vec<MemberId> = (2..2 + K).collect();
    t.back(0, cr, 500.0);
    for &d in &debtors {
        t.back(0, d, 500.0);
    }
    let mut ctl = t.clone_for_control();

    let cids: Vec<ContractId> = debtors.iter().map(|&d| t.lend(cr, d, AMOUNT)).collect();
    let due = t.st.contracts[&cids[0]].maturity_epoch;
    t.goto(due + 1);
    for &d in &debtors {
        let cid = ctl.lend(cr, d, AMOUNT);
        ctl.settle(cid, AMOUNT);
    }
    ctl.goto(due + 1);

    let refusing = standing(&t, cr);
    let signing = standing(&ctl, cr);
    eprintln!("one underwriter, refusing: {refusing:?}");
    eprintln!("one underwriter, signing:  {signing:?}");

    assert!(refusing.capacity < signing.capacity, "the supply its defaults drew is supply that no longer reaches it");
    assert_eq!(refusing.seed_reach, signing.seed_reach, "PROVEN: the write floor is read gross, so it does not move");
    assert_eq!(refusing.headroom, signing.headroom, "nor does the headroom the gate charges against");
    assert_eq!(refusing.allowance, signing.allowance, "nor the free allowance");
    assert_eq!(refusing.open_default, 0, "and nothing is recorded against the creditor of six defaults");
}
