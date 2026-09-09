//! The `f64` / minor-unit boundary.
//!
//! Every quantity in this model exists twice: as an `f64` on the contract book
//! and as an integer in the capacity path. The two are compared for EXACT
//! equality by §Verification invariants 1 and 2, and the audit runs on the commit path —
//! so a disagreement of one hundredth is not a rounding artefact, it is
//! `InvariantViolated` on every honest node at once.
//!
//! > **A quantity that crosses the `f64` / minor-unit boundary is compared
//! > exactly on one side and approximately on the other, and the audit
//! > believes the exact side.**
//!
//! Three halts lived here, all reachable on ordinary traffic with two honest
//! signatures: a partial discharge that leaves at most dust, any
//! re-denomination of a ledger whose holds span two arcs, and a fully drawn
//! community whose cut lands on one of the 6.5% of minor values that do not
//! survive `from_minor` and back. This suite is the gate for all three.
//!
//! **Tested at the quantifier the theorem uses.** The defect was never in one
//! amount, it was in a whole residue class of them — 81.6% of "pay all but one
//! cent" settlements — so the discharge probes below loop over every
//! two-decimal amount rather than sampling three.

mod common;

use common::{key, Chain, MATURITY, SUPPLY};
use edet_kernel::constants as k;
use edet_state::errors::{Error, ET_ARB_BAD_TERMS, ET_CTR_BAD_AMOUNT, ET_SEED_RATE};
use edet_state::invariants::audit;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

/// Every two-decimal amount from 0.02 to 1000.00 — the grid the audit
/// enumerated the halt over, and the one the fix has to survive whole.
fn every_two_decimal_amount() -> impl Iterator<Item = f64> {
    (2..=100_000u64).map(|cents| cents as f64 / 100.0)
}

/// The same, from 0.03. A `Sale` refuses an amount at or under dust, so
/// "all but one cent" of 0.02 is not a sale anybody can submit.
fn every_two_decimal_sale() -> impl Iterator<Item = f64> {
    (3..=100_000u64).map(|cents| cents as f64 / 100.0)
}

/// Drop closed rows. The transition is what is under test, not the book's
/// growth, and an audit whose cost is linear in a hundred thousand settled
/// contracts turns a gate into a timeout. Closed rows carry `outstanding = 0`
/// on both sides of every conservation sum, so removing them changes no
/// invariant.
fn forget_closed(c: &mut Chain) {
    c.st.contracts
        .retain(|_, x| matches!(x.status, ContractStatus::Active | ContractStatus::Expired));
}

/// A community with bonds priced at zero. This suite is about arithmetic at
/// the minor boundary and drives six-figure transition counts through it; the
/// write gate has its own suite, and a member saturated by the grid's own
/// traffic would fail these for a reason that is not what they measure.
fn unbonded(supplies: &[f64], members: usize) -> Chain {
    let mut c = Chain::founded_with(supplies, members).cold_audit_only();
    // The denomination the scene is written in — see `two_arc_ledger`.
    c.st.params.v_base = supplies.iter().copied().fold(0.0f64, f64::max).max(1.0);
    c.st.params.bond_fraction = 0.0;
    c
}

/// Discharge every live row in full, in one pass, and forget the closed ones.
/// The grids need the ledger back where they found it after each amount;
/// clearing it through the alphabet rather than by hand keeps every step under
/// the audit.
fn clear_book(c: &mut Chain) {
    let live: Vec<ContractId> =
        c.st.contracts
            .values()
            .filter(|x| matches!(x.status, ContractStatus::Active | ContractStatus::Expired))
            .map(|x| x.id)
            .collect();
    for cid in live {
        let x = c.st.contracts[&cid].clone();
        if x.outstanding == 0 {
            continue;
        }
        let signers = [key(x.debtor as usize), key(x.creditor as usize)];
        match x.status {
            ContractStatus::Expired => {
                c.ok(Tx::Cure { contract: cid, amount: State::from_minor(x.outstanding) }, &signers)
            }
            _ => c.ok(Tx::Settle { contract: cid, amount: State::from_minor(x.outstanding) }, &signers),
        }
    }
    forget_closed(c);
}

// --------------------------------------------------------- the round trip --

/// **`to_minor` inverts `from_minor`.** It did not: `x * MINOR` is the nearest
/// double to the product rather than the product, and for 6.5% of minor values
/// it lands one ulp low, so the floor took a whole unit off a quantity that was
/// exact. `to_minor(from_minor(29))` was 28.
///
/// Every quantity in the state machine makes this round trip — `conferrable`
/// and `gross_capacity_of_set` are `from_minor` of an integer the kernel
/// computed and are immediately `to_minor`'d back — so the failure is not
/// cosmetic in any of them.
#[test]
fn a_minor_quantity_survives_the_trip_through_f64_and_back() {
    for u in 0..=1_000_000u64 {
        assert_eq!(State::to_minor(State::from_minor(u)), u, "{u} did not survive from_minor -> to_minor");
    }
}

/// And the floor is still a floor. The snap corrects a representation, never
/// an amount: a genuine 0.289 is a third of a unit clear of the boundary and
/// must still round DOWN, because rounding up would let a reservation claim a
/// unit of standing no stake carries.
#[test]
fn a_real_fraction_of_a_minor_unit_still_rounds_down() {
    assert_eq!(State::to_minor(0.289), 28, "0.289 is 28 units and a fraction");
    assert_eq!(State::to_minor(0.2999), 29);
    assert_eq!(State::to_minor(0.019), 1);
    assert_eq!(State::to_minor(0.0099), 0);
    assert_eq!(State::to_minor(1.15), 115, "the f64 for 1.15 is a whisker low, and it means 115");
    assert_eq!(State::to_minor(0.0), 0);
    assert_eq!(State::to_minor(-1.0), 0);
    assert_eq!(State::to_minor(f64::NAN), 0);
    assert_eq!(State::to_minor(f64::INFINITY), 0);
}

/// **A fully drawn community must not fail its own audit.** §Verification invariant 1
/// compares the drawn flow — an integer straight out of the kernel — against
/// `to_minor(gross_capacity_of_set(..))`, which is the same kind of integer
/// after a round trip through `f64`. When that round trip lost a unit and the
/// community was drawn to its ceiling, the drawn flow exceeded the cut by one
/// and every honest node halted.
///
/// Two founding underwriters at 1190.20 and 1190.23 back one member for the
/// whole of each: the cut into them is 238,043 minor units, which is one of
/// the values a naive rescale returns as 238,042. One creditor lends the lot.
#[test]
fn a_community_drawn_to_its_ceiling_audits_clean() {
    let mut c = Chain::founded_with(&[1190.20, 1190.23], 2);
    let (d, cr) = (2, 3);
    c.back(0, d, 1190.20);
    c.back(1, d, 1190.23);
    assert_eq!(c.st.edges[&(0, d as usize)], 119_020, "the fixture's own arithmetic");
    assert_eq!(c.st.edges[&(1, d as usize)], 119_023);
    assert_eq!(State::to_minor(c.cap(d)), 238_043, "the cut is the unlucky value this probe is about");

    // 2380.4301, not 2380.43: the amount has to floor to the whole cut, and
    // the cut itself is one of the values an f64 cannot say exactly.
    let cid = c.lend(cr, d, 2380.4301);
    assert!(c.st.contracts[&cid].insured, "the whole cut is drawn, so the claim is insured");
    assert_eq!(c.st.contracts[&cid].held.amount(), 238_043, "drawn to the ceiling, exactly");
    audit(&c.st).expect("a fully drawn community is legal, and must audit clean");
}

// ---------------------------------------------------- the partial discharge --

/// **A partial discharge that leaves at most dust closes the row, so the cache
/// must follow the ROW and not the payment.** Owe 10.00, pay 9.99: the book
/// moves by the whole 10.00 and the debtor's cached `debt_out` moved by 9.99,
/// leaving the forgiven cent in the cache. §Verification's conservation check compares
/// the two at `1e-6`, on the commit path.
///
/// Enumerated over every two-decimal amount from 0.02 to 1000.00, because that
/// is where the defect lives: whether `outstanding - amount` lands at or under
/// dust is a property of the f64 subtraction, and it did so 81.6% of the time.
#[test]
fn settling_all_but_one_cent_never_breaks_conservation() {
    let mut c = unbonded(&[SUPPLY], 2);
    let (cr, d) = (1, 2);
    c.back(0, d, SUPPLY);
    for amount in every_two_decimal_amount() {
        let cid = c.lend(cr, d, amount);
        c.ok(Tx::Settle { contract: cid, amount: amount - 0.01 }, &[key(d as usize), key(cr as usize)]);
        forget_closed(&mut c);
    }
}

/// The same, through `Cure`. A cure closes a defaulted row at dust exactly as
/// a settlement closes a live one, and it moves a second cached quantity — the
/// debtor's open default — which must follow the row for the same reason.
///
/// Batched: the whole grid in chunks that fall due together, so one epoch
/// boundary expires a hundred claims instead of one. Substitution runs on each
/// of them, so this walks the cure of a SUBROGATED claim as well.
#[test]
fn curing_all_but_one_cent_never_breaks_conservation() {
    const BATCH: usize = 100;
    // A seed large enough that a whole batch stays insured, so the grid walks
    // the cure of SUBROGATED claims — substitution runs at every expiry — and
    // not only of uninsured ones.
    const SEED: f64 = 1_000_000.0;
    let mut c = unbonded(&[SEED], 2);
    let (cr, d) = (1, 2);
    let grid: Vec<f64> = every_two_decimal_amount().collect();
    for chunk in grid.chunks(BATCH) {
        // Refresh the backing: thirty-one epochs of decay pass per batch, and
        // a stake is a peak, so this restores the ceiling the batch needs.
        c.back(0, d, SEED);
        let opened: Vec<ContractId> = chunk.iter().map(|&a| c.lend(cr, d, a)).collect();
        let due = c.st.contracts[&opened[0]].maturity_epoch;
        c.goto(due + 1);
        for cid in opened {
            // Substitution moves an insured claim onto the underwriter, so the
            // creditor of record is whoever holds it now.
            let holder = c.st.contracts[&cid].creditor;
            let owed = State::from_minor(c.st.contracts[&cid].outstanding);
            if owed <= 2.0 * c.st.params.dust {
                continue;
            }
            c.ok(Tx::Cure { contract: cid, amount: owed - 0.01 }, &[key(d as usize), key(holder as usize)]);
        }
        clear_book(&mut c);
    }
}

/// The same, through a `Sale` that nets. Mutual obligations extinguish rather
/// than route, and `net_mutual` closes the row at dust on the same rule.
#[test]
fn netting_all_but_one_cent_never_breaks_conservation() {
    let mut c = unbonded(&[SUPPLY], 2);
    let (buyer, seller) = (1, 2);
    c.back(0, seller, SUPPLY);
    c.back(0, buyer, SUPPLY);
    for amount in every_two_decimal_sale() {
        c.lend(buyer, seller, amount);
        c.ok(
            Tx::Sale {
                seller: Party::Member(seller),
                buyer: Party::Member(buyer),
                amount: amount - 0.01,
                maturity_epochs: MATURITY,
            },
            &[key(seller as usize), key(buyer as usize)],
        );
        forget_closed(&mut c);
    }
}

/// And through the cascade, which closes a third party's row by moving its
/// debtor. `clear_member_debts` books a successor for what the buyer ASSUMES —
/// the payment — while the row it closes moves by the payment plus the
/// forgiven dust, so the two figures have to be read apart.
#[test]
fn a_cascade_that_clears_all_but_one_cent_never_breaks_conservation() {
    let mut c = unbonded(&[SUPPLY], 3);
    let (creditor, seller, buyer) = (1, 2, 3);
    c.back(0, seller, SUPPLY);
    c.back(0, buyer, SUPPLY);
    for amount in every_two_decimal_sale() {
        c.lend(creditor, seller, amount);
        c.ok(
            Tx::Sale {
                seller: Party::Member(seller),
                buyer: Party::Member(buyer),
                amount: amount - 0.01,
                maturity_epochs: MATURITY,
            },
            &[key(seller as usize), key(buyer as usize)],
        );
        // The successor the buyer assumed is this iteration's leftover; clear
        // it so the next amount starts from the ledger this one found.
        clear_book(&mut c);
    }
}

// ------------------------------------------------------- re-denomination --

/// A ledger whose insured holds span two supply arcs, carry odd cents, and
/// include an amount with sub-minor dust in it. Returns the chain and the
/// contract ids in the order they were opened.
///
/// Two underwriters at 10.00 and 10.01 back one debtor for the whole of each,
/// so a claim large enough to need both draws 1000 units through one and 1001
/// through the other — the shape where `Σ floor(arc·π)` and `floor(Σ arc·π)`
/// part company.
fn two_arc_ledger() -> (Chain, Vec<ContractId>) {
    // A community whose whole seed is a few units has a denomination to match,
    // and says so: `v_base` sets the bond unit and the establishment floor, so
    // leaving it at the genesis 1,000 would model members who cannot afford to
    // write in their own community.
    let mut c = Chain::founded_with(&[10.00, 10.01], 2);
    c.st.params.v_base = 10.0;
    let (d, cr) = (2, 3);
    c.back(0, d, 10.00);
    c.back(1, d, 10.01);
    let ids = vec![
        // Spanning both arcs, and an odd number of cents.
        c.lend(cr, d, 15.01),
        // Sub-minor dust: legal, since only `<= dust` is refused, and it
        // reserves 499 units while `outstanding` says 4.999.
        c.lend(cr, d, 4.999),
    ];
    (c, ids)
}

/// **A re-denomination may not halt the chain.** `Σ floor(arc·π)` against
/// `to_minor(outstanding·π)`, compared for exact equality: two arcs of 1000 and
/// 1001 at 2/3 gave 1333 committed against 1334 outstanding insured, and a
/// single arc holding 10.009 gave 666 against 667. A third of two-arc holds and
/// one in eight single-arc two-decimal amounts mismatched.
///
/// An insured obligation owes exactly what it holds, so the hold is
/// re-denominated first and the amount is read back off it.
#[test]
fn a_two_arc_ledger_survives_re_denomination() {
    for (num, den) in [(2u64, 3u64), (3, 2), (5, 7), (7, 5), (9, 8), (8, 9), (11, 10), (10, 11), (13, 8), (5, 8)] {
        let (mut c, ids) = two_arc_ledger();
        c.st.rescale(num as f64 / den as f64);
        audit(&c.st).unwrap_or_else(|v| panic!("re-denomination by {num}/{den} broke an invariant: {v:?}"));
        for cid in &ids {
            let x = &c.st.contracts[cid];
            if x.insured {
                assert_eq!(
                    x.outstanding,
                    x.held.amount(),
                    "contract {cid} owes {} while holding {} after {num}/{den}",
                    x.outstanding,
                    x.held.amount()
                );
            }
        }
    }
}

/// The same over the whole two-decimal grid, on a single-arc hold — one in
/// eight of these mismatched at 2/3. Built once and re-denominated per amount,
/// because the defect is in the amount rather than in the ledger around it.
#[test]
fn every_two_decimal_amount_survives_re_denomination() {
    for cents in 2..=2_000u64 {
        let amount = cents as f64 / 100.0;
        for (num, den) in [(2u64, 3u64), (3, 2)] {
            // A community whose whole seed is a few units has a denomination to match,
            // and says so: `v_base` sets the bond unit and the establishment floor, so
            // leaving it at the genesis 1,000 would model members who cannot afford to
            // write in their own community.
            let mut c = Chain::founded_with(&[20.00], 2);
            c.st.params.v_base = 20.0;
            let (d, cr) = (1, 2);
            c.back(0, d, 20.00);
            let cid = c.lend(cr, d, amount);
            c.st.rescale(num as f64 / den as f64);
            audit(&c.st).unwrap_or_else(|v| panic!("{amount} re-denominated by {num}/{den} broke an invariant: {v:?}"));
            let x = &c.st.contracts[&cid];
            if x.insured {
                assert_eq!(x.outstanding, x.held.amount(), "{amount} at {num}/{den}");
            }
        }
    }
}

/// **One underwriter, two paths, one aggregated supply arc.** `flow::reserve`
/// records one supply entry per underwriter however many paths the
/// augmentation took, so the supply side floors ONCE where the stake side
/// floors four times: two two-hop paths carrying 1000 and 1001 minor units
/// give a supply claim of 1334 at 2/3 against arcs that deliver 666+667 =
/// 1333. Invariant 1 reads the supply side against a cut computed on the
/// floored graph, and 1334 against 1333 is a halt.
///
/// This is the face of the re-denomination halt that flooring plus
/// "`outstanding` follows the hold" does NOT close, and the reason the shave
/// is read per debtor: it is the aggregate that has to stay a flow.
#[test]
fn a_hold_that_took_two_paths_through_one_underwriter_survives_re_denomination() {
    // 0 = underwriter, 1 and 2 = the two hops, 3 = debtor, 4 = creditor.
    //
    // Amounts in the tens, so the arcs floor apart under a two-thirds rescale
    // — which is what this probe is about — and `v_base` to match, so the
    // write gate is proportionate to the community being modelled rather than
    // to the genesis denomination.
    let mut c = Chain::founded(1, 4);
    c.st.params.v_base = 20.0;
    let (a, b, d, cr) = (1, 2, 3, 4);
    c.back(0, a, 10.00);
    c.back(0, b, 10.01);
    c.back(a, d, 10.00);
    c.back(b, d, 10.01);
    let cid = c.lend(cr, d, 20.01);
    let held = &c.st.contracts[&cid].held;
    assert_eq!(held.supply, vec![(0, 2001)], "one underwriter, one supply entry, two paths under it");
    assert_eq!(held.edges.len(), 4, "and four stake arcs, which floor apart");
    c.st.rescale(2.0 / 3.0);
    audit(&c.st).unwrap_or_else(|v| panic!("two paths through one underwriter broke an invariant: {v:?}"));
    let x = &c.st.contracts[&cid];
    assert_eq!(x.held.amount(), 1333, "the supply claim is shaved to what the arcs deliver");
    assert_eq!(x.outstanding, 1333, "and the debt says exactly that");
}

/// **The `rehold` fallback is unreachable after a re-denomination**, which is
/// the property the fallback's own comment claims and the one a re-denomination
/// would otherwise take away. `rehold` releases the whole hold and re-reserves what is
/// still owed; if the amount owed says more than the hold gave back, the
/// re-reservation comes up short and the claim silently stops being insured.
///
/// It cannot, once the amount is read off the hold: releasing `h` units and
/// asking for `to_minor(outstanding) == h` of them back is a question the arcs
/// just answered.
#[test]
fn a_partial_discharge_after_a_re_denomination_stays_insured() {
    for (num, den) in [(2u64, 3u64), (3, 2), (5, 7), (7, 5)] {
        let (mut c, ids) = two_arc_ledger();
        c.st.rescale(num as f64 / den as f64);
        for cid in &ids {
            let x = c.st.contracts[cid].clone();
            // The smallest partial the floor admits — a 128th of the original,
            // rounded up — and never a payment that closes the row: a downward
            // re-denomination can take `dust` below one minor unit, and a
            // payment that rounds to nothing is refused rather than booked,
            // which is the point of the grid but would make this probe measure
            // the refusal instead of the re-hold.
            let pay = x.original.div_ceil(edet_kernel::constants::MAX_INSTALLMENTS).max(1);
            if !x.insured || x.outstanding <= pay + 2 {
                continue;
            }
            c.ok(
                Tx::Settle { contract: *cid, amount: State::from_minor(pay) },
                &[key(x.debtor as usize), key(x.creditor as usize)],
            );
            assert!(
                c.st.contracts[cid].insured,
                "contract {cid} lost its insurance to a re-hold after {num}/{den} — the fallback fired"
            );
        }
    }
}

/// A re-denomination enacted the way one actually arrives: through
/// `Propose`/`Assent`, on a ledger with live insured credit on it. The audit
/// runs on the commit path inside `Chain::apply`, so the assent that enacts it
/// is the probe.
#[test]
fn a_governed_re_denomination_commits_on_a_live_ledger() {
    // A community whose whole seed is a few units has a denomination to match,
    // and says so: `v_base` sets the bond unit and the establishment floor, so
    // leaving it at the genesis 1,000 would model members who cannot afford to
    // write in their own community.
    let mut c = Chain::founded_with(&[10.00, 10.01, 10.00, 10.00], 2);
    c.st.params.v_base = 10.0;
    let (d, cr) = (4, 5);
    c.back(0, d, 10.00);
    c.back(1, d, 10.01);
    let cid = c.lend(cr, d, 15.01);
    assert!(c.st.contracts[&cid].insured);
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::Redenominate { num: 2, den: 3 } }, &[key(0)]);
    for founder in 0..4 {
        if c.st.proposals.get(&pid).is_none_or(|p| p.enacted) {
            break;
        }
        c.ok(Tx::Assent { member: founder, proposal: pid }, &[key(founder as usize)]);
    }
    assert_eq!(c.st.contracts[&cid].outstanding, c.st.contracts[&cid].held.amount());
    assert!(c.st.params.last_redenom_epoch.is_some(), "the fixture must actually re-denominate");
}

// ------------------------------------------------- the running cache --------

/// **A cached total maintained by arithmetic must equal the same total
/// recounted, exactly, however many times it is moved.**
///
/// §Verification's conservation check compares each account's `debt_out`
/// against a recount of the book, and it asks for equality rather than
/// closeness. That is only a claim anybody can keep if the two sides are
/// integers. With the amounts as `f64` the cache is `x - a_1 - a_2 - \dots`
/// and the recount is a left-to-right sum over the live rows, and those are
/// two different real numbers as soon as an amount is not representable —
/// which is every amount with a decimal fraction. Neither figure is wrong; the
/// two simply disagree, by more the more traffic passes, and the audit that
/// compares them halts every honest node at once on an account that has done
/// nothing but be busy.
///
/// Part one is the gate: real transitions, exact equality asserted after every
/// one. Part two is the PREMISE, measured rather than asserted — the same
/// arithmetic in `f64`, at the scale the argument is about.
#[test]
fn a_running_cache_never_drifts_from_the_book() {
    let mut c = unbonded(&[SUPPLY], 2);
    let (cr, d) = (1, 2);
    c.back(0, d, SUPPLY);
    // Amounts whose decimal expansion no binary fraction holds, cycled so the
    // error a float would accumulate does not cancel — each at least a 128th
    // of the 100.00 original, which is the floor a partial payment clears
    // (`MAX_INSTALLMENTS`).
    let steps = [0.79f64, 1.29, 3.33, 0.83, 1.11, 0.97];
    let mut opened = 0;
    for round in 0..40 {
        let cid = c.lend(cr, d, 100.00);
        for i in 0..125 {
            let amount = steps[(round + i) % steps.len()];
            if State::from_minor(c.st.contracts[&cid].outstanding) < amount {
                break;
            }
            c.ok(Tx::Settle { contract: cid, amount }, &[key(d as usize), key(cr as usize)]);
            // The recount, computed the way the audit computes it and asserted
            // here as well, so the probe names the quantity rather than
            // relying on the audit to name it.
            let book: u64 =
                c.st.contracts
                    .values()
                    .filter(|x| x.debtor == d && matches!(x.status, ContractStatus::Active | ContractStatus::Expired))
                    .map(|x| x.outstanding)
                    .sum();
            assert_eq!(c.st.members[&d].debt_out, book, "round {round}, payment {i}");
        }
        opened += 1;
        forget_closed(&mut c);
    }
    assert_eq!(opened, 40, "the probe must actually have run its obligations");
}

/// **The premise under the type**: the same book kept in `f64` DOES drift past
/// a `1e-6` tolerance, on ordinary traffic at institutional size.
///
/// Two thousand live claims of a million, two hundred thousand partial
/// discharges: the cache moves by subtraction on each payment while the audit
/// recounts the rows, and the gap between them reaches **4e-5**, forty times
/// the tolerance that would be watching it. Smaller communities never get
/// there — fifty claims of ten thousand stay at 8e-9 over the same traffic —
/// which is exactly what makes this the kind of fault that ships.
///
/// No ledger state is touched here. It measures the arithmetic the minor grid
/// replaced, so that the reason for the grid is a figure in the suite rather
/// than a sentence in a comment, and so that a proposal to go back has
/// something to answer.
#[test]
fn the_float_book_this_replaces_does_cross_the_tolerance() {
    // A small deterministic generator: the drift is a property of the
    // arithmetic, so the sequence must be the same on every machine.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let measure = |rows: usize, principal: f64, payments: usize, next: &mut dyn FnMut() -> u64| -> f64 {
        let mut outstanding = vec![principal; rows];
        let mut cache = principal * rows as f64;
        let mut worst = 0.0f64;
        for _ in 0..payments {
            let k = (next() % rows as u64) as usize;
            let cents = 1 + next() % 50_000;
            let mut pay = (cents as f64 / 100.0).min(outstanding[k]);
            outstanding[k] -= pay;
            // A row closes at dust, and the cache follows the ROW — the same
            // rule `settle` runs on, which is what makes this the ledger's own
            // arithmetic and not a strawman.
            if outstanding[k] <= 0.01 {
                pay += outstanding[k];
                outstanding[k] = 0.0;
            }
            cache -= pay;
            if outstanding[k] == 0.0 {
                outstanding[k] = principal;
                cache += principal;
            }
            let book: f64 = outstanding.iter().sum();
            worst = worst.max((cache - book).abs());
        }
        worst
    };

    let small = measure(50, 10_000.0, 200_000, &mut next);
    assert!(small < 1e-6, "a small community stays inside the tolerance, which is why this fault ships: {small:e}");

    let large = measure(2_000, 1_000_000.0, 200_000, &mut next);
    assert!(large > 1e-6, "the drift must actually cross the tolerance at institutional size: {large:e}");
}

/// The two refusals the minor grid introduces, which an `f64` amount had to
/// approximate with an epsilon.
///
/// A payment below one minor unit buys nothing: it would burn a replay id,
/// write a stake and move no book, so it is refused rather than booked. And an
/// over-payment is refused with no slack at all — the `1e-12` an `f64`
/// comparison needed was a hole exactly as wide as it was, and the grid closes
/// it by making both sides the same kind of number.
#[test]
fn the_boundary_refuses_what_it_cannot_hold() {
    let mut c = unbonded(&[SUPPLY], 2);
    let (cr, d) = (1, 2);
    c.back(0, d, SUPPLY);
    let cid = c.lend(cr, d, 100.00);
    let signers = [key(d as usize), key(cr as usize)];

    c.err(Tx::Settle { contract: cid, amount: 0.004 }, &signers, ET_CTR_BAD_AMOUNT);
    c.err(Tx::Settle { contract: cid, amount: 100.01 }, &signers, ET_CTR_BAD_AMOUNT);
    assert_eq!(c.st.contracts[&cid].outstanding, State::to_minor(100.00), "and neither moved the book");

    // The smallest PARTIAL payment is a 128th of the original, rounded up to
    // the grid: 0.79 on 100.00. Below it a partial is refused — a settle is
    // free, and its count is what the floor bounds — and at it it is accepted.
    c.err(Tx::Settle { contract: cid, amount: 0.78 }, &signers, ET_CTR_BAD_AMOUNT);
    c.ok(Tx::Settle { contract: cid, amount: 0.79 }, &signers);
    assert_eq!(c.st.contracts[&cid].outstanding, State::to_minor(100.00) - 79);

    // One minor unit is the smallest thing the ledger can hold, and it is
    // accepted wherever it clears the floor: on a 1.00 row a partial of one
    // cent is a 100th of the original. The refusal above is about what rounds
    // to nothing and about splitting a debt into dust, not about small amounts.
    let small = c.lend(cr, d, 1.00);
    c.ok(Tx::Settle { contract: small, amount: State::from_minor(1) }, &signers);
    assert_eq!(c.st.contracts[&small].outstanding, 99);
    c.ok(Tx::Settle { contract: small, amount: 0.99 }, &signers);
    assert_eq!(c.st.contracts[&small].status, ContractStatus::Settled);
}

// ------------------------------------------------------ the top of the grid --

/// The ceiling as a payload names it: the major-unit figure a wire amount of
/// exactly `MAX_AMOUNT_MINOR` carries.
fn ceiling() -> f64 {
    State::from_minor(k::MAX_AMOUNT_MINOR)
}

/// **A clamp is not a refusal, and `to_minor` clamps at BOTH ends.**
///
/// The bottom is already enforced — `to_minor(x) == 0` is a refusal, not a
/// rounding — and the top is the same defect one direction over. `to_minor`
/// ends in a float-to-integer `as` cast, and such a cast SATURATES: a finite,
/// non-negative `1e300` is `u64::MAX` minor units. Nothing about that value is
/// a rounding of what the signer wrote.
///
/// What it costs, unrefused: an obligation of eighteen quintillion sits on the
/// book, and the conservation sum in §Verification — which runs inside
/// `commit_block_unchecked`, on every honest node — adds it to the next live
/// contract and overflows. In a build with overflow checks that is a panic on
/// every validator at the same height; without them both sides of the sum wrap
/// together, consensus stays agreed, and the nonsense debt is hashed into a
/// state the audit can no longer see it in.
///
/// So the ceiling is asked at every ingress, BEFORE the conversion, and the
/// ceiling is where the boundary itself stops being exact rather than where the
/// cast gives out — see `MAX_AMOUNT_MINOR`.
#[test]
fn an_amount_above_the_representable_ceiling_is_refused_at_every_ingress() {
    assert_eq!(State::to_minor(1e300), u64::MAX, "the cast saturates; this is the number a payload would book");
    assert_eq!(
        State::to_minor(ceiling()),
        k::MAX_AMOUNT_MINOR,
        "the ceiling names itself on the wire, so it is an amount somebody can actually send"
    );

    let mut c = unbonded(&[SUPPLY], 4);
    let (cr, d) = (1u64, 2u64);
    let signers = [key(cr as usize), key(d as usize)];

    // Two figures at every door: the saturating one, and the first minor unit
    // past the ceiling — because a bound tested only against `1e300` is a
    // bound on `is_finite` wearing a different name.
    for bad in [1e300, State::from_minor(k::MAX_AMOUNT_MINOR + 1)] {
        c.err(
            Tx::Accept {
                debtor: Party::Member(d),
                creditor: Party::Member(cr),
                amount: bad,
                maturity_epochs: MATURITY,
                arb: None,
            },
            &signers,
            ET_CTR_BAD_AMOUNT,
        );
        c.err(
            Tx::Sale { seller: Party::Member(cr), buyer: Party::Member(d), amount: bad, maturity_epochs: MATURITY },
            &signers,
            ET_CTR_BAD_AMOUNT,
        );
        // A ceremony can mis-type a number too, and genesis is the one door
        // that seats a supply without a signature to check it against.
        assert_eq!(
            State::default().add_underwriter(vec![key(9)], bad),
            Err(Error(ET_CTR_BAD_AMOUNT)),
            "genesis supply"
        );
        // The static half of a seed amendment, refused at proposal as well as
        // at enactment: an amount nothing can ever admit should not occupy a
        // row while the community votes on it.
        c.err(
            Tx::Propose { author: 0, kind: ProposalKind::SeedAmendment { amount: bad } },
            &[key(0)],
            ET_CTR_BAD_AMOUNT,
        );
    }

    // **The cheapest door is arbitration.** `ArbAttest` is a free class, so an
    // arbiter names a figure without paying a bond for it, and the sweep — not
    // the arbiter — is what mints against it.
    let panel = ArbTermsWire { arbiters: [3, 4].into(), quorum: 2, window_epochs: MATURITY - 10, award_cap: 100.0 };
    let cid = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(d),
            creditor: Party::Member(cr),
            amount: 100.0,
            maturity_epochs: MATURITY,
            arb: Some(panel),
        },
        &signers,
    );
    for bad in [1e300, State::from_minor(k::MAX_AMOUNT_MINOR + 1)] {
        c.err(Tx::ArbAttest { contract: cid, arbiter: 3, amount: bad }, &[key(3)], ET_CTR_BAD_AMOUNT);
        // The consented ceiling is an amount as well, converted at acceptance
        // and compared against every attestation for the rest of the window.
        // Refused as a malformed TERM, beside the negative cap it sits next to.
        c.err(
            Tx::Accept {
                debtor: Party::Member(d),
                creditor: Party::Member(cr),
                amount: 100.0,
                maturity_epochs: MATURITY,
                arb: Some(ArbTermsWire { arbiters: [3, 4].into(), quorum: 2, window_epochs: 10, award_cap: bad }),
            },
            &signers,
            ET_ARB_BAD_TERMS,
        );
    }

    // **The ceiling is a bound, not a dust threshold: the amount AT it books.**
    // A community of 2,500 underwrites none of it, so the claim is uninsured —
    // which is what §Recourse says any claim past the cut is, and not a second
    // refusal wearing a different name.
    let big = c.lend(cr, d, ceiling());
    assert_eq!(c.st.contracts[&big].outstanding, k::MAX_AMOUNT_MINOR, "the ceiling itself is bookable");
    assert!(!c.st.contracts[&big].insured, "and uninsured, because the community underwrites 2,500 of it");
    audit(&c.st).expect("a ceiling-sized book is a legal book");
}

/// **The panel is the one place the ledger mints against a figure nobody
/// bonded for.** An attestation is free, the sweep runs `arb_award` inside
/// `begin_block` on every node, and an even panel's median adds the middle
/// pair — so two arbiters naming `u64::MAX` overflow that add on every
/// validator at once, in a transition neither party to the contract signed.
///
/// With the ingress ceiling in force the largest pair the panel can name sums
/// well inside a `u64`, and the median is computed in `u128` regardless, so
/// what is left is the ordinary arithmetic: the median of two ceilings is the
/// ceiling, and the award is what the three consented bounds leave of it.
#[test]
fn an_even_panel_attesting_the_ceiling_mints_an_award_without_overflowing() {
    let mut c = unbonded(&[SUPPLY], 4);
    let (cr, d) = (1u64, 2u64);
    let terms = ArbTermsWire { arbiters: [3, 4].into(), quorum: 2, window_epochs: MATURITY - 10, award_cap: ceiling() };
    let cid = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(d),
            creditor: Party::Member(cr),
            amount: ceiling(),
            maturity_epochs: MATURITY,
            arb: Some(terms),
        },
        &[key(cr as usize), key(d as usize)],
    );
    let minted = c.st.next_contract;
    c.ok(Tx::ArbAttest { contract: cid, arbiter: 3, amount: ceiling() }, &[key(3)]);
    c.ok(Tx::ArbAttest { contract: cid, arbiter: 4, amount: ceiling() }, &[key(4)]);

    // The window closes ten epochs before maturity, so the award mints while
    // the original claim is still Active — the sweep does this, not a caller.
    c.goto(MATURITY - 9);

    let award = &c.st.contracts[&minted];
    assert_eq!((award.debtor, award.creditor), (cr, d), "an award runs from the seller to the buyer");
    assert_eq!(award.original, k::MAX_AMOUNT_MINOR, "the median of two ceilings, under a ceiling-sized cap");
    assert_eq!(c.st.members[&cr].debt_out, k::MAX_AMOUNT_MINOR, "and the loser's cached debt says the same");
    audit(&c.st).expect("both sides of the book are ceiling-sized and the audit must still agree");
}

/// Fill `debtor`'s book with live claims until their cached debt is exactly
/// `target`, written straight into state. What the probes below are about is
/// the arithmetic over a book the alphabet can reach, not the sixteen thousand
/// bonded acceptances that would reach it.
fn fill_book(c: &mut Chain, debtor: MemberId, creditor: MemberId, target: u64) {
    let mut owed = c.st.members[&debtor].debt_out;
    while owed < target {
        let amount = k::MAX_AMOUNT_MINOR.min(target - owed);
        let id = c.st.next_contract;
        c.st.next_contract += 1;
        c.st.contracts.insert(
            id,
            Contract {
                id,
                debtor,
                creditor,
                outstanding: amount,
                original: amount,
                maturity_epoch: u64::MAX,
                status: ContractStatus::Active,
                created_epoch: 0,
                accepted_epoch: 0,
                insured: false,
                held: Default::default(),
                arb: None,
                arb_attestations: Default::default(),
                arb_awarded: false,
            },
        );
        owed += amount;
    }
    c.st.members.get_mut(&debtor).unwrap().debt_out = owed;
}

/// **The ingress bound is per amount and the conservation sum is over a SET.**
/// The ceiling bounds one obligation; nothing bounds how many of them a debtor
/// may accept, so a book of sixteen thousand ceiling-sized claims across two
/// debtors carries more minor units than a `u64` holds while every single row
/// is legal. Summed in `u64` that is a panic inside `commit_block_unchecked`
/// on a ledger with nothing wrong with it; summed in `u128` it is arithmetic,
/// and an overflow anywhere under it can only ever be a `Violation` — the
/// fail-stop the engine already ends its loop on, with the reason named.
///
/// The other half is what happens at the top of one debtor's own book, which
/// IS a `u64`: a claim arriving on a full debtor is refused rather than
/// wrapping, and refused before the write that would otherwise stand alone —
/// the reservation an acceptance takes, and the discharge of the original a
/// transfer performs.
#[test]
fn a_book_too_large_for_a_u64_sum_is_still_audited() {
    let mut c = unbonded(&[SUPPLY], 4);
    let (cr, d1, d2, other) = (1u64, 2u64, 3u64, 4u64);
    // Backed first, and settled, so the acceptance at the end of this probe is
    // one the community can find a reservation for.
    c.back(0, d1, SUPPLY);
    // One debtor a hundred units short of the top, the other filled with whole
    // ceiling-sized rows. Together they carry more than a `u64` counts.
    fill_book(&mut c, d1, cr, u64::MAX - 99);
    fill_book(&mut c, d2, cr, u64::MAX - (u64::MAX % k::MAX_AMOUNT_MINOR));
    let total = c.st.contracts.values().map(|x| x.outstanding as u128).sum::<u128>();
    assert!(total > u64::MAX as u128, "the fixture's own arithmetic: {total} against {}", u64::MAX);
    audit(&c.st).expect("a book larger than a u64 is legal, and the audit must say so rather than die");

    // A hundred units of room left, and a real acceptance of more than that.
    // The debtor is backed, so the reservation this would take is one the
    // community can find — which is what makes the ORDER matter: the refusal
    // has to come before it, or the ledger holds flow for an obligation that
    // does not exist.
    assert!(c.cap(d1) > 100.0, "the reservation this acceptance asks for is available: {}", c.cap(d1));
    let reserved = c.st.reserved.clone();
    c.err(
        Tx::Accept {
            debtor: Party::Member(d1),
            creditor: Party::Member(cr),
            amount: 100.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(cr as usize), key(d1 as usize)],
        ET_CTR_BAD_AMOUNT,
    );
    assert_eq!(c.st.reserved, reserved, "a refused obligation reserves nothing");
    assert_eq!(c.st.members[&d1].debt_out, u64::MAX - 99, "and the book did not move");

    // **A transfer writes before it books**: it releases the original's hold
    // and discharges the old debtor, then mints the successor. A successor
    // refused after that would leave the claim extinguished and nothing
    // standing in its place — the creditor's claim gone, with the audit
    // satisfied on both sides of a sum that no longer counts it.
    let moved = c.lend(cr, other, 100.0);
    let before = c.st.contracts[&moved].clone();
    c.err(
        Tx::Transfer { contract: moved, new_debtor: d1 },
        &[key(other as usize), key(d1 as usize), key(cr as usize)],
        ET_CTR_BAD_AMOUNT,
    );
    assert_eq!(c.st.contracts[&moved].status, ContractStatus::Active, "the original still stands");
    assert_eq!(c.st.contracts[&moved].outstanding, before.outstanding, "for its whole amount");
    assert_eq!(c.st.members[&other].debt_out, before.outstanding, "and its debtor still owes it");
}

/// **A supply is an accepted liability, and the roll is a sum of them.** The
/// per-amendment rate bound says how fast the roll may grow; it says nothing
/// about where it stops, and compounding at β reaches any figure given epochs.
/// So the ceiling is asked of the ROLL, not only of the amendment: an
/// amendment the rate would admit is refused when the seed it lands on could
/// no longer be named.
#[test]
fn a_seed_amendment_that_would_carry_the_roll_past_the_ceiling_is_refused() {
    // Two founders holding the ceiling between them, and an author with
    // nothing — the arrival §Governance is written for.
    let half = State::from_minor(k::MAX_AMOUNT_MINOR / 2);
    let mut c = Chain::founded_with(&[half, half], 1);
    let want = edet_state::seed::headroom(&c.st);
    assert!(want > 0.0, "the rate alone would admit {want}, so the refusal below is the ceiling's");

    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 2, kind: ProposalKind::SeedAmendment { amount: want } }, &[key(2), key(0)]);
    c.err(Tx::Assent { member: 0, proposal: pid }, &[key(0)], ET_SEED_RATE);
    assert!(!c.st.proposals[&pid].enacted, "and the roll did not move");
    assert_eq!(c.st.underwriters[&0], k::MAX_AMOUNT_MINOR / 2);

    // The counterfactual: the same shape of amendment on a roll with room
    // under the ceiling enacts, so what refused above was the ceiling and not
    // the machinery.
    let quarter = State::from_minor(k::MAX_AMOUNT_MINOR / 4);
    let mut c = Chain::founded_with(&[quarter, quarter], 1);
    let want = edet_state::seed::headroom(&c.st);
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 2, kind: ProposalKind::SeedAmendment { amount: want } }, &[key(2), key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert!(c.st.proposals[&pid].enacted, "an amendment inside the ceiling carries");
    assert_eq!(c.st.underwriters[&2], State::to_minor(want), "and the author's supply is what it endorsed");
}
