//! **The adapter builds an envelope that AUTHORISES the transition it names**,
//! for every letter of the alphabet.
//!
//! This is the property the whole crate rests on and the one whose failure is
//! invisible: a strategy emits intents, one adapter builds every envelope, and
//! an adapter that named the wrong signers produces a population whose
//! refusals are `ET-MEM-003` — a run that measured the harness and reported it
//! as behaviour. Six letters are emitted by no archetype in the tree, which is
//! exactly why they are checked here rather than left to a run that might
//! happen to reach them.
//!
//! `edet_state::authorises` is the oracle, because it is the same query-free
//! function of committed state the node applies at the ingress, in the
//! pre-vote screen and at commit. The one intent that must FAIL it is
//! `Stripped`, which exists to produce an envelope authorising nothing.

use std::collections::BTreeSet;

use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;
use edet_swarm::driver::{Audit, Driver};
use edet_swarm::intent::{compose, AgentRef, Intent};
use edet_swarm::keys::{fresh_key, member_key};
use edet_swarm::population::World;

/// Every letter, named once. A variant added to `Intent` fails to compile in
/// `letter` below until it is listed here too, so the alphabet and this test
/// cannot drift apart in silence.
const ALL: [&str; 23] = [
    "Lend",
    "Sell",
    "Settle",
    "Cure",
    "Transfer",
    "Extend",
    "MarkExpired",
    "Declare",
    "Exit",
    "Propose",
    "Assent",
    "ListBeneficiaries",
    "ApproveSupporter",
    "ArbAttest",
    "RegisterGuardians",
    "RotateRequest",
    "RotateVeto",
    "RotateFinalize",
    "SetConsensusKey",
    "ForfeitBonds",
    "Dud",
    "Stripped",
    "Replayed",
];

/// The letter one intent is, by an EXHAUSTIVE match — `Intent::kind` folds
/// `As` into its inner intent, which is right for a summary and wrong for a
/// completeness check.
fn letter(i: &Intent) -> &'static str {
    match i {
        Intent::Lend { .. } => "Lend",
        Intent::Sell { .. } => "Sell",
        Intent::Settle { .. } => "Settle",
        Intent::Cure { .. } => "Cure",
        Intent::Transfer { .. } => "Transfer",
        Intent::Extend { .. } => "Extend",
        Intent::MarkExpired { .. } => "MarkExpired",
        Intent::Declare { .. } => "Declare",
        Intent::Exit => "Exit",
        Intent::Propose(_) => "Propose",
        Intent::Assent(_) => "Assent",
        Intent::ListBeneficiaries(_) => "ListBeneficiaries",
        Intent::ApproveSupporter { .. } => "ApproveSupporter",
        Intent::ArbAttest { .. } => "ArbAttest",
        Intent::RegisterGuardians { .. } => "RegisterGuardians",
        Intent::RotateRequest { .. } => "RotateRequest",
        Intent::RotateVeto => "RotateVeto",
        Intent::RotateFinalize { .. } => "RotateFinalize",
        Intent::SetConsensusKey(_) => "SetConsensusKey",
        Intent::ForfeitBonds { .. } => "ForfeitBonds",
        Intent::Dud => "Dud",
        Intent::Stripped(_) => "Stripped",
        Intent::Replayed { .. } => "Replayed",
        // `As` is not a letter: it names WHICH member acts, and what it
        // composes is the inner one.
        Intent::As { intent, .. } => letter(intent),
    }
}

/// A community with something of everything an intent can name: standing
/// earned by trade, a live obligation, a guardian set with a rotation waiting
/// out its window, and a proposal on the record.
fn scene() -> (Driver, World) {
    let mut st = State::default();
    for i in 0..4 {
        st.add_underwriter(vec![member_key(i)], 2_500.0).expect("founding underwriter");
    }
    for i in 4..8 {
        st.new_account(vec![member_key(i)]);
    }
    let mut d = Driver::new(st).audit_mode(Audit::EveryTransition);
    // Standing, earned the one way there is.
    for debtor in 4..8u64 {
        let cid = d.st.next_contract;
        d.ok(
            Tx::Accept {
                debtor: Party::Member(debtor),
                creditor: Party::Member(0),
                amount: 500.0,
                maturity_epochs: 30,
                arb: None,
            },
            &[member_key(0), member_key(debtor as usize)],
        );
        d.ok(Tx::Settle { contract: cid, amount: 500.0 }, &[member_key(0), member_key(debtor as usize)]);
    }
    // One live obligation, for the letters that name a contract.
    d.ok(
        Tx::Accept {
            debtor: Party::Member(4),
            creditor: Party::Member(5),
            amount: 100.0,
            maturity_epochs: 30,
            arb: Some(ArbTermsWire { arbiters: [6, 7].into(), quorum: 2, window_epochs: 60, award_cap: 100.0 }),
        },
        &[member_key(4), member_key(5)],
    );
    // A guardian set and a rotation waiting out its veto window.
    d.ok(
        Tx::RegisterGuardians { member: 4, guardians: vec![6, 7], threshold: 2, veto_window_epochs: 30 },
        &[member_key(4)],
    );
    d.ok(Tx::RotateRequest { member: 4, new_keys: vec![fresh_key(4, 1)] }, &[member_key(6), member_key(7)]);
    // A proposal on the record, for `Assent`.
    d.ok(
        Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.25 } },
        &[member_key(0)],
    );

    let mut world = World::with_agents(d.st.members.len());
    for id in d.st.members.keys() {
        world.claim(*id, *id as usize);
    }
    (d, world)
}

/// Compose one intent and grant every ask, which is what a population of
/// willing counterparties does.
fn envelope(st: &State, world: &World, actor: MemberId, intent: &Intent) -> (Tx, Vec<Key>) {
    let c = compose(st, world, actor, intent);
    let mut signers = c.signers;
    signers.extend(c.asks.iter().map(|a| a.key));
    signers.sort_by_key(|k| st.member_of_key(k).unwrap_or(MemberId::MAX));
    signers.dedup();
    (c.tx, signers)
}

#[test]
fn every_letter_of_the_alphabet_is_composed_into_an_envelope_that_authorises_it() {
    let (d, world) = scene();
    let st = &d.st;

    let cases: Vec<(MemberId, Intent)> = vec![
        (
            5,
            Intent::Lend {
                creditor: AgentRef::Member(5),
                debtor: AgentRef::Member(6),
                amount_minor: 1_000,
                term: 30,
                arb: None,
            },
        ),
        // A newcomer's first trade: the key nobody holds must SIGN, and
        // `resolve` demands exactly that signature before it seats a row.
        (
            5,
            Intent::Lend {
                creditor: AgentRef::Member(5),
                debtor: AgentRef::Fresh { owner: 5, n: 9 },
                amount_minor: 1_000,
                term: 30,
                arb: None,
            },
        ),
        (5, Intent::Sell { seller: AgentRef::Member(5), buyer: AgentRef::Member(6), amount_minor: 1_000, term: 30 }),
        (4, Intent::Settle { contract: 4, amount_minor: 1_000 }),
        (4, Intent::Cure { contract: 4, amount_minor: 1_000 }),
        (4, Intent::Transfer { contract: 4, new_debtor: 6 }),
        (4, Intent::Extend { contract: 4, new_maturity_epoch: 90 }),
        (7, Intent::MarkExpired { contract: 4 }),
        (0, Intent::Declare { supply_minor: 100_000 }),
        (6, Intent::Exit),
        (0, Intent::Propose(ProposalKind::Suspend { member: 7 })),
        (0, Intent::Assent(0)),
        (5, Intent::ListBeneficiaries(vec![(6, 1.0)])),
        (6, Intent::ApproveSupporter { supporter: 5, approved: true }),
        (6, Intent::ArbAttest { contract: 4, amount_minor: 500 }),
        (5, Intent::RegisterGuardians { guardians: vec![6, 7], threshold: 2, veto_window_epochs: 30 }),
        // A threshold of the member's OWN guardians, which the adapter has to
        // go and ask for: the actor is one of them and never enough alone.
        (6, Intent::RotateRequest { member: 4, new_keys: vec![fresh_key(4, 2)] }),
        (4, Intent::RotateVeto),
        (7, Intent::RotateFinalize { member: 4 }),
        (0, Intent::SetConsensusKey(Some(edet_swarm::keys::consensus_key(0)))),
        (7, Intent::ForfeitBonds { member: 4 }),
        (5, Intent::Dud),
        (5, Intent::Replayed { emission: 0 }),
        (5, Intent::As { member: 5, intent: Box::new(Intent::Dud) }),
    ];

    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    for (actor, intent) in &cases {
        let (tx, signers) = envelope(st, &world, *actor, intent);
        assert!(
            edet_state::authorises(st, &tx, &signers),
            "{}: the adapter built an envelope that authorises nothing — signers {:?}",
            letter(intent),
            signers.iter().filter_map(|k| st.member_of_key(k)).collect::<Vec<_>>()
        );
        seen.insert(letter(intent));
    }

    // **The one that must NOT authorise**, which is what it is for.
    let inner = Intent::Lend {
        creditor: AgentRef::Member(6),
        debtor: AgentRef::Member(5),
        amount_minor: 1_000,
        term: 30,
        arb: None,
    };
    let (tx, signers) = envelope(st, &world, 5, &Intent::Stripped(Box::new(inner)));
    assert_eq!(signers.len(), 1, "a stripped envelope keeps the actor's own signature and nothing else");
    assert!(!edet_state::authorises(st, &tx, &signers), "a stripped envelope must authorise nothing");
    seen.insert("Stripped");

    let missing: Vec<&&str> = ALL.iter().filter(|l| !seen.contains(**l)).collect();
    assert!(missing.is_empty(), "letters of the alphabet nothing here composes: {missing:?}");
    assert_eq!(seen.len(), ALL.len());
}

/// **A key nobody holds signs for itself, and the adapter never asks anybody
/// about it.** That is what makes a newcomer's first trade composable at all:
/// there is no member to put the question to.
#[test]
fn a_fresh_key_signs_for_itself_and_is_asked_of_nobody() {
    let (d, world) = scene();
    let intent = Intent::Lend {
        creditor: AgentRef::Member(5),
        debtor: AgentRef::Fresh { owner: 5, n: 3 },
        amount_minor: 1_000,
        term: 30,
        arb: None,
    };
    let c = compose(&d.st, &world, 5, &intent);
    assert!(c.signers.contains(&fresh_key(5, 3)), "the key being seated has to sign");
    assert!(c.asks.iter().all(|a| a.ask.member != 5), "the actor is never asked about itself");
    assert_eq!(c.asks.len(), 0, "there is nobody to ask about a key nobody holds");
}

/// **A member the actor does not control is ASKED, and its key is on the
/// envelope only if it said yes.** The decline is the pending-signature pool,
/// which is why it is counted apart from a refusal.
#[test]
fn a_counterparty_is_asked_and_a_decline_leaves_its_key_off() {
    let (d, world) = scene();
    let intent = Intent::Lend {
        creditor: AgentRef::Member(5),
        debtor: AgentRef::Member(6),
        amount_minor: 1_000,
        term: 30,
        arb: None,
    };
    let c = compose(&d.st, &world, 5, &intent);
    assert_eq!(c.asks.len(), 1, "one counterparty, one ask");
    assert_eq!(c.asks[0].ask.member, 6);
    assert!(c.asks[0].required, "a debtor who will not sign is not a trade");
    assert!(!edet_state::authorises(&d.st, &c.tx, &c.signers), "without the ask granted it authorises nothing");
}

/// **The creditor's signature on a `Transfer` is the one OPTIONAL ask**: the
/// ledger refuses the uninsured swap without it and takes the insured one
/// without, so which it needed is the ledger's answer and not the adapter's.
#[test]
fn the_creditor_of_a_transfer_is_asked_but_not_required() {
    let (d, world) = scene();
    let c = compose(&d.st, &world, 4, &Intent::Transfer { contract: 4, new_debtor: 6 });
    let creditor = c.asks.iter().find(|a| a.ask.member == 5).expect("the creditor is asked");
    assert!(!creditor.required, "a declining creditor drops their key rather than killing the intent");
    let successor = c.asks.iter().find(|a| a.ask.member == 6).expect("the successor is asked");
    assert!(successor.required, "a debtor swap needs the successor");
}
