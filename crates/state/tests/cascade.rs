//! The sale composite: mutual netting, then the waterfilling support cascade.
//!
//! **The drain cap is now a stake, and that changed every number in this
//! suite.** `ν` times a decayed counter of settled VOLUME
//! between the pair, with a floor — and volume is free to fabricate, since
//! settlement takes two signatures and no delivery, so two colluding accounts
//! could open the drain as wide as they liked. It is now `ν · (stake(a,b) +
//! stake(b,a))`, with no floor at all: a pair with no settled history drains
//! nothing. Zero is absorbing here too, and it is escaped the same way — by
//! trading. Every fixture below therefore has to EARN its cascade edges.

mod common;

use common::{key, Chain, MATURITY, SUPPLY};
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

/// What `debtor` still owes, over every live obligation.
fn owed(c: &Chain, debtor: MemberId) -> f64 {
    c.st.contracts
        .values()
        .filter(|x| x.debtor == debtor && matches!(x.status, ContractStatus::Active | ContractStatus::Expired))
        .map(|x| x.outstanding)
        .sum::<u64>() as f64
        / 100.0
}

/// What `buyer` now owes `creditor` in particular.
fn owed_to(c: &Chain, debtor: MemberId, creditor: MemberId) -> f64 {
    c.st.contracts
        .values()
        .filter(|x| x.debtor == debtor && x.creditor == creditor && x.status == ContractStatus::Active)
        .map(|x| x.outstanding)
        .sum::<u64>() as f64
        / 100.0
}

// --------------------------------------------------------------- netting --

/// Mutual obligations extinguish rather than route. This is how settlement
/// happens in the wallet — to pay a debt back you sell to your creditor — so
/// it must come first, before any capacity question is asked at all.
#[test]
fn a_sale_nets_what_the_seller_already_owes_the_buyer() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let debt = c.lend(2, 1, 200.0); // seller 1 owes buyer 2

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 120.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );
    assert!((c.outstanding(debt) - 80.0).abs() < 1e-9, "netted, not routed: {}", c.outstanding(debt));
    assert_eq!(owed(&c, 2), 0.0, "and the buyer owes nothing new for it");
}

/// A sale larger than the mutual debt nets what it can and books the rest.
#[test]
fn a_sale_beyond_the_netting_line_books_the_remainder() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let debt = c.lend(2, 1, 100.0);

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 250.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );
    assert_eq!(c.st.contracts[&debt].status, ContractStatus::Settled, "the mutual debt goes first");
    assert!((owed_to(&c, 2, 1) - 150.0).abs() < 1e-9, "and the rest is genesis debt toward the seller");
}

/// Netting cures an EXPIRED obligation as well as settling a live one — a
/// defaulted debt is still a debt, and paying it is still paying it.
///
/// Uninsured, and it has to be: an insured default is substituted at
/// `MarkExpired` (§Recourse), which moves the claim onto the underwriters. After
/// that the seller no longer owes the buyer anything, so a sale to the buyer
/// has nothing to net — correctly, because the debt has moved rather than
/// disappeared.
#[test]
fn netting_cures_an_expired_obligation() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, 10.0);
    c.back(0, 2, SUPPLY);
    let debt = c.lend(2, 1, 100.0);
    assert!(!c.st.contracts[&debt].insured);
    c.default_on(debt);
    assert!(c.st.members[&1].rep.open_default > 0);

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 100.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );
    assert_eq!(c.st.contracts[&debt].status, ContractStatus::Cured);
    assert!(
        c.st.members[&1].rep.open_default <= c.st.params.dust_minor(),
        "the default is cured, not merely paid around"
    );
}

/// Suspension revokes ORIGINATION and leaves discharge open, and a sale is
/// both: netting is discharge, the cascade and any remainder are new credit.
/// So a suspended seller may sell exactly as far as the netting line and no
/// further — no partial sale that nets what it can and originates the rest,
/// because that is precisely the "sell through the cascade" origination the
/// sanction exists to stop.
#[test]
fn a_suspended_seller_may_net_and_no_further() {
    let mut c = Chain::founded(2, 2);
    c.back(0, 2, SUPPLY);
    c.back(0, 3, SUPPLY);
    let debt = c.lend(3, 2, 200.0); // seller 2 owes buyer 3

    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::Suspend { member: 2 } }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert_eq!(c.status(2), MemberStatus::Suspended);

    c.err(
        Tx::Sale { seller: Party::Member(2), buyer: Party::Member(3), amount: 250.0, maturity_epochs: MATURITY },
        &[key(2), key(3)],
        ET_MEM_SUSPENDED_NO_ORIGINATION,
    );
    assert!((c.outstanding(debt) - 200.0).abs() < 1e-9, "a refused sale extinguishes nothing");

    c.ok(
        Tx::Sale { seller: Party::Member(2), buyer: Party::Member(3), amount: 200.0, maturity_epochs: MATURITY },
        &[key(2), key(3)],
    );
    assert_eq!(c.st.contracts[&debt].status, ContractStatus::Settled);
    assert_eq!(owed(&c, 3), 0.0, "and nothing was originated toward the suspended seller");
}

/// A suspended BUYER is refused outright: they would be taking on new credit,
/// which is exactly what the sanction closes.
#[test]
fn a_suspended_buyer_is_refused_outright() {
    let mut c = Chain::founded(2, 2);
    c.back(0, 2, SUPPLY);
    c.back(0, 3, SUPPLY);
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::Suspend { member: 3 } }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);

    c.err(
        Tx::Sale { seller: Party::Member(2), buyer: Party::Member(3), amount: 50.0, maturity_epochs: MATURITY },
        &[key(2), key(3)],
        ET_MEM_NOT_ACTIVE,
    );
}

// --------------------------------------------------------------- listing --

/// The listing is the supporter's half of the consent; the beneficiary's half
/// is `ApproveSupporter`, and a drain needs both. Weights must be real and
/// positive, the list is bounded, and approving somebody who never listed you
/// is refused.
#[test]
fn a_listing_is_validated_and_reversible() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);

    c.err(Tx::ListBeneficiaries { supporter: 1, entries: vec![(2, 0.0)] }, &[key(1)], ET_CAS_BAD_WEIGHT);
    c.err(Tx::ListBeneficiaries { supporter: 1, entries: vec![(2, f64::NAN)] }, &[key(1)], ET_CAS_BAD_WEIGHT);
    let too_many: Vec<(MemberId, f64)> = (0..65).map(|i| (i % 3, 1.0)).collect();
    c.err(Tx::ListBeneficiaries { supporter: 1, entries: too_many }, &[key(1)], ET_CAS_TOO_MANY);
    c.err(Tx::ApproveSupporter { beneficiary: 2, supporter: 1, approved: true }, &[key(2)], ET_CAS_NOT_LISTED);

    c.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(2, 1.0)] }, &[key(1)]);
    assert!(c.st.members[&2].supporters_of.contains(&1), "the reverse edge is installed");
    c.ok(Tx::ApproveSupporter { beneficiary: 2, supporter: 1, approved: true }, &[key(2)]);
    c.ok(Tx::ApproveSupporter { beneficiary: 2, supporter: 1, approved: false }, &[key(2)]);
    assert!(!c.st.members[&2].approved_supporters.contains(&1), "and the gate closes again");

    // Replacing a listing removes the stale reverse edges rather than adding.
    c.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(0, 1.0)] }, &[key(1)]);
    assert!(!c.st.members[&2].supporters_of.contains(&1));
}

// --------------------------------------------------------------- draining --

/// A seller with no listing at all keeps the simple default: the whole sale
/// clears their own obligations, oldest first.
#[test]
fn a_seller_with_no_listing_clears_their_own_debts() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    let old = c.lend(3, 1, 60.0);
    let new = c.lend(3, 1, 90.0);

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 100.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );
    assert_eq!(c.st.contracts[&old].status, ContractStatus::Transferred, "oldest first");
    assert!((c.outstanding(new) - 50.0).abs() < 1e-9, "then partially into the next: {}", c.outstanding(new));
    assert!((owed_to(&c, 2, 3) - 100.0).abs() < 1e-9, "the buyer assumes them, aggregated per creditor");
    assert!(c.st.members[&2].debt_out == State::to_minor(100.0), "and owes exactly the sale");
}

/// **A pair with no settled history drains nothing.** The cap is a stake, and
/// an approval with no trade behind it is evidence of nothing — which is
/// surprising in a client that lets you approve anybody, and correct by the
/// same rule the rest of the model runs on.
#[test]
fn an_approval_with_no_trade_behind_it_drains_nothing() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    let theirs = c.lend(3, 2, 80.0); // the beneficiary owes somebody
    c.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(2, 1.0)] }, &[key(1)]);
    c.ok(Tx::ApproveSupporter { beneficiary: 2, supporter: 1, approved: true }, &[key(2)]);

    // Seller 1 and beneficiary 2 have never traded, so `pair_stake` is zero.
    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(3), amount: 100.0, maturity_epochs: MATURITY },
        &[key(1), key(3)],
    );
    assert!((c.outstanding(theirs) - 80.0).abs() < 1e-9, "untouched: the drain cap is zero");
    assert!((owed_to(&c, 3, 1) - 100.0).abs() < 1e-9, "the whole sale is genesis debt toward the seller");
}

/// Listed but not approved: the beneficiary's own gate is closed, so the
/// channel does not exist however much the pair has traded.
#[test]
fn an_unapproved_beneficiary_is_skipped() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    c.back(1, 2, 200.0); // a real relationship, so the cap is not the reason
    let theirs = c.lend(3, 2, 80.0);
    c.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(2, 1.0)] }, &[key(1)]);

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(3), amount: 100.0, maturity_epochs: MATURITY },
        &[key(1), key(3)],
    );
    assert!((c.outstanding(theirs) - 80.0).abs() < 1e-9, "untouched without the beneficiary's approval");
    assert!((owed_to(&c, 3, 1) - 100.0).abs() < 1e-9);
}

/// The listing IS the breakdown, self entry included: one waterfill over
/// {own share, beneficiaries}, each edge capped by what the pair has staked in
/// one another, the surplus a saturated target frees redistributed among those
/// with room, and whatever the cascade cannot absorb booked as genesis debt.
///
/// Seller 1 owes 50; beneficiaries 3 and 4 owe 80 and 300; the seller has
/// staked 200 in each of them. Budget 300, equal weights: 100 each, the self
/// entry saturates at 50, and its surplus redistributes to 125 each — so 3
/// clears its whole 80 and 4 takes 125.
#[test]
fn the_listing_is_the_breakdown_and_the_surplus_redistributes() {
    let mut c = Chain::founded(1, 5);
    for m in 1..=5 {
        c.back(0, m, SUPPLY);
    }
    c.back(1, 3, 200.0);
    c.back(1, 4, 200.0);

    let own = c.lend(5, 1, 50.0);
    let k1 = c.lend(5, 3, 80.0);
    let k2 = c.lend(5, 4, 300.0);
    c.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(1, 1.0), (3, 1.0), (4, 1.0)] }, &[key(1)]);
    c.ok(Tx::ApproveSupporter { beneficiary: 3, supporter: 1, approved: true }, &[key(3)]);
    c.ok(Tx::ApproveSupporter { beneficiary: 4, supporter: 1, approved: true }, &[key(4)]);

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 300.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );

    assert_eq!(c.st.contracts[&own].status, ContractStatus::Transferred, "the self share clears the seller's own 50");
    assert_eq!(c.st.contracts[&k1].status, ContractStatus::Transferred, "3's whole 80 is cleared");
    assert!((c.outstanding(k2) - 175.0).abs() < 1e-9, "4 drained 125 of 300: {}", c.outstanding(k2));
    assert!((owed_to(&c, 2, 5) - 255.0).abs() < 1e-9, "one successor per creditor: {}", owed_to(&c, 2, 5));
    assert!((owed_to(&c, 2, 1) - 45.0).abs() < 1e-9, "and the unabsorbed remainder is the seller's");
    assert!(c.st.members[&2].debt_out == State::to_minor(300.0), "the buyer's total obligation is exactly the sale");
}

/// An intermediate with no debt of its own passes its unused allocation one
/// level deeper, so support reaches through a chain of listings rather than
/// stopping at the first hop that happens to be solvent.
#[test]
fn an_intermediate_passes_its_share_one_level_deeper() {
    let mut c = Chain::founded(1, 4);
    for m in 1..=4 {
        c.back(0, m, SUPPLY);
    }
    c.back(1, 3, 200.0); // seller -> mid
    c.back(3, 4, 200.0); // mid -> k
                         // Owed to somebody who is NOT the buyer: a debt the buyer already holds
                         // is not routed, it is netted, and that is a different mechanism.
    let theirs = c.lend(0, 4, 40.0);

    c.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(3, 1.0)] }, &[key(1)]);
    c.ok(Tx::ApproveSupporter { beneficiary: 3, supporter: 1, approved: true }, &[key(3)]);
    c.ok(Tx::ListBeneficiaries { supporter: 3, entries: vec![(4, 1.0)] }, &[key(3)]);
    c.ok(Tx::ApproveSupporter { beneficiary: 4, supporter: 3, approved: true }, &[key(4)]);

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 100.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );
    assert_eq!(c.st.contracts[&theirs].status, ContractStatus::Transferred, "k is drained through the chain");
    assert!((owed_to(&c, 2, 1) - 60.0).abs() < 1e-9, "and the rest is genesis: {}", owed_to(&c, 2, 1));
}

/// A drained beneficiary's debt lands on its ORIGINAL creditor, carried by the
/// buyer — the seller's sale clearing somebody else's obligation is the whole
/// of what the cascade couples, and it couples production rather than failure.
///
/// *There is no `co_signers` field to assert (`types.rs`): it would record
/// which members were cleared into a
/// successor so that a default could be "attributed" to them, and nothing ever
/// attributed anything. What it left behind is the half that is real — where
/// the obligation actually goes, and whose it stops being.*
#[test]
fn a_drained_beneficiarys_debt_lands_on_its_original_creditor() {
    let mut c = Chain::founded(1, 4);
    for m in 1..=4 {
        c.back(0, m, SUPPLY);
    }
    c.back(1, 3, 200.0);
    c.lend(4, 3, 80.0);
    c.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(3, 1.0)] }, &[key(1)]);
    c.ok(Tx::ApproveSupporter { beneficiary: 3, supporter: 1, approved: true }, &[key(3)]);

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 50.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );
    let successor =
        c.st.contracts
            .values()
            .find(|x| x.debtor == 2 && x.creditor == 4 && x.status == ContractStatus::Active)
            .expect("a successor toward the original creditor");
    assert!(successor.outstanding == State::to_minor(50.0), "the buyer carries exactly what was cleared");
    // And the beneficiary's own row toward that creditor shrank by the same
    // amount: the debt MOVED, it was not duplicated.
    let left: u64 =
        c.st.contracts
            .values()
            .filter(|x| x.debtor == 3 && x.creditor == 4)
            .map(|x| x.outstanding)
            .sum::<u64>();
    assert_eq!(left, State::to_minor(30.0), "the beneficiary keeps only what the sale could not carry");
    // The seller cleared none of their own — selling earns no standing and
    // clears no debt of the seller's toward a third party here.
    assert_eq!(c.st.members[&1].debt_out, 0, "the seller had nothing of their own in this sale");
}

/// Nobody is drained twice in one sale, however the listings loop back on each
/// other — the recursion is bounded by a visited set, not by hope.
#[test]
fn a_cycle_of_listings_drains_each_member_once() {
    let mut c = Chain::founded(1, 4);
    for m in 1..=4 {
        c.back(0, m, SUPPLY);
    }
    c.back(1, 3, 200.0);
    c.back(3, 1, 200.0);
    let theirs = c.lend(4, 3, 500.0);

    c.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(3, 1.0)] }, &[key(1)]);
    c.ok(Tx::ApproveSupporter { beneficiary: 3, supporter: 1, approved: true }, &[key(3)]);
    c.ok(Tx::ListBeneficiaries { supporter: 3, entries: vec![(1, 1.0)] }, &[key(3)]);
    c.ok(Tx::ApproveSupporter { beneficiary: 1, supporter: 3, approved: true }, &[key(1)]);

    let before = c.outstanding(theirs);
    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 600.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );
    let drained = before - c.outstanding(theirs);
    assert!(drained <= 400.0 + 1e-9, "one edge, one drain: {drained} against a cap of ν x (200 + 200)");
    assert!(c.st.members[&2].debt_out == State::to_minor(600.0), "and the buyer still owes exactly the sale");
}

/// A sale is never capacity-GATED. Capacity bounds what the community
/// underwrites, not what a member may choose to risk: the successors reserve
/// flow if it is there and are uninsured if it is not, exactly as `Accept` is.
/// A sale refused for want of headroom would make capacity a permission again.
#[test]
fn a_sale_beyond_the_buyers_capacity_is_uninsured_rather_than_refused() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, 100.0);
    assert!(c.cap(2) < 500.0, "the buyer is thinly backed");

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 5_000.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );
    let successor =
        c.st.contracts
            .values()
            .find(|x| x.debtor == 2 && x.creditor == 1)
            .expect("the sale was booked");
    assert!(!successor.insured, "beyond the cut it is uninsured, and the creditor bears it alone");
    assert!(successor.outstanding == State::to_minor(5_000.0));
}

// ------------------------------------------ a discharge may not downgrade --

/// The rule, stated once: clearing an obligation through a sale is a debtor
/// swap — the row closes as `Transferred`, which is
/// one — so it owes what `Transfer` already requires. There the creditor signs
/// exactly when the successor would not be insured, because that is when they
/// lose something. Here the affected creditors are discovered inside the
/// transition and would refuse every time, so the same rule binds the AMOUNT:
/// an insured claim is cleared only as far as the buyer can carry it insured.
#[test]
fn an_insured_claim_is_not_cleared_into_an_uninsured_successor() {
    let mut c = Chain::founded(1, 3);
    c.back(0, 1, 500.0);
    let claim = c.lend(3, 1, 200.0);
    assert!(c.st.contracts[&claim].insured, "the creditor holds an INSURED claim on the seller");
    assert_eq!(c.cap(2), 0.0, "and the buyer has no standing to carry it");

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 200.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );

    assert_eq!(c.st.contracts[&claim].status, ContractStatus::Active, "the claim stays where it was");
    assert!((c.outstanding(claim) - 200.0).abs() < 1e-9, "in full");
    assert_eq!(owed_to(&c, 2, 3), 0.0, "the creditor was never handed a successor they did not consent to");
    assert!((owed_to(&c, 2, 1) - 200.0).abs() < 1e-9, "and the budget it could not absorb is the genesis remainder");
}

/// **An UNINSURED claim does not route either**, and this probe asserted the
/// opposite.
///
/// The old reasoning was that "its creditor never had recourse to lose, which
/// is the same test `Transfer` applies". It is not `Transfer`'s test.
/// `transfer` demands the creditor's signature whenever the SUCCESSOR would be
/// uninsured, whatever the original was — so the cascade, which bounded only
/// insured ORIGINALS, was the same economic act under the opposite consent
/// rule, one tier further down. What it let through was not a neutral move: the
/// debtor's `debt_out` went to zero and their capacity came back, while the
/// creditor was handed a claim against an account with nothing behind it.
///
/// So the rule is the successor's insurability for every original. Here the
/// buyer can carry none of it, so none of it moves — and the sale still
/// happens, as the genesis remainder toward the seller.
#[test]
fn an_uninsured_claim_does_not_route_to_a_buyer_with_no_standing() {
    let mut c = Chain::founded(1, 3);
    c.back(0, 1, 100.0);
    let claim = c.lend(3, 1, 400.0);
    assert!(!c.st.contracts[&claim].insured, "beyond the seller's cut, so uninsured from the start");
    assert_eq!(c.cap(2), 0.0);
    let debt_before = c.st.members[&1].debt_out;

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 400.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );

    assert_eq!(c.st.contracts[&claim].status, ContractStatus::Active, "the claim stays where it was");
    assert!((c.outstanding(claim) - 400.0).abs() < 1e-9, "in full");
    assert_eq!(owed_to(&c, 2, 3), 0.0, "the creditor is handed no successor against an account with nothing");
    assert!(c.st.members[&1].debt_out == debt_before, "and the seller sheds nothing");
    assert!((owed_to(&c, 2, 1) - 400.0).abs() < 1e-9, "the budget falls through to the seller, as it always could");
}

/// Partial, because a claim the buyer cannot carry whole may still be carried
/// in part — and the part they cannot carry stays with its original debtor
/// rather than being refused outright.
#[test]
fn an_insured_claim_is_cleared_as_far_as_the_buyer_can_carry_it() {
    let mut c = Chain::founded(1, 3);
    c.back(0, 1, 500.0);
    c.back(0, 2, 120.0); // the buyer can carry 120 of it, and no more
    let claim = c.lend(3, 1, 400.0);
    assert!(c.st.contracts[&claim].insured);
    assert!((c.cap(2) - 120.0).abs() < 1e-9, "buyer capacity: {}", c.cap(2));

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 400.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );

    let moved = 400.0 - c.outstanding(claim);
    assert!((moved - 120.0).abs() < 1e-9, "exactly the insurable part moved, not {moved}");
    let successor =
        c.st.contracts
            .values()
            .find(|x| x.debtor == 2 && x.creditor == 3)
            .expect("a successor for the moved part");
    assert!(successor.insured, "and it arrived insured, which is the whole point");
    assert!((owed_to(&c, 2, 1) - 280.0).abs() < 1e-9, "the rest is the seller's genesis remainder");
}

/// **One successor row per creditor, and it is always insured.**
///
/// It asserts ONE row rather than two — one per `insured` flag — because an
/// insured original and an uninsured one could not share a contract, and
/// aggregating them would have decided both by whether the SUM fitted. That
/// distinction is gone: every original, insured or not, now moves only as far
/// as the buyer can carry it INSURED, so there is no second kind of successor
/// left for a key to separate. The buyer's capacity, not the original's flag,
/// is what decides how much travels.
#[test]
fn one_successor_row_per_creditor_and_it_arrives_insured() {
    let mut c = Chain::founded(1, 3);
    c.back(0, 3, SUPPLY); // the creditor lends twice, so it needs its own headroom
    c.back(0, 1, 200.0);
    c.back(0, 2, 200.0); // the buyer can carry 200, and no more
    let insured = c.lend(3, 1, 200.0); // fits the seller's cut
    let plain = c.lend(3, 1, 300.0); // does not: uninsured
    assert!(c.st.contracts[&insured].insured && !c.st.contracts[&plain].insured, "one of each");

    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 500.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );

    let rows: Vec<&Contract> =
        c.st.contracts
            .values()
            .filter(|x| x.debtor == 2 && x.creditor == 3 && x.status == ContractStatus::Active)
            .collect();
    assert_eq!(rows.len(), 1, "one row, because there is only one kind of successor now");
    assert!(rows[0].insured, "and it arrived insured, which is the whole of the rule");
    assert!(rows[0].outstanding == State::to_minor(200.0), "carrying exactly what the buyer could insure");

    // The buyer's room went to the oldest original; the younger one did not
    // move at all, and its creditor keeps their claim against the seller
    // rather than a worse one against the buyer.
    assert_eq!(c.st.contracts[&insured].status, ContractStatus::Transferred);
    assert_eq!(c.st.contracts[&plain].status, ContractStatus::Active);
    assert!((c.outstanding(plain) - 300.0).abs() < 1e-9);
    assert!((owed_to(&c, 2, 1) - 300.0).abs() < 1e-9, "the rest is the seller's genesis remainder");
}

// ------------------------------------------- the 4b class, closed as one rule --
//
// Three findings, one rule (the "4b shape" audit). A debtor swap
// is not a settlement, so it stakes nothing; and in a `Sale` every original —
// insured or not — moves only as far as the buyer can carry it INSURED, with
// the buyer's reservation taken at the moment that room is measured.

/// What `from` has staked in `to`.
fn staked(c: &Chain, from: MemberId, to: MemberId) -> f64 {
    edet_state::state::State::from_minor(c.st.edges.get(&(from as usize, to as usize)).copied().unwrap_or(0))
}

/// **The seller's remainder must not take the buyer's bottleneck.**
///
/// The successor is measured against the buyer's residual with the original's
/// hold released — a promise — and the seller's own re-hold must not run between
/// that measurement and the booking. With U2 the buyer's ONLY source and U1
/// behind the seller, the re-held remainder took U2 first (Dinic scans
/// underwriters in id order) and the successor booked UNINSURED against a
/// residual of 200: original 50 insured, successor 250 not, six invariants
/// green, no creditor signature anywhere. The reservation is now taken when the
/// room is measured, so both ends of the split stay insured.
#[test]
fn a_reheld_remainder_cannot_take_the_buyers_bottleneck() {
    let mut c = Chain::founded_with(&[250.0, 300.0], 3);
    let (cr, s, b): (MemberId, MemberId, MemberId) = (2, 3, 4);
    c.back(1, s, 300.0); // U1 (id 1) backs the seller
    c.back(0, b, 250.0); // U2 (id 0) is the buyer's only source
    let claim = c.lend(cr, s, 300.0);
    assert!(c.st.contracts[&claim].insured, "insured, and held on U1");
    c.back(0, s, 250.0); // one honoured purchase from U2 -> the shared edge
    assert!((c.cap(b) - 250.0).abs() < 1e-9, "the buyer can carry 250 and only via U2");

    c.ok(
        Tx::Sale { seller: Party::Member(s), buyer: Party::Member(b), amount: 250.0, maturity_epochs: MATURITY },
        &[key(s as usize), key(b as usize)],
    );

    let successor =
        c.st.contracts
            .values()
            .find(|x| x.debtor == b && x.creditor == cr)
            .expect("a successor for the moved part");
    assert!(successor.outstanding == State::to_minor(250.0));
    assert!(successor.insured, "PROVEN: the successor keeps the insurance its bound promised");
    assert!((c.outstanding(claim) - 50.0).abs() < 1e-9, "and the remainder stays with the seller");
    assert!(c.st.contracts[&claim].insured, "insured too — the split costs the creditor nothing");
}

/// **An uninsured original is not shed onto a key that cannot carry it.**
///
/// `Transfer` refuses this exact swap without the creditor; the cascade
/// performed it for two signatures. One economic act, two code paths, opposite
/// consent rules — the 4b tell, one tier below where it was first found.
#[test]
fn a_sale_cannot_shed_an_uninsured_claim_onto_a_fresh_key() {
    let mut c = Chain::founded(1, 3);
    let (cr, s, b): (MemberId, MemberId, MemberId) = (1, 2, 3);
    c.back(0, cr, 300.0);
    // One honoured purchase, above the establishment floor so the seller has
    // a write allowance — and well below the 300 claim, so that claim is
    // uninsured, which is what this probe is about.
    c.back(cr, s, 100.0);
    let claim = c.lend(cr, s, 300.0);
    assert!(!c.st.contracts[&claim].insured, "uninsured from the start");
    assert_eq!(c.cap(b), 0.0, "and the successor would have nothing behind it");

    // The two paths, side by side. `Transfer` tests the SUCCESSOR, which is why
    // the old reading — "an uninsured original routes freely, which is
    // `Transfer`'s own test" — was wrong about the test it was citing.
    c.err(Tx::Transfer { contract: claim, new_debtor: b }, &[key(s as usize), key(b as usize)], ET_MEM_NOT_SIGNER);
    c.ok(
        Tx::Sale { seller: Party::Member(s), buyer: Party::Member(b), amount: 300.0, maturity_epochs: MATURITY },
        &[key(s as usize), key(b as usize)],
    );

    assert_eq!(c.st.contracts[&claim].status, ContractStatus::Active, "PROVEN: the claim does not move");
    assert!((c.outstanding(claim) - 300.0).abs() < 1e-9);
    assert!(c.st.members[&s].debt_out == State::to_minor(300.0), "the debtor sheds nothing");
    assert!((c.cap(s) - 100.0).abs() < 1e-9, "and collects no standing for it");
    assert!((staked(&c, cr, s) - 100.0).abs() < 1e-9, "the only stake is the one honest purchase");
    assert_eq!(owed_to(&c, b, cr), 0.0, "the creditor is handed nothing against an empty account");
    assert!((owed_to(&c, b, s) - 300.0).abs() < 1e-9, "and the sale still happens, toward the seller");
}

/// **A debtor swap stakes nothing, at the quantifier.**
///
/// `move_debtor` and `clear_member_debts` both wrote `stake(C, old debtor)`,
/// so a ring passing ONE accepted claim around collected a stake per hop.
/// Measured with a stake written per hop: `Σ stake(C, ·)` linear in the ring — 600 at k=2,
/// 3000 at k=10 — from a single 300 acceptance C signed once. The cut still
/// bounded the coalition's aggregate at C's own inflow, which is why no
/// invariant fired; a cut does not bound each member's OWN standing, and
/// `conferrable` is what `bond_headroom`, the drain cap and the establishment
/// floor all read.
#[test]
fn a_ring_of_debtor_swaps_stakes_nothing() {
    for ring in [2usize, 3, 5, 10] {
        let mut c = Chain::founded(1, 1 + ring);
        let cr: MemberId = 1;
        c.back(0, cr, 300.0);
        let members: Vec<MemberId> = (2..2 + ring as MemberId).collect();
        for &m in &members {
            c.back(0, m, 300.0);
        }

        let mut cid = c.lend(cr, members[0], 300.0);
        assert!(c.st.contracts[&cid].insured, "the fixture needs an insured original");
        for i in 1..ring {
            let (from, to) = (members[i - 1], members[i]);
            c.ok(Tx::Transfer { contract: cid, new_debtor: to }, &[key(from as usize), key(to as usize)]);
            cid = c.st.next_contract - 1;
        }

        let staked_total: f64 = members.iter().map(|&m| staked(&c, cr, m)).sum();
        assert_eq!(staked_total, 0.0, "ring {ring}: {ring} hops, {staked_total} of stake, and C signed nothing");

        // The successor's own settlement is what earns an edge — once, against
        // the member who actually paid.
        c.settle(cid, 300.0);
        let after: f64 = members.iter().map(|&m| staked(&c, cr, m)).sum();
        assert!((after - 300.0).abs() < 1e-9, "ring {ring}: one settlement, one stake, not {after}");
    }
}

/// **The merge is on the minor grid.** Several cleared originals can join one
/// successor row, so its hold is `Σ to_minor(take_i)` while its book is
/// `Σ take_i`, and §Verification invariant 2 compares the two exactly. Two takes of 1.005
/// would hold 200 against a book of 201. It holds because the room bound
/// answers in minor units and caps at `to_minor(take)`, so every take lands on
/// the grid — a property of the measure, gated here because the fix depends on
/// it from a distance.
#[test]
fn merged_successor_holds_stay_exact() {
    let mut c = Chain::founded(1, 3);
    let (cr, s, b): (MemberId, MemberId, MemberId) = (1, 2, 3);
    c.back(0, s, 300.0);
    c.back(0, b, 300.0);
    let a = c.lend(cr, s, 1.005);
    let d = c.lend(cr, s, 1.005);
    assert!(c.st.contracts[&a].insured && c.st.contracts[&d].insured);

    // The audit runs after this, and invariant 2 is exact — a drift halts here.
    c.ok(
        Tx::Sale { seller: Party::Member(s), buyer: Party::Member(b), amount: 2.01, maturity_epochs: MATURITY },
        &[key(s as usize), key(b as usize)],
    );

    let rows: Vec<&Contract> = c.st.contracts.values().filter(|x| x.debtor == b && x.creditor == cr).collect();
    assert_eq!(rows.len(), 1, "one merged successor");
    assert!(rows[0].insured);
    assert_eq!(
        rows[0].outstanding,
        rows[0].held.amount(),
        "an insured obligation owes exactly what it holds, merged or not"
    );
}

/// **A sale never lowers a creditor's insured total**, over 400 randomised
/// scenes with a deterministic generator.
///
/// This is the property the reordering has to buy. The successor now takes its
/// flow before the seller's remainder is re-held, so the split has a priority
/// where a race would otherwise be — and the question that leaves is whether the
/// part that STAYS can be starved by the part that moves. It cannot, and the
/// reason is that the released hold is exactly the demand: the buyer takes
/// `take` from a pool the original just freed, and the seller needs the rest of
/// that same pool, which is still reachable because it was reachable before.
/// Measured rather than argued, because "still reachable" is a claim about
/// routing and Dinic is the one doing it.
#[test]
fn a_sale_never_lowers_a_creditors_insured_total() {
    let mut seed = 0x2026_0817_u64;
    let mut rng = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let insured_to = |c: &Chain, creditor: MemberId| -> u64 {
        c.st.contracts
            .values()
            .filter(|x| x.creditor == creditor && x.insured)
            .filter(|x| matches!(x.status, ContractStatus::Active | ContractStatus::Expired))
            .map(|x| x.outstanding)
            .sum::<u64>()
    };

    let (mut exercised, mut split) = (0, 0);
    for scene in 0..400 {
        let (s1, s2) = (50.0 + (rng() % 40) as f64 * 10.0, 50.0 + (rng() % 40) as f64 * 10.0);
        let mut c = Chain::founded_with(&[s1, s2], 3);
        // The write gate is not what this probe is about, and a small founding
        // supply leaves little headroom for the fixture's own traffic.
        c.st.params.bond_free_allowance = 10_000;
        let (cr, sel, buy): (MemberId, MemberId, MemberId) = (2, 3, 4);
        // Every fixture step is TOLERANT: a randomised scene may legitimately
        // hit a capacity floor, and a step that does not land simply leaves the
        // scene smaller. Only the sale's effect is asserted.
        let round_trip = |c: &mut Chain, cred: MemberId, debt: MemberId, amt: f64| {
            let id = c.st.next_contract;
            let signers = [key(cred as usize), key(debt as usize)];
            let accept = Tx::Accept {
                debtor: Party::Member(debt),
                creditor: Party::Member(cred),
                amount: amt,
                maturity_epochs: MATURITY,
                arb: None,
            };
            if c.apply(accept, &signers).is_ok() {
                let _ = c.apply(Tx::Settle { contract: id, amount: amt }, &signers);
            }
        };
        round_trip(&mut c, 0, cr, 100.0 + (rng() % 30) as f64 * 10.0);
        round_trip(&mut c, 0, sel, 10.0 + (rng() % 40) as f64 * 10.0);
        for (u, m) in [(1u64, sel), (0, buy), (1, buy)] {
            if rng() % 4 != 0 {
                round_trip(&mut c, u, m, 10.0 + (rng() % 40) as f64 * 10.0);
            }
        }
        for _ in 0..(1 + rng() % 2) {
            let amt = 10.0 + (rng() % 50) as f64 * 10.0;
            let _ = c.apply(
                Tx::Accept {
                    debtor: Party::Member(sel),
                    creditor: Party::Member(cr),
                    amount: amt,
                    maturity_epochs: MATURITY,
                    arb: None,
                },
                &[key(cr as usize), key(sel as usize)],
            );
        }

        let before = insured_to(&c, cr);
        if before == 0 {
            continue;
        }
        exercised += 1;
        let sale = 10.0 + (rng() % 60) as f64 * 10.0;
        let _ = c.apply(
            Tx::Sale { seller: Party::Member(sel), buyer: Party::Member(buy), amount: sale, maturity_epochs: MATURITY },
            &[key(sel as usize), key(buy as usize)],
        );
        let after = insured_to(&c, cr);
        assert!(
            after >= before,
            "scene {scene}: the creditor's insured total fell {before} -> {after} across a sale of {sale}"
        );
        if owed_to(&c, buy, cr) > 0.0 && owed_to(&c, sel, cr) > 0.0 {
            split += 1;
        }
    }
    // Non-vacuity: a probe that never split a claim between the two debtors
    // would be green about a case it never reached, which is the shape this
    // whole item exists to distrust.
    assert!(exercised > 100, "only {exercised} scenes had an insured claim to lose");
    assert!(split > 10, "only {split} scenes actually split a claim across buyer and seller");
}

// ------------------------------------------------- what a drain is worth --
// **A drain relieves; it does not rehabilitate**, and it was written down the
// other way round twice. A proposal to automate it argued the beneficiary's approval was a footgun pointed at its
// holder because "`discharge_credit` applies §Standing's stake rule identically
// whether the debtor paid or a supporter did, so being drained toward lowers
// your debt, raises your standing"; the paper said it "confers exactly the
// standing self-payment confers", twice. That a debtor swap is not a settlement
// is what made that false, and nothing re-read these: a drain is a debtor
// swap, the creditor never signs it, and **a stake is only ever placed by a
// transition the creditor signed**. The
// paper's own `rem:only-settlement`, sixty lines below the sentence, already
// said so.
//
// The three probes below are the measurement that decided it, so they are
// gates rather than curiosities: the first two price one drain, and the third
// is the construction that makes the beneficiary's approval load-bearing.

/// **A drain confers none of the standing paying would**, measured against the
/// self-payment it is described as equalling.
#[test]
fn a_drain_confers_none_of_the_standing_paying_would() {
    // 0 underwriter, 1 supporter/seller, 2 buyer, 3 beneficiary, 4 their creditor.
    let scene = || {
        let mut c = Chain::founded(1, 4);
        for m in 1..=4 {
            c.back(0, m, SUPPLY);
        }
        c.back(1, 3, 200.0); // the pair stake the drain cap reads
        c
    };

    let mut drained = scene();
    let claim = drained.lend(4, 3, 80.0);
    drained.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(3, 1.0)] }, &[key(1)]);
    drained.ok(Tx::ApproveSupporter { beneficiary: 3, supporter: 1, approved: true }, &[key(3)]);
    drained.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 80.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );
    assert_eq!(drained.st.contracts[&claim].status, ContractStatus::Transferred, "the fixture must actually drain");
    assert_eq!(drained.st.members[&3].debt_out, 0, "and the relief is real: the debt is gone");

    // The control: the identical obligation, honoured by the member themselves.
    let mut paid = scene();
    let own = paid.lend(4, 3, 80.0);
    paid.settle(own, 80.0);

    assert_eq!(staked(&drained, 4, 3), 0.0, "PROVEN: a drain writes no stake — the creditor signed nothing");
    assert!((staked(&paid, 4, 3) - 80.0).abs() < 1e-9, "and the same 80 paid by the debtor writes 80");
}

/// **One transition, two paths, opposite standing** — and the difference is
/// exactly who signed. A `Sale` that NETS what the seller owes the buyer is a
/// settlement between the two parties in front of it, so it stakes; the same
/// sale clearing the seller's debt to a THIRD party is a debtor swap that
/// creditor never saw, so it stakes nothing. The cascade's self entry is on the
/// second path, which is the half no document states.
#[test]
fn the_sellers_own_cascade_share_stakes_nothing_and_netting_does() {
    let scene = || {
        let mut c = Chain::founded(1, 3);
        for m in 1..=3 {
            c.back(0, m, SUPPLY);
        }
        c
    };

    // No listing at all, so the whole sale clears the seller's own debts.
    let mut cascaded = scene();
    let own = cascaded.lend(3, 1, 80.0);
    cascaded.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 80.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );
    assert_eq!(cascaded.st.contracts[&own].status, ContractStatus::Transferred);
    assert_eq!(staked(&cascaded, 3, 1), 0.0, "PROVEN: clearing your own debt through a sale earns nothing");

    // The same debt, discharged by selling to the creditor instead.
    let mut netted = scene();
    let same = netted.lend(3, 1, 80.0);
    netted.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(3), amount: 80.0, maturity_epochs: MATURITY },
        &[key(1), key(3)],
    );
    assert_eq!(netted.st.contracts[&same].status, ContractStatus::Settled);
    assert!((staked(&netted, 3, 1) - 80.0).abs() < 1e-9, "and selling to the creditor earns all of it");
}

/// **Why the beneficiary's approval is load-bearing, and it is not the reason
/// first written down.**
///
/// Standing is the only source of capacity and it is written by exactly one
/// act. A supporter who clears a member's debts before that member can honour
/// them spends the member's obligations without the member ever earning the
/// edge — so a supporter can hold a beneficiary at the standing they started
/// with, indefinitely, at the supporter's own expense. Measured: three loans of
/// 60, identical in every respect but who discharged them.
///
/// This is what the approval governs, and why deleting it — either for that proposal's
/// signature or outright — is refused. The whitelist is not moderation against
/// coupling; it is the member deciding **relief now against standing later**,
/// which is a trade only they can weigh and which no cap can bound, since the
/// drain cap is a ceiling on the amount rather than on the substitution.
#[test]
fn being_carried_leaves_a_member_exactly_where_it_started() {
    // The target is NOT backed by the underwriter, so its capacity is precisely
    // what its own creditors have staked in it — the quantity a drain skips.
    let scene = || {
        let mut c = Chain::founded(1, 6);
        for m in [1u64, 2, 4, 5, 6] {
            c.back(0, m, SUPPLY);
        }
        c.back(1, 3, 200.0); // one honoured purchase: the pair stake, and 200 of capacity
        c
    };

    let mut carried = scene();
    assert!((carried.cap(3) - 200.0).abs() < 1e-9, "both arms start here");
    carried.ok(Tx::ListBeneficiaries { supporter: 1, entries: vec![(3, 1.0)] }, &[key(1)]);
    carried.ok(Tx::ApproveSupporter { beneficiary: 3, supporter: 1, approved: true }, &[key(3)]);
    for cr in 4..=6u64 {
        carried.lend(cr, 3, 60.0);
        // **The ceiling and the room under it are different quantities, and a
        // drain moves the second one** (§Medium). The loan reserves its flow,
        // so the published figure falls while it stands, and the discharge
        // returns exactly what was taken (`flow::release`) and hands it to
        // nobody. Both lines sit inside the loop because the end state alone
        // does not distinguish them: book these obligations UNINSURED and the
        // arm still finishes at 200 with every invariant and the conservation
        // harness green — mutated, the first line is the one that fails.
        assert!((carried.cap(3) - 140.0).abs() < 1e-9, "the loan reserves its flow: {}", carried.cap(3));
        carried.ok(
            Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 60.0, maturity_epochs: MATURITY },
            &[key(1), key(2)],
        );
        assert!((carried.cap(3) - 200.0).abs() < 1e-9, "and the drain releases it: {}", carried.cap(3));
    }

    let mut honoured = scene();
    for cr in 4..=6u64 {
        let cid = honoured.lend(cr, 3, 60.0);
        honoured.settle(cid, 60.0);
    }

    // Same debt outcome on both arms: the relief the cascade promises is real.
    assert_eq!(carried.st.members[&3].debt_out, 0);
    assert_eq!(honoured.st.members[&3].debt_out, 0);
    let carried_stake: f64 = (4..=6).map(|cr| staked(&carried, cr, 3)).sum();
    let honoured_stake: f64 = (4..=6).map(|cr| staked(&honoured, cr, 3)).sum();
    assert_eq!(carried_stake, 0.0, "PROVEN: three rounds of being carried leave Σ stake at zero");
    assert!((honoured_stake - 180.0).abs() < 1e-9, "the same three loans honoured write 180");
    assert!((carried.cap(3) - 200.0).abs() < 1e-9, "so capacity is exactly where it began");
    assert!((honoured.cap(3) - 380.0).abs() < 1e-9, "against 380 for the member who paid");
}

// ------------------------------------------------------- the review's probes --

mod final_review_dates {
    use super::*;

    fn lend_at(c: &mut Chain, debtor: MemberId, creditor: MemberId, amount: f64, maturity_epochs: u64) -> ContractId {
        let cid = c.st.next_contract;
        c.ok(
            Tx::Accept {
                debtor: Party::Member(debtor),
                creditor: Party::Member(creditor),
                amount,
                maturity_epochs,
                arb: None,
            },
            &[key(debtor as usize), key(creditor as usize)],
        );
        cid
    }

    /// **A routed claim inherits the earlier of its own date and the sale's.**
    /// Booked at the sale's date, a claim due in thirty epochs matured ten
    /// thousand epochs out on the signatures of two other members — the exact
    /// re-dating `move_debtor` refuses for a transfer — insured throughout, so
    /// no default ever fired.
    ///
    /// Mutation that bites: key `Assumed` by creditor and book at the sale's
    /// maturity.
    #[test]
    fn a_routed_claim_inherits_the_earlier_of_its_date_and_the_sales() {
        let mut c = Chain::founded(1, 3);
        let (u, s, b, cr) = (0u64, 1u64, 2u64, 3u64);
        c.back(u, s, 500.0);
        c.back(u, b, 500.0);
        let orig = lend_at(&mut c, s, cr, 300.0, 30);
        let due = c.st.contracts[&orig].maturity_epoch;
        let succ = c.st.next_contract;
        c.ok(
            Tx::Sale { seller: Party::Member(s), buyer: Party::Member(b), amount: 300.0, maturity_epochs: 10_000 },
            &[key(s as usize), key(b as usize)],
        );
        assert_eq!(c.st.contracts[&orig].status, ContractStatus::Transferred);
        let n = &c.st.contracts[&succ];
        assert_eq!((n.debtor, n.creditor), (b, cr));
        assert_eq!(n.maturity_epoch, due, "the creditor is paid no later than the original said");
        assert!(n.insured);
    }

    /// The other direction: a sale on shorter terms accelerates the routed
    /// claim to them, which is no later than the buyer signed for.
    #[test]
    fn a_sale_on_shorter_terms_accelerates_the_routed_claim_to_them() {
        let mut c = Chain::founded(1, 3);
        let (u, s, b, cr) = (0u64, 1u64, 2u64, 3u64);
        c.back(u, s, 500.0);
        c.back(u, b, 500.0);
        lend_at(&mut c, s, cr, 300.0, 100);
        let succ = c.st.next_contract;
        c.ok(
            Tx::Sale { seller: Party::Member(s), buyer: Party::Member(b), amount: 300.0, maturity_epochs: 30 },
            &[key(s as usize), key(b as usize)],
        );
        assert_eq!(c.st.contracts[&succ].maturity_epoch, c.st.epoch + 30);
    }

    /// Two originals from one creditor on two dates are two successor rows,
    /// each on its own date.
    #[test]
    fn two_originals_from_one_creditor_on_different_dates_are_two_successor_rows() {
        let mut c = Chain::founded(1, 3);
        let (u, s, b, cr) = (0u64, 1u64, 2u64, 3u64);
        c.back(u, s, 1_000.0);
        c.back(u, b, 1_000.0);
        lend_at(&mut c, s, cr, 300.0, 30);
        lend_at(&mut c, s, cr, 200.0, 100);
        let first = c.st.next_contract;
        c.ok(
            Tx::Sale { seller: Party::Member(s), buyer: Party::Member(b), amount: 500.0, maturity_epochs: 10_000 },
            &[key(s as usize), key(b as usize)],
        );
        let mut successors: Vec<(u64, u64)> =
            c.st.contracts
                .range(first..)
                .filter(|(_, x)| x.debtor == b && x.creditor == cr)
                .map(|(_, x)| (x.maturity_epoch, x.outstanding))
                .collect();
        successors.sort();
        assert_eq!(successors, vec![(30, State::to_minor(300.0)), (100, State::to_minor(200.0))]);
    }
}
