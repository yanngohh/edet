//! The seat slot: one self-act a seat paid for. Bounded by seats, which are
//! bounded by the seed, so it is a stock and never a rate.

mod common;

use common::*;
use edet_kernel::constants as k;
use edet_state::errors::*;
use edet_state::tx::Tx;
use edet_state::types::*;

fn kk(id: MemberId) -> Key {
    key(id as usize)
}

/// Seat `fresh` by a bonded trade with `sponsor` as its creditor.
fn seat(c: &mut Chain, sponsor: MemberId, fresh: Key) -> MemberId {
    c.ok(
        Tx::Accept {
            debtor: Party::Key(fresh),
            creditor: Party::Member(sponsor),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[kk(sponsor), fresh],
    );
    c.st.member_of_key(&fresh).expect("the trade seated the row")
}

fn guardians(row: MemberId, g: [MemberId; 2]) -> Tx {
    Tx::RegisterGuardians {
        member: row,
        guardians: g.to_vec(),
        threshold: 2,
        veto_window_epochs: k::VETO_WINDOW_EPOCHS,
    }
}

/// Mutation that bites: never set `seat_slot` in `seat_pair`, or never
/// answer `Due::Slot`.
#[test]
fn a_seated_row_registers_guardians_once_alone_and_then_needs_a_budget() {
    let mut c = Chain::founded(1, 2);
    let (u, s, t) = (0, 1, 2);
    c.back(u, s, 1000.0);
    let fresh = stranger_key(1);
    let row = seat(&mut c, s, fresh);
    assert!(c.st.members[&row].seat_slot);
    assert_eq!(edet_state::bond::free_remaining(&c.st, row), 0, "nothing behind the row: no allowance");
    c.ok(guardians(row, [s, t]), &[fresh]);
    assert!(!c.st.members[&row].seat_slot, "spent");
    assert!(c.st.members[&row].guardian.is_some());
    c.err(guardians(row, [s, t]), &[fresh], ET_BOND_EXHAUSTED);
}

/// Mutation that bites: charge the slot after dispatch instead of at the gate.
#[test]
fn a_refused_first_write_spends_the_slot() {
    let mut c = Chain::founded(1, 2);
    let (u, s, t) = (0, 1, 2);
    c.back(u, s, 1000.0);
    let fresh = stranger_key(2);
    let row = seat(&mut c, s, fresh);
    c.err(
        Tx::RegisterGuardians {
            member: row,
            guardians: vec![s, t],
            threshold: 1,
            veto_window_epochs: k::VETO_WINDOW_EPOCHS,
        },
        &[fresh],
        ET_ROT_THRESHOLD,
    );
    assert!(!c.st.members[&row].seat_slot, "a refused first write is not retried free");
    c.err(guardians(row, [s, t]), &[fresh], ET_BOND_EXHAUSTED);
}

/// A free class refused at dispatch spends the slot by the rule
/// `price_refusal` applies to an allowance, and keeps its id.
#[test]
fn a_refused_free_class_self_act_spends_the_slot_too() {
    let mut c = Chain::founded(1, 2);
    let (u, s) = (0, 1);
    c.back(u, s, 1000.0);
    let fresh = stranger_key(3);
    let row = seat(&mut c, s, fresh);
    let before = c.st.applied_count();
    c.err(Tx::Exit { member: row }, &[fresh], ET_LIF_OUTSTANDING_DEBT);
    assert!(!c.st.members[&row].seat_slot);
    assert_eq!(c.st.applied_count(), before + 1, "paid for, so the id is kept");
}

/// Only the row's own key spends it, on a transition about that row.
#[test]
fn nobody_burns_another_members_slot_and_a_trade_never_spends_one() {
    let mut c = Chain::founded(1, 3);
    let (u, s, t, w) = (0, 1, 2, 3);
    c.back(u, s, 1000.0);
    let fresh = stranger_key(4);
    let row = seat(&mut c, s, fresh);
    // S registers ITS guardians with the newcomer co-signing: S pays, the
    // newcomer's slot is untouched.
    c.ok(guardians(s, [t, w]), &[kk(s), fresh]);
    assert!(c.st.members[&row].seat_slot);
    // The newcomer's own act with a co-signer who holds allowance present:
    // the slot goes first, because it is the row's own and the row has no
    // allowance of its own.
    let used = c.st.members[&s].bond_free_used;
    c.ok(guardians(row, [s, t]), &[kk(s), fresh]);
    assert!(!c.st.members[&row].seat_slot);
    assert_eq!(c.st.members[&s].bond_free_used, used, "the co-signer was not billed");
    // A trade is nobody's self-act.
    let fresh2 = stranger_key(5);
    let row2 = seat(&mut c, s, fresh2);
    c.ok(
        Tx::Accept {
            debtor: Party::Member(row2),
            creditor: Party::Member(s),
            amount: 5.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[kk(s), fresh2],
    );
    assert!(c.st.members[&row2].seat_slot);
}

/// A farm of N rows holds N slots and no more, this epoch or the next.
#[test]
fn a_farm_of_n_rows_holds_n_slots_and_no_more() {
    let mut c = Chain::founded_with(&[10_000.0], 3);
    let (u, s, t, w) = (0, 1, 2, 3);
    c.back(u, s, 1000.0);
    // Twenty distinct keys of its own: `stranger_key` has sixteen.
    let farm_key = |i: u8| -> Key {
        let mut k = [0xE0u8; 32];
        k[31] = i;
        k
    };
    let n = 20u8;
    let rows: Vec<(MemberId, Key)> = (0..n)
        .map(|i| {
            let f = farm_key(i);
            (seat(&mut c, s, f), f)
        })
        .collect();
    assert_eq!(rows.iter().map(|&(r, _)| r).collect::<std::collections::BTreeSet<_>>().len(), n as usize);
    for &(row, f) in &rows {
        c.ok(guardians(row, [s, t]), &[f]);
    }
    for &(row, f) in &rows {
        c.err(guardians(row, [s, w]), &[f], ET_BOND_EXHAUSTED);
    }
    let next = c.st.epoch + 1;
    c.goto(next);
    for &(row, f) in &rows {
        c.err(guardians(row, [s, w]), &[f], ET_BOND_EXHAUSTED);
    }
}

/// A row seated on a returned seat holds a slot of its own.
#[test]
fn a_row_seated_on_a_returned_seat_holds_a_slot_of_its_own() {
    let mut c = Chain::founded(1, 1);
    let (u, s) = (0, 1);
    c.back(u, s, 1000.0);
    let fresh = stranger_key(6);
    let row = seat(&mut c, s, fresh);
    let cid = *c.st.contracts.keys().max().unwrap();
    c.ok(Tx::Settle { contract: cid, amount: 10.0 }, &[fresh, kk(s)]);
    let far = c.st.epoch + k::ROW_RETENTION_EPOCHS + k::CLOSED_RETENTION_EPOCHS + 20;
    c.goto(far);
    assert!(!c.st.members.contains_key(&row), "empty for the retention, the row is retired and its seat returned");
    c.back(u, s, 1000.0);
    let again = seat(&mut c, s, fresh);
    assert_ne!(again, row, "a new row under the old key");
    assert!(c.st.members[&again].seat_slot);
}
