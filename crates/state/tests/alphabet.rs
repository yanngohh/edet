//! Every transition in §Medium, and every named rejection code, driven through
//! `apply`.
//!
//! `model.rs` proves the capacity model and `adversarial.rs` pins the attacks;
//! this is the breadth suite — the one that notices when a code becomes
//! unreachable, a guard stops guarding, or a handler mutates on its way to
//! returning an error. Each transition is audited, so nothing here can pass by
//! leaving the ledger inconsistent.

mod common;

use common::{key, stranger_key, Chain, MATURITY, SUPPLY};
use edet_kernel::constants as k;
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

// ------------------------------------------------------------- accounts --

/// **An account is seated by its first TRADE, and that is the whole of
/// creation.**
///
/// It proves nothing but key control, which is the honest content of an
/// account nobody has vouched for — the same proof `OpenAccount` took, arriving
/// through the signature `Accept` already required from both sides. What has
/// changed is who pays for the row: the established counterparty, out of the
/// same allowance that carries every other write they make, rather than nobody.
///
/// The newcomer is worth exactly zero the moment they exist, by arithmetic
/// rather than by rule, and the obligation that seated them is uninsured —
/// which is the first risk §Recourse already named.
#[test]
fn a_first_trade_is_what_seats_an_account() {
    let mut c = Chain::founded(1, 0);
    let newcomer = stranger_key(1);
    let id = c.st.next_member;

    let cid = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Key(newcomer),
            creditor: Party::Member(0),
            amount: 400.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[newcomer, key(0)],
    );
    assert_eq!(c.st.member_of_key(&newcomer), Some(id), "the key resolves to the account its trade seated");
    assert_eq!(c.st.members[&id].keys, vec![newcomer], "holding exactly the key that signed");
    assert_eq!(c.cap(id), 0.0, "and it is worth exactly zero");
    assert!(!c.st.contracts[&cid].insured, "nothing backs a newcomer, so the founder bears it alone");

    // Named by key a second time, the same account answers — naming a key is
    // never a way to displace or duplicate an account that already exists.
    let again = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Key(newcomer),
            creditor: Party::Member(0),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[newcomer, key(0)],
    );
    assert_eq!(c.st.contracts[&again].debtor, id);
    assert_eq!(c.st.members.len(), 2, "and no second row appeared");

    // A key nobody holds and nobody signed for cannot be seated at all, which
    // is the key-control proof.
    c.err(
        Tx::Accept {
            debtor: Party::Key(stranger_key(2)),
            creditor: Party::Member(0),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(0)],
        ET_MEM_NOT_SIGNER,
    );
    assert_eq!(c.st.members.len(), 2, "a refused trade seats nobody");
}

/// **A refused trade leaves no row behind**, on every path that can refuse one.
///
/// `dispatch` has no rollback, so this is a property of WHERE the seating
/// happens rather than of any cleanup: `accept` and `sale` resolve their
/// parties, run every check that can fail, and seat last. A row that appeared
/// beside a failed transition would be an account somebody paid for and nobody
/// can trade with, and it would put the storage term back — growing on
/// transactions that do nothing.
#[test]
fn a_refused_trade_seats_nobody() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    let before = c.st.members.len();
    let fresh = stranger_key(3);

    let refusals: Vec<(Tx, Code)> = vec![
        (
            Tx::Accept {
                debtor: Party::Key(fresh),
                creditor: Party::Member(1),
                amount: 0.0,
                maturity_epochs: MATURITY,
                arb: None,
            },
            ET_CTR_BAD_AMOUNT,
        ),
        (
            Tx::Accept {
                debtor: Party::Key(fresh),
                creditor: Party::Member(1),
                amount: 10.0,
                maturity_epochs: 0,
                arb: None,
            },
            ET_CTR_MATURITY_TOO_SHORT,
        ),
        (
            Tx::Accept {
                debtor: Party::Key(fresh),
                creditor: Party::Member(1),
                amount: 10.0,
                maturity_epochs: k::MAX_HORIZON_EPOCHS + 1,
                arb: None,
            },
            ET_CTR_MATURITY_TOO_LONG,
        ),
        (
            Tx::Accept {
                debtor: Party::Key(fresh),
                creditor: Party::Member(1),
                amount: 10.0,
                maturity_epochs: MATURITY,
                arb: Some(ArbTermsWire { arbiters: [1].into(), quorum: 3, window_epochs: 10, award_cap: 1.0 }),
            },
            ET_ARB_BAD_TERMS,
        ),
        (
            Tx::Sale { seller: Party::Member(1), buyer: Party::Key(fresh), amount: 0.0, maturity_epochs: MATURITY },
            ET_CTR_BAD_AMOUNT,
        ),
        (
            Tx::Sale { seller: Party::Member(1), buyer: Party::Key(fresh), amount: 10.0, maturity_epochs: 0 },
            ET_CTR_MATURITY_TOO_SHORT,
        ),
    ];
    for (tx, code) in refusals {
        c.err(tx, &[fresh, key(1)], code);
        assert_eq!(c.st.members.len(), before, "a refusal must not leave an account behind: {code}");
        assert_eq!(c.st.member_of_key(&fresh), None);
    }

    // And a Suspended seller, which is the one refusal that sits AFTER the
    // netting dry pass — the last point at which `sale` can still say no, and
    // therefore the one the seating order has to clear.
    //
    // The founder co-signs so that the BOND gate is not what refuses this: a
    // suspended member's capacity is zero, so their own headroom is too, and
    // without a third signer to bill the transition would never reach dispatch
    // at all. The bill goes to the first signer in canonical order who CAN
    // pay, which is exactly the established-member-carries-the-newcomer case.
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::Suspend { member: 1 } }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert_eq!(c.status(1), MemberStatus::Suspended);
    c.err(
        Tx::Sale { seller: Party::Member(1), buyer: Party::Key(fresh), amount: 10.0, maturity_epochs: MATURITY },
        &[fresh, key(1), key(0)],
        ET_MEM_SUSPENDED_NO_ORIGINATION,
    );
    assert_eq!(c.st.members.len(), before, "including the refusal past the dry pass");
}

/// **A key is a second name for its holder, never a second identity.**
///
/// Naming a party twice under two encodings is the shape a self-deal would take
/// if `resolve` compared the NAMES instead of what they resolve to: an obligation
/// to oneself is not free money, but it puts a diagonal entry in the contagion
/// operator and tightens the community brake for everyone, which is why the
/// entry path refuses it. Both directions are checked, and so is the case where
/// the key belongs to somebody who may no longer originate — an exited account
/// cannot come back as a fresh one by naming its own key.
#[test]
fn naming_a_key_is_never_a_second_identity_for_its_holder() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let fresh = stranger_key(4);

    // The same member, named both ways, on either side.
    for (debtor, creditor) in [
        (Party::Member(1), Party::Key(key(1))),
        (Party::Key(key(1)), Party::Member(1)),
        (Party::Key(key(1)), Party::Key(key(1))),
        (Party::Key(fresh), Party::Key(fresh)),
    ] {
        c.err(
            Tx::Accept { debtor, creditor, amount: 10.0, maturity_epochs: MATURITY, arb: None },
            &[key(1), fresh, key(0)],
            ET_CTR_SELF_DEAL,
        );
    }
    assert_eq!(c.st.member_of_key(&fresh), None, "and the self-dealing pair seated nobody");

    // An exited account's key still resolves to THEM, so it cannot seat a fresh
    // account with a clean slate — it is refused as the account it is.
    c.ok(Tx::Exit { member: 2 }, &[key(2)]);
    assert_eq!(c.status(2), MemberStatus::Exited);
    let rows = c.st.members.len();
    c.err(
        Tx::Accept {
            debtor: Party::Key(key(2)),
            creditor: Party::Member(1),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(2), key(1)],
        ET_MEM_NOT_ACTIVE,
    );
    assert_eq!(c.st.members.len(), rows, "and no second row appeared for them");
}

/// **The censorship lever is gone, and what replaced it is the bond.**
///
/// Measured with a ledger-wide counter instead: an attacker holding no standing at all
/// opened the full 1024 accounts in one epoch for a total bond spend of ZERO,
/// after which an honest newcomer was refused `ET-ADM-004` and onboarding was
/// shut for everybody until the epoch boundary. Both halves are asserted here:
/// a ring of free keys now seats nothing at all, and the honest newcomer's
/// first trade is admitted whatever anyone else has been doing.
///
/// The refusal is `ET-BND-004` rather than a rate code — there is no rate. What
/// the attacker cannot afford is the write, and the write is priced the same
/// for everybody.
#[test]
fn a_ring_of_free_keys_seats_no_accounts_and_censors_nobody() {
    let mut c = Chain::founded(1, 0);
    let before = c.st.members.len();

    for n in 0..2000u32 {
        let mut a = [0x11u8; 32];
        a[0..4].copy_from_slice(&n.to_be_bytes());
        let mut b = [0x22u8; 32];
        b[0..4].copy_from_slice(&n.to_be_bytes());
        c.err(
            Tx::Accept {
                debtor: Party::Key(a),
                creditor: Party::Key(b),
                amount: 10.0,
                maturity_epochs: MATURITY,
                arb: None,
            },
            &[a, b],
            ET_BOND_NO_PAYER,
        );
    }
    assert_eq!(c.st.members.len(), before, "2000 attempts by keys with no standing seat nothing");

    // And onboarding is open for the newcomer who has somebody to trade with,
    // in the same epoch, with no counter anywhere to have been filled.
    let honest = stranger_key(7);
    c.ok(
        Tx::Accept {
            debtor: Party::Key(honest),
            creditor: Party::Member(0),
            amount: 400.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[honest, key(0)],
    );
    assert_eq!(c.st.member_of_key(&honest), Some(before as MemberId));
}

/// The storage term, stated as what it now is: account rows are bounded by
/// what the seating member could pay for, which is a quantity they had to earn
/// — **and seating does not chain**, which is the half that had to be measured
/// rather than assumed.
///
/// The old bound was a ledger-wide counter of 1024 per epoch that anyone could
/// fill; this one binds against the member doing the seating rather than against
/// everybody else. Measured with the free allowance closed, so every seat is a
/// real bond: the gate engages, and the count of rows never exceeds the count of
/// admitted writes.
///
/// **The write floor's lesson, applied to the seating channel.** That defect was a
/// write channel that DOUBLED per accomplice because `bond_headroom` read a
/// declared supply, and the entry had measured it at one length. So the question
/// here is not "how many rows can one member seat" but "can the rows they seat
/// seat more" — and the answer is no, by construction rather than by cap:
/// `bond_headroom` reads `seed_reach`, a seated row has no incident stakes, so
/// its reach is **0** and it can afford nothing. Measured over a whole wave:
/// `Σ seed_reach` of every row seated is 0, and none of them can seat another.
/// The way to earn the ability is the model's own — settle, and be staked.
#[test]
fn seating_an_account_is_bounded_by_what_the_seater_can_pay_for() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.tighten(); // free allowance 0, so every seat is a real bond
    let rows_before = c.st.members.len();

    let mut seated: Vec<Key> = Vec::new();
    for n in 0..500u32 {
        let mut fresh = [0x33u8; 32];
        fresh[0..4].copy_from_slice(&n.to_be_bytes());
        match c.apply(
            Tx::Accept {
                debtor: Party::Member(1),
                creditor: Party::Key(fresh),
                amount: 1.0,
                maturity_epochs: MATURITY,
                arb: None,
            },
            &[fresh, key(1)],
        ) {
            Ok(()) => seated.push(fresh),
            Err(Error(ET_BOND_EXHAUSTED)) => break,
            other => panic!("unexpected at {n}: {other:?}"),
        }
    }
    assert!(!seated.is_empty(), "an established member can still bring a counterparty in");
    assert!(seated.len() < 500, "but not without limit — the gate engaged at {}", seated.len());
    assert_eq!(c.st.members.len(), rows_before + seated.len(), "one row per admitted write, and not one more");

    // The wave cannot make a second wave.
    let reach: f64 = seated
        .iter()
        .filter_map(|k| c.st.member_of_key(k))
        .map(|id| c.st.seed_reach(id))
        .sum();
    assert_eq!(reach, 0.0, "every seated row must reach zero of the seed, so Σ is 0 — got {reach}");
    for (i, s) in seated.iter().take(10).enumerate() {
        let mut next = [0x44u8; 32];
        next[0..4].copy_from_slice(&(i as u32).to_be_bytes());
        c.err(
            Tx::Accept {
                debtor: Party::Key(*s),
                creditor: Party::Key(next),
                amount: 1.0,
                maturity_epochs: MATURITY,
                arb: None,
            },
            &[*s, next],
            ET_BOND_EXHAUSTED,
        );
    }
    assert_eq!(c.st.members.len(), rows_before + seated.len(), "and the second wave seated nobody");
}

/// The key-list bound, on the only transition that can grow one.
///
/// A bound on a creation transition, where the list is at most eight, is
/// never checked on `RotateRequest`, where it is whatever the transaction
/// carries. Measured unbounded: **100,000 keys** into `pending_rotation`
/// and then into `key_index`, for a headroom spend of **zero** — the guardians'
/// free allowance covered the one bond, and a bond is charged per transaction
/// and says nothing about the size of its payload. Deleting the transition
/// that held the bound would have left it enforcing nothing.
#[test]
fn a_rotation_cannot_write_an_unbounded_key_list() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    c.ok(Tx::RegisterGuardians { member: 1, guardians: vec![2, 3], threshold: 2, veto_window_epochs: 30 }, &[key(1)]);

    let too_many: Vec<Key> = (0..9u8).map(|n| [0xA0 | n; 32]).collect();
    c.err(Tx::RotateRequest { member: 1, new_keys: too_many }, &[key(2), key(3)], ET_ADM_BAD_KEYS);
    c.err(Tx::RotateRequest { member: 1, new_keys: vec![] }, &[key(2), key(3)], ET_ADM_BAD_KEYS);
    assert!(c.st.members[&1].pending_rotation.is_none(), "and nothing was parked on the way to the refusal");

    let at_the_bound: Vec<Key> = (0..8u8).map(|n| [0xB0 | n; 32]).collect();
    c.ok(Tx::RotateRequest { member: 1, new_keys: at_the_bound }, &[key(2), key(3)]);
    assert_eq!(c.st.members[&1].pending_rotation.as_ref().expect("parked").new_keys.len(), 8);
}

/// A transition naming an account that does not exist is refused by name, not
/// by accident. A key that does not exist is a different statement — it seats
/// one — which is exactly why the alphabet carries both.
#[test]
fn a_transition_naming_an_unknown_account_is_refused() {
    let mut c = Chain::founded(1, 0);
    c.err(
        Tx::Accept {
            debtor: Party::Member(999),
            creditor: Party::Member(0),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(0)],
        ET_MEM_UNKNOWN,
    );
}

// ------------------------------------------------------------ contracts --

/// Nobody may owe themselves. It is not free money, but it puts a diagonal
/// entry in the contagion operator and tightens the community brake for
/// everyone, so the entry path refuses it and the audit re-checks the book.
#[test]
fn nobody_may_owe_themselves() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.err(
        Tx::Accept {
            debtor: Party::Member(1),
            creditor: Party::Member(1),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(1)],
        ET_CTR_SELF_DEAL,
    );
}

/// Amounts are bounded on both sides of every discharge: nothing at or below
/// dust, nothing above what is outstanding, nothing non-finite.
#[test]
fn a_discharge_is_bounded_by_what_is_owed() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        c.err(
            Tx::Accept {
                debtor: Party::Member(1),
                creditor: Party::Member(2),
                amount: bad,
                maturity_epochs: MATURITY,
                arb: None,
            },
            &[key(1), key(2)],
            ET_CTR_BAD_AMOUNT,
        );
    }
    let cid = c.lend(2, 1, 100.0);
    c.err(Tx::Settle { contract: cid, amount: 100.01 }, &[key(1), key(2)], ET_CTR_BAD_AMOUNT);
    c.err(Tx::Settle { contract: cid, amount: -1.0 }, &[key(1), key(2)], ET_CTR_BAD_AMOUNT);
    c.settle(cid, 100.0);
}

/// A discharge is authorised by the party who LOSES if it is wrong, which is
/// the creditor. A debtor declaring their own payment is the shape a forged
/// discharge would take, and it hands one account the community's whole
/// ceiling.
#[test]
fn a_debtor_cannot_declare_their_own_payment() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let cid = c.lend(2, 1, 100.0);
    c.err(Tx::Settle { contract: cid, amount: 100.0 }, &[key(1)], ET_MEM_NOT_SIGNER);
    assert_eq!(c.outstanding(cid), 100.0, "and nothing moved");
}

/// The default crank is permissionless and validates against current state: it
/// fires only past maturity, and never against a contract that is not live.
///
/// **And nobody has to call it.** The epoch sweep runs it at every boundary
/// (`apply::sweep_cranks`), so an uncranked default is no longer a thing that
/// can exist — which removes the discretion that would otherwise sit with whoever
/// noticed one first. Calling the transition by hand is still legal and is now
/// a way to be early rather than the only way it happens at all.
#[test]
fn the_default_crank_fires_only_on_a_live_overdue_claim() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let overdue = c.lend(2, 1, 100.0);
    let paid = c.lend(2, 1, 50.0);

    c.err(Tx::MarkExpired { contract: overdue }, &[], ET_CTR_NOT_DUE);
    c.err(Tx::MarkExpired { contract: 12_345 }, &[], ET_CTR_UNKNOWN);
    c.settle(paid, 50.0);

    c.goto(MATURITY + 2);
    assert_eq!(c.st.contracts[&overdue].status, ContractStatus::Expired, "the sweep fired it at the boundary");
    assert_eq!(c.st.contracts[&paid].status, ContractStatus::Settled, "and left the discharged one alone");
    c.err(Tx::MarkExpired { contract: paid }, &[], ET_CTR_BAD_STATE);
    c.err(Tx::MarkExpired { contract: overdue }, &[], ET_CTR_BAD_STATE);
}

/// **There is no window in which a default is due and uncranked**, which is
/// the whole of what the sweep buys. `MarkExpired` becomes unreachable as an
/// accepted transition: the epoch it would first be legal in is the epoch the
/// sweep already ran in, so every hand call after a boundary finds the work
/// done. That is the defect closed — an uncranked default is not merely
/// unlikely, it cannot exist.
#[test]
fn no_epoch_passes_with_a_default_left_uncranked() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    // Three claims falling due in three different epochs.
    let mut ids = Vec::new();
    for (i, d) in [1u64, 2, 3].into_iter().enumerate() {
        let id = c.st.next_contract;
        c.ok(
            Tx::Accept {
                debtor: Party::Member(d),
                creditor: Party::Member(0),
                amount: 100.0,
                maturity_epochs: MATURITY + i as u64,
                arb: None,
            },
            &[key(d as usize), key(0)],
        );
        ids.push(id);
    }

    for epoch in 1..MATURITY + 6 {
        c.goto(epoch);
        for &id in &ids {
            let due = c.st.contracts[&id].maturity_epoch;
            if epoch > due {
                assert_eq!(
                    c.st.contracts[&id].status,
                    ContractStatus::Expired,
                    "contract {id} was due at {due} and is still Active at epoch {epoch}"
                );
                assert_eq!(
                    c.apply(Tx::MarkExpired { contract: id }, &[]),
                    Err(Error(ET_CTR_BAD_STATE)),
                    "and there is nothing left for a hand crank to do"
                );
            }
        }
    }
}

/// `Extend` moves a maturity and needs both signatures, because moving it is
/// the creditor's concession. It only ever moves forward, and never past the
/// horizon.
#[test]
fn extending_a_maturity_is_the_creditors_concession() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let cid = c.lend(2, 1, 100.0);
    let due = c.st.contracts[&cid].maturity_epoch;

    c.err(Tx::Extend { contract: cid, new_maturity_epoch: due + 10 }, &[key(1)], ET_MEM_NOT_SIGNER);
    c.err(Tx::Extend { contract: cid, new_maturity_epoch: due }, &[key(1), key(2)], ET_CTR_BAD_AMOUNT);
    c.err(
        Tx::Extend { contract: cid, new_maturity_epoch: c.st.epoch + k::MAX_HORIZON_EPOCHS + 1 },
        &[key(1), key(2)],
        ET_CTR_MATURITY_TOO_LONG,
    );
    c.ok(Tx::Extend { contract: cid, new_maturity_epoch: due + 10 }, &[key(1), key(2)]);
    assert_eq!(c.st.contracts[&cid].maturity_epoch, due + 10);
}

/// A transfer conserves total debt exactly, records the realized transfer
/// edge — and **writes no stake**.
///
/// The opposite reading — "the outgoing debtor's
/// discharge is real discharge". It is not. §Standing grows the graph only at
/// settlement, and an insured→insured `Transfer` asks the creditor for no
/// signature at all, so a stake written here is an edge no creditor placed.
/// The test had pinned the defect as intended behaviour, which is the reason
/// it went unseen for so long — and is worth remembering the next time a
/// green suite is offered as evidence.
#[test]
fn a_transfer_conserves_debt_and_writes_no_stake() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    let cid = c.lend(3, 1, 400.0);
    let before = c.total_debt();

    let successor = c.st.next_contract;
    c.ok(Tx::Transfer { contract: cid, new_debtor: 2 }, &[key(1), key(2)]);

    assert!((c.total_debt() - before).abs() < 1e-9, "a transfer moves who owes, not how much is owed");
    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Transferred, "the row closes as a transfer");
    assert_eq!(c.st.contracts[&successor].debtor, 2);
    assert!(c.st.members[&2].debt_out == State::to_minor(400.0));
    assert!(!c.st.edges.contains_key(&(3, 1)), "PROVEN: a swap is not a settlement, so it stakes nothing");
    assert!(!c.st.edges.contains_key(&(3, 2)), "and the successor earns nothing by receiving it either");

    // The successor's own settlement is what writes the edge — the one the
    // creditor did accept, against the debtor who actually paid.
    c.settle(successor, 400.0);
    assert!(c.st.edges.contains_key(&(3, 2)), "settlement, and only settlement, grows the graph");

    c.err(Tx::Transfer { contract: cid, new_debtor: 2 }, &[key(1), key(2)], ET_CTR_BAD_STATE);
}

// ----------------------------------------------------------- key custody --

/// The whole rotation: register a guardian set, have a threshold of it open a
/// request, wait out the window, finalize. The veto is the account holder's
/// stop, and the window is the time they have to use it.
#[test]
fn the_guardian_rotation_flow_end_to_end() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    let fresh = stranger_key(9);

    c.err(Tx::RotateRequest { member: 1, new_keys: vec![fresh] }, &[key(2), key(3)], ET_ROT_NO_GUARDIANS);
    c.err(
        Tx::RegisterGuardians { member: 1, guardians: vec![2], threshold: 2, veto_window_epochs: 30 },
        &[key(1)],
        ET_ROT_THRESHOLD,
    );
    c.err(
        Tx::RegisterGuardians { member: 1, guardians: vec![1, 2], threshold: 2, veto_window_epochs: 30 },
        &[key(1)],
        ET_ROT_THRESHOLD,
    );
    c.ok(Tx::RegisterGuardians { member: 1, guardians: vec![2, 3], threshold: 2, veto_window_epochs: 30 }, &[key(1)]);

    c.err(Tx::RotateFinalize { member: 1 }, &[], ET_ROT_NO_REQUEST);
    c.err(Tx::RotateRequest { member: 1, new_keys: vec![fresh] }, &[key(2)], ET_ROT_THRESHOLD);
    c.ok(Tx::RotateRequest { member: 1, new_keys: vec![fresh] }, &[key(2), key(3)]);
    c.err(Tx::RotateFinalize { member: 1 }, &[], ET_ROT_WINDOW_OPEN);

    c.goto(31);
    c.ok(Tx::RotateFinalize { member: 1 }, &[]);
    assert_eq!(c.st.member_of_key(&fresh), Some(1), "the new key resolves");
    assert_eq!(c.st.member_of_key(&key(1)), None, "and the old one is retired");
}

/// The veto is the defence, and it must survive the window it was cast in.
/// It DELETES the request: a finalize after it finds nothing to finalize, a
/// second veto finds nothing to veto, and a fresh request is what the
/// guardians would have to open again.
///
/// Mutation that bites: flag the request instead of deleting it. The second
/// veto is admitted, free, and spends a durable replay id — fifty of fifty
/// were, against one request.
#[test]
fn a_vetoed_rotation_can_never_be_finalized_and_a_veto_consumes_its_request() {
    let mut c = Chain::founded(1, 3);
    for m in 1..=3 {
        c.back(0, m, SUPPLY);
    }
    c.ok(Tx::RegisterGuardians { member: 1, guardians: vec![2, 3], threshold: 2, veto_window_epochs: 30 }, &[key(1)]);
    c.err(Tx::RotateVeto { member: 1 }, &[key(1)], ET_ROT_NO_REQUEST);
    c.ok(Tx::RotateRequest { member: 1, new_keys: vec![stranger_key(9)] }, &[key(2), key(3)]);
    c.err(Tx::RotateVeto { member: 1 }, &[key(2)], ET_MEM_NOT_SIGNER);
    c.ok(Tx::RotateVeto { member: 1 }, &[key(1)]);
    assert!(c.st.members[&1].pending_rotation.is_none(), "the veto consumed the request");
    let ids = c.st.applied_count();
    for _ in 0..8 {
        c.err(Tx::RotateVeto { member: 1 }, &[key(1)], ET_ROT_NO_REQUEST);
    }
    assert!(c.st.applied_count() <= ids + 8, "a refused veto is priced from the allowance or forgotten");

    c.goto(31);
    c.err(Tx::RotateFinalize { member: 1 }, &[], ET_ROT_NO_REQUEST);
    assert_eq!(c.st.member_of_key(&key(1)), Some(1), "the victim keeps their key");

    // The guardians may ask again, and this time nobody vetoes.
    c.ok(Tx::RotateRequest { member: 1, new_keys: vec![stranger_key(9)] }, &[key(2), key(3)]);
    c.goto(62);
    c.ok(Tx::RotateFinalize { member: 1 }, &[]);
    assert_eq!(c.st.member_of_key(&stranger_key(9)), Some(1));
}

// ----------------------------------------------------------- arbitration --

/// A panel whose window closes BEFORE the obligation falls due, which is the
/// ordinary shape: a dispute about delivery is settled before the payment is
/// owed. It no longer has to be — the panel's terms name the parties they
/// bind, so a row that becomes an underwriter's claim at a default keeps its
/// panel and a window that outlives the default still reaches the remedy
/// (`loss.rs`'s probes) — but it is the shape a buyer who means to pay on time
/// wants.
fn panel_terms(arbiters: Vec<MemberId>, cap: f64) -> ArbTermsWire {
    ArbTermsWire { arbiters: arbiters.into_iter().collect(), quorum: 3, window_epochs: MATURITY - 10, award_cap: cap }
}

/// An award is authorised by a panel BOTH parties named, capped and time-boxed
/// before the obligation existed — which is what makes it an authorisation by
/// the party who loses rather than by a third party who does not. The award is
/// the median over every attestation the WINDOW received, capped, minted once
/// at the window's close, as an ordinary restitution debt.
#[test]
fn an_arbitration_award_is_the_capped_median_minted_once() {
    let mut c = Chain::founded(1, 5);
    for m in 1..=5 {
        c.back(0, m, SUPPLY);
    }
    let terms = panel_terms(vec![3, 4, 5], 500.0);
    let cid = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(1),
            creditor: Party::Member(2),
            amount: 400.0,
            maturity_epochs: MATURITY,
            arb: Some(terms),
        },
        &[key(1), key(2)],
    );

    c.err(Tx::ArbAttest { contract: cid, arbiter: 1, amount: 100.0 }, &[key(1)], ET_ARB_NOT_PANEL);
    c.ok(Tx::ArbAttest { contract: cid, arbiter: 3, amount: 100.0 }, &[key(3)]);
    c.err(Tx::ArbAttest { contract: cid, arbiter: 3, amount: 120.0 }, &[key(3)], ET_ARB_ALREADY_ATTESTED);
    c.ok(Tx::ArbAttest { contract: cid, arbiter: 4, amount: 300.0 }, &[key(4)]);

    // Reaching the quorum mints nothing: the window is still open, and every
    // attestation it receives is in the median.
    let minted = c.st.next_contract;
    c.ok(Tx::ArbAttest { contract: cid, arbiter: 5, amount: 200.0 }, &[key(5)]);
    assert!(!c.st.contracts.contains_key(&minted), "a quorum is a floor, not a trigger");
    assert!(!c.st.contracts[&cid].arb_awarded);

    // The sweep mints it when the window closes.
    c.goto(MATURITY - 9);
    let award = &c.st.contracts[&minted];
    assert!(award.original == State::to_minor(200.0), "the median of 100, 300, 200 is 200");
    assert_eq!((award.debtor, award.creditor), (2, 1), "the creditor who failed to deliver owes the debtor");

    // And a late arbiter is told the accurate thing — the window closed — not
    // that an award already exists.
    c.err(Tx::ArbAttest { contract: cid, arbiter: 3, amount: 400.0 }, &[key(3)], ET_ARB_WINDOW_CLOSED);
}

/// The window closes, and an obligation with no consented terms has the
/// channel structurally shut — there is no arbitration anyone can impose after
/// the fact.
#[test]
fn arbitration_is_shut_without_terms_and_after_the_window() {
    let mut c = Chain::founded(1, 5);
    for m in 1..=5 {
        c.back(0, m, SUPPLY);
    }
    let plain = c.lend(2, 1, 100.0);
    c.err(Tx::ArbAttest { contract: plain, arbiter: 3, amount: 10.0 }, &[key(3)], ET_ARB_NO_TERMS);

    let mut terms = panel_terms(vec![3, 4, 5], 500.0);
    terms.window_epochs = 10;
    let cid = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(1),
            creditor: Party::Member(2),
            amount: 400.0,
            maturity_epochs: MATURITY,
            arb: Some(terms),
        },
        &[key(1), key(2)],
    );
    c.goto(11);
    c.err(Tx::ArbAttest { contract: cid, arbiter: 3, amount: 10.0 }, &[key(3)], ET_ARB_WINDOW_CLOSED);
}

/// Terms are validated at acceptance, when both parties are still signing
/// them: a panel containing a party, an impossible quorum, an unbounded window
/// or a negative cap are all refused before the obligation exists.
#[test]
fn arbitration_terms_are_validated_where_both_parties_consent_to_them() {
    let mut c = Chain::founded(1, 5);
    for m in 1..=5 {
        c.back(0, m, SUPPLY);
    }
    let bad = [
        ArbTermsWire { arbiters: [3, 4, 5].into(), quorum: 0, window_epochs: 10, award_cap: 1.0 },
        ArbTermsWire { arbiters: [3].into(), quorum: 3, window_epochs: 10, award_cap: 1.0 },
        ArbTermsWire { arbiters: [3, 4, 5].into(), quorum: 3, window_epochs: 0, award_cap: 1.0 },
        ArbTermsWire { arbiters: [1, 4, 5].into(), quorum: 3, window_epochs: 10, award_cap: 1.0 },
        ArbTermsWire { arbiters: [3, 4, 5].into(), quorum: 3, window_epochs: 10, award_cap: -1.0 },
    ];
    for terms in bad {
        c.err(
            Tx::Accept {
                debtor: Party::Member(1),
                creditor: Party::Member(2),
                amount: 100.0,
                maturity_epochs: MATURITY,
                arb: Some(terms),
            },
            &[key(1), key(2)],
            ET_ARB_BAD_TERMS,
        );
    }
}

/// **What an award IS.**
///
/// The paper described a remedy "on a disputed default", "from debtor to
/// creditor", "subject to the debtor's capacity gate". All three were wrong and
/// the code is the coherent half: an award runs from the original CREDITOR to
/// the original DEBTOR, so it is the **buyer's remedy for non-delivery** — and
/// the case that most needs it is the one the audit called a defect, a buyer
/// who has PAID IN FULL and received nothing. Gating on the row still being open
/// would shut the remedy at the moment it falls due, so there is deliberately no
/// status check.
///
/// Three things measured here, none of them gated before: the award fires against
/// a SETTLED row, it is capped by the ORIGINAL amount as well as by the agreed
/// ceiling, and what it costs the loser is the WRITE channel and not capacity —
/// an uninsured obligation reserves nothing, so "it consumes the loser's
/// capacity, which is the sanction" describes no rule this ledger has.
#[test]
fn an_award_is_the_buyers_remedy_and_bites_only_the_write_channel() {
    let mut c = Chain::founded(1, 4);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let terms = ArbTermsWire { arbiters: [3, 4].into(), quorum: 2, window_epochs: MATURITY - 10, award_cap: 5000.0 };
    let cid = c.st.next_contract;
    c.ok(
        Tx::Accept {
            debtor: Party::Member(1),
            creditor: Party::Member(2),
            amount: 100.0,
            maturity_epochs: MATURITY,
            arb: Some(terms),
        },
        &[key(1), key(2)],
    );
    // Paid in full. No default anywhere in the scene.
    c.settle(cid, 100.0);
    assert_eq!(c.st.contracts[&cid].status, ContractStatus::Settled);

    // **Measured against a control, not against a before.** The award mints
    // when the window closes, and closing it means advancing the clock, which
    // decays every stake — so a before/after comparison would credit the
    // decay to the award. The control is the same scene with no attestation,
    // aged identically.
    let mut control = c.clone_for_control();
    control.goto(MATURITY - 9);

    let minted = c.st.next_contract;
    c.ok(Tx::ArbAttest { contract: cid, arbiter: 3, amount: 9999.0 }, &[key(3)]);
    c.ok(Tx::ArbAttest { contract: cid, arbiter: 4, amount: 9999.0 }, &[key(4)]);
    c.goto(MATURITY - 9);
    let (cap_before, head_before) = (control.cap(2), control.st.bond_headroom(2));

    let award = &c.st.contracts[&minted];
    assert_eq!((award.debtor, award.creditor), (2, 1), "the seller who did not deliver owes the buyer");
    assert!(!award.insured, "uninsured, so it reserves nothing and substitutes nothing");
    assert!(
        award.original == State::to_minor(100.0),
        "capped by the ORIGINAL amount, not just the agreed ceiling of 5000: {}",
        award.original
    );
    assert_eq!(c.cap(2), cap_before, "an uninsured award does not touch the capacity path");
    assert!(
        (c.st.bond_headroom(2) - (head_before - 100.0)).abs() < 1e-9,
        "what it does bite is the write channel, by exactly the award: {} against {}",
        c.st.bond_headroom(2),
        head_before - 100.0
    );
}

// ------------------------------------------------------------- lifecycle --

/// Leaving is unilateral, and it has three floors: no debt, no encumbrance, no
/// standing declared. Each is checked before anything is written, so a refused
/// exit leaves the member exactly as they were.
#[test]
fn leaving_requires_clean_books() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let cid = c.lend(2, 1, 100.0);

    c.err(Tx::Exit { member: 1 }, &[key(1)], ET_LIF_OUTSTANDING_DEBT);
    c.err(Tx::Exit { member: 1 }, &[key(2)], ET_MEM_NOT_SIGNER);
    c.settle(cid, 100.0);

    c.st.members.get_mut(&1).unwrap().bonds.insert(9_999, State::to_minor(10.0));
    c.err(Tx::Exit { member: 1 }, &[key(1)], ET_LIF_OUTSTANDING_BONDS);
    c.st.members.get_mut(&1).unwrap().bonds.clear();

    // A supply is seated by a ceremony and by nothing else, so this
    // is how a member comes to be holding one at all.
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 1, kind: ProposalKind::SeedAmendment { amount: 20.0 } }, &[key(1), key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert!(c.st.proposals[&pid].enacted, "the ceremony seats the underwriter");
    c.err(Tx::Exit { member: 1 }, &[key(1)], ET_UWR_STILL_DECLARED);
    c.ok(Tx::DeclareSupply { member: 1, supply: 0.0 }, &[key(1)]);

    c.ok(Tx::Exit { member: 1 }, &[key(1)]);
    assert_eq!(c.status(1), MemberStatus::Exited);
    assert!(c.st.edges.contains_key(&(0, 1)), "the stake graph is deliberately not swept");
}

/// An exited account is out, and stays out for everything origination touches.
#[test]
fn an_exited_account_can_no_longer_originate() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    c.ok(Tx::Exit { member: 1 }, &[key(1)]);
    c.err(
        Tx::Accept {
            debtor: Party::Member(1),
            creditor: Party::Member(2),
            amount: 10.0,
            maturity_epochs: MATURITY,
            arb: None,
        },
        &[key(1), key(2)],
        ET_MEM_NOT_ACTIVE,
    );
    assert_eq!(c.cap(1), 0.0, "and capacity reads zero for them whatever the graph says");
}

// ------------------------------------------------------------ governance --

/// The whole governance path: an unknown proposal, a suspension enacted by a
/// coalition carrying the bar, and a validator seated and removed.
#[test]
fn governance_suspends_and_seats_by_coalition_weight() {
    let mut c = Chain::founded(4, 1);
    c.validator(0);
    c.back(0, 4, SUPPLY);
    c.err(Tx::Assent { member: 0, proposal: 77 }, &[key(0)], ET_GOV_UNKNOWN_PROPOSAL);

    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::Suspend { member: 4 } }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert_eq!(c.status(4), MemberStatus::Active, "one of four underwriters is below the bar");
    c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
    assert_eq!(c.status(4), MemberStatus::Suspended, "two carry it");
    c.err(Tx::Assent { member: 2, proposal: pid }, &[key(2)], ET_GOV_UNKNOWN_PROPOSAL);

    // A suspended member is not eligible to be seated as a validator.
    let pid = c.st.next_proposal;
    c.err(
        Tx::Propose { author: 0, kind: ProposalKind::ValidatorPower { member: 4, power: 1 } },
        &[key(0)],
        ET_VAL_NOT_ELIGIBLE,
    );
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::Unsuspend { member: 4 } }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
    assert_eq!(c.status(4), MemberStatus::Active);

    // Seating a validator takes THREE of four, not two: a change to who
    // orders the ledger answers to `theta_adopt_validator`. The member
    // registers the key their validator will sign with first — the ledger
    // will not seat one whose signing key it cannot name.
    c.ok(Tx::SetConsensusKey { member: 4, key: Some(common::consensus_key(4)) }, &[key(4)]);
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::ValidatorPower { member: 4, power: 3 } }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
    assert_eq!(c.st.validators.get(&4), None, "two of four is below the validator bar");
    c.ok(Tx::Assent { member: 2, proposal: pid }, &[key(2)]);
    assert_eq!(c.st.validators.get(&4), Some(&3));
}

/// A re-denomination outside the band is refused: it is a change of unit, and
/// one large enough to be something else is refused as something else.
#[test]
fn a_redenomination_outside_the_band_is_refused() {
    let mut c = Chain::founded(2, 0);
    for (num, den) in [(0u64, 1u64), (1, 0), (100, 1), (1, 100)] {
        c.err(Tx::Propose { author: 0, kind: ProposalKind::Redenominate { num, den } }, &[key(0)], ET_GOV_BAND);
    }
}

/// A re-denomination is exact: every denomination-valued quantity moves by π
/// together, so every comparison of an amount against a capacity has both
/// sides scaled alike and each member's real position is unchanged.
#[test]
fn a_redenomination_preserves_every_real_position() {
    let mut c = Chain::founded(2, 2);
    c.back(0, 2, SUPPLY);
    c.back(0, 3, SUPPLY);
    c.lend(3, 2, 300.0);

    let members: Vec<MemberId> = vec![0, 1, 2, 3];
    let caps_before: Vec<f64> = members.iter().map(|&m| c.st.gross_capacity_of_set(&[m])).collect();
    let debt_before = c.st.members[&2].debt_out;
    let committed_before = c.st.committed_total();
    let v_base_before = c.st.params.v_base;

    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::Redenominate { num: 3, den: 2 } }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert!(c.st.proposals[&pid].enacted, "one of two underwriters is exactly the bar");

    let pi = 1.5;
    for (&m, before) in members.iter().zip(&caps_before) {
        let after = c.st.gross_capacity_of_set(&[m]);
        assert!((after - pi * before).abs() < 1e-6 * (1.0 + before), "capacity rescales: {before} -> {after}");
    }
    assert_eq!(c.st.members[&2].debt_out, State::to_minor(pi * State::from_minor(debt_before)), "and so does the debt");
    assert!((c.st.committed_total() - pi * committed_before).abs() < 1e-6, "and what is committed behind it");
    assert!((c.st.params.v_base - pi * v_base_before).abs() < 1e-9);
}

/// The cooldown binds per parameter, and only where a change would actually
/// enact — banking an assent below the bar is not an amendment.
#[test]
fn the_cooldown_binds_per_parameter() {
    let mut c = Chain::founded(2, 0);
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 } }, &[key(0)]);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert_eq!(c.st.params.risk_k, 1.0);

    // A DIFFERENT parameter is untouched by the first one's cooldown.
    let pid = c.st.next_proposal;
    c.ok(
        Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::SealAmounts, value: 1.0 } },
        &[key(0)],
    );
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert_eq!(c.st.params.seal_amounts, 1.0);

    // The same one is not, until the cooldown elapses.
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 2.0 } }, &[key(0)]);
    c.err(Tx::Assent { member: 0, proposal: pid }, &[key(0)], ET_GOV_COOLDOWN);
    c.goto(c.st.params.gov_cooldown_epochs + 1);
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert_eq!(c.st.params.risk_k, 2.0);
}

// -------------------------------------------------------- the envelope --

/// Replay, expiry and the window ceiling, checked before dispatch and in that
/// order — the window checks are meaningless against a stale epoch, which is
/// why the clock advances first.
#[test]
fn the_envelope_rules_bind_before_anything_is_dispatched() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    c.goto(10);

    let tx = Tx::Settle { contract: 0, amount: 1.0 };
    let id = c.tx_id();
    assert_eq!(c.apply_raw(tx.clone(), id, 5, &[key(0)]), Err(Error(ET_TX_EXPIRED)));
    assert_eq!(
        c.apply_raw(tx.clone(), id, 10 + k::MAX_TX_LIFETIME_EPOCHS + 1, &[key(0)]),
        Err(Error(ET_TX_WINDOW_TOO_LONG))
    );
    assert!(!c.st.is_applied(&id, 10 + k::MAX_TX_LIFETIME_EPOCHS + 1), "a refused envelope is not recorded at all");

    // Exactly at the ceiling is legal, and spends the id.
    let _ = c.apply_raw(tx.clone(), id, 10 + k::MAX_TX_LIFETIME_EPOCHS, &[key(0)]);
    assert!(c.st.is_applied(&id, 10 + k::MAX_TX_LIFETIME_EPOCHS));
    assert_eq!(c.apply_raw(tx, id, 10 + k::MAX_TX_LIFETIME_EPOCHS, &[key(0)]), Err(Error(ET_TX_REPLAY)));
}

/// The replay cache is bounded by the buckets it is kept in: an id can never
/// be retained longer than its own claimed window, and once it is forgotten
/// the expiry check is what refuses the replay instead. The two mechanisms
/// together are what make the cache finite AND the guarantee total.
#[test]
fn the_replay_cache_is_pruned_and_expiry_takes_over() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);

    let id = c.tx_id();
    let _ = c.apply_raw(Tx::Settle { contract: 0, amount: 1.0 }, id, 2, &[key(0)]);
    assert!(c.st.is_applied(&id, 2));

    c.goto(5);
    assert!(!c.st.is_applied(&id, 2), "the id is forgotten once its window has elapsed");
    assert!(
        !c.st.applied_by_expiry.contains_key(&2),
        "and the bucket goes with it — an emptied bucket left behind is a row nothing reads"
    );
    assert_eq!(
        c.apply_raw(Tx::Settle { contract: 0, amount: 1.0 }, id, 2, &[key(0)]),
        Err(Error(ET_TX_EXPIRED)),
        "forgetting it reopens nothing: the window check refuses it regardless"
    );
}

/// Identical content under distinct ids is two transactions, not one — the id
/// is the envelope's identity, and the nonce is what a client varies to retry.
#[test]
fn identical_content_under_distinct_ids_applies_twice() {
    let mut c = Chain::founded(1, 2);
    c.back(0, 1, SUPPLY);
    c.back(0, 2, SUPPLY);
    let before = c.st.contracts.len();
    let tx = Tx::Accept {
        debtor: Party::Member(1),
        creditor: Party::Member(2),
        amount: 10.0,
        maturity_epochs: MATURITY,
        arb: None,
    };
    for _ in 0..2 {
        let id = c.tx_id();
        let not_after = c.st.epoch + 5;
        c.apply_raw(tx.clone(), id, not_after, &[key(1), key(2)])
            .expect("distinct ids are distinct transactions");
    }
    assert_eq!(c.st.contracts.len(), before + 2);
}

// ------------------------------------------------------------ the soup --

struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Three hundred transitions chosen at random over the whole alphabet, most of
/// them refused, every one of them audited.
///
/// The point is not the transitions, it is the audit inside `Chain::apply`:
/// §Verification's invariants must hold after a REFUSED transition exactly as after an
/// accepted one, because `apply` has no rollback and a handler that mutates on
/// its way to an error leaves the mutation on the ledger. Deterministic seed,
/// never randomness — a probe that finds a break on one run in ten is not a
/// gate.
#[test]
fn a_random_soup_of_transitions_never_breaks_an_invariant() {
    let mut c = Chain::founded(2, 4);
    for m in 2..6 {
        c.back(0, m, SUPPLY);
    }
    let mut rng = XorShift(0x00E5_EED0_5EED_CAFE);
    let mut epoch = 0u64;
    for _ in 0..300 {
        let a = rng.pick(6);
        let b = rng.pick(6);
        let last = c.st.next_contract.max(1);
        let tx = match rng.pick(9) {
            0 => Tx::Accept {
                debtor: Party::Member(a),
                creditor: Party::Member(b),
                amount: 10.0 + rng.pick(400) as f64,
                maturity_epochs: MATURITY + rng.pick(20),
                arb: None,
            },
            1 => Tx::Transfer { contract: rng.pick(last), new_debtor: b },
            2 => Tx::Settle { contract: rng.pick(last), amount: 1.0 + rng.pick(200) as f64 },
            3 => Tx::MarkExpired { contract: rng.pick(last) },
            4 => Tx::Cure { contract: rng.pick(last), amount: 1.0 + rng.pick(200) as f64 },
            5 => Tx::DeclareSupply { member: a, supply: rng.pick(3000) as f64 },
            6 => Tx::Sale {
                seller: Party::Member(a),
                buyer: Party::Member(b),
                amount: 10.0 + rng.pick(300) as f64,
                maturity_epochs: MATURITY,
            },
            7 => Tx::ListBeneficiaries { supporter: a, entries: vec![(b, 1.0)] },
            _ => Tx::Extend { contract: rng.pick(last), new_maturity_epoch: c.st.epoch + 40 + rng.pick(40) },
        };
        let _ = c.apply(tx, &[key(a as usize), key(b as usize)]);
        if rng.pick(4) == 0 {
            epoch += rng.pick(8);
            c.goto(epoch);
        }
    }
}

// ---------------------------------------------- what the alphabet no longer has --

/// **The loss pool is not in the alphabet, and this is the gate that says so
/// (§Recourse).**
///
/// `PoolCovenant`, `PoolCommit` and `PoolClaim` were a mutualization layer
/// funded by signatures: a commitment was checked only against the member's own
/// self-declared ceiling, reserved nothing, and was displayed to creditors as
/// coverage. Measured with such a layer, 1.0 of standing pledges 1e9 and mints
/// 399.9998 of a 400 claim while an honest member's 500 minted nothing; and a
/// claim insured solely by its creditor's own arc was pool-claimable, which
/// released the defaulter and drained an honest covenanter to 200.60. Both
/// constructions are gated in `tests/loss.rs` as the model that has no pool in
/// it.
///
/// This one is deliberately at the WIRE, because a retired mechanism comes back
/// through a client or a stored transaction rather than through a call site the
/// compiler can see. Deserialization must refuse these three, and a fourth form
/// asserts the test is not passing for the trivial reason that nothing parses.
#[test]
fn the_signature_funded_coverage_layer_does_not_deserialize() {
    for wire in [
        r#"{"PoolCovenant":{"member":1,"cap_event":100.0,"cap_outstanding":200.0}}"#,
        r#"{"PoolCommit":{"member":1,"amount":1000000000.0}}"#,
        r#"{"PoolClaim":{"contract":0}}"#,
    ] {
        assert!(
            serde_json::from_str::<Tx>(wire).is_err(),
            "a retired transition must not parse — it would be applied by every node that did: {wire}"
        );
    }
    assert!(
        serde_json::from_str::<Tx>(r#"{"MarkExpired":{"contract":0}}"#).is_ok(),
        "and the alphabet that remains still parses, or the assertions above are vacuous"
    );
}

/// **Account creation is not in the alphabet either, and this is the gate at
/// the wire.**
///
/// `OpenAccount` was unbillable by construction — a key nobody knows has no
/// headroom — which is what kept it unapprovable and what left it bounded by
/// nothing but a ledger-wide per-epoch counter. That counter was a censorship
/// lever: measured, an attacker with no standing filled the whole 1024 for a
/// bond spend of zero and shut onboarding for everybody until the boundary.
///
/// At the wire for the same reason as the pool: a retired transition comes back
/// through a client or a stored transaction, not through a call site a compiler
/// can see. And a party named by key must still parse, or the assertion above
/// it is about a format nothing uses.
#[test]
fn account_creation_is_not_a_transition_any_more() {
    for wire in [
        r#"{"OpenAccount":{"keys":[[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]]}}"#,
        r#"{"OpenAccount":{"keys":[]}}"#,
    ] {
        assert!(
            serde_json::from_str::<Tx>(wire).is_err(),
            "a retired transition must not parse — it would be applied by every node that did: {wire}"
        );
    }
    let seating = r#"{"Accept":{"debtor":{"Key":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]},
        "creditor":{"Member":0},"amount":400.0,"maturity_epochs":30,"arb":null}}"#;
    assert!(
        serde_json::from_str::<Tx>(seating).is_ok(),
        "and what replaced it does parse, or the assertions above are vacuous"
    );
}

/// **Who must sign each transition, stated once and forced by the compiler.**
///
/// The client composes a pending entry from a *claimed* signer set
/// (`tx` in `ui/src/lib/api.ts`, `plan.signers`), and the node takes that set on
/// trust: it verifies the signatures it is given and assembles at `min_sigs`,
/// but it never derives the set from the transaction. So a client that asks for
/// too FEW signers builds an entry that fills, submits, and is refused at apply
/// with `ET-MEM-003` — a failure the member cannot diagnose and cannot fix.
///
/// This is the "pending pool not re-verified against the alphabet"
/// Verified here, transition by transition,
/// and the match
/// below is exhaustive so a NEW transition cannot be added without somebody
/// saying who signs it — which is the only part of this that survives the next
/// change.
#[test]
fn every_transition_states_who_must_sign_it() {
    /// The parties whose signature `apply` demands, as the client claims them.
    /// `&[]` means permissionless: nobody's signature, and therefore no payer,
    /// which only works because these are priced at zero.
    fn who_signs(tx: &Tx) -> &'static [&'static str] {
        match tx {
            // Both parties to a trade, and either may be named by KEY, in
            // which case that key must be among the signers.
            Tx::Accept { .. } => &["debtor", "creditor"],
            Tx::Sale { .. } => &["seller", "buyer"],
            // Discharge: the creditor is the party who loses if it is wrong.
            Tx::Settle { .. } | Tx::Cure { .. } => &["debtor", "creditor"],
            // Deferring a maturity is the creditor's concession to make.
            Tx::Extend { .. } => &["debtor", "creditor"],
            // The creditor signs only when the successor would be uninsured,
            // and the client cannot tell which case it is from public state —
            // so it collects the creditor always. Over-collecting is safe;
            // under-collecting is a refusal the member cannot diagnose.
            Tx::Transfer { .. } => &["debtor", "new_debtor", "creditor (when the successor would be uninsured)"],
            Tx::ArbAttest { .. } => &["arbiter"],
            // The member's own acts.
            Tx::DeclareSupply { .. } | Tx::Exit { .. } => &["member"],
            // Registering the key an operator's VALIDATOR signs with is the
            // member's own act, and only theirs: it is the one key on the
            // ledger that never signs a transaction, so nobody else's consent
            // is at stake.
            Tx::SetConsensusKey { .. } => &["member"],
            Tx::RegisterGuardians { .. } | Tx::RotateVeto { .. } => &["member"],
            Tx::ListBeneficiaries { .. } => &["supporter"],
            Tx::ApproveSupporter { .. } => &["beneficiary"],
            Tx::Propose { .. } => &["author"],
            Tx::Assent { .. } => &["member"],
            // A threshold of the member's OWN guardians, which is the whole of
            // key recovery: the member is by assumption unable to sign.
            Tx::RotateRequest { .. } => &["threshold of guardians"],
            // The three permissionless ones. Anyone may crank them, so nobody
            // is named — and that only works because they cost nothing: an
            // unpayable priced transition is refused (`ET-BND-004`).
            Tx::MarkExpired { .. } | Tx::ForfeitBonds { .. } | Tx::RotateFinalize { .. } => &[],
        }
    }

    // The alphabet, one of each. Hand-listed, but the match above is not: a
    // new variant fails to compile until its signers are stated.
    let alphabet: Vec<Tx> = vec![
        Tx::RegisterGuardians { member: 0, guardians: vec![], threshold: 2, veto_window_epochs: 30 },
        Tx::RotateRequest { member: 0, new_keys: vec![] },
        Tx::RotateVeto { member: 0 },
        Tx::RotateFinalize { member: 0 },
        Tx::Exit { member: 0 },
        Tx::ListBeneficiaries { supporter: 0, entries: vec![] },
        Tx::ApproveSupporter { beneficiary: 0, supporter: 1, approved: true },
        Tx::Sale { seller: Party::Member(0), buyer: Party::Member(1), amount: 1.0, maturity_epochs: 30 },
        Tx::DeclareSupply { member: 0, supply: 1.0 },
        Tx::SetConsensusKey { member: 0, key: Some([9u8; 32]) },
        Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 1.0,
            maturity_epochs: 30,
            arb: None,
        },
        Tx::Transfer { contract: 0, new_debtor: 1 },
        Tx::Settle { contract: 0, amount: 1.0 },
        Tx::Extend { contract: 0, new_maturity_epoch: 99 },
        Tx::MarkExpired { contract: 0 },
        Tx::Cure { contract: 0, amount: 1.0 },
        Tx::ArbAttest { contract: 0, arbiter: 0, amount: 1.0 },
        Tx::Propose { author: 0, kind: ProposalKind::Suspend { member: 1 } },
        Tx::Assent { member: 0, proposal: 0 },
        Tx::ForfeitBonds { member: 0 },
    ];
    assert_eq!(alphabet.len(), 20, "the alphabet is twenty transitions");

    let st = edet_state::state::State::default();
    for tx in &alphabet {
        let signers = who_signs(tx);
        // **A transition nobody signs must be free**, or the bond gate refuses
        // it for having no payer and the crank is unreachable by anybody but a
        // member willing to pay for a chore that is not theirs.
        if signers.is_empty() {
            assert_eq!(
                edet_state::bond::bond_multiple(&st, tx),
                0.0,
                "{tx:?} names no signer, so it must cost nothing — otherwise ET-BND-004"
            );
        }
    }

    // And the converse, which is the half that would rot silently: exactly
    // three transitions are permissionless. A fourth appearing here without a
    // reason is a free write channel; one disappearing is a crank that now
    // needs a volunteer to fund it.
    let permissionless: Vec<&Tx> = alphabet.iter().filter(|tx| who_signs(tx).is_empty()).collect();
    assert_eq!(permissionless.len(), 3, "MarkExpired, ForfeitBonds, RotateFinalize — and nothing else");
}

/// **A consensus key is a claimed key.** `set_consensus_key` refused a member
/// key, and nothing refused the other direction: a rotation or a seating could
/// take a validator's consensus key as a member key, and one key was two
/// identities with the audit green.
///
/// Mutation that bites: drop the `consensus_key` clause from `key_is_claimed`.
#[test]
fn a_consensus_key_cannot_become_a_member_key() {
    let mut c = Chain::founded(3, 0);
    c.validator(0);
    let ck = common::consensus_key(0);
    c.ok(Tx::RegisterGuardians { member: 1, guardians: vec![0, 2], threshold: 2, veto_window_epochs: 30 }, &[key(1)]);
    c.err(Tx::RotateRequest { member: 1, new_keys: vec![ck] }, &[key(0), key(2)], ET_ADM_DUP_KEY);
    c.err(
        Tx::Accept { debtor: Party::Key(ck), creditor: Party::Member(1), amount: 1.0, maturity_epochs: 30, arb: None },
        &[ck, key(1)],
        ET_ADM_DUP_KEY,
    );
    assert!(c.st.add_underwriter(vec![ck], 1.0).is_err(), "nor may a ceremony seat one");
    // And the audit refuses a state built with the overlap by hand.
    let mut st = c.st.clone();
    st.key_index.insert(ck, 1);
    st.members.get_mut(&1).expect("member 1").keys.push(ck);
    assert!(edet_state::invariants::audit(&st).is_err(), "one key, two identities, is not a state that can exist");
}
