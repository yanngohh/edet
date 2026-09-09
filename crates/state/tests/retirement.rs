//! **An empty row is retired, and its seat comes back.**
//!
//! A row is a stock priced by a seat on the seed's reach that no transition
//! releases; a row that holds nothing, owes nothing and is named by nothing
//! is not a stock, and the epoch sweep drops it once `ROW_RETENTION_EPOCHS`
//! have passed since it was seated. Every condition is something only the
//! member, a counterparty or a ceremony can put there — an edge in either
//! direction, a contract in any status, a bond, a default, a supply, voting
//! power, a pending rotation, a place on a panel, a proposal or a guardian
//! roll — so nobody can empty another member's row, and the rule is not a
//! lever. What comes back is the seat, to the sponsor's reach, and the key,
//! which a later trade seats again for the price of a seat.
//!
//! Every probe drives `Chain`, so the seven-invariant audit and both caches
//! run after each transition and the root's two implementations are held
//! equal across every sweep, which is where a retirement writes.

mod common;
use common::*;
use edet_kernel::constants as k;
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

/// A key belonging to nobody.
fn stranger(tag: u8, n: u32) -> Key {
    let mut key = [tag; 32];
    key[0..4].copy_from_slice(&n.to_be_bytes());
    key
}

/// Seat one row through `sponsor` and settle the trade at once, so the row
/// holds a closed obligation and one in-edge of `amount` — the two things a
/// first trade leaves behind — and nothing else.
fn seat_and_settle(c: &mut Chain, sponsor: MemberId, key: Key, amount: f64) -> MemberId {
    let contract = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Key(key),
            creditor: Party::Member(sponsor),
            amount,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key, common::key(sponsor as usize)],
    );
    let row = c.st.member_of_key(&key).expect("the trade seated the key");
    c.ok(Tx::Settle { contract, amount }, &[key, common::key(sponsor as usize)]);
    row
}

fn seat_total(st: &State) -> u64 {
    st.seat_committed.values().sum()
}

/// **A row with nothing in it is retired a year after its last trade closed,
/// and its seat is released.** The seating trade settles at once; the closed
/// obligation names the row for `CLOSED_RETENTION_EPOCHS`, the in-edge of
/// 1.00 decays out of the graph inside that, and the sweep that drops the
/// obligation drops the row with it. The key is free again, and seating it
/// again costs a seat again.
///
/// Mutation that bites: skip `retire_empty_rows` in the sweep. The row stays,
/// the seat stays spent, and `member_of_key` still answers.
#[test]
fn an_empty_row_is_retired_after_a_year_and_its_seat_comes_back() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    let key = stranger(0x61, 0);
    let seated_at = c.st.epoch;
    let row = seat_and_settle(&mut c, 1, key, 1.0);
    let unit = State::to_minor(c.st.params.bond_unit());
    assert_eq!(seat_total(&c.st), unit, "one row, one seat");
    assert_eq!(c.st.member_of_key(&key), Some(row));

    // The last epoch the closed obligation is kept, the row is still named.
    c.goto(seated_at + k::CLOSED_RETENTION_EPOCHS);
    assert!(c.st.members.contains_key(&row), "a closed obligation still names the row");
    assert_eq!(seat_total(&c.st), unit);
    // The sponsor keeps trading, or the same sweep would retire it too — its
    // own backing has decayed out and its last obligation is this one.
    c.back(0, 1, 500.0);

    c.goto(c.st.epoch + 1);
    assert!(!c.st.members.contains_key(&row), "nothing names the row, and it is gone");
    assert_eq!(c.st.member_of_key(&key), None, "the key is free");
    assert_eq!(seat_total(&c.st), 0, "the seat came back");
    assert!(c.st.seat_reserved.is_empty(), "and holds no arc");

    // Seated again, it is a new row at a new id, for the price of a seat.
    c.back(0, 1, 500.0);
    let again = seat_and_settle(&mut c, 1, key, 1.0);
    assert!(again > row, "an id is never reused");
    assert_eq!(seat_total(&c.st), unit);
}

/// **Retirement counts from the seat, not from the last trade**: a row seated
/// today and traded with never is kept the whole window too.
#[test]
fn a_row_is_kept_for_the_retention_window_from_its_seating() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    let key = stranger(0x62, 0);
    let seated_at = c.st.epoch;
    let row = seat_and_settle(&mut c, 1, key, 1.0);
    c.goto(seated_at + k::ROW_RETENTION_EPOCHS);
    assert!(c.st.members.contains_key(&row), "kept through the window");
    c.goto(c.st.epoch + 1);
    assert!(!c.st.members.contains_key(&row), "and retired past it");
}

/// **Each thing a row can hold keeps it**, one probe per clause of the
/// precondition, on the row that would otherwise be retired. An edge that
/// outlives the closed obligation; a live obligation; a default, which is
/// live for ever; a supply; voting power; a place on a panel; a proposal;
/// a guardian roll; a suspension.
///
/// Mutation that bites: drop one clause from `retire_empty_rows` and its
/// probe goes red — the row is retired with the thing still in it.
#[test]
fn an_edge_that_outlives_the_closed_obligation_keeps_the_row() {
    // An in-edge of 5,000.00 decays out of the graph about eighty epochs
    // after a closed obligation is dropped — integer decay takes at least one
    // minor unit an epoch below 43 of them, so an edge of 500.00 is gone in
    // 346 — and between the two the edge is the only thing naming the row.
    let mut c = Chain::founded_with(&[10_000.0], 1);
    c.back(0, 1, 10_000.0);
    let key = stranger(0x63, 0);
    let seated_at = c.st.epoch;
    let row = seat_and_settle(&mut c, 1, key, 5_000.0);
    c.goto(seated_at + k::CLOSED_RETENTION_EPOCHS);
    // The sponsor keeps trading, for the reason the first probe gives.
    c.back(0, 1, 10_000.0);
    c.goto(c.st.epoch + 1);
    assert!(c.st.contracts.values().all(|x| x.debtor != row), "the obligation is dropped");
    assert!(c.st.edges.contains_key(&(1, row as usize)), "the edge is still there");
    assert!(c.st.members.contains_key(&row), "and the edge keeps the row");
    c.goto(c.st.epoch + 200);
    assert!(!c.st.edges.contains_key(&(1, row as usize)), "the edge has decayed out");
    assert!(!c.st.members.contains_key(&row), "and the row goes with it");
}

#[test]
fn a_live_obligation_keeps_the_row() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    let key = stranger(0x64, 0);
    let seated_at = c.st.epoch;
    seat_and_settle(&mut c, 1, key, 1.0);
    let row = c.st.member_of_key(&key).expect("seated");
    // A second obligation, the longest the ledger allows, left open.
    c.ok(
        Tx::Accept {
            debtor: Party::Member(row),
            creditor: Party::Member(1),
            amount: 1.0,
            maturity_epochs: k::MAX_HORIZON_EPOCHS,
            arb: None,
        },
        &[key, common::key(1)],
    );
    c.goto(seated_at + k::ROW_RETENTION_EPOCHS + 200);
    assert!(c.st.members.contains_key(&row), "an open obligation names the row");
}

#[test]
fn a_default_keeps_the_row_for_ever() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    let key = stranger(0x65, 0);
    let seated_at = c.st.epoch;
    let contract = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Key(key),
            creditor: Party::Member(1),
            amount: 1.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key, common::key(1)],
    );
    let row = c.st.member_of_key(&key).expect("seated");
    c.goto(seated_at + MATURITY + 1);
    assert_eq!(c.st.contracts[&contract].status, ContractStatus::Expired, "the sweep expired it");
    c.goto(seated_at + 3 * k::ROW_RETENTION_EPOCHS);
    assert!(c.st.members.contains_key(&row), "a default is live, and a live row is never retired");
}

#[test]
fn a_supply_and_voting_power_keep_a_row_nobody_trades_with() {
    // Two founders and a validator, none of whom ever trade: a supply and a
    // vote are what a ceremony put there, and they are not nothing.
    let mut c = Chain::founded(2, 0);
    c.ok(Tx::SetConsensusKey { member: 0, key: Some([0x99; 32]) }, &[common::key(0)]);
    c.st.validators.insert(0, 1);
    c.goto(c.st.epoch + k::ROW_RETENTION_EPOCHS + 5);
    assert!(c.st.members.contains_key(&0) && c.st.members.contains_key(&1));
}

#[test]
fn a_suspension_keeps_the_row() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    let key = stranger(0x66, 0);
    let seated_at = c.st.epoch;
    let row = seat_and_settle(&mut c, 1, key, 1.0);
    c.st.members.get_mut(&row).expect("the row").status = MemberStatus::Suspended;
    c.goto(seated_at + k::ROW_RETENTION_EPOCHS + 5);
    assert!(c.st.members.contains_key(&row), "a sanction is not nothing");
}

#[test]
fn a_guardian_roll_and_a_panel_keep_the_rows_they_name() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, 500.0);
    c.back(0, 2, 500.0);
    let (ka, kb) = (stranger(0x67, 0), stranger(0x67, 1));
    let seated_at = c.st.epoch;
    let a = seat_and_settle(&mut c, 1, ka, 1.0);
    let b = seat_and_settle(&mut c, 1, kb, 1.0);
    // `a` is on member 2's guardian roll; `b` is on a panel of a live
    // obligation between 1 and 2.
    c.ok(
        Tx::RegisterGuardians { member: 2, guardians: vec![a, 1], threshold: 2, veto_window_epochs: MATURITY },
        &[common::key(2)],
    );
    c.ok(
        Tx::Accept {
            debtor: Party::Member(2),
            creditor: Party::Member(1),
            amount: 10.0,
            maturity_epochs: k::MAX_HORIZON_EPOCHS,
            arb: Some(ArbTermsWire {
                arbiters: [b].into_iter().collect(),
                quorum: 1,
                window_epochs: k::MAX_HORIZON_EPOCHS,
                award_cap: 10.0,
            }),
        },
        &[common::key(1), common::key(2)],
    );
    c.goto(seated_at + k::ROW_RETENTION_EPOCHS + 5);
    assert!(c.st.members.contains_key(&a), "a guardian roll names the row");
    assert!(c.st.members.contains_key(&b), "a panel names the row");
}

/// **A farm gains nothing by letting its rows go empty.** A sponsor behind
/// one edge of 500.00 seats 25 rows and is refused the 26th; the rows are
/// traded with never, so a year on they are retired and the seats return —
/// and the sponsor seats 25 more. Live rows never exceed the ceiling, and
/// the rows seated over the whole run are the ceiling times one plus the
/// run's length over the retention window: a rate the release bounds, on
/// rows that were carrying no allowance and no capacity while they lived.
///
/// Mutation that bites: give the seats back on `Exit` rather than on
/// retirement, and a farm cycles its rows every epoch instead of every year.
#[test]
fn a_farm_cannot_recycle_seats_faster_than_the_retention_window() {
    let mut c = Chain::founded(1, 1);
    let unit = c.st.params.bond_unit();
    let ceiling = (500.0 / unit) as u32;
    let mut seated_total = 0u32;
    let mut live_peak = 0usize;
    let start = c.st.epoch;
    let mut n = 0u32;
    for wave in 0..2u32 {
        // Seat to the ceiling, renewing the backing at each boundary so what
        // runs out is the seat and never the work bond.
        let mut wave_seated = 0u32;
        'seating: loop {
            c.back(0, 1, 500.0);
            loop {
                let key = stranger(0x70 + wave as u8, n);
                n += 1;
                match c.apply(
                    Tx::Accept {
                        debtor: Party::Key(key),
                        creditor: Party::Member(1),
                        amount: 1.0,
                        maturity_epochs: MATURITY,
                        arb: None,
                    },
                    &[key, common::key(1)],
                ) {
                    Ok(()) => {
                        let contract = c.st.next_contract - 1;
                        c.ok(Tx::Settle { contract, amount: 1.0 }, &[key, common::key(1)]);
                        wave_seated += 1;
                        seated_total += 1;
                    }
                    Err(Error(ET_BOND_SEAT_UNBACKED)) => break 'seating,
                    Err(Error(ET_BOND_EXHAUSTED)) => break,
                    other => panic!("seating row {n}: {other:?}"),
                }
            }
            c.goto(c.st.epoch + 1);
        }
        assert_eq!(wave_seated, ceiling, "wave {wave}: the ceiling is the edge over the unit");
        live_peak = live_peak.max(c.st.members.len() - 2);
        // A year with the backing renewed and the rows left alone.
        for _ in 0..=k::ROW_RETENTION_EPOCHS {
            c.back(0, 1, 500.0);
            c.goto(c.st.epoch + 1);
        }
        assert_eq!(c.st.members.len(), 2, "wave {wave}: every empty row is retired");
        assert_eq!(seat_total(&c.st), 0, "and every seat is back");
    }
    let epochs = c.st.epoch - start;
    let bound = ceiling as f64 * (1.0 + epochs as f64 / k::ROW_RETENTION_EPOCHS as f64);
    assert!(live_peak as u32 <= ceiling, "live rows never exceed the ceiling: {live_peak}");
    assert!(
        (seated_total as f64) <= bound,
        "rows seated over {epochs} epochs are bounded by the ceiling over the window: {seated_total} against {bound:.0}"
    );
    assert_eq!(seated_total, 2 * ceiling, "two waves, two ceilings");
}

// ------------------------------------------------------------ the sponsor --

/// **A row that sponsors a live seat is not empty.** The seat names its
/// sponsor for the life of the row it bought, and invariant 7 asks that a
/// sponsor be a member — so the sweep retiring a sponsor whose own trade had
/// gone quiet, while the row it seated lived on, orphaned the seat and halted
/// every node at that very boundary. No adversary: a member who brought a
/// newcomer in and then traded nothing for a year.
///
/// Mutation that bites: drop the seat's sponsor from `named` in
/// `retire_empty_rows`. A is retired at epoch 390 and the audit refuses the
/// sweep with "a seat names sponsor 1, who is not a member".
#[test]
fn a_sponsor_stays_seated_until_the_rows_it_seated_are_gone() {
    let mut c = Chain::founded(1, 2);
    let (u, a, x) = (0u64, 1u64, 2u64);
    c.back(u, a, 500.0);
    c.back(u, x, 500.0);
    let nk = stranger(0x71, 0);
    let n = seat_and_settle(&mut c, a, nk, 1.0);
    assert_eq!(c.st.members[&n].seat.as_ref().map(|s| s.sponsor), Some(a));
    // N stays alive on a long-dated claim; A trades nothing more.
    let claim = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(x),
            creditor: Party::Member(n),
            amount: 10.0,
            maturity_epochs: 9_000,
            arb: None,
        },
        &[key(x as usize), nk],
    );
    for e in (30..=800).step_by(30) {
        c.goto(e);
        assert!(c.st.members.contains_key(&a), "epoch {e}: A sponsors a live seat, so A is not empty");
        assert!(c.st.members.contains_key(&n), "epoch {e}: N holds a live claim");
    }
    assert!(c.st.edges.get(&(u as usize, a as usize)).is_none(), "A's own backing decayed away long ago");

    // N's claim closes; the closed row names N for its window, then N is
    // retired — and A goes at the boundary AFTER N's, never the same one.
    c.ok(Tx::Settle { contract: claim, amount: 10.0 }, &[key(x as usize), nk]);
    let closed_at = c.st.epoch;
    c.goto(closed_at + k::CLOSED_RETENTION_EPOCHS + 1);
    assert!(!c.st.members.contains_key(&n), "N is empty and past its window");
    assert!(c.st.members.contains_key(&a), "A was named by N's seat in the sweep that retired N");
    c.goto(c.st.epoch + 1);
    assert!(!c.st.members.contains_key(&a), "and is empty at the next boundary");
    assert_eq!(seat_total(&c.st), 0, "every seat came back");
}

/// The set quantifier: a sponsor of five rows stays while ONE of them lives.
#[test]
fn a_sponsor_of_many_rows_stays_while_any_of_them_lives() {
    let mut c = Chain::founded(1, 2);
    let (u, a, x) = (0u64, 1u64, 2u64);
    c.back(u, a, 500.0);
    c.back(u, x, 500.0);
    let rows: Vec<(MemberId, Key)> = (0..5u32)
        .map(|i| {
            let nk = stranger(0x72, i);
            (seat_and_settle(&mut c, a, nk, 1.0), nk)
        })
        .collect();
    // Only the last row holds a live claim.
    let (last, lk) = rows[4];
    c.ok(
        Tx::Accept {
            debtor: Party::Member(x),
            creditor: Party::Member(last),
            amount: 10.0,
            maturity_epochs: 9_000,
            arb: None,
        },
        &[key(x as usize), lk],
    );
    c.goto(800);
    for (row, _) in &rows[..4] {
        assert!(!c.st.members.contains_key(row), "an empty row is retired");
    }
    assert!(c.st.members.contains_key(&last));
    assert!(c.st.members.contains_key(&a), "one live seat keeps its sponsor");
    edet_state::invariants::audit(&c.st).expect("clean at every boundary");
}
