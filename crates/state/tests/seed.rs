//! Growing the seed (the paper's §Governance).
//!
//! A supply can be seated by a ceremony and by nothing else — genesis, or an
//! amendment through the existing `Propose`/`Assent`. Without this transition
//! the ledger's external-commitment door would close at genesis, permanently,
//! for everyone: a founder could not raise their supply, a newcomer bringing
//! backing from outside would have no transition to arrive through, and a
//! co-op forming in year three could never have underwriters at all.
//! Externality is the one property no set of signatures can prove (§Security),
//! so the repair is not a proof but a repeatable ceremony.
//!
//! **The sharpest probe in this file is the one about conferred capacity.**
//! Standing the community itself hands a member is worth nothing here: not a
//! declaration, not a vote, not a unit of amendment headroom. A `DeclareSupply`
//! that admitted a raise up to the declarer's own capacity would buy a coalition
//! 2000x the seed in insured credit, issued to sybils it controls, with every
//! invariant satisfied. Everything else here is a consequence.

mod common;

use common::{key, Chain, SUPPLY};
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

/// β at genesis. The suite reads it rather than hard-coding 0.02, so a change
/// to the constant moves the expectations with it instead of turning every
/// probe red for a reason that is not a defect.
fn beta(c: &Chain) -> f64 {
    c.st.params.seed_rate_bounded()
}

/// What `debtor` owes `creditor` over live claims.
fn owed_to(c: &Chain, debtor: MemberId, creditor: MemberId) -> f64 {
    c.st.contracts
        .values()
        .filter(|x| x.debtor == debtor && x.creditor == creditor)
        .filter(|x| matches!(x.status, ContractStatus::Active | ContractStatus::Expired))
        .map(|x| x.outstanding)
        .sum::<u64>() as f64
        / 100.0
}

/// Ask the community to endorse `author`'s external commitment of `amount`,
/// and have `assenters` weigh in — in order, until the proposal carries.
/// Returns the proposal id.
///
/// The author co-signs with an established member, which is the ordinary case
/// and the one §Governance is written for: a member bringing backing from outside has
/// nothing on-ledger yet, so the write is billed to whoever is willing to
/// carry it — the same cost §Recourse already names for a newcomer's first trade.
///
/// Stopping at enactment is not tidiness. Every amendment enlarges the external
/// seed, which is governance's own denominator (§Governance), so how many assenters a
/// proposal needs MOVES as the seed grows — a fixed list would be measuring the
/// coalition rather than the mechanism.
fn amend(c: &mut Chain, author: MemberId, amount: f64, assenters: &[MemberId]) -> ProposalId {
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author, kind: ProposalKind::SeedAmendment { amount } }, &[key(author as usize), key(0)]);
    for &a in assenters {
        if c.st.proposals[&pid].enacted {
            break;
        }
        c.ok(Tx::Assent { member: a, proposal: pid }, &[key(a as usize)]);
    }
    pid
}

fn supply_of(c: &Chain, id: MemberId) -> f64 {
    State::from_minor(c.st.underwriters.get(&id).copied().unwrap_or(0))
}

/// A member's ceremony-seated supply — the same map as `supply_of`, since
/// every supply is ceremony-seated. Kept as a separate reading because the two
/// answer different questions.
fn external_of(c: &Chain, id: MemberId) -> f64 {
    supply_of(c, id)
}

// ------------------------------------------------------------ the ceremony --

/// The door the capacity cap cannot open. A member nobody has backed may
/// declare exactly nothing — that cap is Sybil-critical and stays — and the
/// community can still seat them as an underwriter, because a ceremony is a
/// different act from a declaration.
#[test]
fn an_amendment_seats_an_underwriter_the_capacity_cap_never_could() {
    let mut c = Chain::founded(4, 2);
    assert_eq!(c.cap(4), 0.0, "nobody has backed them");
    // Refused at the write gate — a supply raise is bonded, and a key
    // with nothing behind it has nothing to reserve. The capacity cap below it
    // is the same answer one layer in: `ET_UWR_ABOVE_CAPACITY` is what it gets
    // once somebody has backed it enough to afford the question at all.
    c.err(Tx::DeclareSupply { member: 4, supply: 100.0 }, &[key(4)], ET_BOND_EXHAUSTED);

    amend(&mut c, 4, 100.0, &[0, 1]);
    assert_eq!(supply_of(&c, 4), 100.0, "the ceremony seats them");
    assert_eq!(external_of(&c, 4), 100.0, "and records the commitment as external");

    // And it is a real source arc: standing now flows from them like anyone's.
    c.back(4, 5, 100.0);
    assert_eq!(c.cap(5), 100.0, "the amended supply reaches a member through ordinary trade");
}

/// A ledger that underwrote nothing cannot amend its way into a seed. Zero is
/// absorbing, and stays absorbing by two independent routes — the rate base is
/// a fraction of nothing, and a community with no declared supply has no
/// governance denominator either, so no proposal of any kind can be enacted.
///
/// The write itself never even lands: with no supply anywhere, no signer has
/// bond headroom, so the proposal is refused at the gate. All three are the
/// free-signature bound in different clothes.
#[test]
fn a_ledger_that_underwrote_nothing_cannot_amend_its_way_into_a_seed() {
    let mut c = Chain::founded(0, 3);
    assert_eq!(edet_state::seed::headroom(&c.st), 0.0, "a fraction of nothing is nothing");
    c.err(
        Tx::Propose { author: 0, kind: ProposalKind::SeedAmendment { amount: 1000.0 } },
        &[key(0), key(1)],
        ET_BOND_EXHAUSTED,
    );
    assert!(c.st.underwriters.is_empty());
    assert_eq!(c.st.external_seed(), 0.0);
}

/// **The consent is the author's own signature, and there is no other way to
/// give it.** A supply is a standing consent to inherit debts (§Recourse), so an
/// amendment naming somebody else would volunteer a member to underwrite. The
/// kind names nobody: its beneficiary is its author, structurally, so a
/// proposal the beneficiary did not sign is not a proposal at all.
#[test]
fn an_amendment_cannot_be_made_on_somebody_elses_behalf() {
    let mut c = Chain::founded(4, 1);
    c.err(
        Tx::Propose { author: 4, kind: ProposalKind::SeedAmendment { amount: 100.0 } },
        &[key(0), key(1)],
        ET_MEM_NOT_SIGNER,
    );
    assert!(c.st.proposals.is_empty(), "nothing was recorded against a member who never signed");
}

/// The establishment floor is waived for an amendment and for nothing else.
/// A member with nothing on-ledger is exactly who §Standing names as the growth path,
/// so the floor would close the door on the case it exists to open — but the
/// write is still bonded, so a newcomer's amendment is carried by an
/// established member, and every other kind of proposal still needs standing.
#[test]
fn only_an_amendment_may_be_proposed_by_a_member_with_no_standing() {
    let mut c = Chain::founded(4, 1);
    // Alone, the newcomer cannot write at all: no capacity, no headroom.
    c.err(Tx::Propose { author: 4, kind: ProposalKind::SeedAmendment { amount: 100.0 } }, &[key(4)], ET_BOND_EXHAUSTED);
    // Co-signed by somebody with something to lose, it lands.
    c.ok(Tx::Propose { author: 4, kind: ProposalKind::SeedAmendment { amount: 100.0 } }, &[key(4), key(0)]);
    // The floor still stands everywhere else.
    c.err(
        Tx::Propose { author: 4, kind: ProposalKind::Suspend { member: 3 } },
        &[key(4), key(0)],
        ET_GOV_NOT_ESTABLISHED,
    );
}

/// An amendment is a decision of the community, at the same bar as every other
/// governance act: below Θ_adopt of the external seed (§Governance) nothing moves, and
/// the assent that carries it enacts it.
#[test]
fn an_amendment_needs_the_communitys_assent() {
    let mut c = Chain::founded(4, 1);
    let pid = amend(&mut c, 4, 100.0, &[0]);
    assert!(!c.st.proposals[&pid].enacted, "one of four underwriters is below the bar");
    assert_eq!(supply_of(&c, 4), 0.0);

    c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
    assert!(c.st.proposals[&pid].enacted, "two carry it");
    assert_eq!(supply_of(&c, 4), 100.0);
}

/// **A member may not assent their own amendment.** The author is the
/// beneficiary, and the governance weight counts an assenting underwriter's
/// own declared supply — so without this a dominant underwriter would vote
/// their own raise through on the strength of the quantity the raise enlarges,
/// each one carrying the next more easily.
///
/// This is also §Adoption's founder case: raising takes effect immediately is true
/// DOWNWARD only, because a founder's own capacity is near zero. The amendment
/// is the way up, and it is the community's to grant rather than the founder's
/// to take.
#[test]
fn a_member_may_not_assent_their_own_amendment() {
    let mut c = Chain::founded(4, 0);
    c.err(Tx::DeclareSupply { member: 0, supply: SUPPLY + 100.0 }, &[key(0)], ET_UWR_ABOVE_CAPACITY);

    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::SeedAmendment { amount: 100.0 } }, &[key(0)]);
    c.err(Tx::Assent { member: 0, proposal: pid }, &[key(0)], ET_SEED_CONFLICTED);
    assert!(!c.st.proposals[&pid].assents.contains(&0), "and the vote is refused, not silently discounted");

    c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
    c.ok(Tx::Assent { member: 2, proposal: pid }, &[key(2)]);
    assert_eq!(supply_of(&c, 0), SUPPLY + 100.0, "the community can raise a founder the founder cannot");
    assert_eq!(external_of(&c, 0), SUPPLY + 100.0, "and the whole of it is external");
}

/// And the obvious way around that rule — assent from a second identity the
/// author controls — is worth nothing, because it was already worth nothing.
///
/// The conflict rule is not what stops this and never needed to be: assent is
/// weighted by the external seed (§Governance), and a key no ceremony has seated holds
/// none of it. This is the free-signature bound reaching the one new door in
/// the alphabet, and it is the reason the door needed no Sybil machinery of its
/// own. Since §Governance the refusal is explicit rather than arithmetic — the assent
/// would otherwise be accepted and counted zero, and a vote the ledger shows and does
/// not use is a preference rather than a fact about state.
#[test]
fn an_author_gains_nothing_by_assenting_from_a_second_identity() {
    let mut c = Chain::founded(4, 2);
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 4, kind: ProposalKind::SeedAmendment { amount: 100.0 } }, &[key(4), key(0)]);

    // Member 5 is the author's other key by supposition, and free keys are
    // free — so this is exactly as many as an attacker wants.
    c.err(Tx::Assent { member: 5, proposal: pid }, &[key(5)], ET_GOV_NO_MANDATE);
    assert!(!c.st.proposals[&pid].enacted, "a key outside the electorate carries nothing");
    assert!(c.st.proposals[&pid].assents.is_empty(), "and leaves no trace, however many keys try");
    assert_eq!(supply_of(&c, 4), 0.0);

    // What carries it is members the community has actually put something
    // behind, which is the whole of the mechanism.
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
    assert_eq!(supply_of(&c, 4), 100.0);
}

// -------------------------------------------------------------- the bound --

/// **No amount of conferred capacity moves the amendment door**, and there are
/// three independent refusals in the way. This is the security content of the
/// whole mechanism.
///
/// Under a capacity-capped declaration the scene ends in a declared total of
/// 64× the seed: each joiner
/// backed by every underwriter so far, declaring exactly the capacity that
/// bought — the same shape as the 4096× in twelve joiners. That inflation was
/// called real and harmless on its own, on the argument that what those
/// members can owe TOGETHER stays at the seed. Measuring who the credit goes to
/// instead of who declared it gives 2000× the seed in ledger-labelled insured
/// credit. So the declaration is refused at the door, and the joiners' capacity
/// buys them nothing anywhere: not a supply, not a vote, and not a unit of
/// amendment headroom.
#[test]
fn conferred_capacity_never_reaches_the_amendment_door() {
    let mut c = Chain::founded(1, 6);
    let external_before = c.st.external_seed();
    let headroom_before = edet_state::seed::headroom(&c.st);
    assert_eq!(external_before, SUPPLY);

    // Each joiner is backed by every underwriter so far, so its capacity is
    // their sum. Nothing here is an attack: every step is an ordinary trade.
    // The declaration that would follow is the first refusal.
    for j in 1..=6u64 {
        for creditor in 0..j {
            c.back(creditor, j, 1_000_000.0);
        }
        let cap = c.cap(j);
        assert!(cap > 0.0, "the joiner really does hold capacity — that is what made this work");
        c.err(Tx::DeclareSupply { member: j, supply: cap }, &[key(j as usize)], ET_UWR_ABOVE_CAPACITY);
    }

    assert_eq!(c.st.external_seed(), external_before, "the roll is the founder's seed, and only that");
    assert_eq!(
        edet_state::seed::headroom(&c.st),
        headroom_before,
        "so the amendment door is exactly where it was, whatever the joiners can borrow"
    );

    // The bound answers to the seed, and to nothing the community told itself.
    let over = headroom_before + 1.0;
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 1, kind: ProposalKind::SeedAmendment { amount: over } }, &[key(1)]);

    // Second: the joiner with the largest capacity in the community holds no
    // supply, and since the weight became the external seed (§Governance) it
    // carries no vote at all — the assent is refused outright, so the rate
    // bound is never even consulted.
    c.err(Tx::Assent { member: 6, proposal: pid }, &[key(6)], ET_GOV_NO_MANDATE);
    assert!(!c.st.proposals[&pid].enacted, "conferred standing must not carry a proposal one step");

    // Third, and the one this probe was written for: the founder — who holds
    // the whole external seed — CAN carry it, and the rate bound refuses on
    // the amount. So the door is shut by arithmetic even for the electorate
    // that is entitled to open it.
    c.err(Tx::Assent { member: 0, proposal: pid }, &[key(0)], ET_SEED_RATE);
}

/// The bound binds per epoch and releases at the boundary: amendments inside
/// one epoch sum to at most β × the seed the epoch opened with, and an assent
/// refused for the rate is not banked — so the same member re-assenting next
/// epoch is a real decision rather than a formality.
#[test]
fn the_rate_bound_binds_per_epoch_and_releases_at_the_boundary() {
    let mut c = Chain::founded(2, 2);
    let seed = 2.0 * SUPPLY;
    let allowance = beta(&c) * seed;
    assert_eq!(edet_state::seed::headroom(&c.st), allowance);

    // Three quarters of the epoch's allowance, endorsed.
    let first = allowance * 0.75;
    amend(&mut c, 2, first, &[0, 1]);
    assert_eq!(supply_of(&c, 2), first);
    assert_eq!(edet_state::seed::headroom(&c.st), allowance - first, "the epoch has this much left");

    // A second amendment for more than the remainder cannot enact.
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 3, kind: ProposalKind::SeedAmendment { amount: allowance * 0.5 } }, &[key(3), key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    c.err(Tx::Assent { member: 1, proposal: pid }, &[key(1)], ET_SEED_RATE);
    assert!(!c.st.proposals[&pid].assents.contains(&1), "a refused assent is not banked");
    assert_eq!(supply_of(&c, 3), 0.0);

    // One that fits does.
    let pid_small = amend(&mut c, 3, allowance * 0.25, &[0, 1]);
    assert!(c.st.proposals[&pid_small].enacted);
    assert_eq!(edet_state::seed::headroom(&c.st), 0.0, "the epoch is spent to the unit");

    // The boundary releases it, against a base that now includes what the
    // epoch admitted — the compounding is real, and it is what "β × the
    // TRACKED external seed" means.
    c.goto(1);
    let grown = seed + allowance;
    assert_eq!(c.st.external_seed(), grown);
    assert_eq!(edet_state::seed::headroom(&c.st), beta(&c) * grown);
    c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
    assert!(c.st.proposals[&pid].enacted, "and the assent that was refused is a decision again");
}

/// What the bound is honestly worth, measured rather than asserted: at β
/// pinned to its constitutional ceiling, a community that assents to every
/// amendment it can, every epoch, multiplies its declared external seed by a
/// bounded and unremarkable factor over a month.
///
/// This is the residual risk §Governance states in the open — a capacity-majority
/// plus inattentive lenders can inflate slowly — with the "slowly" turned into
/// a number. Nothing here is a defence against a community lying to itself;
/// what the bound buys is that the lie takes 30 public, attributable steps
/// during which lenders can simply stop delivering goods.
///
/// Two beneficiaries take turns and assent each other, which is not a trick:
/// each amendment enlarges the declared total, so a fixed coalition is diluted
/// by the very seed it admits and stalls below Θ_adopt within a few epochs.
/// **Amending is self-limiting under the governance weight as well as under
/// the rate**, and this is the arrangement that keeps the harder of the two
/// bounds — the rate — the one actually being measured.
#[test]
fn a_month_of_maximum_amendment_is_bounded_and_public() {
    let mut c = Chain::founded(2, 2);
    let (_, ceiling) = edet_state::params::Params::safe_range(ParamKey::SeedRate);
    c.st.params.seed_rate = ceiling;
    let seed = c.st.external_seed();

    for epoch in 1..=30u64 {
        let (author, other) = if epoch % 2 == 0 { (2, 3) } else { (3, 2) };
        let take = edet_state::seed::headroom(&c.st);
        let pid = amend(&mut c, author, take, &[0, 1, other]);
        assert!(c.st.proposals[&pid].enacted, "epoch {epoch}: the community assented to everything it could");
        c.goto(epoch);
    }
    let growth = c.st.external_seed() / seed;
    assert!(
        (growth - 4.32).abs() < 0.01,
        "30 epochs at the ceiling must stay a slow leak rather than an explosion: {growth}x"
    );
    // And every unit of it is somebody's declared liability, not a figure.
    let endorsed = supply_of(&c, 2) + supply_of(&c, 3);
    assert!((endorsed - (c.st.external_seed() - seed)).abs() < 1e-6);
}

// ------------------------------------------------------ what it endorses --

/// **An amendment endorses a §Recourse liability, and the liability is real.** That
/// is why substitution had to be built first: without it an amendment endorses
/// vapor. Measured end to end — the community seats an underwriter who has
/// never traded, credit flows through them, the debtor defaults, and the
/// creditor's claim lands on the underwriter as an ordinary debt they now owe
/// in goods and services.
#[test]
fn an_amendment_endorses_a_liability_that_actually_lands() {
    let mut c = Chain::founded(4, 3);
    let (u, debtor, creditor) = (4, 5, 6);
    amend(&mut c, u, 200.0, &[0, 1]);
    c.back(u, debtor, 200.0);

    let cid = c.lend(creditor, debtor, 200.0);
    assert!(c.st.contracts[&cid].insured, "the amended supply insures it");
    c.default_on(cid);

    assert_eq!(owed_to(&c, debtor, creditor), 0.0, "the creditor is whole");
    assert!((owed_to(&c, u, creditor) - 200.0).abs() < 1e-9, "and the endorsed underwriter owes it");
    assert!((owed_to(&c, debtor, u) - 200.0).abs() < 1e-9, "with the defaulter's debt subrogated to them");
}

/// An amendment adds exactly what it says to the cut, and no more. The set
/// form is the security statement: everyone drawing through the amended supply
/// can owe, together, what that supply carries — not what it carries times the
/// number of them.
///
/// Tested at the quantifier the theorem uses, because every defect this model
/// has carried was a bound asserted over a set and checked over a singleton.
#[test]
fn an_amendment_moves_the_cut_by_exactly_what_it_endorsed() {
    let mut c = Chain::founded(4, 4);
    let before = c.st.gross_capacity_of_set(&[5, 6, 7]);
    amend(&mut c, 4, 100.0, &[0, 1]);
    for d in [5, 6, 7] {
        c.back(4, d, 100.0);
        assert_eq!(c.cap(d), 100.0, "the limit propagates undiminished to each of them");
    }
    assert_eq!(
        c.st.gross_capacity_of_set(&[5, 6, 7]),
        before + 100.0,
        "but simultaneous use is one supply, however many draw on it"
    );
}

/// A late-arriving community can seed on an existing ledger instead of
/// founding its own — §Model's one-ledger-many-communities claim quietly assumed
/// every community was present at genesis, and per region zero is absorbing.
///
/// The two co-ops stay separate for as long as nobody trades across, which is
/// the whole of §Model: the boundary is the absence of a path, and the second
/// co-op's credit comes from its own endorsed underwriter rather than from the
/// first co-op's seed.
#[test]
fn a_community_that_forms_in_year_three_can_be_seeded_on_the_ledger() {
    let mut c = Chain::founded(1, 3);
    // The old co-op: underwriter 0 backing member 1.
    c.back(0, 1, SUPPLY);
    // The new one, formed later: 2 is endorsed, and backs 3.
    amend(&mut c, 2, 40.0, &[0]);
    c.back(2, 3, 40.0);

    assert_eq!(c.cap(1), SUPPLY, "the old co-op is where it was");
    assert_eq!(c.cap(3), 40.0, "the new one has credit of its own");
    assert_eq!(
        c.st.gross_capacity_of_set(&[1, 3]),
        SUPPLY + 40.0,
        "and the two are one ledger's arithmetic, not two ledgers' trust"
    );
}

// ------------------------------------------------------- the external record --

/// **Earning capacity adds nothing to what the ceremony seated**, and the
/// declaration a member holds is exactly what was endorsed until they lower it.
///
/// There is no ORDERING to pin. A declaration part endorsed and part declared
/// against the member's own capacity would need one — the endorsement sitting
/// ABOVE the capacity cap so that earning capacity later did not quietly erase
/// it, and a withdrawal retracting the internal half first — and capacity buys
/// no declaration at all, so there is one part and what a withdrawal takes is
/// that part. What stays true is that the endorsement is not swallowed by the
/// capacity it sits above, because nothing about capacity touches it.
#[test]
fn the_ceremony_is_the_whole_of_a_declaration() {
    let mut c = Chain::founded(4, 2);
    amend(&mut c, 4, 100.0, &[0, 1]);
    c.back(0, 4, SUPPLY);
    assert_eq!(c.cap(4), SUPPLY, "they have since earned standing of their own");
    assert_eq!(external_of(&c, 4), 100.0, "and it is worth nothing at this door");

    // Neither the capacity nor the sum of the two is declarable.
    c.err(Tx::DeclareSupply { member: 4, supply: SUPPLY + 100.0 }, &[key(4)], ET_UWR_ABOVE_CAPACITY);
    c.err(Tx::DeclareSupply { member: 4, supply: SUPPLY }, &[key(4)], ET_UWR_ABOVE_CAPACITY);
    c.err(Tx::DeclareSupply { member: 4, supply: 101.0 }, &[key(4)], ET_UWR_ABOVE_CAPACITY);
    assert_eq!(external_of(&c, 4), 100.0, "the endorsement is not swallowed by the capacity it sits above");
    // Restating it is not a raise, so it stands.
    c.ok(Tx::DeclareSupply { member: 4, supply: 100.0 }, &[key(4)]);
    assert_eq!(external_of(&c, 4), 100.0);

    // Trimming takes the endorsement with it, in whatever step it is asked for.
    c.ok(Tx::DeclareSupply { member: 4, supply: 60.0 }, &[key(4)]);
    assert_eq!(external_of(&c, 4), 60.0);
    c.ok(Tx::DeclareSupply { member: 4, supply: 0.0 }, &[key(4)]);
    assert_eq!(external_of(&c, 4), 0.0, "and leaving the role retracts it entirely");
    assert_eq!(c.st.external_seed(), 4.0 * SUPPLY, "the ledger's seed is back to its genesis figure");
}

/// A re-denomination is a change of unit, so it must carry the external record
/// by the same factor as the supplies it describes — otherwise the amendment
/// door would silently change width at every rescale.
#[test]
fn a_redenomination_carries_the_external_record() {
    let mut c = Chain::founded(2, 1);
    amend(&mut c, 2, 20.0, &[0]);
    let (seed, headroom) = (c.st.external_seed(), edet_state::seed::headroom(&c.st));

    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::Redenominate { num: 3, den: 2 } }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
    assert!(c.st.proposals[&pid].enacted, "the amendment enlarged the denominator: it takes both founders now");

    let pi = 1.5;
    assert!((c.st.external_seed() - pi * seed).abs() < 1e-6, "the seed rescales with everything else");
    assert!((edet_state::seed::headroom(&c.st) - pi * headroom).abs() < 1e-6, "and so does the door");
    for id in c.st.underwriters.keys() {
        assert!(c.st.members.contains_key(id), "and every rescaled supply still belongs to a member");
    }
}

/// The amount is transition payload and is checked as one: a nonsense
/// commitment is refused where it is written rather than enacted into state.
#[test]
fn a_nonsense_amount_is_refused_at_the_proposal() {
    let mut c = Chain::founded(2, 1);
    for amount in [0.0, -1.0, f64::NAN, f64::INFINITY, 0.001] {
        c.err(
            Tx::Propose { author: 2, kind: ProposalKind::SeedAmendment { amount } },
            &[key(2), key(0)],
            ET_CTR_BAD_AMOUNT,
        );
    }
}
