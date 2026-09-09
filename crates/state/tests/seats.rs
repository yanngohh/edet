//! **A row is a stock, and its price is a reservation on the graph.**
//!
//! A seat takes one bond unit of flow from the community's seed to the
//! SPONSOR, on the stake graph itself, on a reservation pair the credit layer
//! never reads, and it is never given back. Shared and permanent, both: per
//! account a farm child and an honest newcomer are the same reading, so the
//! reservation has to be on the shared graph where the child's residual is the
//! cut less what its parent and siblings already drew; and a released seat
//! prices a rate where a row is a stock.
//!
//! The bound, for every set `S` of non-underwriters and every superset `S'`
//! holding none either: `unit x seats(S) <= Σ peak(e)` over the arcs into
//! `S'`, with a supply arc counted at its all-time maximum declaration. Time
//! appears nowhere in it, which is why decay frees nothing and renewal to an
//! old peak opens nothing.
//!
//! Every probe drives `Chain`, so the whole seven-invariant audit and both
//! caches run after each transition, and each names the mutation that bites.

mod common;
use common::*;
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

/// One bond unit at the genesis parameters: 0.02 x 1,000.
const UNIT: f64 = 20.0;

/// A key belonging to nobody. `tag` separates the families a probe uses so two
/// waves never collide.
fn stranger(tag: u8, n: u32) -> Key {
    let mut k = [tag; 32];
    k[0..4].copy_from_slice(&n.to_be_bytes());
    k
}

/// Seat one row through `sponsor`, who takes the CREDITOR side so the trade
/// moves no capacity of theirs: the stranger is the debtor, reaches nothing,
/// and the obligation is uninsured.
fn seat_through(c: &mut Chain, sponsor: MemberId, k: Key) -> Res<()> {
    c.apply(
        Tx::Accept {
            debtor: Party::Key(k),
            creditor: Party::Member(sponsor),
            amount: 1.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[k, key(sponsor as usize)],
    )
}

/// Seat through `sponsor` until the seat gate refuses, renewing the work bond
/// at each epoch boundary so what runs out is the SEAT and never the bond.
/// Returns how many rows were seated.
fn seat_until_refused(c: &mut Chain, sponsor: MemberId, tag: u8, renew: Option<(MemberId, f64)>) -> u32 {
    let mut n = 0u32;
    let mut seated = 0u32;
    for _ in 0..64 {
        if let Some((creditor, amount)) = renew {
            c.back(creditor, sponsor, amount);
        }
        loop {
            let k = stranger(tag, n);
            n += 1;
            match seat_through(c, sponsor, k) {
                Ok(()) => seated += 1,
                Err(Error(ET_BOND_SEAT_UNBACKED)) => return seated,
                // The work bond is a RATE and refills at the boundary; the
                // seat does not. Waiting is what separates the two budgets.
                Err(Error(ET_BOND_EXHAUSTED)) => break,
                other => panic!("unexpected outcome seating row {n}: {other:?}"),
            }
        }
        c.goto(c.st.epoch + 1);
    }
    panic!("the seat gate never refused — the seat layer is holding nothing");
}

/// **A seat holds one bond unit of the sponsor's reach, and nothing gives it
/// back.** The credit layer does not move for it, and the row keeps its own
/// record of what bought it.
///
/// Mutation that bites: point `reserve_seat` at a throwaway map, and invariant
/// 7 refuses the first seat outright — the maps are a cache of what the seats
/// hold, and nothing else may write them.
#[test]
fn a_seat_holds_one_bond_unit_of_the_sponsors_reach_for_the_life_of_the_row() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    let (reach, cap) = (c.st.seat_reach(1), c.cap(1));
    assert_eq!(reach, 500.0, "the write layer reads the whole edge before any seat");

    seat_through(&mut c, 1, stranger(0x31, 0)).expect("an established member brings a newcomer in");
    let seated = c.st.next_member - 1;
    assert_eq!(c.st.seat_reach(1), reach - UNIT, "one row, one bond unit of reach");
    assert_eq!(c.cap(1), cap, "and the credit layer does not move for it");

    let seat = c.st.members[&seated].seat.clone().expect("the row records what bought it");
    assert_eq!(seat.sponsor, 1);
    assert_eq!(seat.held.amount(), State::to_minor(UNIT), "one unit, on the arcs the flow took");
    assert_eq!(c.st.seat_reserved.get(&(0, 1)).copied(), Some(State::to_minor(UNIT)));

    // Four hundred epochs later the arc has faded — a seat is not a floor —
    // and the seat still holds exactly what it took.
    c.goto(c.st.epoch + 400);
    assert!(c.st.edges.get(&(0, 1)).is_none(), "an unrenewed edge decays away");
    assert_eq!(
        c.st.seat_reserved.get(&(0, 1)).copied(),
        Some(State::to_minor(UNIT)),
        "and the seat is still spent on it"
    );
    assert_eq!(c.st.members[&seated].seat.as_ref().map(|s| s.held.amount()), Some(State::to_minor(UNIT)));
}

/// **A farm behind one edge of 500.00 stops at 25 rows, and at 250 behind
/// 5,000.00.** Linear in the backing, and in nothing else: the operator
/// renews the edge to its peak at every boundary, so the work bond refills
/// every epoch and what runs out is the seat.
///
/// The farm's gross cut as a SET stays exactly the one edge throughout, which
/// is the capacity claim this rule does not touch.
///
/// Mutation that bites: point `reserve_seat` at a throwaway map and drop
/// invariant 7's conservation clauses with it — the seat gate then never
/// refuses at all, and the same edge of 500.00 carries another 25 rows at every
/// boundary for as long as the operator renews it.
#[test]
fn a_farm_behind_one_edge_of_500_stops_at_25_rows_and_at_250_behind_5000() {
    for (backing, rows) in [(500.0, 25u32), (5_000.0, 250)] {
        let mut c = Chain::founded_with(&[10_000.0], 1);
        c.back(0, 1, backing);
        let seated = seat_until_refused(&mut c, 1, 0x32, Some((0, backing)));
        assert_eq!(seated, rows, "{backing} of backing carries {rows} rows and no more");

        let farm: Vec<MemberId> = (1..c.st.next_member).collect();
        assert_eq!(
            c.st.gross_capacity_of_set(&farm),
            backing,
            "and the set's gross cut is still the one edge behind it"
        );
        assert_eq!(
            seat_through(&mut c, 1, stranger(0x39, 0)),
            Err(Error(ET_BOND_SEAT_UNBACKED)),
            "every further seat names the seat layer, not the work bond"
        );
    }
}

/// **Decay frees no seat, and renewal to the old peak frees none either.**
/// The bound is over all-time PEAKS, so time appears in it nowhere.
///
/// Mutation that bites: floor `flow::decay` at `seat_reserved` as well, or
/// release a seat on any schedule — either way the count grows without new
/// backing.
#[test]
fn decay_frees_no_seat_and_renewal_to_the_old_peak_frees_none() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    let filled = seat_until_refused(&mut c, 1, 0x33, Some((0, 500.0)));
    assert_eq!(filled, 25);
    let rows = c.st.members.len();

    for _ in 0..400 {
        c.goto(c.st.epoch + 1);
        c.back(0, 1, 500.0); // renewal to the same peak
        assert_eq!(
            seat_through(&mut c, 1, stranger(0x34, 0)),
            Err(Error(ET_BOND_SEAT_UNBACKED)),
            "a renewed peak is not new backing"
        );
    }
    assert_eq!(c.st.members.len(), rows, "four hundred epochs of renewal seat nobody");
}

/// **A second backer adds exactly its own peak over the unit**, and raising an
/// existing one adds only the increment.
#[test]
fn a_second_backer_adds_exactly_its_own_peak_over_the_unit() {
    let mut c = Chain::founded(2, 1);
    c.back(0, 2, 500.0);
    assert_eq!(seat_until_refused(&mut c, 2, 0x35, Some((0, 500.0))), 25);

    // A second underwriter's own edge, at the same size: exactly 25 more.
    c.back(1, 2, 500.0);
    assert_eq!(seat_until_refused(&mut c, 2, 0x36, Some((1, 500.0))), 25, "a second peak of 500.00 is 25 more rows");

    // Renewing either edge to the peak it already reached opens nothing; a
    // raise opens exactly the increment over the unit.
    c.back(0, 2, 500.0);
    assert_eq!(
        seat_through(&mut c, 2, stranger(0x37, 0)),
        Err(Error(ET_BOND_SEAT_UNBACKED)),
        "renewal to the old peak is not backing"
    );
    c.back(0, 2, 600.0);
    assert_eq!(seat_until_refused(&mut c, 2, 0x38, Some((0, 600.0))), 5, "a raise of 100.00 is five rows");
}

/// **The arc under a seat is not floored, and the sponsor's capacity goes with
/// it.** Flooring it would hand a sponsor 500.00 of permanent capacity for 25
/// seats — the exploit rather than the fix — so invariant 7 deliberately does
/// not claim `seat_reserved <= edges`.
///
/// Mutation that bites: floor `decay` at the seat map, and the sponsor keeps
/// 500.00 of capacity for ever.
#[test]
fn the_arc_under_a_seat_decays_to_nothing_and_is_not_floored() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    assert_eq!(seat_until_refused(&mut c, 1, 0x40, Some((0, 500.0))), 25);
    assert_eq!(c.st.seat_reserved.get(&(0, 1)).copied(), Some(State::to_minor(500.0)));

    c.goto(c.st.epoch + 400);
    assert!(c.st.edges.get(&(0, 1)).is_none(), "the stake behind the seats has faded away");
    assert_eq!(
        c.st.seat_reserved.get(&(0, 1)).copied(),
        Some(State::to_minor(500.0)),
        "and the seats are still spent on it"
    );
    assert_eq!(c.cap(1), 0.0, "the sponsor's capacity fades with the stake, seats or no seats");
}

/// **Credit capacity does not read the seat layer.** Two chains aged
/// identically, one of them seating to its ceiling: every credit figure is
/// equal across the pair, and a full-size reservation still succeeds on both.
///
/// Mutation that bites: point `capacity_of_set_minor` at the seat maps, and
/// the seated chain reads 500.00 lower everywhere.
#[test]
fn credit_capacity_does_not_read_the_seat_layer() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, 500.0);
    c.back(0, 2, 500.0);
    let mut control = c.clone_for_control();

    assert_eq!(seat_until_refused(&mut c, 1, 0x41, Some((0, 500.0))), 25);
    // The control ages the same ticks and renews the same edge, so what is
    // being measured is the seats and never the clock.
    for _ in 0..(c.st.epoch - control.st.epoch) {
        control.goto(control.st.epoch + 1);
        control.back(0, 1, 500.0);
    }
    for _ in 0..(c.st.epoch - control.st.epoch) {
        control.goto(control.st.epoch + 1);
    }

    for id in [0u64, 1, 2] {
        assert_eq!(c.cap(id), control.cap(id), "capacity of {id} must not move for a seat");
        assert_eq!(c.st.conferrable(id), control.st.conferrable(id), "conferrable of {id}");
        assert_eq!(c.st.seed_reach(id), control.st.seed_reach(id), "the write floor's reach for {id}");
    }
    assert_eq!(c.st.reserve_capacity(1, 400.0).map(|h| h.amount()), Some(State::to_minor(400.0)));
    assert_eq!(control.st.reserve_capacity(1, 400.0).map(|h| h.amount()), Some(State::to_minor(400.0)));
}

/// **An underwriter seats from its own supply.** A founder holds supply and no
/// in-stakes, so a rule that excluded a target's own supply arc — the credit
/// layer's no-self-underwriting clause — would leave the first members of a
/// community with nobody able to bring them in.
///
/// Mutation that bites: build the seat query with `flow::capacity` instead of
/// `capacity_with_own_supply`, and `Chain::founded(1, 0)` can seat nobody.
#[test]
fn an_underwriter_seats_from_its_own_supply() {
    let mut c = Chain::founded(2, 0);
    assert_eq!(c.cap(0), 0.0, "a founding underwriter's own capacity is zero");
    assert_eq!(c.st.seat_reach(0), SUPPLY, "and its write reach is its own declaration");

    seat_through(&mut c, 0, stranger(0x42, 0)).expect("a founder brings the first member in");
    assert_eq!(c.st.seat_committed.get(&0).copied(), Some(State::to_minor(UNIT)));
    assert_eq!(c.st.seat_committed.get(&1).copied(), None, "a second founder's supply is untouched");
    assert_eq!(c.st.seat_reach(0), SUPPLY - UNIT);
    assert_eq!(c.st.seat_reach(1), SUPPLY);
}

/// **A community seats its supply over the unit, and then nobody.** The
/// ceiling is real — a bound that never binds is not one — and a ceremony is
/// the door that reopens it, which is the same door every supply arc comes
/// through.
#[test]
fn a_community_seats_its_supply_over_the_unit_and_then_nobody() {
    let mut c = Chain::founded(3, 0);
    let seed = 3.0 * SUPPLY;
    let mut seated = 0u32;
    for (tag, sponsor) in (0x43u8..).zip(0..3u64) {
        seated += seat_until_refused(&mut c, sponsor, tag, None);
    }
    assert_eq!(seated as f64, seed / UNIT, "the seed over the unit, exactly");
    assert_eq!(
        State::from_minor(c.st.seat_committed.values().sum::<u64>()),
        seed,
        "and every unit of the seed is spent on a row"
    );
    for sponsor in 0..3u64 {
        assert_eq!(
            seat_through(&mut c, sponsor, stranger(0x4a, sponsor as u32)),
            Err(Error(ET_BOND_SEAT_UNBACKED)),
            "with the seed spent, nobody seats"
        );
    }

    // A ceremony is what reopens it: 2,000.00 more supply is exactly 100 rows.
    let amended = c.st.next_member;
    c.st.add_underwriter(vec![key(amended as usize)], 2_000.0)
        .expect("an amended underwriter");
    let opened = seat_until_refused(&mut c, amended, 0x4b, None);
    assert_eq!(opened, 100, "2,000.00 of new seed opens exactly 100 rows");
}

/// **A seat refusal arms no saturation and charges no bond.** A member who has
/// brought in as many people as their backing carries has abused nothing, and
/// arming the counter on it would let the epoch sweep forfeit their bonds for
/// exactly that.
///
/// Mutation that bites: set `bond_denied_this_epoch` in the `ET-BND-006`
/// branch of `bond_gate`, and `ForfeitBonds` succeeds after three epochs.
#[test]
fn a_seat_refusal_arms_no_saturation_and_charges_no_bond() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    assert_eq!(seat_until_refused(&mut c, 1, 0x44, Some((0, 500.0))), 25);

    let enc = c.st.members[&1].bond_enc();
    for _ in 0..3 {
        c.goto(c.st.epoch + 1);
        c.back(0, 1, 500.0);
        for n in 0..4u32 {
            assert_eq!(seat_through(&mut c, 1, stranger(0x45, n)), Err(Error(ET_BOND_SEAT_UNBACKED)));
        }
        assert!(!c.st.members[&1].bond_denied_this_epoch, "a seat refusal is not an exhausted budget");
        assert_eq!(c.st.members[&1].bond_saturated_epochs, 0, "so nothing accumulates toward forfeiture");
    }
    assert_eq!(c.st.members[&1].bond_enc(), enc, "and a refused seat charges nothing");
    c.err(Tx::ForfeitBonds { member: 1 }, &[key(0)], ET_BOND_NOT_SATURATED);
}

/// **Two fresh keys under one sponsor cost two seats.** A trade may name two
/// strangers under a third established signer, and each of them is a row; the
/// work bond stays one per transaction, and the seat count is what bounds that
/// payload.
///
/// Mutation that bites: make `fresh_keys` a flag again. The WRITE still takes
/// two seats — `seat_pair` reserves one per fresh party — so what breaks is the
/// gate: `admits` says yes to a trade `apply` then refuses, which is the one
/// thing the ingress, the pre-vote screen and commit may never disagree about.
#[test]
fn two_fresh_keys_under_one_sponsor_cost_two_seats() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 500.0);
    let reach = c.st.seat_reach(1);

    let (a, b) = (stranger(0x46, 0), stranger(0x46, 1));
    c.ok(
        Tx::Accept {
            debtor: Party::Key(a),
            creditor: Party::Key(b),
            amount: 1.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[a, b, key(1)],
    );
    assert_eq!(c.st.seat_reach(1), reach - 2.0 * UNIT, "two rows, two bond units");

    // Drawn down to a single unit and a half, a two-row trade is refused where
    // a one-row trade is admitted.
    while c.st.seat_reach(1) > 1.5 * UNIT {
        let next = c.st.next_member as u32;
        seat_through(&mut c, 1, stranger(0x47, next)).expect("room for one more");
        if c.st.members[&1].bond_enc() > 0 {
            c.goto(c.st.epoch + 1);
            c.back(0, 1, 500.0);
        }
    }
    let (x, y) = (stranger(0x48, 0), stranger(0x48, 1));
    let two = Tx::Accept {
        debtor: Party::Key(x),
        creditor: Party::Key(y),
        amount: 1.0,
        maturity_epochs: MATURITY,
        arb: None,
    };
    assert!(
        !edet_state::bond::admits(&c.st, &two, &[x, y, key(1)]),
        "the gate must count both rows: one unit of reach does not carry two"
    );
    assert_eq!(c.apply(two, &[x, y, key(1)]), Err(Error(ET_BOND_SEAT_UNBACKED)), "and the write agrees");
    let one = Tx::Accept {
        debtor: Party::Key(x),
        creditor: Party::Member(1),
        amount: 1.0,
        maturity_epochs: MATURITY,
        arb: None,
    };
    assert!(edet_state::bond::admits(&c.st, &one, &[x, key(1)]), "and it does carry one");
    c.ok(one, &[x, key(1)]);
}

/// **The branching shape**: two unit paths into the sponsor that share
/// nothing but a cross arc.
///
/// ```text
///   U → A → X → S        U → B → Y → S        and A → Y
/// ```
///
/// Every arc is one bond unit. A maximum flow into `S` is two units; a single
/// unit taken first runs `U→A→Y→S` — `A`'s arcs are walked in id order and
/// `Y` sorts before `X` — and leaves the residual with no path at all, because
/// a reservation never reroutes an earlier one. So this is where "two seats
/// as one flow" and "two seats one after the other" give different answers,
/// and the gate must give the write's.
///
/// The bond unit is raised to `0.10 × v_base` (the top of `BondFraction`'s
/// range) so that one unit of backing also clears the establishment floor:
/// every member here writes from an allowance, and the shape is built in one
/// epoch, undecayed.
fn diamond() -> (Chain, MemberId) {
    let mut c = Chain::founded(1, 5);
    c.st.params.bond_fraction = 0.10;
    let unit = c.st.params.bond_unit();
    let (u, a, b, y, x, s) = (0, 1, 2, 3, 4, 5);
    c.back(u, a, unit);
    c.back(u, b, unit);
    c.back(a, y, unit);
    c.back(a, x, unit);
    c.back(b, y, unit);
    // `X → S` before `Y → S`: once `S` has an in-edge its next loan is
    // insured, and the reservation for it takes `U→A`, which is `X`'s only
    // backing — a stake is capped by the creditor's residual conferrable at
    // the moment of settlement, so `X` would confer nothing for a loan whose
    // own reservation shadowed it.
    c.back(x, s, unit);
    c.back(y, s, unit);
    assert_eq!(c.st.seat_reach(s), 2.0 * unit, "two unit paths reach the sponsor");
    (c, s)
}

/// **A two-row trade the gate admits is a two-row trade the write seats.**
/// On the branching shape one flow of two units fits and two holds in
/// sequence do not; the gate asks the sequence, so both refuse, and the same
/// sponsor still seats one row — and then, the residual having no path, not
/// a second one, which is the stated cost of never rerouting an earlier seat.
///
/// Mutation that bites: ask `can_seat_minor` for one flow of `rows × unit`
/// again. `admits` says yes, `apply` says `ET-BND-006`, and the first
/// assertion fails with the work bond charged for a trade the gate approved.
#[test]
fn the_gate_asks_the_sequence_of_holds_the_write_takes() {
    let (mut c, s) = diamond();
    let (k, j) = (stranger(0x51, 0), stranger(0x52, 0));
    let two = Tx::Accept {
        debtor: Party::Key(k),
        creditor: Party::Key(j),
        amount: 1.0,
        maturity_epochs: MATURITY,
        arb: None,
    };
    let signers = [k, j, key(s as usize)];
    let admitted = edet_state::bond::admits(&c.st, &two, &signers);
    let written = c.apply(two, &signers);
    assert_eq!(
        admitted,
        written.is_ok(),
        "the gate and the write must agree on the branching shape: admitted {admitted}, written {written:?}"
    );
    assert_eq!(
        written,
        Err(Error(ET_BOND_SEAT_UNBACKED)),
        "two holds in sequence do not fit where one flow of two would"
    );

    seat_through(&mut c, s, stranger(0x53, 0)).expect("one row fits on either path");
    assert_eq!(
        seat_through(&mut c, s, stranger(0x53, 1)),
        Err(Error(ET_BOND_SEAT_UNBACKED)),
        "the first hold took the cross arc and left no path: a seat is refused that a re-solve would fit"
    );
}

/// **The gate and the write agree on every state.** The gate asks
/// `can_seat_minor` for the sequence of holds `reserve_seat` will take on the
/// same state, so an admitted seat is a written row and a refused one leaves
/// the root untouched — over one-edge backings of every size and over the
/// branching shape.
///
/// Mutation that bites: give the gate the credit reservation maps, and an
/// admitted trade reaches `seat`'s unreachable `None`.
#[test]
fn the_seat_gate_and_the_seat_write_agree_on_every_state() {
    for backing in [0.0f64, 20.0, 41.0, 137.5, 500.0] {
        let mut c = Chain::founded(1, 1);
        if backing > 0.0 {
            c.back(0, 1, backing);
        }
        agree_over(&mut c, 1, &format!("backing {backing}"));
    }
    let (mut c, s) = diamond();
    agree_over(&mut c, s, "the branching shape");
}

/// Forty trades through `sponsor`, alternating one- and two-stranger trades,
/// with the gate's verdict held to the write's at each.
fn agree_over(c: &mut Chain, sponsor: MemberId, label: &str) {
    for n in 0..40u32 {
        if n % 7 == 6 {
            c.goto(c.st.epoch + 1);
        }
        // Alternating one- and two-stranger trades, because the gate counts
        // rows and the write takes one reservation per row: a gate that
        // counted a flag would admit a two-row trade with one row of reach
        // left, and a gate that asked one flow of two units would admit a
        // two-row trade two holds cannot fit.
        let k = stranger(0x49, n);
        let pair = n % 3 == 2;
        let j = stranger(0x4c, n);
        let tx = Tx::Accept {
            debtor: Party::Key(k),
            creditor: if pair { Party::Key(j) } else { Party::Member(sponsor) },
            amount: 1.0,
            maturity_epochs: MATURITY,
            arb: None,
        };
        let signers: Vec<Key> = if pair { vec![k, j, key(sponsor as usize)] } else { vec![k, key(sponsor as usize)] };
        let admitted = edet_state::bond::admits(&c.st, &tx, &signers);
        let want = if pair { 2 } else { 1 };
        let rows = c.st.members.len();
        let seats = c.st.seat_committed.values().sum::<u64>();
        let root = edet_state::root::state_root(&c.st).expect("a root");
        match c.apply(tx, &signers) {
            Ok(()) => {
                assert!(admitted, "{label}, row {n}: the gate refused a write that succeeded");
                assert_eq!(c.st.members.len(), rows + want, "one row per fresh key");
                assert_eq!(
                    c.st.seat_committed.values().sum::<u64>(),
                    seats + want as u64 * State::to_minor(c.st.params.bond_unit()),
                    "and one seat per row"
                );
            }
            Err(Error(ET_BOND_SEAT_UNBACKED)) => {
                assert!(!admitted, "{label}, row {n}: the gate admitted a write that refused");
                assert_eq!(
                    edet_state::root::state_root(&c.st).expect("a root"),
                    root,
                    "a refused seat leaves the root untouched — no half-seated row, no spent reservation"
                );
            }
            Err(Error(ET_BOND_EXHAUSTED)) | Err(Error(ET_BOND_NO_PAYER)) => {
                assert!(!admitted, "{label}, row {n}: the gate admitted an unpayable write");
            }
            other => panic!("{label}, row {n}: {other:?}"),
        }
    }
}

/// **A re-denomination carries the seat layer.** Every seat's hold is scaled at
/// the ROUTE level so it is still a flow, the maps are rebuilt as the exact
/// sums, and the community's remaining room is the same room in the new unit.
#[test]
fn redenomination_carries_the_seat_layer() {
    for (num, den) in [(1u64, 2u64), (2, 1), (2, 3), (7, 5)] {
        let mut c = Chain::founded(1, 1);
        c.back(0, 1, 500.0);
        for n in 0..12u32 {
            seat_through(&mut c, 1, stranger(0x50, n)).expect("half a ceiling");
            if c.st.members[&1].bond_enc() >= State::to_minor(200.0) {
                c.goto(c.st.epoch + 1);
                c.back(0, 1, 500.0);
            }
        }
        let mut control = c.clone_for_control();
        let pi = num as f64 / den as f64;
        c.st.rescale(pi);
        c.audit_now("after a re-denomination");

        let left = seat_until_refused(&mut c, 1, 0x51, Some((0, 500.0 * pi)));
        let same = seat_until_refused(&mut control, 1, 0x51, Some((0, 500.0)));
        assert_eq!(left, same, "re-denominating at {num}/{den} must not change how many rows are left");
    }
}

// ------------------------------------------------------------ the unit moves --

/// **Moving the unit leaves every standing seat where it was, and the audit
/// clean.** A seat is taken at the unit in force and never moves; invariant 7
/// compared each seat against the LIVE price, so the first downward amendment
/// of `BondFraction` — the one dial the design offers for the seat count —
/// halted every node ("a seat on 1 holds 2000 against a seat price of 1000").
/// Both bounds on a seat's size are properties of the write.
///
/// Mutation that bites: reinstate the `held.amount() > price` clause in
/// invariant 7; the amendment's own block fails the audit.
#[test]
fn moving_the_unit_leaves_every_standing_seat_where_it_was() {
    let mut c = Chain::founded(1, 2);
    c.st.params.gov_cooldown_epochs = 0;
    c.back(0, 1, 500.0);
    c.back(0, 2, 500.0);
    let enact = |c: &mut Chain, kind: ProposalKind| {
        let pid = c.st.next_proposal;
        c.ok(Tx::Propose { author: 0, kind }, &[key(0)]);
        c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
        assert!(c.st.proposals[&pid].enacted, "the sole underwriter carries the whole seed");
    };

    seat_through(&mut c, 1, stranger(0x81, 0)).expect("a seat at 0.02");
    let old = c.st.next_member - 1;
    enact(&mut c, ProposalKind::ParamChange { key: ParamKey::BondFraction, value: 0.01 });
    edet_state::invariants::audit(&c.st).expect("a lawful amendment must not halt the chain");

    seat_through(&mut c, 2, stranger(0x82, 0)).expect("a seat at 0.01");
    let new = c.st.next_member - 1;
    let held = |c: &Chain, id: MemberId| c.st.members[&id].seat.as_ref().expect("seated").held.amount();
    assert_eq!(held(&c, old), State::to_minor(UNIT), "the old seat holds what it took");
    assert_eq!(held(&c, new), State::to_minor(UNIT / 2.0), "the new one holds the new unit");

    enact(&mut c, ProposalKind::ParamChange { key: ParamKey::BondFraction, value: 0.02 });
    edet_state::invariants::audit(&c.st).expect("and back up is lawful too");
    assert_eq!((held(&c, old), held(&c, new)), (State::to_minor(UNIT), State::to_minor(UNIT / 2.0)));

    // The control: a re-denomination scales seats and price together and was
    // always clean; it stays clean.
    enact(&mut c, ProposalKind::Redenominate { num: 2, den: 3 });
    edet_state::invariants::audit(&c.st).expect("a re-denomination with seats standing");
}
