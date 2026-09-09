//! The insured horizon: an underwriter's exposure in TIME, bounded from the
//! acceptance epoch and never from now. Every scene drives the real
//! transition function, audited after each call.

mod common;

use common::*;
use edet_kernel::constants as k;
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

fn kk(id: MemberId) -> Key {
    key(id as usize)
}

fn accept(c: &mut Chain, creditor: MemberId, debtor: MemberId, amount: f64, maturity_epochs: u64) -> ContractId {
    let id = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(debtor),
            creditor: Party::Member(creditor),
            amount,
            maturity_epochs,
            arb: None,
        },
        &[kk(creditor), kk(debtor)],
    );
    id
}

/// Mutation that bites: drop the horizon test in `book`.
#[test]
fn a_claim_past_the_horizon_is_booked_uninsured_though_capacity_carries_it() {
    let mut c = Chain::founded(1, 2);
    let (u, cr, d) = (0, 1, 2);
    c.back(u, cr, 1000.0);
    c.back(cr, d, 500.0);
    let h = c.st.params.insured_horizon_epochs();
    assert_eq!(h, k::INSURED_HORIZON_EPOCHS);
    let at = accept(&mut c, cr, d, 100.0, h);
    assert!(c.st.contracts[&at].insured, "at the horizon it is insured");
    let past = accept(&mut c, cr, d, 100.0, h + 1);
    assert!(!c.st.contracts[&past].insured, "one epoch past it, uninsured, with 400 of capacity to spare");
    assert_eq!(c.cap(d), 400.0, "and it reserved nothing");
}

/// Mutation that bites: drop `drops_insurance` in `extend`.
#[test]
fn an_extension_past_the_horizon_drops_the_insurance_and_returns_the_flow() {
    let mut c = Chain::founded(1, 2);
    let (u, cr, d) = (0, 1, 2);
    c.back(u, cr, 1000.0);
    c.back(cr, d, 500.0);
    let h = c.st.params.insured_horizon_epochs();
    let loan = accept(&mut c, cr, d, 500.0, 30);
    assert!(c.st.contracts[&loan].insured);
    assert_eq!(c.st.supply_floor(u), State::to_minor(500.0));
    let inside = c.st.epoch + h;
    c.ok(Tx::Extend { contract: loan, new_maturity_epoch: inside }, &[kk(d), kk(cr)]);
    assert!(c.st.contracts[&loan].insured, "inside the horizon the hold stays");
    assert_eq!(c.st.supply_floor(u), State::to_minor(500.0));
    c.ok(Tx::Extend { contract: loan, new_maturity_epoch: inside + 1 }, &[kk(d), kk(cr)]);
    assert!(!c.st.contracts[&loan].insured, "one past it, the insurance drops");
    assert!(c.st.contracts[&loan].held.edges.is_empty() && c.st.contracts[&loan].held.supply.is_empty());
    assert_eq!(c.st.supply_floor(u), 0, "the flow returned: the underwriter may leave");
    assert_eq!(c.cap(d), 500.0, "the debtor's capacity is back, because the claim holds nothing");
    assert_eq!(c.outstanding(loan), 500.0, "and the debt is untouched");
}

/// The base is the ACCEPTANCE. Extend in steps, each inside the horizon
/// measured from now, and the step that crosses acceptance + H drops the
/// hold all the same. Mutation that bites: measure the horizon from
/// `state.epoch` in `extend`.
#[test]
fn a_chain_of_extensions_cannot_roll_an_insured_claim_past_the_horizon() {
    let mut c = Chain::founded(1, 2);
    let (u, cr, d) = (0, 1, 2);
    c.back(u, cr, 1000.0);
    c.back(cr, d, 500.0);
    let h = c.st.params.insured_horizon_epochs();
    let accepted = c.st.epoch;
    let loan = accept(&mut c, cr, d, 500.0, 30);
    let mut due = c.st.contracts[&loan].maturity_epoch;
    let mut crossed = false;
    for _ in 0..5 {
        c.goto(due - 5);
        due += 100;
        c.ok(Tx::Extend { contract: loan, new_maturity_epoch: due }, &[kk(d), kk(cr)]);
        let inside = due <= accepted + h;
        assert_eq!(c.st.contracts[&loan].insured, inside, "insured exactly while due {due} is inside {accepted} + {h}");
        crossed |= !inside;
    }
    assert!(crossed, "the scene must reach the horizon to say anything");
    assert!(!c.st.contracts[&loan].insured);
    assert_eq!(c.st.supply_floor(u), 0);
}

/// A transfer is a debtor swap and keeps the original acceptance as its
/// horizon base, while the row's own `created_epoch` restarts. Mutation that
/// bites: pass `state.epoch` for the base in `move_debtor`.
#[test]
fn a_transfer_keeps_the_original_acceptance_as_the_horizon_base() {
    let mut c = Chain::founded(1, 3);
    let (u, cr, d, e) = (0, 1, 2, 3);
    c.back(u, cr, 1000.0);
    c.back(cr, d, 500.0);
    c.back(cr, e, 500.0);
    let h = c.st.params.insured_horizon_epochs();
    let accepted = c.st.epoch;
    let loan = accept(&mut c, cr, d, 300.0, 30);
    c.goto(accepted + 20);
    c.ok(Tx::Transfer { contract: loan, new_debtor: e }, &[kk(d), kk(e)]);
    let succ = *c.st.contracts.keys().max().unwrap();
    assert!(c.st.contracts[&succ].insured, "E carries it insured, on two signatures");
    assert_eq!(c.st.contracts[&succ].accepted_epoch, accepted);
    assert_eq!(c.st.contracts[&succ].created_epoch, accepted + 20, "the row's own epoch restarts; the base does not");
    c.ok(Tx::Extend { contract: succ, new_maturity_epoch: accepted + h }, &[kk(e), kk(cr)]);
    assert!(c.st.contracts[&succ].insured);
    c.ok(Tx::Extend { contract: succ, new_maturity_epoch: accepted + h + 1 }, &[kk(e), kk(cr)]);
    assert!(!c.st.contracts[&succ].insured, "measured from the original acceptance, not from the swap");
}

/// A claim already past the horizon is not upgraded by a swap: the successor
/// is uninsured whatever capacity carries, so the creditor signs.
#[test]
fn a_claim_past_the_horizon_moves_only_with_the_creditor_and_stays_uninsured() {
    let mut c = Chain::founded(1, 3);
    let (u, cr, d, e) = (0, 1, 2, 3);
    c.back(u, cr, 1000.0);
    c.back(cr, d, 500.0);
    c.back(cr, e, 500.0);
    let h = c.st.params.insured_horizon_epochs();
    let long = accept(&mut c, cr, d, 300.0, h + 1);
    assert!(!c.st.contracts[&long].insured);
    c.err(Tx::Transfer { contract: long, new_debtor: e }, &[kk(d), kk(e)], ET_MEM_NOT_SIGNER);
    c.ok(Tx::Transfer { contract: long, new_debtor: e }, &[kk(d), kk(e), kk(cr)]);
    let succ = *c.st.contracts.keys().max().unwrap();
    assert!(!c.st.contracts[&succ].insured, "not upgraded past the horizon");
    assert_eq!(c.cap(e), 500.0, "E's capacity untouched");
}

/// The cascade: a routed successor keeps its original's base, and an original
/// whose date is past that base's horizon stays with its debtor rather than
/// move onto the buyer insured. Mutation that bites: drop the horizon test in
/// `clear_member_debts`, or pass `state.epoch` as the slot's base.
#[test]
fn a_routed_successor_keeps_its_original_base_and_a_past_horizon_original_stays_put() {
    let mut c = Chain::founded(1, 3);
    let (u, cr, s, b) = (0, 1, 2, 3);
    c.st.params.stake_decay = 999.0;
    c.back(u, cr, 2000.0);
    c.back(cr, s, 500.0);
    c.back(cr, b, 500.0);
    let h = c.st.params.insured_horizon_epochs();
    let accepted = c.st.epoch;
    let loan = accept(&mut c, cr, s, 300.0, 30);
    c.goto(accepted + 20);
    c.ok(
        Tx::Sale { seller: Party::Member(s), buyer: Party::Member(b), amount: 300.0, maturity_epochs: 30 },
        &[kk(s), kk(b)],
    );
    assert_eq!(c.st.contracts[&loan].status, ContractStatus::Transferred, "routed onto the buyer");
    let succ = *c.st.contracts.keys().max().unwrap();
    assert_eq!((c.st.contracts[&succ].debtor, c.st.contracts[&succ].creditor), (b, cr));
    assert!(c.st.contracts[&succ].insured);
    assert_eq!(c.st.contracts[&succ].accepted_epoch, accepted, "the successor inherits the base");
    assert_eq!(c.st.contracts[&succ].created_epoch, accepted + 20);

    // Now an original booked past the horizon, at an epoch where the earlier
    // of its date and the sale's is still past acceptance + H.
    let long = accept(&mut c, cr, s, 200.0, h + 1);
    assert!(!c.st.contracts[&long].insured);
    let base = c.st.contracts[&long].accepted_epoch;
    // Late enough that the sale's own date no longer clips the claim back
    // inside its horizon: the earlier of the two is past base + H.
    c.goto(base + h - 10);
    c.back(cr, b, 500.0);
    let before = c.st.contracts.len();
    c.ok(
        Tx::Sale { seller: Party::Member(s), buyer: Party::Member(b), amount: 200.0, maturity_epochs: 30 },
        &[kk(s), kk(b)],
    );
    assert_eq!(c.st.contracts[&long].status, ContractStatus::Active, "the past-horizon original stays put");
    assert_eq!(c.outstanding(long), 200.0);
    assert_eq!(c.st.contracts.len(), before + 1, "the whole sale fell through to the seller as new debt");
    let rem = *c.st.contracts.keys().max().unwrap();
    assert_eq!((c.st.contracts[&rem].debtor, c.st.contracts[&rem].creditor), (b, s));
}

/// The dial: refused outside its range, and read by the next acceptance once
/// enacted.
#[test]
fn the_horizon_is_governed_inside_its_range_and_the_next_acceptance_reads_it() {
    let mut c = Chain::founded(2, 2);
    let (a, cr, d) = (0, 2, 3);
    c.back(a, cr, 1000.0);
    c.back(cr, d, 500.0);
    for bad in [k::MIN_MATURITY_EPOCHS as f64 - 1.0, k::MAX_HORIZON_EPOCHS as f64 + 1.0] {
        c.err(
            Tx::Propose { author: a, kind: ProposalKind::ParamChange { key: ParamKey::InsuredHorizon, value: bad } },
            &[kk(a)],
            ET_GOV_OUT_OF_RANGE,
        );
    }
    c.ok(
        Tx::Propose { author: a, kind: ProposalKind::ParamChange { key: ParamKey::InsuredHorizon, value: 90.0 } },
        &[kk(a)],
    );
    let pid = *c.st.proposals.keys().max().unwrap();
    c.ok(Tx::Assent { member: a, proposal: pid }, &[kk(a)]);
    assert!(c.st.proposals[&pid].enacted, "half the seed enacts");
    assert_eq!(c.st.params.insured_horizon_epochs(), 90);
    let inside = accept(&mut c, cr, d, 100.0, 90);
    assert!(c.st.contracts[&inside].insured);
    let past = accept(&mut c, cr, d, 100.0, 91);
    assert!(!c.st.contracts[&past].insured);
}
