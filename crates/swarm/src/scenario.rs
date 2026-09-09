//! **The fixed conformance scenario**: one list of `(actor, intent)` steps
//! that a node and this crate's driver both play, so "the driver is
//! `apply_block_to`" is a claim somebody checked rather than a design note.
//!
//! Everything happens inside ONE epoch and nothing here advances one. A fresh
//! chain climbs ten thousand epochs a block until its clock catches up, so a
//! window computed before that catch-up is already expired when the proposer
//! includes it — and a scenario that advanced the epoch would be measuring
//! that race rather than the alphabet.
//!
//! **The scenario is a function of the epoch it will be played at**, because
//! `Extend` names an ABSOLUTE maturity by the ledger's own shape: a step
//! naming epoch 200 succeeds on a chain at epoch 0 and is refused on one at
//! wall clock, and a conformance test that hid that would be comparing two
//! different scenarios.
//!
//! There is no consent question in a script. Every counterparty here agreed by
//! being written into the step, which is why [`play`] grants every ask: what
//! the scenario tests is the ADAPTER and the transition function, and a
//! strategy's judgement would only make the two sides disagree for a reason
//! that is not about either.

use edet_state::state::State;
use edet_state::types::*;

use crate::driver::{Audit, Driver};
use crate::intent::{compose, AgentRef, Intent, Outcome};
use crate::population::World;

/// One scripted step: who acts, and what they ask for.
#[derive(Clone, Debug)]
pub struct Step {
    pub actor: MemberId,
    pub intent: Intent,
}

fn step(actor: MemberId, intent: Intent) -> Step {
    Step { actor, intent }
}

fn lend(creditor: MemberId, debtor: MemberId, amount_minor: u64, term: u64) -> Intent {
    Intent::Lend {
        creditor: AgentRef::Member(creditor),
        debtor: AgentRef::Member(debtor),
        amount_minor,
        term,
        arb: None,
    }
}

/// The scenario, over six members that already exist at genesis.
///
/// The order is the point: an uninsured first trade, the settlement that
/// confers the standing, the insured trade that standing pays for, a sale
/// that nets against what the buyer already owes, a debtor swap, a proposal
/// carried by two of six, one step the ledger refuses on its own terms, and a
/// stripped envelope followed by the same transaction properly signed.
pub fn conformance(epoch: u64) -> Vec<Step> {
    const T: u64 = 30;
    let mut s: Vec<Step> = Vec::new();
    let mut next: ContractId = 0;
    // Contract ids are dense and assigned in order, so the `n`-th booking the
    // ledger ACCEPTS is contract `n` — which is why the two refused bookings
    // below do not advance it.
    let mut book = |s: &mut Vec<Step>, c: MemberId, d: MemberId, amount: u64, term: u64| -> ContractId {
        s.push(step(c, lend(c, d, amount, term)));
        next += 1;
        next - 1
    };

    // Four uninsured first trades, each settled. Nobody has any standing at
    // genesis, so this is the only edge a community with no graph can write,
    // and the settlement is what confers the standing.
    for (c, d) in [(0u64, 1u64), (2, 3), (4, 5), (1, 2)] {
        let id = book(&mut s, c, d, 10_000, T);
        s.push(step(d, Intent::Settle { contract: id, amount_minor: 10_000 }));
    }
    // The insured trades that standing pays for.
    let first_insured = book(&mut s, 0, 1, 5_000, T);
    let lent_to_three = book(&mut s, 2, 3, 5_000, T);
    book(&mut s, 4, 5, 5_000, T);
    book(&mut s, 1, 2, 5_000, T);

    // A partial discharge, then the rest. An obligation stores its `Held`, and
    // a partial discharge releases the whole and re-takes the remainder.
    s.push(step(1, Intent::Settle { contract: first_insured, amount_minor: 2_000 }));
    s.push(step(1, Intent::Settle { contract: first_insured, amount_minor: 3_000 }));

    // An extension, then a DEBTOR SWAP on the same row: the old debtor
    // discharges, the successor is booked against their own standing, and no
    // stake is written for it.
    let swapped = book(&mut s, 0, 1, 3_000, T);
    s.push(step(1, Intent::Extend { contract: swapped, new_maturity_epoch: epoch + 200 }));
    s.push(step(1, Intent::Transfer { contract: swapped, new_debtor: 2 }));

    // Governance. Two of six underwriters carry a parameter change at the
    // half, and every one of them holds external supply, so every assent
    // counts.
    s.push(step(0, Intent::Propose(ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.25 })));
    s.push(step(0, Intent::Assent(0)));
    s.push(step(1, Intent::Assent(0)));

    // Three refusals the ledger owes on its own terms: a term below the
    // maturity floor, a settlement on a row that is already closed, and a
    // permissionless crank on a row that does not exist — the last of which
    // must leave no replay id behind, and is the only one of the three that
    // legitimately carries no signature at all.
    s.push(step(0, lend(0, 1, 1_000, T - 1)));
    s.push(step(1, Intent::Settle { contract: 0, amount_minor: 1_000 }));
    s.push(step(0, Intent::MarkExpired { contract: 9_999 }));

    // **An envelope that authorises nothing, and the same transaction
    // properly signed.** The first is refunded, unbilled and re-appliable —
    // burning its id would let anyone who saw a pending request kill the
    // genuine transaction for free.
    s.push(step(0, Intent::Stripped(Box::new(lend(0, 3, 2_000, T)))));
    book(&mut s, 0, 3, 2_000, T);

    // Ordinary traffic between the remaining pairs, so the block stream is
    // not four transitions long.
    for (c, d) in [(0u64, 2u64), (2, 4), (4, 0), (1, 3), (3, 5), (5, 1), (0, 4)] {
        let id = book(&mut s, c, d, 2_000, T);
        s.push(step(d, Intent::Settle { contract: id, amount_minor: 2_000 }));
    }

    // A sale that NETS, last, because its remainder is the one row whose
    // existence depends on arithmetic rather than on order. Member 5 owes
    // member 4, and selling to them extinguishes it: mutual obligations net
    // rather than route, which is how the wallet pays.
    book(&mut s, 4, 5, 4_000, T);
    s.push(step(
        5,
        Intent::Sell { seller: AgentRef::Member(5), buyer: AgentRef::Member(4), amount_minor: 4_000, term: T },
    ));
    let _ = lent_to_three;
    s
}

/// Play the scenario in-process and report what each step came to.
///
/// The driver is in `Audit::EveryTransition`, because a conformance run that
/// did not audit would be checking that two implementations agree without
/// checking that either is right.
pub fn play(st: State, steps: &[Step], epoch: u64) -> (Driver, Vec<Outcome>) {
    let members = st.members.len();
    let mut world = World::with_agents(members);
    for id in st.members.keys() {
        world.claim(*id, *id as usize);
    }
    let mut d = Driver::new(st).audit_mode(Audit::EveryTransition);
    // **Caught up to the epoch the scenario is written for**, in steps,
    // because the epoch clamp is per BLOCK: one `begin_block` at wall clock
    // leaves a fresh state thousands of epochs short and every window computed
    // against it is `ET-TX-003`.
    for _ in 0..64 {
        if d.st.epoch >= epoch {
            break;
        }
        let next = (d.st.epoch + edet_kernel::constants::MAX_EPOCH_ADVANCE_PER_BLOCK).min(epoch);
        d.begin(next * edet_kernel::constants::EPOCH_SECS);
    }
    let mut out = Vec::with_capacity(steps.len());
    for s in steps {
        let composed = compose(&d.st, &world, s.actor, &s.intent);
        // A script IS the consent: every counterparty agreed by being written
        // into the step.
        let mut signers = composed.signers;
        signers.extend(composed.asks.iter().map(|a| a.key));
        signers.sort_by_key(|k| d.st.member_of_key(k).unwrap_or(MemberId::MAX));
        signers.dedup();
        out.push(match d.apply(composed.tx, &signers) {
            Ok(()) => Outcome::Applied,
            Err(e) => Outcome::Refused(e.0),
        });
    }
    (d, out)
}
