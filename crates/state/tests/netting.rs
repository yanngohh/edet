//! **Rings of defaults, netted by their minimum, with no signature.**
//!
//! A ring is A owing B owing C owing A. Netting the smallest of the three
//! relieves each party of exactly as much debt as claim, so nobody loses — and
//! §Recourse already states that rule for the bilateral case a sale runs. This
//! is the same act over a longer cycle, and the same code
//! (`cascade::discharge_hop`).
//!
//! **Substitution breaks a ring of INSURED defaults, and that is not a
//! defect.** `mark_expired` moves an insured claim onto the underwriters, so
//! its creditor becomes an underwriter and the cycle it was part of is a cycle
//! no longer — what is left is a star into the underwriters plus their
//! substitution legs, which are live rather than in default. So what the sweep
//! finds in practice is rings of UNINSURED defaults, which is exactly where
//! nothing else was going to help: no underwriter stands behind them, and the
//! creditors were bearing them alone.
//!
//! Every probe runs on the shared harness, so every transition and every epoch
//! crank here is audited over all six invariants, both audits are driven, and
//! the stored holds are re-checked as a conserved flow.

mod common;
use common::*;

use edet_state::types::ContractStatus;

/// The three-member community every ring below is built in: three founders
/// with a supply each, so all three can trade, and standing earned by trading.
fn ring_of(n: usize) -> Chain {
    Chain::founded(n, 0)
}

/// Book `amount` from `creditor` to `debtor` and let it fall due, so the row
/// is `Expired` and its holder cannot collect. Returns the contract.
fn defaulted(c: &mut Chain, creditor: MemberId, debtor: MemberId, amount: f64) -> ContractId {
    c.lend(creditor, debtor, amount)
}

/// Advance past every open maturity, which expires them all at once.
fn expire_all(c: &mut Chain) {
    let due = c.st.contracts.values().map(|k| k.maturity_epoch).max().unwrap_or(0);
    c.goto(due + 1);
}

fn outstanding_of(c: &Chain, id: ContractId) -> f64 {
    State::from_minor(c.st.contracts[&id].outstanding)
}

use edet_state::state::State;
use edet_state::types::{ContractId, MemberId};

/// **A ring of three defaults is netted by its minimum.**
///
/// Exact amounts on every hop: the smallest claim closes and the other two
/// fall by exactly that much. Nobody is relieved of more debt than claim, or
/// of less.
///
/// Mutation that bites: net the MAXIMUM instead. A hop then discharges more
/// than it holds, `discharge_hop` clamps to the outstanding, and the ring is
/// no longer netted by one figure — the two larger claims close while the
/// smallest debtor is relieved of a debt nobody's claim paid for, which the
/// conservation clause of the audit refuses on the spot.
#[test]
fn a_ring_of_three_defaults_is_netted_by_its_minimum() {
    let mut c = ring_of(3);
    // 0 lends to 1, 1 to 2, 2 to 0 — so the DEBT runs 1 -> 0, 2 -> 1, 0 -> 2,
    // which is the ring.
    let a = defaulted(&mut c, 0, 1, 100.0);
    let b = defaulted(&mut c, 1, 2, 60.0);
    let d = defaulted(&mut c, 2, 0, 80.0);
    expire_all(&mut c);

    // The minimum is 60: it closes, and the other two fall by 60.
    assert_eq!(outstanding_of(&c, b), 0.0);
    assert_eq!(c.st.contracts[&b].status, ContractStatus::Cured);
    assert_eq!(outstanding_of(&c, a), 40.0);
    assert_eq!(outstanding_of(&c, d), 20.0);
}

/// **Netting writes standing on every hop**, because it IS a discharge: the
/// debtor gave up a claim of their own worth exactly what they were relieved
/// of. Capped by what the creditor may confer, like every other stake.
#[test]
fn netting_writes_standing_on_every_hop() {
    let mut c = ring_of(3);
    defaulted(&mut c, 0, 1, 100.0);
    defaulted(&mut c, 1, 2, 60.0);
    defaulted(&mut c, 2, 0, 80.0);
    let before: Vec<f64> = (0..3).map(|i| c.cap(i)).collect();
    expire_all(&mut c);
    for id in 0..3u64 {
        assert!(
            c.cap(id) >= before[id as usize],
            "member {id}'s standing must not fall through a netting: {} against {}",
            c.cap(id),
            before[id as usize]
        );
    }
    // And the stake really is written creditor -> debtor on each hop.
    for &(creditor, debtor) in &[(0usize, 1usize), (1, 2), (2, 0)] {
        assert!(
            c.st.edges.get(&(creditor, debtor)).copied().unwrap_or(0) > 0,
            "no stake on the hop {creditor} -> {debtor}"
        );
    }
}

/// **A ring through a live obligation is left alone.**
///
/// The consent argument is that every hop is in DEFAULT, so every party is
/// already due and nobody is paid early. Drop that and netting hands a
/// creditor an early payment they never agreed to take.
#[test]
fn a_ring_through_an_active_obligation_is_left_alone() {
    let mut c = ring_of(3);
    let a = defaulted(&mut c, 0, 1, 100.0);
    let b = defaulted(&mut c, 1, 2, 60.0);
    // The third hop is booked LATER, so it is still live when the first two
    // fall due.
    c.goto(c.st.contracts[&a].maturity_epoch - 1);
    let d = c.lend(2, 0, 80.0);
    c.goto(c.st.contracts[&a].maturity_epoch + 1);

    assert_eq!(c.st.contracts[&a].status, ContractStatus::Expired, "a and b defaulted");
    assert_eq!(outstanding_of(&c, a), 100.0, "and nothing was netted");
    assert_eq!(outstanding_of(&c, b), 60.0);
    assert_eq!(c.st.contracts[&d].status, ContractStatus::Active);
    assert_eq!(outstanding_of(&c, d), 80.0);
}

/// **A ring with an arbitrated hop is left alone.** A claim under a window is
/// the panel's to decide, and the sweep must not settle it out from under
/// them.
#[test]
fn a_ring_with_an_arbitrated_hop_is_left_alone() {
    let mut c = ring_of(4);
    let a = defaulted(&mut c, 0, 1, 100.0);
    let b = defaulted(&mut c, 1, 2, 60.0);
    // The third hop names a panel.
    let d = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(2),
            amount: 80.0,
            maturity_epochs: MATURITY,
            arb: Some(ArbTermsWire {
                arbiters: [3].into_iter().collect(),
                quorum: 1,
                window_epochs: 30,
                award_cap: 80.0,
            }),
        },
        &[key(2), key(0)],
    );
    expire_all(&mut c);

    assert_eq!(outstanding_of(&c, a), 100.0, "the ring is broken by the arbitrated hop");
    assert_eq!(outstanding_of(&c, b), 60.0);
    assert_eq!(outstanding_of(&c, d), 80.0);
}

/// **A ring through a suspended member is left alone.** Suspension revokes
/// origination, and a discharge that writes a stake is an origination of
/// standing.
#[test]
fn a_ring_through_a_suspended_member_is_left_alone() {
    let mut c = ring_of(4);
    let a = defaulted(&mut c, 0, 1, 100.0);
    let b = defaulted(&mut c, 1, 2, 60.0);
    let d = defaulted(&mut c, 2, 0, 80.0);

    // Suspended directly rather than through the governance path: what this
    // probe is about is `eligible_hop`'s status test, and `governance.rs` is
    // where the vote that reaches it is measured.
    c.st.members.get_mut(&2).expect("member 2").status = MemberStatus::Suspended;
    assert_eq!(c.status(2), MemberStatus::Suspended);

    expire_all(&mut c);
    assert_eq!(outstanding_of(&c, a), 100.0, "every hop of the ring touches the suspended member");
    assert_eq!(outstanding_of(&c, b), 60.0);
    assert_eq!(outstanding_of(&c, d), 80.0);
}

/// **The sweep is bounded per epoch**, because it is zero-priced and nobody
/// signs it. What it does not reach this epoch it reaches the next: the
/// precondition is a property of the book rather than of a moment.
#[test]
fn the_sweep_is_bounded_per_epoch() {
    // Two members per ring, each a founder in their own right: an underwriter
    // is established by their OWN supply, so every pair can book on their
    // allowance — and nobody has staked on anybody, so capacity is zero and
    // every ring hop is UNINSURED, which is the shape a ring survives in (see
    // the module doc).
    let rings = edet_kernel::constants::NETTING_MAX_RINGS_PER_EPOCH + 4;
    let mut c = Chain::founded(2 * rings, 0);
    // The write budget is not what this probe is about: a hundred and thirty
    // rings in one epoch is far past one member's allowance, and an epoch
    // boundary in the middle would expire the first rings before the last are
    // written.
    c.st.params.bond_free_allowance = 100_000;
    let mut ids = Vec::new();
    for r in 0..rings {
        let (x, y) = ((2 * r) as MemberId, (2 * r + 1) as MemberId);
        ids.push(c.lend(x, y, 10.0));
        ids.push(c.lend(y, x, 10.0));
    }
    expire_all(&mut c);

    let netted = ids
        .iter()
        .filter(|id| c.st.contracts[id].status == ContractStatus::Cured)
        .count();
    assert_eq!(netted, 2 * edet_kernel::constants::NETTING_MAX_RINGS_PER_EPOCH, "one sweep nets its cap and no more");

    // And the rest waits for the next boundary rather than being forgotten.
    c.goto(c.st.epoch + 1);
    let after = ids
        .iter()
        .filter(|id| c.st.contracts[id].status == ContractStatus::Cured)
        .count();
    assert_eq!(after, ids.len(), "the next sweep takes the remainder");
}

/// **Netting is a function of state, not of insertion order.** Two chains that
/// reach the same book by different routes net identically, or two replicas
/// applying the same blocks in the same order could still diverge.
#[test]
fn netting_is_a_function_of_state_not_of_insertion_order() {
    let build = |order: [(MemberId, MemberId, f64); 3]| {
        let mut c = ring_of(3);
        for (creditor, debtor, amount) in order {
            defaulted(&mut c, creditor, debtor, amount);
        }
        expire_all(&mut c);
        let mut rows: Vec<(ContractId, u64, ContractStatus)> =
            c.st.contracts.values().map(|k| (k.id, k.outstanding, k.status)).collect();
        rows.sort_by_key(|r| r.0);
        (rows, c.st.edges.clone())
    };
    // The same three obligations, booked in two different orders. The contract
    // IDS differ, so compare what each PAIR ended up owing rather than the row
    // ids.
    let (rows_a, edges_a) = build([(0, 1, 100.0), (1, 2, 60.0), (2, 0, 80.0)]);
    let (rows_b, edges_b) = build([(2, 0, 80.0), (0, 1, 100.0), (1, 2, 60.0)]);
    let owed = |rows: &[(ContractId, u64, ContractStatus)]| -> Vec<u64> {
        let mut v: Vec<u64> = rows.iter().map(|r| r.1).collect();
        v.sort_unstable();
        v
    };
    assert_eq!(owed(&rows_a), owed(&rows_b), "the same book must net to the same book");
    assert_eq!(edges_a, edges_b, "and to the same standing");
}

/// **A netted default lowers `open_default` by exactly what was netted.** The
/// figure is what the free allowance and the risk score read, so a discharge
/// that moved the book and not this would leave a member sanctioned for a
/// default that no longer exists.
#[test]
fn a_netted_default_lowers_open_default_by_what_was_netted() {
    let mut c = ring_of(3);
    defaulted(&mut c, 0, 1, 100.0);
    defaulted(&mut c, 1, 2, 60.0);
    defaulted(&mut c, 2, 0, 80.0);
    expire_all(&mut c);

    // Member 2's own debt was 60 and it netted in full, so nothing of it is
    // open any more.
    assert_eq!(c.st.members[&2].rep.open_default, 0, "a fully netted default is not an open one");
    // Members 0 and 1 keep the remainder, and only the remainder.
    assert_eq!(State::from_minor(c.st.members[&0].rep.open_default), 20.0);
    assert_eq!(State::from_minor(c.st.members[&1].rep.open_default), 40.0);
}

use edet_state::tx::Tx;
use edet_state::types::{ArbTermsWire, MemberStatus, Party};
