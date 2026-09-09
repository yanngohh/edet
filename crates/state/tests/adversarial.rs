//! Regression gates for the attacks this model has actually carried.
//!
//! Each test here pins an attack that once SUCCEEDED, names the property that
//! breaks if it regresses, and drives it through the transition function — the
//! codes alone do not say why a refusal matters.
//!
//! An attack on a mechanism this alphabet does not have is not a regression
//! gate, it is a fossil — so nothing here attacks sponsors, vouches,
//! attestations, promotion, a trial channel or a probationary tier. Those are
//! all answers to "who may borrow", and capacity is the answer this model
//! gives.

mod common;

use common::{bonded_dud, key, stranger_key, Chain, MATURITY, SUPPLY};
use edet_kernel::constants as k;
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

// ------------------------------------------------------- C-1: the horizon --

/// A maturity far enough out is not a long loan, it is arithmetic that either
/// panics every validator (`overflow-checks`, the debug default) or wraps to a
/// deadline in the PAST, manufacturing an instantly-defaulted contract. Both
/// were reachable with a trivial amount, so no capacity and no standing were
/// needed to halt the chain.
#[test]
fn an_unbounded_maturity_is_refused_rather_than_overflowing_the_epoch() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 400.0);
    for horizon in [u64::MAX, u64::MAX / 2, k::MAX_HORIZON_EPOCHS + 1] {
        assert_eq!(
            c.apply(
                Tx::Accept {
                    debtor: Party::Member(1),
                    creditor: Party::Member(0),
                    amount: 40.0,
                    maturity_epochs: horizon,
                    arb: None
                },
                &[key(0), key(1)],
            ),
            Err(Error(ET_CTR_MATURITY_TOO_LONG)),
            "maturity {horizon} must be refused, not added"
        );
    }
    assert!(
        c.st.contracts.values().all(|x| x.maturity_epoch < u64::MAX),
        "no contract may exist carrying a saturated deadline"
    );
}

/// The `Sale` cascade opens obligations through the same addition and had the
/// same hole.
#[test]
fn the_sale_cascade_refuses_an_unbounded_maturity_too() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let before = c.st.contracts.len();
    assert_eq!(
        c.apply(
            Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 10.0, maturity_epochs: u64::MAX },
            &[key(1), key(2)]
        ),
        Err(Error(ET_CTR_MATURITY_TOO_LONG))
    );
    assert_eq!(c.st.contracts.len(), before, "a refused sale must book nothing");
}

/// The ceiling must not have closed the door on ordinary credit terms.
#[test]
fn a_maturity_at_the_ceiling_is_still_accepted() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 400.0);
    c.ok(
        Tx::Accept {
            debtor: Party::Member(1),
            creditor: Party::Member(0),
            amount: 40.0,
            maturity_epochs: k::MAX_HORIZON_EPOCHS,
            arb: None,
        },
        &[key(0), key(1)],
    );
}

/// A maturity below the floor is refused too. The floor is what gives
/// `MarkExpired` a window worth waiting out: a one-epoch loan is a default
/// crank armed the moment it is signed.
#[test]
fn a_maturity_below_the_floor_is_refused() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, 400.0);
    c.err(
        Tx::Accept {
            debtor: Party::Member(1),
            creditor: Party::Member(0),
            amount: 40.0,
            maturity_epochs: 1,
            arb: None,
        },
        &[key(0), key(1)],
        ET_CTR_MATURITY_TOO_SHORT,
    );
}

// ------------------------------------------------------ H-2: the transfer --

/// `Transfer` moves who owes, not when it is due. The creditor need not sign
/// when the successor is insured, so re-dating the claim let two colluding
/// members hand a debt back and forth and push its maturity out of reach
/// forever — `MarkExpired` never fires, so no default is ever recorded and no
/// remedy is ever reachable.
#[test]
fn a_transfer_carries_the_original_maturity_rather_than_minting_a_fresh_clock() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    let cid = c.lend(3, 1, 300.0);
    let due = c.st.contracts[&cid].maturity_epoch;

    c.goto(MATURITY - 1);
    let successor = c.st.next_contract;
    c.ok(Tx::Transfer { contract: cid, new_debtor: 2 }, &[key(1), key(2)]);
    assert_eq!(
        c.st.contracts[&successor].maturity_epoch, due,
        "the successor inherits the creditor's original due date"
    );

    c.default_on(successor);
}

/// A transfer is a discharge for the outgoing debtor, and every discharge must
/// be authorised by the party who loses if it is wrong. The creditor loses
/// something only when the claim stops being one the community stands behind —
/// so their signature is required exactly when the successor would be
/// uninsured, and not otherwise. Which case it is, is a fact about the graph.
#[test]
fn a_transfer_that_drops_the_insurance_needs_the_creditor() {
    let mut c = Chain::founded(1, 3);
    c.back(0, 1, SUPPLY);
    c.back(0, 3, SUPPLY);
    // Member 2 is a key nobody has backed, so a claim moved onto them would
    // stop being insured.
    assert_eq!(c.cap(2), 0.0);
    let cid = c.lend(3, 1, 300.0);
    assert!(c.st.contracts[&cid].insured);

    c.err(Tx::Transfer { contract: cid, new_debtor: 2 }, &[key(1), key(2)], ET_MEM_NOT_SIGNER);
    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Active, "the refused transfer moved nothing");

    // With the creditor's consent it goes through, uninsured.
    let successor = c.st.next_contract;
    c.ok(Tx::Transfer { contract: cid, new_debtor: 2 }, &[key(1), key(2), key(3)]);
    assert!(!c.st.contracts[&successor].insured, "the community no longer stands behind it");
}

// ------------------------------------------------------ H-3: the envelope --

/// The signed digest covers `(tx, nonce, not_after_epoch)` and deliberately
/// not `signers`, so an envelope with a co-signature stripped is a different
/// wire message with the SAME replay id — and the pending pool gossips
/// partially-signed envelopes network-wide by design, so the under-signed copy
/// exists before the complete one. Burning the id on it let anyone who saw a
/// request permanently kill the genuine transaction for free, and bill its
/// bond to a party who never submitted anything.
#[test]
fn an_unauthorised_envelope_does_not_burn_the_genuine_transactions_id() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let cid = c.lend(2, 1, 300.0);

    let tx = Tx::Settle { contract: cid, amount: 40.0 };
    let id = c.tx_id();
    let not_after = c.st.epoch + 5;

    // The attacker replays the debtor's half alone, under the genuine id.
    assert_eq!(
        c.apply_raw(tx.clone(), id, not_after, &[key(1)]),
        Err(Error(ET_MEM_NOT_SIGNER)),
        "an under-signed envelope authorises nothing"
    );

    c.apply_raw(tx, id, not_after, &[key(1), key(2)])
        .expect("PROVEN CLOSED: the genuine transaction is not censored by the stripped copy");
    assert!((c.outstanding(cid) - 260.0).abs() < 1e-9, "and it actually discharged");
}

/// The release is scoped to AUTHORISATION only. A transaction rejected for an
/// economic reason still spends its id, or the deferred replay that
/// record-before-dispatch exists to close would reopen: hold a transaction
/// that fails today, replay the identical signed bytes once it would succeed.
#[test]
fn an_economically_rejected_transaction_still_spends_its_id() {
    let mut c = Chain::founded(1, 1);
    // Thinly backed, so the write GATE admits the transition (the write gate prices a
    // supply raise, and a member with nothing at all cannot pay for one) while
    // the transition itself still fails on its merits. The distinction is the
    // point of this probe: the id must be spent by a rejection at DISPATCH, not
    // only by one at the gate.
    c.back(0, 1, 100.0);
    let tx = Tx::DeclareSupply { member: 1, supply: 1000.0 };
    let id = c.tx_id();
    let not_after = c.st.epoch + 5;
    assert_eq!(
        c.apply_raw(tx.clone(), id, not_after, &[key(1)]),
        Err(Error(ET_UWR_ABOVE_CAPACITY)),
        "a declaration above what the community has put behind you is refused"
    );

    // Later, with standing, the SAME signed bytes must not become applicable.
    c.back(0, 1, SUPPLY);
    assert_eq!(c.cap(1), SUPPLY, "the declaration would now be legal on its merits");
    assert_eq!(
        c.apply_raw(tx, id, not_after, &[key(1)]),
        Err(Error(ET_TX_REPLAY)),
        "the deferred-replay defence is intact"
    );
    assert!(!c.st.underwriters.contains_key(&1));
}

/// **A replay id is durable state, so it must not be free.**
/// `applied_by_expiry` is hashed into the state root and retained for up to
/// `MAX_TX_LIFETIME_EPOCHS`, and `bond_gate`'s own note says what must never be
/// possible: *"submit transitions designed to fail, consume a consensus round
/// and a durable write for each, and pay nothing"*. Recording before the gate
/// was exactly that, in the two branches that charge nothing on the way out.
///
/// Measured under the other ordering: a member at their ceiling writes
/// **200** ids with **0.00** encumbered, and two fresh keys signing an `Accept`
/// between themselves wrote **100** — no account, no standing, no headroom, and
/// the mempool screen the only thing in the way of the second.
#[test]
fn a_refusal_at_the_gate_leaves_no_durable_trace() {
    let mut c = Chain::founded(2, 1);
    c.back(0, 2, 300.0);
    c.tighten();
    c.exhaust(key(2));

    // A member past their ceiling.
    let ids = c.st.applied_count();
    let enc = c.st.members[&2].bond_enc();
    for _ in 0..64 {
        assert_eq!(c.apply(bonded_dud(), &[key(2)]), Err(Error(ET_BOND_EXHAUSTED)));
    }
    assert_eq!(c.st.applied_count(), ids, "64 refusals wrote 64 ids nobody paid for");
    assert!(c.st.members[&2].bond_enc() == enc, "and charged nothing for them");

    // A ring of free keys, which is the same hole without an account in it.
    for i in 0..64u64 {
        assert_eq!(
            c.apply(
                Tx::Accept {
                    debtor: Party::Key(stranger_key(1)),
                    creditor: Party::Key(stranger_key(2)),
                    amount: 10.0 + i as f64,
                    maturity_epochs: MATURITY,
                    arb: None,
                },
                &[stranger_key(1), stranger_key(2)],
            ),
            Err(Error(ET_BOND_NO_PAYER))
        );
    }
    assert_eq!(c.st.applied_count(), ids, "a key belonging to nobody wrote into the state root");
}

/// **The other half of the sentence above `an_unauthorised_envelope_...`, which
/// was prose.** That note says an under-signed envelope "never spends the id,
/// and the bond charged for it is returned"; the id was released and the bond
/// was not. Measured: an `Accept` replayed with the creditor's signature
/// stripped moved the debtor's encumbrance **0.00 → 20.00** and left it there.
///
/// The pending pool gossips partially-signed envelopes by design, so this is
/// reachable by anyone who sees a request: repeat it and the victim's headroom
/// is gone, after which their own honest traffic starts arming
/// `bond_denied_this_epoch` for the forfeiture crank. **Check that a claim's
/// MEASUREMENT exists, not that its wording matches.**
#[test]
fn an_unauthorised_envelope_returns_the_bond_it_was_billed() {
    let mut c = Chain::founded(2, 2);
    c.back(0, 2, 300.0);
    c.back(0, 3, 300.0);
    c.st.params.bond_free_allowance = 0;
    c.st.params.bond_release_epochs = 100;

    let before = c.st.members[&2].bond_enc();
    let tx = Tx::Accept {
        debtor: Party::Member(2),
        creditor: Party::Member(3),
        amount: 10.0,
        maturity_epochs: MATURITY,
        arb: None,
    };
    for _ in 0..16 {
        assert_eq!(c.apply(tx.clone(), &[key(2)]), Err(Error(ET_MEM_NOT_SIGNER)));
    }
    assert!(
        c.st.members[&2].bond_enc() == before,
        "16 stripped envelopes billed a party who never submitted anything: {before:.2} -> {:.2}",
        c.st.members[&2].bond_enc()
    );
    assert!(!c.st.members[&2].bond_denied_this_epoch, "and must not arm the forfeiture crank either");

    // The allowance is the other thing the gate can spend, and it unwinds too.
    let mut c = Chain::founded(2, 2);
    c.back(0, 2, 300.0);
    c.back(0, 3, 300.0);
    let used = c.st.members[&2].bond_free_used;
    assert_eq!(c.apply(tx, &[key(2)]), Err(Error(ET_MEM_NOT_SIGNER)));
    assert_eq!(c.st.members[&2].bond_free_used, used, "a free slot was spent on an envelope that authorised nothing");
}

/// **A permissionless crank that found nothing to do spends nothing**, because
/// there is no signature to defer and it mutated nothing to protect.
///
/// `MarkExpired` and `ForfeitBonds` are free from the schedule BEFORE `due`
/// asks who would pay, so a key belonging to nobody could crank contracts that
/// do not exist: measured, **200 of 200** wrote a replay id into state the root
/// hashes, and `bond::admits` returned true for every one. The deferred-replay
/// policy is about holding somebody's SIGNATURE until it would succeed, and
/// these carry none — anyone may submit a fresh one at any time, which is what
/// makes replaying an old one worth nothing.
#[test]
fn a_crank_that_finds_nothing_to_do_spends_nothing() {
    let mut c = Chain::founded(2, 2);
    c.tighten();
    let ids = c.st.applied_count();
    for i in 0..64u64 {
        assert_eq!(c.apply(Tx::MarkExpired { contract: 10_000 + i }, &[stranger_key(1)]), Err(Error(ET_CTR_UNKNOWN)));
        assert_eq!(c.apply(Tx::ForfeitBonds { member: 2 }, &[stranger_key(1)]), Err(Error(ET_BOND_NOT_SATURATED)));
    }
    assert_eq!(c.st.applied_count(), ids, "an unsigned crank wrote into the state root for nothing");

    // And a crank that DOES something is unaffected — it is the sweep's own
    // path, so this is also the guard on the carve-out not swallowing the work.
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, 300.0);
    let ctr = c.lend(1, 2, 100.0);
    c.default_on(ctr);
    assert_eq!(c.st.contracts[&ctr].status, ContractStatus::Expired);
}

// ------------------------------------------------------ H-4: the sanction --

/// `Unsuspend` must not set `Active` unconditionally. There is no ladder
/// for it to climb — `Active` is the resting state of every account that has
/// not been suspended or left — but there is still a status it must not
/// manufacture: a member who WITHDREW. Resurrecting an exited account through
/// the sanction-lifting door would hand a coalition a way to re-enrol somebody
/// who left, over their own signature's objection.
#[test]
fn lifting_a_suspension_cannot_resurrect_a_member_who_left() {
    let mut c = Chain::founded(4, 1);
    c.back(0, 4, 100.0);
    c.ok(Tx::Exit { member: 4 }, &[key(4)]);
    assert_eq!(c.status(4), MemberStatus::Exited);

    enact(&mut c, ProposalKind::Unsuspend { member: 4 });
    assert_eq!(
        c.status(4),
        MemberStatus::Exited,
        "PROVEN CLOSED: governance may lift a sanction it imposed, and nothing else"
    );
}

/// Suspension revokes ORIGINATION, on both sides of a trade: extending credit
/// is originating just as much as accepting it.
#[test]
fn a_suspended_member_cannot_originate_credit_on_either_side() {
    let mut c = Chain::founded(4, 2);
    c.back(0, 4, SUPPLY);
    c.back(0, 5, SUPPLY);
    enact(&mut c, ProposalKind::Suspend { member: 4 });
    assert_eq!(c.status(4), MemberStatus::Suspended);

    c.err(
        Tx::Accept {
            debtor: Party::Member(4),
            creditor: Party::Member(5),
            amount: 40.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(4), key(5)],
        ET_MEM_NOT_ACTIVE,
    );
    c.err(
        Tx::Accept {
            debtor: Party::Member(5),
            creditor: Party::Member(4),
            amount: 40.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(4), key(5)],
        ET_MEM_NOT_ACTIVE,
    );
}

// ------------------------------------------------------ H-5: key custody --

/// Replacing a guardian set withdraws the authority any pending rotation was
/// made under. Leaving the request standing inverted the victim's only
/// defence: revoking compromised guardians and installing trusted ones with a
/// SHORTER window left the attacker's request in place and measured it against
/// the new window — making the takeover finalizable sooner than doing nothing.
#[test]
fn replacing_guardians_cancels_the_rotation_they_had_opened() {
    let mut c = Chain::founded(1, 4);
    for m in 1..=4 {
        c.back(0, m, SUPPLY);
    }
    c.ok(Tx::RegisterGuardians { member: 1, guardians: vec![2, 3], threshold: 2, veto_window_epochs: 100 }, &[key(1)]);
    c.ok(Tx::RotateRequest { member: 1, new_keys: vec![stranger_key(7)] }, &[key(2), key(3)]);
    assert!(c.st.members[&1].pending_rotation.is_some());

    c.ok(Tx::RegisterGuardians { member: 1, guardians: vec![4, 2], threshold: 2, veto_window_epochs: 30 }, &[key(1)]);
    assert!(
        c.st.members[&1].pending_rotation.is_none(),
        "PROVEN CLOSED: the request does not survive the set that authorised it"
    );

    c.goto(200);
    c.err(Tx::RotateFinalize { member: 1 }, &[], ET_ROT_NO_REQUEST);
    assert!(c.st.key_index.contains_key(&key(1)), "the victim keeps their key");
}

/// A zero veto window makes `RotateVeto` structurally unreachable — request
/// and finalize in one block — so a threshold of compromised guardians is an
/// immediate takeover. `VETO_WINDOW_EPOCHS` is the floor it was written to be,
/// and `MAX_HORIZON_EPOCHS` the ceiling that stops a member locking their own
/// rotation out forever.
#[test]
fn a_veto_window_outside_the_constitutional_range_is_refused() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    for window in [0, 1, k::VETO_WINDOW_EPOCHS - 1, k::MAX_HORIZON_EPOCHS + 1] {
        c.err(
            Tx::RegisterGuardians { member: 1, guardians: vec![2, 3], threshold: 2, veto_window_epochs: window },
            &[key(1)],
            ET_ROT_BAD_WINDOW,
        );
    }
    c.ok(
        Tx::RegisterGuardians {
            member: 1,
            guardians: vec![2, 3],
            threshold: 2,
            veto_window_epochs: k::VETO_WINDOW_EPOCHS,
        },
        &[key(1)],
    );
}

/// `rotate_request` parks the incoming keys in public, replicated state
/// without indexing them until finalize. A key sitting in that gap could be
/// claimed by an ordinary account creation, and the later finalize would
/// overwrite the index entry — leaving one key in two members' `keys`, one of
/// them unresolvable by `member_of_key` for the rest of its life.
///
/// Creation lives inside the trade transitions, so the way in is
/// naming the parked key as a party. `apply::resolve` reaches the same
/// `key_is_claimed` check the deleted transition did, which is the point of
/// re-pointing this probe rather than deleting it with the transition: the gap
/// is a property of `rotate_request`, not of whatever walks into it.
#[test]
fn a_key_awaiting_rotation_cannot_be_claimed_by_a_new_account() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    let pending = stranger_key(7);
    c.ok(Tx::RegisterGuardians { member: 1, guardians: vec![2, 3], threshold: 2, veto_window_epochs: 30 }, &[key(1)]);
    c.ok(Tx::RotateRequest { member: 1, new_keys: vec![pending] }, &[key(2), key(3)]);

    c.err(
        Tx::Accept {
            debtor: Party::Key(pending),
            creditor: Party::Member(2),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[pending, key(2)],
        ET_ADM_DUP_KEY,
    );
    assert_eq!(c.st.member_of_key(&pending), None, "and it is still nobody's until the rotation finalizes");
}

// -------------------------------------------------------- M-4: validators --

/// Consensus sums the validator set's powers into one `u64`
/// (`engine_context::total_voting_power`), so an unbounded power is not a
/// governance concern but a safety one: a large enough value plus any second
/// validator overflows that sum — a panic on every node in debug, and in
/// release a WRAP to a small total, against which a single vote satisfies
/// quorum.
#[test]
fn validator_power_is_bounded_so_the_consensus_sum_cannot_overflow() {
    let mut c = Chain::founded(4, 0);
    c.validator(0);
    c.err(
        Tx::Propose { author: 0, kind: ProposalKind::ValidatorPower { member: 1, power: u64::MAX } },
        &[key(0)],
        ET_VAL_POWER_TOO_HIGH,
    );
    assert!(
        c.st.validators
            .values()
            .copied()
            .try_fold(0u64, |a, p| a.checked_add(p))
            .is_some(),
        "the set's total must stay summable"
    );
}

/// The floor under the validator set, checked on the sharpest of the three
/// removal paths: `Exit` is unilateral, so one member can drive it alone.
#[test]
fn the_last_validator_cannot_be_drained_away() {
    let mut c = Chain::founded(1, 1);
    c.validator(1);
    c.err(Tx::Exit { member: 1 }, &[key(1)], ET_VAL_LAST_VALIDATOR);
    assert!(c.st.validators.contains_key(&1), "the set must stay untouched, not emptied then rejected");
}

// -------------------------------------------------------- M-5: governance --

/// A governed constant that nothing reads is worse than no constant at all:
/// the ledger and the client report the amended value while the machinery goes
/// on using the genesis number. Decay is the one this model can check end to
/// end — amend it, close an epoch, and the stake graph must fall faster.
#[test]
fn amending_the_decay_actually_moves_the_stake_graph() {
    let edge = (0usize, 4usize);
    let mut c = Chain::founded(4, 1);
    c.back(0, 4, SUPPLY);
    let placed = c.st.edges[&edge];

    c.goto(1);
    let slow_drop = placed - c.st.edges[&edge];
    assert!(slow_drop > 0, "the genesis decay must be doing something in the first place");

    let (lo, _) = edet_state::params::Params::safe_range(ParamKey::StakeDecay);
    enact(&mut c, ProposalKind::ParamChange { key: ParamKey::StakeDecay, value: lo });
    assert_eq!(c.st.params.stake_decay, lo, "the ledger records the amendment");

    let before = c.st.edges[&edge];
    c.goto(2);
    let fast_drop = before - c.st.edges[&edge];
    assert!(
        fast_drop > slow_drop,
        "and the epoch crank must read it: {fast_drop} against {slow_drop} at the genesis rate"
    );
}

/// The constitutional bounds are enforced at the proposal, so a value outside
/// them is unproposable rather than merely unadopted. The bond fraction is the
/// sharpest case: bounded ABOVE as strictly as below, because a bond set too
/// high is censorship by arithmetic — every rule still reads as neutral while
/// ordinary members are priced out of writing.
#[test]
fn a_parameter_outside_its_safe_range_is_unproposable() {
    let (lo, hi) = edet_state::params::Params::safe_range(ParamKey::BondFraction);
    assert!(hi > lo && hi <= 0.10, "the ceiling must be bounded and modest, got [{lo}, {hi}]");
    let mut c = Chain::founded(2, 0);
    for value in [hi * 10.0, 0.0] {
        c.err(
            Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::BondFraction, value } },
            &[key(0)],
            ET_GOV_OUT_OF_RANGE,
        );
    }
}

/// `apply` has no rollback, so a handler that mutates and then returns `Err`
/// leaves the mutation on the ledger while the transaction is reported failed.
/// A member told their assent had been refused had it banked anyway, and a
/// later assent could enact the change on a support set including one nobody
/// consented to leaving there.
#[test]
fn an_assent_refused_by_the_cooldown_is_not_recorded() {
    let mut c = Chain::founded(4, 0);
    enact(&mut c, ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 });
    assert_eq!(c.st.params.risk_k, 1.0);

    // Again, inside the cooldown.
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.5 } }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert!(!c.st.proposals[&pid].enacted, "one underwriter of four is below the bar");

    c.err(Tx::Assent { member: 1, proposal: pid }, &[key(1)], ET_GOV_COOLDOWN);
    assert!(
        !c.st.proposals[&pid].assents.contains(&1),
        "PROVEN CLOSED: a transaction reported as failed left no trace on the ledger"
    );
    assert_eq!(c.st.params.risk_k, 1.0, "and nothing was enacted");
}

/// A proposal from an account nobody has backed is a free signature, and free
/// signatures decide nothing here. The floor is the same "has something to
/// lose" test the write surface uses, so it excludes free keys by arithmetic
/// rather than by a quota.
///
/// Two locks, and the second is the one worth pinning. Alone, the free key is
/// stopped by the BOND gate before `propose` is ever reached — it has no
/// headroom, so it cannot write at all. Put a well-backed member in the
/// envelope and the bond lands on them instead, which is exactly the case the
/// establishment floor exists for: somebody else may pay for your traffic,
/// nobody may lend you standing to govern with.
#[test]
fn an_unbacked_key_cannot_propose_even_when_somebody_else_pays() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 2, SUPPLY);
    assert_eq!(c.st.conferrable(1), 0.0, "member 1 is a key nobody has backed");

    let kind = ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 };
    c.err(Tx::Propose { author: 1, kind: kind.clone() }, &[key(1)], ET_BOND_EXHAUSTED);
    c.err(Tx::Propose { author: 1, kind }, &[key(1), key(2)], ET_GOV_NOT_ESTABLISHED);
    assert!(c.st.proposals.is_empty(), "neither refusal may leave a proposal behind");
}

// ------------------------------------------------------- P-4: the reader --

/// Cross-pin with `ui/src/lib/proof.ts` and its vitest
/// (`ui/src/lib/__tests__/proof.test.ts`, the `RUST_*` constants).
///
/// The client verifies inclusion proofs by reimplementing `root.rs`'s hashing
/// byte for byte, and a reimplementation checked only against itself proves
/// nothing — both sides can be wrong the same way. So both pin the SAME fixed
/// vector: a change to the leaf preimage, the salt derivation, the fold or the
/// section tags breaks a test on whichever side changed, rather than silently
/// only the TypeScript one.
///
/// The state is the one `cargo run -p edet-state --example proof_fixture`
/// emits: five founding underwriters at 2500, member `i` holding key
/// `[i+1; 32]`, on the dev root salt.
///
/// **What this test adds over the vitest is a failure on the RUST side.** A
/// change here breaks a Rust test, so the drift is caught by whoever caused
/// it; `just proof-fixture-check` then re-derives the client's copy. Without
/// this half, a format change failed nothing at all — which is how the client
/// came to reject every proof the node served while both suites stayed green.
#[test]
fn a_member_proof_matches_the_vector_the_client_verifier_pins() {
    use edet_state::root;
    use edet_state::state::State;

    fn hexs(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    let mut st = State::default();
    for i in 0..5u8 {
        st.add_underwriter(vec![[i + 1; 32]], 2500.0).expect("founding underwriter");
    }

    assert_eq!(
        root::Section::ALL.len(),
        7,
        "the top tree's leaf count is part of the format: `SECTIONS` in ui/src/lib/proof.ts must match"
    );
    assert_eq!(
        hexs(&root::state_root(&st).expect("root")),
        "630afe60f946c5e96db22de7be3e815753e2541419dc70ddc978b314e7ec8d25",
        "RUST_ROOT in proof.test.ts"
    );

    let p = root::prove_member(&st, 2).expect("prove").expect("member 2 is in the section");
    assert_eq!((p.index, p.leaf_count), (2, 5));
    assert_eq!(hexs(&p.key), "0000000000000002");
    assert_eq!(
        hexs(&p.leaf_salt),
        "777242b699f5997858dceb216c686d01c0d766a21942a58d10fef968f9b5742b",
        "RUST_MEMBER_2_OF_5.leaf_salt"
    );
    assert_eq!(
        p.section_path.iter().map(|h| hexs(h)).collect::<Vec<_>>(),
        vec![
            "de93f7e4fc9b745907e8ddc11f92a9345f0fa5fa733df3c5a38a64cbb6787fa8",
            "5db2e061c36fed6c6c67cb93a71ef23ba6819d806158915058a75bc32aec8371",
            "7ec121c0b7ee97981bf90e9bead6bebdc32bf7298a1989090882bc53d2cd50bf",
        ],
        "RUST_MEMBER_2_OF_5.section_path — the half that breaks, and the half a \
         leaf-only pin is blind to"
    );
    assert!(root::verify(&root::state_root(&st).expect("root"), &p), "and it verifies on this side too");

    // The section whose TAG (5) and top-tree POSITION (4) differ: a verifier
    // that derived one from the other passes every members-section fixture
    // and fails only here.
    let s = root::prove(&st, root::Section::Stakes, &2u64.to_be_bytes())
        .expect("prove")
        .expect("every account holds a stake row, empty or not");
    assert_eq!((s.index, s.leaf_count), (2, 5));
    assert_eq!(hexs(&s.value), "0000000000000000", "RUST_STAKES_2_OF_5.value — an account with no out-edge");
    assert_eq!(
        hexs(&s.leaf_salt),
        "7a936ee8dd50a26ad9b6da6e3e8becf4db34bd0c7adb42507e89fe6526f5064f",
        "RUST_STAKES_2_OF_5.leaf_salt"
    );
    assert!(root::verify(&root::state_root(&st).expect("root"), &s));
}

/// **Shedding an insured obligation onto a key with no standing.**
///
/// A `Sale` takes two signatures and no delivery — §Standing's own argument about
/// fabricated volume, applied one transition over — and an account costs
/// nothing to create. So a debtor made a fresh key, sold to it, and walked
/// away: the sale cleared their obligations by routing them to the buyer, and
/// the successor was insured only if the BUYER had headroom. A key with none
/// left the creditor holding an uninsured claim against nothing, while the
/// defaulter's capacity came back in full.
///
/// It is the free-signature bound pointed the wrong way. Everywhere else a
/// free key can only ever OBTAIN nothing; here one could TAKE something —
/// the creditor's recourse — from somebody who never signed. Seven invariants
/// audit every step of it and none of them fires, because total obligation
/// is conserved by the move: it is recourse that is destroyed, and nothing was
/// counting that.
///
/// Closed by bounding the discharge rather than by demanding a signature: an
/// insured claim is cleared only as far as the buyer can carry it insured
/// (`cascade.rs`), which is the rule `Transfer` already applies to the same
/// debtor swap.
#[test]
fn an_insured_obligation_cannot_be_shed_onto_a_free_key() {
    let mut c = Chain::founded(1, 3);
    c.back(0, 1, 500.0);
    let claim = c.lend(3, 1, 200.0);
    assert!(c.st.contracts[&claim].insured, "the creditor is standing on the community's recourse");

    let debt_before = c.st.members[&1].debt_out;
    let cap_before = c.cap(1);
    assert_eq!(c.cap(2), 0.0, "member 2 is the attacker's own fresh key");

    // One fictitious sale. Two signatures, no delivery, nothing moved.
    c.ok(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Member(2), amount: 200.0, maturity_epochs: MATURITY },
        &[key(1), key(2)],
    );

    assert!(c.st.members[&1].debt_out == debt_before, "the debt is not shed");
    assert!((c.cap(1) - cap_before).abs() < 1e-9, "and the capacity it consumed is not handed back");
    assert_eq!(c.st.contracts[&claim].status, ContractStatus::Active, "the creditor's claim stands");
    assert!(c.st.contracts[&claim].insured, "and it is still one the community stands behind");
}

/// The same move through `Transfer` was always refused, and that asymmetry is
/// what named the defect: one economic act, two code paths, opposite consent
/// rules. This pins the half that was already right.
#[test]
fn transfer_refuses_the_uninsured_swap_without_the_creditor() {
    let mut c = Chain::founded(1, 3);
    c.back(0, 1, 500.0);
    let claim = c.lend(3, 1, 200.0);
    assert!(c.st.contracts[&claim].insured);

    c.err(Tx::Transfer { contract: claim, new_debtor: 2 }, &[key(1), key(2)], ET_MEM_NOT_SIGNER);
    c.ok(Tx::Transfer { contract: claim, new_debtor: 2 }, &[key(1), key(2), key(3)]);
}

// ------------------------------------------------------ hollow insurance --

/// **A coalition cannot cause the ledger to issue more insured credit to the
/// accounts it controls than the community's external seed.**
///
/// The construction that produces it satisfies every invariant while the
/// ledger labels 204,800 of credit `insured` against an external seed of 100,
/// which is why the bound is asserted here rather than left to the audit.
///
/// The construction. Twelve joiners, each wash-backed by every earlier joiner
/// (borrow, settle, no delivery: worthless by the cut, which is the point) and
/// each declaring the maximum its own capacity admitted — 100, 100, 200, 400,
/// … 102,400, declared total **204,900**. Each joiner then wash-backs a fresh
/// sybil for its whole supply, and honest creditors lend those sybils 204,800
/// **insured**. Everyone defaults. Substitution hands the honest creditors
/// twelve UNINSURED legs on the joiners, the joiners default on those, and the
/// honest creditors are out 204,800 against a seed of 100.
///
/// **What made it invisible.** The aggregate statement everybody repeated —
/// "the twelve together can owe exactly 100" — is true, and it is about the
/// DECLARERS as a set. The credit went to the sybils, who are not in that set.
/// Ask WHICH PARTY.
///
/// The bound asserted here is the one a creditor can act on: total insured
/// credit outstanding to accounts the coalition controls, at or below the
/// external seed.
#[test]
fn a_coalition_cannot_insure_the_accounts_it_controls() {
    // F = 0 (ext 100). Joiners 1..=12. Sybils 13..=24. Honest creditors 25..=36.
    let mut c = Chain::founded_with(&[100.0], 36);
    c.st.params.bond_free_allowance = 10_000;
    let f: MemberId = 0;
    let joiners: Vec<MemberId> = (1..=12).collect();
    let sybils: Vec<MemberId> = (13..=24).collect();
    let honest: Vec<MemberId> = (25..=36).collect();
    let seed = c.st.external_seed();
    assert_eq!(seed, 100.0);

    c.back(f, joiners[0], 100.0);
    for &h in &honest {
        c.back(f, h, 100.0);
    }

    // The chain. Every wash-backing still succeeds — it is two signatures and
    // a settlement, and refusing it is not the model's business — and every
    // declaration that would turn it into supply is refused.
    for (i, &j) in joiners.iter().enumerate() {
        for &prev in &joiners[..i] {
            let reach = c.st.conferrable(prev);
            if reach > 0.0 {
                c.back(prev, j, reach);
            }
        }
        let cap = c.st.capacity_raw(j);
        c.err(Tx::DeclareSupply { member: j, supply: cap }, &[key(j as usize)], ET_UWR_ABOVE_CAPACITY);
    }
    assert_eq!(c.st.external_seed(), seed, "the declared total was 204,900 here; the roll is the seed");
    assert!(joiners.iter().all(|j| !c.st.underwriters.contains_key(j)), "not one joiner holds a supply");

    // The sybils, backed by the joiners for everything the joiners can confer.
    for (&j, &sy) in joiners.iter().zip(&sybils) {
        let reach = c.st.conferrable(j);
        if reach > 0.0 {
            c.back(j, sy, reach);
        }
    }

    // Now the issuance, which is the quantity the finding is about: honest
    // creditors lend each sybil everything the ledger will insure.
    let mut insured_to_sybils = 0.0;
    for (i, &sy) in sybils.iter().enumerate() {
        let cap = c.cap(sy);
        if cap <= 0.0 {
            continue;
        }
        let cid = c.lend(honest[i], sy, cap);
        if c.st.contracts[&cid].insured {
            insured_to_sybils += cap;
        }
    }
    assert!(
        insured_to_sybils <= seed + 1e-9,
        "the coalition's sybils drew {insured_to_sybils} of insured credit against a seed of {seed}"
    );

    // And the loss, which is the same bound one default later. The whole
    // coalition walks; what the honest creditors cannot recover is capped by
    // the seed rather than by 2,048x it.
    c.goto(MATURITY + 2);
    c.goto(2 * MATURITY + 4);
    let dead: u64 =
        c.st.contracts
            .values()
            .filter(|k| honest.contains(&k.creditor))
            .filter(|k| matches!(k.status, ContractStatus::Expired))
            .map(|k| k.outstanding)
            .sum();
    assert!(dead <= State::to_minor(seed), "unrecoverable loss {dead} against a seed of {seed}");
}

/// **A debtor cannot launder a real debt through a sybil that a hollow
/// underwriter makes look insured.**
///
/// The sharper half, because it needs no coalition
/// and no default to profit. `Transfer` asks the creditor's consent only when
/// the successor would be UNINSURED (`transfer`), which is the right rule
/// against the right measure. Against a hollow one it was a door: D, honestly
/// backed for 2,500, borrowed 2,500 insured from X and transferred the claim
/// to a sybil S backed by a hollow underwriter J — signed by D and S alone,
/// because S "is insured". D then owed nothing, had its whole standing back,
/// recorded no default, and borrowed the same 2,500 again from Y. When S
/// defaulted, X's recourse was an uninsured claim on J, who had put up nothing.
///
/// The fix is upstream of the consent rule, which is why the rule is unchanged:
/// J cannot become an underwriter by declaring against what the community
/// conferred, so the flow behind S is the SEED's, and the swap is insured by
/// the same external underwriter that insured D. Three things follow, and all
/// three are what makes the swap harmless — the creditor's recourse is a real
/// external underwriter, the community's total exposure does not move, and the
/// debtor gets NO standing back to repeat with.
#[test]
fn a_debt_cannot_be_laundered_through_a_hollow_backed_sybil() {
    let mut c = Chain::founded_with(&[2500.0], 5);
    c.st.params.bond_free_allowance = 10_000;
    let (f, d, x, j, s, y) = (0u64, 1u64, 2u64, 3u64, 4u64, 5u64);
    for m in [d, x, y, j] {
        c.back(f, m, 2500.0);
    }
    // J's declaration against its own capacity is the hollow supply, refused.
    c.err(Tx::DeclareSupply { member: j, supply: 2500.0 }, &[key(j as usize)], ET_UWR_ABOVE_CAPACITY);
    assert!(!c.st.underwriters.contains_key(&j), "J is not an underwriter, and cannot make itself one");
    c.back(j, s, 2500.0);

    let loan = c.lend(x, d, 2500.0);
    assert!(c.st.contracts[&loan].insured);
    assert_eq!(c.cap(d), 0.0);

    // The swap still takes two signatures — the successor IS insured — and the
    // question is what it is insured BY.
    c.ok(Tx::Transfer { contract: loan, new_debtor: s }, &[key(d as usize), key(s as usize)]);
    let successor =
        c.st.contracts
            .values()
            .find(|k| k.debtor == s && k.creditor == x && matches!(k.status, ContractStatus::Active))
            .expect("the claim moved to S")
            .clone();
    assert!(successor.insured);
    assert_eq!(
        successor.held.supply.iter().map(|&(u, _)| u as MemberId).collect::<Vec<_>>(),
        vec![f],
        "the arc behind it is the founder's ceremony-seated supply, not a promise on a promise"
    );

    // And the community's exposure did not move, which is the whole of it:
    // the seed carries 2,500 before the swap and 2,500 after, so D's discharge
    // bought the coalition no second helping.
    assert_eq!(c.st.committed_total(), 2500.0);
    assert_eq!(c.st.members[&d].debt_out, 0, "D owes nothing — a transfer is a discharge");
    assert_eq!(c.cap(d), 0.0, "and gets no standing back, because the seed is where it was");
    let second = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(d),
            creditor: Party::Member(y),
            amount: 2500.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(y as usize), key(d as usize)],
    );
    assert!(!c.st.contracts[&second].insured, "D may borrow again, and Y carries it alone — which Y can see");

    // When S defaults, X's recourse is the underwriter who actually put
    // something up, rather than an uninsured claim on J.
    c.goto(MATURITY + 2);
    let recourse: Vec<&Contract> =
        c.st.contracts
            .values()
            .filter(|k| k.creditor == x && k.outstanding > 0)
            .collect();
    assert_eq!(recourse.len(), 1, "one claim, substituted onto the underwriter behind it");
    assert_eq!(recourse[0].debtor, f, "and the debtor of it is the founder, not the hollow J");
}

// ------------------------------------------------------- the arbitration panel --

/// **A minority that attests first does not decide the award.**
///
/// Minting at QUORUM makes the panel a race. On a panel of sixteen with a
/// quorum of nine, five colluding arbiters attest the maximum before four
/// honest ones attest zero: the median of those nine is the maximum, it mints,
/// and the remaining seven are refused as too late. Five of sixteen decide,
/// and the panel both parties agreed to is a formality after the ninth
/// signature. A median is a statement about a SET; taking it over whichever
/// prefix arrived first measures the race.
///
/// The award is the median over every attestation the window received, minted
/// by the epoch sweep at the window's close, so the quorum is the floor for
/// minting at all rather than the trigger.
#[test]
fn a_minority_that_attests_first_does_not_decide_the_award() {
    let mut c = Chain::founded_with(&[2500.0], 18);
    let (f, seller, buyer) = (0u64, 1u64, 2u64);
    c.back(f, seller, 2500.0);
    c.back(f, buyer, 2500.0);
    let arbiters: Vec<MemberId> = (3..=18).collect();
    for &a in &arbiters {
        // Every arbiter has to be able to write, which is a standing they earn
        // like anybody else.
        c.back(f, a, 100.0);
    }
    let window = MATURITY - 10;
    let terms = ArbTermsWire {
        arbiters: arbiters.iter().copied().collect(),
        quorum: 9,
        window_epochs: window,
        award_cap: 1000.0,
    };
    let cid = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(buyer),
            creditor: Party::Member(seller),
            amount: 1000.0,
            maturity_epochs: MATURITY,
            arb: Some(terms),
        },
        &[key(buyer as usize), key(seller as usize)],
    );

    // The five who collude, first, at the maximum.
    for &a in &arbiters[..5] {
        c.ok(Tx::ArbAttest { contract: cid, arbiter: a, amount: 1000.0 }, &[key(a as usize)]);
    }
    // Four honest ones bring the count to the quorum. Nothing mints.
    for &a in &arbiters[5..9] {
        c.ok(Tx::ArbAttest { contract: cid, arbiter: a, amount: 0.0 }, &[key(a as usize)]);
    }
    assert!(!c.st.contracts[&cid].arb_awarded, "a quorum is a floor, not a trigger");

    // And the other seven are still heard, which is the whole of the fix.
    for &a in &arbiters[9..] {
        c.ok(Tx::ArbAttest { contract: cid, arbiter: a, amount: 0.0 }, &[key(a as usize)]);
    }
    assert_eq!(c.st.contracts[&cid].arb_attestations.len(), 16, "every seat on the panel attested");

    c.goto(window + 1);
    let award: Vec<&Contract> =
        c.st.contracts
            .values()
            .filter(|k| k.debtor == seller && k.creditor == buyer)
            .collect();
    assert!(award.is_empty(), "the median of five 1000s and eleven zeros is zero, so nothing is minted");
    assert!(c.st.contracts[&cid].arb_awarded, "and the panel is resolved, so a late attestation is refused");
}

/// **The median is over the ATTESTATIONS, not over the panel**, so the quorum
/// two parties consent to is as much of the terms as the panel is.
///
/// An arbiter who does not attest hands the median to whoever did. On a panel
/// of sixteen with a quorum of three, two colluding arbiters at the ceiling
/// and one honest arbiter at zero make the median the ceiling: two of sixteen
/// decide the whole award, and the thirteen who stayed silent are the reason.
/// Nothing forces an arbiter to answer, and nothing can — an attestation is a
/// judgement, and a panel that could be compelled would be a panel that could
/// be conscripted.
///
/// So a quorum below a majority of the panel is a MINORITY LEVER, and the
/// pair to consent to is the panel AND the quorum together. It is not the
/// race the award's timing closes: every attestation the window received is in
/// this median, and the same two colluders decide nothing once a majority
/// answers (`a_minority_that_attests_first_does_not_decide_the_award`).
#[test]
fn a_quorum_below_a_majority_of_the_panel_is_decided_by_whoever_answers() {
    let mut c = Chain::founded_with(&[2500.0], 18);
    let (f, seller, buyer) = (0u64, 1u64, 2u64);
    c.back(f, seller, 2500.0);
    c.back(f, buyer, 2500.0);
    let arbiters: Vec<MemberId> = (3..=18).collect();
    for &a in &arbiters {
        c.back(f, a, 100.0);
    }
    let window = MATURITY - 10;
    let terms = ArbTermsWire {
        arbiters: arbiters.iter().copied().collect(),
        // Three of sixteen, which reads as a modest floor and is the lever.
        quorum: 3,
        window_epochs: window,
        award_cap: 1000.0,
    };
    let cid = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(buyer),
            creditor: Party::Member(seller),
            amount: 1000.0,
            maturity_epochs: MATURITY,
            arb: Some(terms),
        },
        &[key(buyer as usize), key(seller as usize)],
    );

    // Two colluders at the ceiling, one honest arbiter at nothing, thirteen
    // silent.
    for &a in &arbiters[..2] {
        c.ok(Tx::ArbAttest { contract: cid, arbiter: a, amount: 1000.0 }, &[key(a as usize)]);
    }
    c.ok(Tx::ArbAttest { contract: cid, arbiter: arbiters[2], amount: 0.0 }, &[key(arbiters[2] as usize)]);
    assert_eq!(c.st.contracts[&cid].arb_attestations.len(), 3, "three of sixteen answered");

    c.goto(window + 1);
    let award: Vec<&Contract> =
        c.st.contracts
            .values()
            .filter(|k| k.debtor == seller && k.creditor == buyer)
            .collect();
    assert_eq!(award.len(), 1, "the median of what the window received is the ceiling, so an award mints");
    assert_eq!(award[0].original, State::to_minor(1000.0), "and it is the whole of it");
    assert!(!award[0].insured, "an award is minted uninsured, whatever it is worth");
}

// ------------------------------------------------------------------ util --

/// Enact `kind` through `Propose`/`Assent`. Two of four founding underwriters
/// carry exactly `THETA_ADOPT` — the numerator is `capacity(assenters) + Σ
/// supply they declared`, and at genesis nobody has backed anybody, so the cut
/// term is zero and the supply term is the whole of it.
fn enact(c: &mut Chain, kind: ProposalKind) {
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
    assert!(c.st.proposals[&pid].enacted, "two of four underwriters must carry a proposal");
}
