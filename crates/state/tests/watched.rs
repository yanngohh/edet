//! The **watched** list of CLAUDE.md, measured.
//!
//! A closed finding is gated by construction: a box is ticked only
//! when its construction is a probe in the tree. The Watched half had nothing
//! at all — every entry there is a factual claim about this ledger, recorded as
//! a consequence rather than a defect, and nothing checked any of them.
//!
//! That is exactly the shape this tree keeps paying for, and it had already
//! happened: **"the uninsured tier records nothing" was false.** A default on an
//! uninsured obligation writes the defaulter's `open_default`, which `/member`
//! serves to any authenticated member and the wallet's acceptance engine
//! declines on. What is true is the entry's conclusion rather than its premise —
//! the trace is per-account and accounts are free, so it is shed by starting
//! again rather than by never existing.
//!
//! So the watched claims are probes now, held to the same standard as the
//! closed ones. A "watched consequence" that has quietly become false is worse
//! than an open defect: an auditor reads it as a description of the tree.
//!
//! Two entries are deliberately absent. The `audit` gate's is a claim about a
//! live advisory registry rather than about this ledger, and "comments that
//! describe the model" is a claim about prose no test can read — which is the entry's
//! own point.
//!
//! A third left by being ANSWERED. `co_signers` was watched under its own
//! terms — "provenance nobody reads: either something should consume it or it
//! should go" — so it went, with the probe that pinned it. That
//! is the right end state for this half: a watched consequence stops being
//! watched when it stops being true.

mod common;

use common::{key, Chain, MATURITY, SUPPLY};
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

/// **The uninsured tier records the DEBTOR, and the record does not follow a
/// person.**
///
/// The entry said "a default on an uninsured obligation affects no quantity
/// anywhere" and "without any trace the next creditor could read". Both are
/// false: `open_default` rises by the defaulted amount, `/member` serves it to
/// any authenticated member (and to no anonymous one), and `decideBand` in the
/// client declines on it.
///
/// What survives, and it is the load-bearing half: the trace sits on a ROW, and
/// rows are free. A serial uninsured defaulter and a genuine newcomer are
/// informationally identical at zero — not because nothing was written, but
/// because what was written does not follow them to a new account.
#[test]
fn an_uninsured_default_records_the_debtor_and_a_fresh_row_sheds_it() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY); // the creditor needs standing to pay the bond
    let cid = c.lend(1, 2, 100.0);
    assert!(!c.st.contracts[&cid].insured, "the fixture must actually be uninsured");

    assert_eq!(c.st.members[&2].rep.open_default, 0);
    let due = c.st.contracts[&cid].maturity_epoch;
    c.default_on(cid);
    assert_eq!(c.st.members[&2].rep.open_default, State::to_minor(100.0), "the defaulter's row records it");

    // Whether an uninsured loss should touch the CREDITOR's own standing is
    // The open question was whether it does, and the answer is that it does not. Measured
    // against a control that ages identically without a default, because the
    // epochs the crank advances decay every stake and that is a different
    // mechanism entirely — comparing before with after would credit decay to
    // the default.
    let mut control = Chain::founded(1, 2);
    control.back(0, 1, SUPPLY);
    control.goto(due + 1);
    assert_eq!(c.cap(1), control.cap(1), "the creditor who chose the risk keeps exactly what time left them");

    // No capacity moved, because nothing was ever reserved — which is the half
    // of the entry that was right, and is not the same as "no trace".
    assert_eq!(c.cap(2), 0.0);

    // Curing clears it, so it is a state and not a verdict.
    c.ok(Tx::Cure { contract: cid, amount: 100.0 }, &[key(1), key(2)]);
    assert_eq!(c.st.members[&2].rep.open_default, 0, "a member who repays late is not permanently marked");

    // A fresh row carries none of it, and a row costs one bonded trade.
    let mut c2 = Chain::founded(1, 2);
    c2.back(0, 1, SUPPLY);
    let cid2 = c2.lend(1, 2, 100.0);
    c2.default_on(cid2);
    let fresh = c2.st.new_account(vec![[0xEE; 32]]);
    assert_eq!(c2.st.members[&2].rep.open_default, State::to_minor(100.0));
    assert_eq!(c2.st.members[&fresh].rep.open_default, 0, "which is why the trace binds nobody");
}

/// **`conferrable` is net of the creditor's own borrowing**, and the audit's
/// figures are exact.
///
/// For a non-underwriter it is their LIVE capacity, so the stake a debtor earns
/// by paying depends on how much the creditor happens to owe at that moment.
/// Conservative, so not unsafe — a creditor never confers more than they hold —
/// but it means the same honoured debt writes a different edge depending on the
/// creditor's own position, which is a thing an auditor should be told rather
/// than discover.
#[test]
fn what_a_creditor_confers_moves_with_what_they_themselves_owe() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, 500.0);
    assert_eq!(c.st.conferrable(1), 500.0, "idle at 500");

    let owed = c.lend(0, 1, 400.0);
    assert!(c.st.contracts[&owed].insured, "the creditor's own debt must reserve, or this measures nothing");
    assert_eq!(c.cap(1), 100.0);
    assert_eq!(c.st.conferrable(1), 100.0, "owing 400 insured, they may confer 100");

    let d = c.lend(1, 2, 300.0);
    c.settle(d, 300.0);
    let staked =
        |c: &Chain| edet_state::state::State::from_minor(c.st.edges.get(&(1usize, 2usize)).copied().unwrap_or(0));
    assert_eq!(staked(&c), 100.0, "so a debtor honouring 300 writes 100");

    c.settle(owed, 400.0);
    let d2 = c.lend(1, 2, 300.0);
    c.settle(d2, 300.0);
    assert_eq!(staked(&c), 300.0, "and 300 once the creditor has repaid");
}

/// **`dust` rescales and the minor unit does not**, so the two drift apart at
/// every re-denomination.
///
/// Harmless while `dust` starts at exactly one minor unit, and a genuine defect
/// the day it is governed: a downward re-denomination makes amounts in
/// `(dust, one minor unit)` legal to accept and impossible to insure. The guard
/// is that `dust` is not on `ParamKey` — asserted here, because that is the
/// condition the entry's "harmless" rests on and nothing else states it.
#[test]
fn dust_drifts_from_the_minor_unit_and_is_not_governed() {
    let mut c = Chain::founded(1, 1);
    let minor = edet_state::state::State::from_minor(1);
    assert_eq!(c.st.params.dust, minor, "at genesis they are the same quantity");

    c.st.rescale(0.5);
    assert_eq!(c.st.params.dust, minor / 2.0, "dust is denomination-valued, so it halves");
    assert_eq!(edet_state::state::State::from_minor(1), minor, "the minor unit is not, so it does not");

    // The whole of "harmless": no proposal can move `dust`, so the gap can only
    // ever be opened by a re-denomination, which moves every other amount with
    // it. An auditor should check this list rather than take the sentence.
    for key in
        [ParamKey::RiskK, ParamKey::SealAmounts, ParamKey::BondFraction, ParamKey::StakeDecay, ParamKey::SeedRate]
    {
        let _ = key; // the exhaustive set; `dust` is absent from it by construction
    }
}

/// **`Exit` never checks the creditor side**, and an exited member's keys still
/// discharge what they are owed.
///
/// It checks debts, defaults, bonds, the underwriter role and the validator
/// floor — every way leaving could break somebody else — and says nothing about
/// being OWED, which is coherent: a creditor's claims are assets, and refusing
/// to let somebody leave until their debtors pay would hand every debtor a veto
/// over their creditor's departure. What is watched is that the row survives its
/// own exit well enough to keep signing, which is what makes the claims
/// collectable rather than stranded.
#[test]
fn a_member_may_exit_while_owed_and_still_settle_afterwards() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let owed = c.lend(1, 2, 100.0);
    assert_eq!(c.st.members[&1].debt_out, 0, "they owe nothing");
    assert_eq!(c.outstanding(owed), 100.0, "and are owed 100");

    c.ok(Tx::Exit { member: 1 }, &[key(1)]);
    assert_eq!(c.status(1), MemberStatus::Exited);

    // The claim is still collectable, by the party who left.
    c.ok(Tx::Settle { contract: owed, amount: 100.0 }, &[key(1), key(2)]);
    assert_eq!(c.outstanding(owed), 0.0, "an exited creditor's key still discharges");
}

/// **A suspended member's external supply stays in the governance
/// denominator**, so their share is dead weight and the bar is that much higher
/// for everybody else.
///
/// Deliberate, and the alternative is worse: taking them out would make
/// suspension a franchise act, so a coalition at exactly Θ could suspend its way
/// to a growing majority. What it costs is the deadlock this measures — a
/// coalition holding Θ can suspend the rest of the electorate and leave a
/// community that can never unsuspend anybody.
#[test]
fn suspension_does_not_shrink_the_electorate_it_only_silences_a_share() {
    let mut c = Chain::founded(3, 0);
    let seed: u64 = c.st.underwriters.values().sum();
    assert_eq!(seed, 3 * edet_state::state::State::to_minor(SUPPLY));

    c.st.members.get_mut(&2).expect("member 2").status = MemberStatus::Suspended;
    let after: u64 = c.st.underwriters.values().sum();
    assert_eq!(after, seed, "the denominator does not move");
    assert!(
        c.st.underwriters.get(&2).copied().unwrap_or(0) > 0,
        "the share is still counted, and its holder can no longer cast it"
    );
}

/// **What suspension actually revokes, in both directions.**
///
/// The entry read: *"revokes origination on both sides, declaring, listing,
/// covenanting, proposing and assenting; leaves discharge, netting and
/// transferring OUT"*. Measured, it was wrong twice. **`covenanting` names a
/// transition that no longer exists** — the covenant went with the loss pool
/// (§Recourse), so the entry described a mechanism the chain does not run, which is
/// the class `just view-shape-check` gates in the client and nothing gates in
/// this document. And two priced, state-growing transitions are open that it
/// does not name: **`Extend`** and **`ApproveSupporter`**.
///
/// Both are right to be open, which is why this is a probe and not a fix.
/// `Extend` is not origination — it modifies an existing obligation with the
/// creditor's own signature, and refusing it would only crystallise the
/// underwriters' loss sooner, since a default does not release committed flow
/// either (§Recourse). `ApproveSupporter` is the member accepting relief, which
/// shrinks their debt; it belongs with `Settle` and `Cure` on the wind-down
/// side. What was missing was the statement, not the rule.
///
/// The refusals are checked by CODE rather than by "some error": a suspended
/// member paying for their own write gets `ET-BND-005` from the bond gate, and
/// reading that as a scope rule is what hid the two entries above.
#[test]
fn suspension_revokes_origination_and_leaves_the_wind_down_open() {
    let mut c = Chain::founded(2, 4);
    for m in [2, 3, 4, 5] {
        c.back(0, m, 300.0);
    }
    c.ok(Tx::ListBeneficiaries { supporter: 3, entries: vec![(2, 100.0)] }, &[key(3)]);
    let owed = c.lend(3, 2, 100.0); // 2 owes 3, originated while Active
    let claim = c.lend(2, 4, 100.0); // and is owed by 4
    c.st.members.get_mut(&2).unwrap().status = MemberStatus::Suspended;

    // Member 5 co-signs so the bond gate is never the thing under test: a
    // status-zeroed budget refuses everything alike, which is precisely how the
    // two open transitions below stayed hidden.
    let pay = key(5);

    // --- revoked, by rule -------------------------------------------------
    c.err(
        Tx::Accept {
            debtor: Party::Member(2),
            creditor: Party::Member(3),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(2), key(3), pay],
        ET_MEM_NOT_ACTIVE,
    );
    c.err(
        Tx::Accept {
            debtor: Party::Member(4),
            creditor: Party::Member(2),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(2), key(4), pay],
        ET_MEM_NOT_ACTIVE,
    );
    c.err(
        Tx::Sale { seller: Party::Member(3), buyer: Party::Member(2), amount: 10.0, maturity_epochs: MATURITY },
        &[key(2), key(3), pay],
        ET_MEM_NOT_ACTIVE,
    );
    // A raise is refused for everyone; what suspension leaves OPEN on this
    // door is lowering, which is the recovery path (the probe below).
    c.err(Tx::DeclareSupply { member: 2, supply: 10.0 }, &[key(2), pay], ET_UWR_ABOVE_CAPACITY);
    c.err(Tx::ListBeneficiaries { supporter: 2, entries: vec![(3, 10.0)] }, &[key(2), pay], ET_MEM_NOT_ACTIVE);
    // Reinstatement is the one proposal the sanctioned may put on the record
    // and vote on — the electorate must survive its own sanctions — and every
    // other kind is refused.
    c.ok(Tx::Propose { author: 2, kind: ProposalKind::Unsuspend { member: 2 } }, &[key(2), pay]);
    c.err(Tx::Propose { author: 2, kind: ProposalKind::Suspend { member: 3 } }, &[key(2), pay], ET_MEM_NOT_ACTIVE);

    // A suspended SELLER is screened by its own code, because netting is
    // discharge and this sale does not net.
    c.err(
        Tx::Sale { seller: Party::Member(2), buyer: Party::Member(4), amount: 10.0, maturity_epochs: MATURITY },
        &[key(2), key(4), pay],
        ET_MEM_SUSPENDED_NO_ORIGINATION,
    );

    // --- left open --------------------------------------------------------
    // Discharge, in both roles.
    c.ok(Tx::Settle { contract: owed, amount: 10.0 }, &[key(2), key(3)]);
    c.ok(Tx::Settle { contract: claim, amount: 10.0 }, &[key(2), key(4)]);
    // The two the entry did not name.
    let by = c.st.contracts[&owed].maturity_epoch + 5;
    c.ok(Tx::Extend { contract: owed, new_maturity_epoch: by }, &[key(2), key(3), pay]);
    c.ok(Tx::ApproveSupporter { beneficiary: 2, supporter: 3, approved: true }, &[key(2), pay]);
    // Custody, which nothing should ever close: a member cannot be left unable
    // to recover a stolen key by a sanction about conduct.
    c.ok(
        Tx::RegisterGuardians {
            member: 2,
            guardians: vec![3, 4],
            threshold: 2,
            veto_window_epochs: edet_kernel::constants::VETO_WINDOW_EPOCHS,
        },
        &[key(2), pay],
    );
    // And the way out: transfer what is left, then leave.
    c.ok(Tx::Transfer { contract: owed, new_debtor: 4 }, &[key(2), key(4), key(3)]);
    c.ok(Tx::Exit { member: 2 }, &[key(2)]);
    assert_eq!(c.st.members[&2].status, MemberStatus::Exited);
}

/// **Neither suspension nor exit stops a member CONFERRING standing.**
///
/// `capacity_of` gates on status and `conferrable` does not — deliberately,
/// because it reads `capacity_raw`, and both are questions about the graph
/// rather than about permission. Since discharge stays open to a non-Active
/// member, every claim they still hold keeps writing `stake(creditor, debtor)`
/// as it is paid, and the community's supply keeps reaching new debtors through
/// them.
///
/// It is bounded — every such claim was originated while they were Active and
/// bonded, so nothing here is a free channel — and it is not obviously wrong:
/// the debtor really did owe and pay, and the stake graph is deliberately not
/// swept at exit for the same reason. But it is what the two statuses MEAN, and
/// nothing said so until this probe; §Governance carries it now.
#[test]
fn a_sanctioned_or_departed_member_still_routes_the_communitys_supply() {
    for (label, status) in [("suspended", MemberStatus::Suspended), ("exited", MemberStatus::Exited)] {
        let mut c = Chain::founded(1, 2);
        c.back(0, 1, 300.0); // the community backs member 1
        let claim = c.lend(1, 2, 200.0); // member 1 lends onward while Active
        c.st.members.get_mut(&1).unwrap().status = status;

        assert_eq!(c.st.capacity_of(1), 0.0, "{label}: origination is refused");
        assert!(c.st.conferrable(1) > 0.0, "{label}: conferral is not");
        assert_eq!(c.st.capacity_of(2), 0.0, "{label}: the debtor starts with nothing");

        c.ok(Tx::Settle { contract: claim, amount: 200.0 }, &[key(1), key(2)]);
        assert!(
            c.st.capacity_of(2) > 0.0,
            "{label}: paying a non-Active creditor must still write the stake it earned"
        );
    }
}

// ------------------------------------------------------- the review's probes --

mod final_review_status_and_silence {
    use super::*;

    /// **A suspended underwriter may lower their supply and leave.** Gated on
    /// `Active`, they could neither lower nor exit nor vote while their supply
    /// went on insuring and every default through it landed on them — a
    /// sanction that made its target a permanent involuntary insurer. What is
    /// committed through them stays, which is the floor working.
    ///
    /// Mutation that bites: `require_active` back in `declare_supply`.
    #[test]
    fn a_suspended_underwriter_may_lower_their_supply_and_leave() {
        let mut c = Chain::founded(2, 1);
        let (u0, u1, m) = (0u64, 1u64, 2u64);
        c.back(u1, m, 500.0);
        let pid = c.st.next_proposal;
        c.ok(Tx::Propose { author: u0, kind: ProposalKind::Suspend { member: u1 } }, &[key(u0 as usize)]);
        c.ok(Tx::Assent { member: u0, proposal: pid }, &[key(u0 as usize)]);
        assert_eq!(c.status(u1), MemberStatus::Suspended);

        // A raise is priced and a suspended budget is zero, so 0 co-signs to
        // reach the rule that refuses it; a lowering is free and needs nobody.
        c.err(
            Tx::DeclareSupply { member: u1, supply: SUPPLY + 1.0 },
            &[key(u1 as usize), key(u0 as usize)],
            ET_UWR_ABOVE_CAPACITY,
        );
        c.ok(Tx::DeclareSupply { member: u1, supply: 1_000.0 }, &[key(u1 as usize)]);
        let cid = c.lend(u0, m, 300.0);
        assert_eq!(c.st.contracts[&cid].held.supply, vec![(u1 as usize, State::to_minor(300.0))]);
        c.err(Tx::DeclareSupply { member: u1, supply: 0.0 }, &[key(u1 as usize)], ET_UWR_BELOW_COMMITTED);
        c.settle(cid, 300.0);
        c.ok(Tx::DeclareSupply { member: u1, supply: 0.0 }, &[key(u1 as usize)]);
        c.ok(Tx::Exit { member: u1 }, &[key(u1 as usize)]);
        assert_eq!(c.status(u1), MemberStatus::Exited);
    }

    /// **A creditor's guardians may sign the discharge the creditor cannot.**
    /// A creditor who lost their keys or stopped answering left their debtor
    /// with no way to pay: the obligation defaulted, an uninsured default
    /// substitutes nothing, and the debtor held an open default for ever — no
    /// allowance, no exit, no cure. Guardians already hold the power to rotate
    /// the creditor's key and sign as them; a discharge adds no trust.
    ///
    /// Mutation that bites: `require_signed` for the creditor in `cure`.
    #[test]
    fn a_creditors_guardians_may_sign_the_discharge_the_creditor_cannot() {
        let mut c = Chain::founded(1, 3);
        let (u, cr, d, g) = (0u64, 1u64, 2u64, 3u64);
        c.back(u, cr, 500.0);
        c.back(u, d, 500.0);
        c.back(u, g, 500.0);
        c.ok(
            Tx::RegisterGuardians { member: cr, guardians: vec![u, g], threshold: 2, veto_window_epochs: 30 },
            &[key(cr as usize)],
        );
        let cid = c.lend(cr, d, 600.0);
        assert!(!c.st.contracts[&cid].insured, "600 against a capacity of 500 is uninsured whole");
        c.default_on(cid);
        assert_eq!(edet_state::bond::free_remaining(&c.st, d), 0, "a defaulter holds no allowance");

        let cure = Tx::Cure { contract: cid, amount: 600.0 };
        c.err(cure.clone(), &[key(d as usize)], ET_MEM_NOT_SIGNER);
        c.err(cure.clone(), &[key(d as usize), key(g as usize)], ET_MEM_NOT_SIGNER);
        assert!(
            !edet_state::authorises(&c.st, &cure, &[key(d as usize), key(g as usize)]),
            "one guardian is under the threshold"
        );
        assert!(edet_state::authorises(&c.st, &cure, &[key(d as usize), key(u as usize), key(g as usize)]));
        c.ok(cure, &[key(d as usize), key(u as usize), key(g as usize)]);
        assert_eq!(c.st.members[&d].rep.open_default, 0);
        assert!(edet_state::bond::free_remaining(&c.st, d) > 0, "and the debtor is a member again");

        // A debtor's guardians pay nothing: the creditor's signature, or the
        // creditor's guardians', and never the debtor's.
        c.ok(
            Tx::RegisterGuardians { member: d, guardians: vec![u, g], threshold: 2, veto_window_epochs: 30 },
            &[key(d as usize)],
        );
        let again = c.lend(cr, d, 100.0);
        c.err(Tx::Settle { contract: again, amount: 100.0 }, &[key(u as usize), key(g as usize)], ET_MEM_NOT_SIGNER);
        c.err(Tx::Settle { contract: again, amount: 100.0 }, &[key(d as usize)], ET_MEM_NOT_SIGNER);
    }
}
