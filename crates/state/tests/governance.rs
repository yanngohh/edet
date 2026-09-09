//! Who governs, and why the measure is a share of the external seed.
//!
//! The assent weight was a cut — `capacity(assenters) + Σ supply the assenters
//! declared`, over the declared total — and it was capturable, and underneath
//! the capture it was measuring the wrong thing. Both are constructions here,
//! and both are the reason the weight is now a share of the EXTERNAL seed.
//!
//! **The capture.** Every declared supply entered that numerator twice at face
//! value: as a source arc feeding the tail's capacity, and as the tail's own
//! supply. So a chain of accomplices, each fake-backed by all the previous ones
//! (two signatures and no delivery) and each declaring the maximum the §Standing cap
//! allows, doubled the tail's weight per link while the coalition's real
//! capacity never moved.
//!
//! **The deeper fault, which no cap on the capture would have fixed.** A cut
//! measures who the seed REACHES, not who put it up: a founder declaring the
//! whole seed and backing one member for all of it has capacity zero, and the
//! member they backed enacted alone. That is the measure meaning the wrong
//! thing rather than an attack on it, and it is why the split that was proposed —
//! constitutional kinds on the seed, "advisory" constants left on the cut —
//! was not taken. There is no tier on which a wrong measure is fine.
//!
//! So: **the ceremony governs, for every kind.** `Σ external supply of the
//! assenters` over `Σ external supply`. Members still propose.

mod common;

use std::collections::BTreeSet;

use common::{key, Chain, SUPPLY};
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

fn supply_of(c: &Chain, id: MemberId) -> f64 {
    State::from_minor(c.st.underwriters.get(&id).copied().unwrap_or(0))
}

/// A member's ceremony-seated supply. The same map as `supply_of` since the
/// two are one map: a declaration cannot be raised, so every unit of every
/// supply came through genesis or an amendment. Kept as a separate reading
/// because the two answer different questions.
fn external_of(c: &Chain, id: MemberId) -> f64 {
    supply_of(c, id)
}

/// Every governed constant, bit for bit, so a probe can assert that a refused
/// coalition moved NOTHING rather than only the one field it thought to check.
fn constants(c: &Chain) -> Vec<u64> {
    [ParamKey::RiskK, ParamKey::SealAmounts, ParamKey::BondFraction, ParamKey::StakeDecay, ParamKey::SeedRate]
        .into_iter()
        .map(|k| c.st.params.get(k).to_bits())
        .chain([c.st.params.v_base.to_bits(), c.st.params.dust.to_bits(), c.st.params.theta_adopt.to_bits()])
        .collect()
}

/// The construction: one honest round-trip seeds the head of a
/// chain, and every accomplice after it is fake-backed by ALL the previous
/// ones and then tries to declare the maximum its own capacity would carry.
///
/// Nothing here is forged. Every settlement takes the two signatures it is
/// supposed to take, and the community's real capacity is exactly the one
/// honest trade throughout — which is the point.
///
/// **Every declaration in it is refused**: a supply may only be seated by a
/// ceremony, so the chain cannot start, let alone double per link. The helper
/// asserts the refusal rather than skipping the call, because the coalition's
/// whole leverage is that the call SUCCEEDS — a version of this that simply
/// stopped declaring would stay green if the door opened.
fn chain_of_accomplices(c: &mut Chain, founders: usize, honest: f64, links: usize) -> Vec<MemberId> {
    let mut js = Vec::new();
    let j1 = founders as MemberId;
    c.back(0, j1, honest);
    js.push(j1);
    c.err(Tx::DeclareSupply { member: j1, supply: c.cap(j1) }, &[key(j1 as usize)], ET_UWR_ABOVE_CAPACITY);
    for n in 1..links {
        let j = (founders + n) as MemberId;
        for &prev in &js {
            c.back(prev, j, honest * 4096.0);
        }
        let cap = c.cap(j);
        c.err(Tx::DeclareSupply { member: j, supply: cap }, &[key(j as usize)], ET_UWR_ABOVE_CAPACITY);
        js.push(j);
    }
    assert!(js.iter().all(|j| supply_of(c, *j) == 0.0), "not one accomplice holds a supply");
    js
}

/// Would the OLD weight have carried this coalition? Asserted inside each
/// capture probe rather than described in prose, so the gate says exactly what
/// it is defending and fails loudly if the construction ever stops being one.
fn the_old_weight(c: &Chain, assenters: &[MemberId]) -> f64 {
    let declared: u64 = c.st.underwriters.values().sum();
    let own: u64 = assenters.iter().filter_map(|id| c.st.underwriters.get(id)).sum();
    (c.st.capacity_of_set(assenters) + State::from_minor(own)) / State::from_minor(declared)
}

// ------------------------------------------------------------ the capture --

/// **Seed 15,000, one honest 300 trade, seven accomplices — and the tail alone
/// took the validator set.**
///
/// The realistic scene: six founders at 2500 apiece. The whole coalition's real
/// capacity is the single 300 trade that seeded it, start to finish. Under the
/// cut the tail's own weight was 9,600 of capacity plus 9,600 of declared
/// supply over a declared total of 34,200 — 0.56, past Θ — and it enacted
/// `ValidatorPower{tail, 1e6}`, removed the founder's power, and suspended the
/// founder.
///
/// **Two independent refusals now stand between the coalition and the vote**,
/// and the probe holds both. The declarations are refused outright, so the
/// inflation never happens — 15,000 declared, not 34,200, and the tail's
/// capacity stays at the one honest trade rather than reaching 9,600. And the
/// weight is a share of the seed, so even the capacity they DO hold buys
/// nothing. The second is what this file is about, so the scene keeps a member
/// the founders genuinely backed as well: real capacity, no ceremony, no vote.
#[test]
fn a_chain_of_internal_declarations_carries_no_vote() {
    let mut c = Chain::founded(6, 12);
    c.validator(0);
    let js = chain_of_accomplices(&mut c, 6, 300.0, 7);
    let tail = *js.last().unwrap();

    // The construction, pinned. If any of these move, the probe below is
    // measuring some other scene and its green means nothing.
    assert_eq!(c.st.external_seed(), 15_000.0, "the roll is the seed the founders put up, and nothing else");
    assert_eq!(supply_of(&c, tail), 0.0, "the tail declares nothing, because it may not");
    assert_eq!(c.cap(tail), 300.0, "and its capacity is the one honest trade, not 9,600");
    assert_eq!(c.st.capacity_of_set(&js), 300.0, "one honest trade is the whole coalition's real capacity");
    assert!(js.iter().all(|j| external_of(&c, *j) == 0.0), "no ceremony seated any of them");

    // The other half, which the refusal above must not be allowed to hide:
    // a member the founders backed for real holds capacity the old measure
    // would have counted, and holds no vote.
    let honest_recipient: MemberId = 17;
    c.back(0, honest_recipient, 2500.0);
    assert_eq!(c.cap(honest_recipient), 2500.0);
    assert!(
        the_old_weight(&c, &[honest_recipient]) >= c.st.params.theta_adopt / 6.0,
        "PROVEN: the old weight gave the recipient a share it never earned, {}",
        the_old_weight(&c, &[honest_recipient])
    );
    let honest_pid = c.st.next_proposal;
    c.ok(
        Tx::Propose { author: honest_recipient, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 } },
        &[key(honest_recipient as usize)],
    );
    c.err(
        Tx::Assent { member: honest_recipient, proposal: honest_pid },
        &[key(honest_recipient as usize)],
        ET_GOV_NO_MANDATE,
    );

    // The tail registers a consensus key first, so the proposal below is
    // admissible on its own terms and the refusal this probe is about is the
    // MANDATE — a probe whose target is refused for a second reason measures
    // the second reason.
    c.ok(Tx::SetConsensusKey { member: tail, key: Some(common::consensus_key(tail as usize)) }, &[key(tail as usize)]);

    // And now it carries nothing. Not the tail alone, and not the coalition
    // entire — which under the old rule held 34,200 of declared standing
    // between them and today holds none at all.
    let pid = c.st.next_proposal;
    c.ok(
        Tx::Propose { author: tail, kind: ProposalKind::ValidatorPower { member: tail, power: 1_000_000 } },
        &[key(tail as usize)],
    );
    for &j in &js {
        c.err(Tx::Assent { member: j, proposal: pid }, &[key(j as usize)], ET_GOV_NO_MANDATE);
    }
    assert!(!c.st.proposals[&pid].enacted);
    assert!(c.st.proposals[&pid].assents.is_empty(), "a refused assent leaves no trace, so it cannot accumulate");
    assert_eq!(c.st.validators, [(0, 1)].into_iter().collect(), "the validator set is untouched");
    assert_eq!(c.status(0), MemberStatus::Active, "and so is the founder");
}

/// The smallest form the audit found, and the sharpest: seed 100, one
/// founder, three links — a declared 500 against an external 100 — and the
/// tail alone enacts `ValidatorPower{tail, 1e6}` against a founder holding a
/// power of 1.
#[test]
fn the_smallest_capture_is_refused_too() {
    let mut c = Chain::founded_with(&[100.0], 8);
    c.validator(0);
    let js = chain_of_accomplices(&mut c, 1, 100.0, 3);
    let tail = *js.last().unwrap();
    c.ok(Tx::SetConsensusKey { member: tail, key: Some(common::consensus_key(tail as usize)) }, &[key(tail as usize)]);

    assert_eq!(c.st.external_seed(), 100.0, "500 was declared here once; the door it came through is shut");
    assert_eq!(c.st.capacity_of_set(&js), 100.0, "the coalition never held more than the one honest trade");

    let pid = c.st.next_proposal;
    c.ok(
        Tx::Propose { author: tail, kind: ProposalKind::ValidatorPower { member: tail, power: 1_000_000 } },
        &[key(tail as usize)],
    );
    c.err(Tx::Assent { member: tail, proposal: pid }, &[key(tail as usize)], ET_GOV_NO_MANDATE);
    assert_eq!(c.st.validators, [(0, 1)].into_iter().collect());
}

/// **One measure, every kind** — the half of the decision that goes beyond
/// what was proposed.
///
/// One proposal offered to leave the "advisory constants" on the cut. `ParamKey` does
/// not have any: `StakeDecay` at either end of its safe range is a credit
/// freeze enacted as a rounding rule, `BondFraction` at its ceiling is
/// censorship by arithmetic (both in `Params::safe_range`'s own words), and
/// `SeedRate` governs how fast the electorate itself may grow now that the
/// seed is the electorate. What was left as genuinely advisory was one key,
/// and a second weight for one key would install "one economic act, two code
/// paths, opposite rules" — this model's most expensive shape — deliberately.
///
/// So the coalition that captured the cut is walked over the WHOLE alphabet of
/// proposals, and the probe asserts not just that each was refused but that
/// every governed constant is bit-unchanged afterwards.
#[test]
fn every_proposal_kind_answers_to_the_ceremony() {
    let mut c = Chain::founded(6, 12);
    c.validator(0);
    let js = chain_of_accomplices(&mut c, 6, 300.0, 7);
    let tail = *js.last().unwrap();
    let accomplice = js[0];
    c.ok(Tx::SetConsensusKey { member: tail, key: Some(common::consensus_key(tail as usize)) }, &[key(tail as usize)]);
    let before = constants(&c);

    let kinds = [
        ProposalKind::ParamChange { key: ParamKey::RiskK, value: 0.25 },
        ProposalKind::ParamChange { key: ParamKey::SealAmounts, value: 1.0 },
        ProposalKind::ParamChange { key: ParamKey::BondFraction, value: 0.10 },
        ProposalKind::ParamChange { key: ParamKey::StakeDecay, value: 900.0 },
        ProposalKind::ParamChange { key: ParamKey::SeedRate, value: 0.001 },
        ProposalKind::Redenominate { num: 3, den: 2 },
        ProposalKind::Suspend { member: 0 },
        ProposalKind::Unsuspend { member: 0 },
        ProposalKind::ValidatorPower { member: tail, power: 1_000_000 },
        ProposalKind::SeedAmendment { amount: 100.0 },
    ];
    for kind in kinds {
        let pid = c.st.next_proposal;
        c.ok(Tx::Propose { author: tail, kind }, &[key(tail as usize)]);
        // The tail assents its own where it may, and an accomplice assents the
        // amendment — whose author is its beneficiary and may not vote on it.
        let voter =
            if matches!(c.st.proposals[&pid].kind, ProposalKind::SeedAmendment { .. }) { accomplice } else { tail };
        c.err(Tx::Assent { member: voter, proposal: pid }, &[key(voter as usize)], ET_GOV_NO_MANDATE);
        assert!(!c.st.proposals[&pid].enacted, "nothing the coalition proposed may enact");
    }
    assert_eq!(constants(&c), before, "every governed constant bit-unchanged");
    assert_eq!(c.st.validators, [(0, 1)].into_iter().collect());
    assert_eq!(c.status(0), MemberStatus::Active);
    // Proposing was never the thing to refuse: members put changes on the
    // record, and the ceremony decides. Ten proposals stand, none enacted.
    assert_eq!(c.st.proposals.len(), 10);
    assert!(c.st.proposals.values().all(|p| !p.enacted && p.assents.is_empty()));
}

// -------------------------------------------- the measure, not the capture --

/// **The seed votes where it was put up, not where it reached** — the second
/// caveat, and the one that decided against keeping a cut anywhere.
///
/// A founder declares the whole seed and backs one member for all of it. The
/// founder's own capacity is zero: a cut into an underwriter draws on the OTHER
/// underwriters, and there are none. So under the cut the member they backed
/// held 100% of the weight and enacted alone, while the only person who had
/// accepted a liability held their supply term and nothing else.
///
/// This is not a capture — every step is honest and intended — which is
/// exactly why capping the capture would not have reached it.
#[test]
fn the_seed_votes_where_it_was_put_up_not_where_it_reached() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    assert_eq!(c.cap(0), 0.0, "the founder who put up the whole seed has no capacity of their own");
    assert_eq!(c.cap(1), SUPPLY, "and the member they backed holds all of it");
    assert!(
        the_old_weight(&c, &[1]) >= c.st.params.theta_adopt,
        "PROVEN: under the cut, the recipient was the electorate"
    );

    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 1, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 } }, &[key(1)]);
    c.err(Tx::Assent { member: 1, proposal: pid }, &[key(1)], ET_GOV_NO_MANDATE);
    assert_eq!(c.st.params.risk_k, edet_kernel::constants::RISK_K, "the recipient enacts nothing");

    // The founder does, and alone — they are the whole ceremony.
    c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
    assert_eq!(c.st.params.risk_k, 1.0);
}

/// **The enacting sets are upward-closed**: assenting can only ever help.
///
/// That is the least a voting rule must promise, and under the cut it was a
/// patch rather than a property — the second term existed precisely because a
/// cut over a set draws supply only from underwriters OUTSIDE it, so an
/// underwriter joining a coalition removed their own supply from what fed it.
/// A share of the seed is a plain sum over a deduplicated set, so monotonicity
/// is structural. Walked over all 32 subsets of five unequal founders.
#[test]
fn assenting_can_only_ever_help() {
    let supplies = [100.0, 200.0, 400.0, 800.0, 1600.0];
    let enacts = |members: &[MemberId]| -> bool {
        let mut c = Chain::founded_with(&supplies, 0);
        let pid = c.st.next_proposal;
        c.ok(
            Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 } },
            &[key(0)],
        );
        for &m in members {
            c.ok(Tx::Assent { member: m, proposal: pid }, &[key(m as usize)]);
        }
        c.st.proposals[&pid].enacted
    };
    let all: Vec<MemberId> = (0..supplies.len() as MemberId).collect();
    let subset = |mask: u32| -> Vec<MemberId> { all.iter().copied().filter(|&m| mask >> m & 1 == 1).collect() };

    let carries: Vec<bool> = (0..32).map(|mask| enacts(&subset(mask))).collect();
    for mask in 0..32u32 {
        for bit in 0..5 {
            let bigger = mask | 1 << bit;
            assert!(
                !carries[mask as usize] || carries[bigger as usize],
                "{:?} enacted and its superset {:?} did not",
                subset(mask),
                subset(bigger)
            );
        }
    }
    assert!(carries[0b11111], "the whole community must be able to govern itself");
    assert!(!carries[0b01111], "and 1500 of 3100 must not — Θ is a real bar, not a formality");
    assert!(carries[0b10000], "while 1600 alone clears it");
}

// ------------------------------------------------------------ the electorate --

/// **The electorate grows by exactly the door that grows the seed**, and by no
/// other. An endorsed newcomer votes the amount that was endorsed — not the
/// capacity they may later earn, and not the supply they may later declare
/// against it.
#[test]
fn the_electorate_grows_only_through_the_ceremony() {
    let mut c = Chain::founded(4, 2);
    let newcomer = 4;
    assert_eq!(c.st.external_seed(), 4.0 * SUPPLY);

    // Before: backed to the hilt, capacity 2500, and mute.
    c.back(0, newcomer, SUPPLY);
    assert_eq!(c.cap(newcomer), SUPPLY);
    let pid = c.st.next_proposal;
    c.ok(
        Tx::Propose { author: newcomer, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 } },
        &[key(newcomer as usize)],
    );
    c.err(Tx::Assent { member: newcomer, proposal: pid }, &[key(newcomer as usize)], ET_GOV_NO_MANDATE);

    // A declaration against that capacity is refused outright — it is the
    // capture in miniature, and it is why the weight is not the declared
    // total. **Both refusals are asserted**, because the vote must stay
    // unbuyable if the declaration door ever opens.
    c.err(Tx::DeclareSupply { member: newcomer, supply: SUPPLY }, &[key(newcomer as usize)], ET_UWR_ABOVE_CAPACITY);
    assert_eq!(supply_of(&c, newcomer), 0.0);
    c.err(Tx::Assent { member: newcomer, proposal: pid }, &[key(newcomer as usize)], ET_GOV_NO_MANDATE);

    // The ceremony does, and for exactly what it endorsed.
    let amount = 100.0;
    let amendment = c.st.next_proposal;
    c.ok(
        Tx::Propose { author: newcomer, kind: ProposalKind::SeedAmendment { amount } },
        &[key(newcomer as usize), key(0)],
    );
    for a in [0u64, 1] {
        c.ok(Tx::Assent { member: a, proposal: amendment }, &[key(a as usize)]);
    }
    assert!(c.st.proposals[&amendment].enacted);
    assert_eq!(external_of(&c, newcomer), amount, "seated for what was endorsed");
    assert_eq!(c.st.external_seed(), 4.0 * SUPPLY + amount, "and the denominator grew by the same");
    c.ok(Tx::Assent { member: newcomer, proposal: pid }, &[key(newcomer as usize)]);
    assert!(!c.st.proposals[&pid].enacted, "100 of 10,100 is not Θ — the seat is real, and it is small");
}

/// **Amending dilutes the coalition that amends** — the second thing to
/// weigh, and it survives the change intact.
///
/// Every amendment enlarges the seed, which is now governance's own
/// denominator, so a fixed coalition is diluted by exactly the commitment it
/// admits. Measured: three of six founders are Θ on the nose, they endorse
/// one amendment, and they are below it — reaching the rate bound at all takes
/// a community that keeps re-forming its coalition around the members it has
/// just seated.
#[test]
fn amending_dilutes_the_coalition_that_amends() {
    let mut c = Chain::founded(6, 2);
    let seed = 6.0 * SUPPLY;
    assert_eq!(c.st.external_seed(), seed);

    // Three of six is Θ exactly, so this coalition can act and has nothing
    // spare — the sharpest place to measure a dilution.
    let amount = c.st.params.seed_rate_bounded() * seed;
    let first = c.st.next_proposal;
    c.ok(Tx::Propose { author: 6, kind: ProposalKind::SeedAmendment { amount } }, &[key(6), key(0)]);
    for a in [0u64, 1, 2] {
        c.ok(Tx::Assent { member: a, proposal: first }, &[key(a as usize)]);
    }
    assert!(c.st.proposals[&first].enacted, "three of six is Θ on the nose");
    assert_eq!(c.st.external_seed(), seed + amount);

    // The same three, next epoch, with the epoch's allowance restored: the
    // coalition is now 7500 of 15,300 and carries nothing.
    c.goto(1);
    let second = c.st.next_proposal;
    c.ok(Tx::Propose { author: 7, kind: ProposalKind::SeedAmendment { amount } }, &[key(7), key(0)]);
    for a in [0u64, 1, 2] {
        c.ok(Tx::Assent { member: a, proposal: second }, &[key(a as usize)]);
    }
    assert!(!c.st.proposals[&second].enacted, "the coalition was diluted by the seed it admitted");

    // A fourth founder — or the member they just seated — carries it. The
    // coalition has to re-form, which is the property worth keeping.
    c.ok(Tx::Assent { member: 6, proposal: second }, &[key(6)]);
    assert!(c.st.proposals[&second].enacted, "the member the ceremony seated is in the electorate now");
}

/// **Zero is absorbing here too, and this is the one behaviour the change
/// costs.** Under the cut, a community whose external seed had lapsed could
/// still govern on the strength of internal declarations. It cannot now: the
/// denominator is the seed, and a community that has withdrawn every external
/// commitment has no mandate to change anything — including, and this is the
/// honest half, the amendment that would give it one.
///
/// That is the same statement §Adoption makes about credit, applied to votes: what a
/// community can never do is leave the state of having underwritten nothing.
#[test]
fn a_community_that_has_withdrawn_its_seed_cannot_govern() {
    let mut c = Chain::founded(1, 1);
    c.back(0, 1, SUPPLY);
    // The member the founder backed cannot take the role over: a declaration
    // against conferred capacity is refused, so the community's
    // underwriting cannot survive its founder by this route.
    c.err(Tx::DeclareSupply { member: 1, supply: SUPPLY }, &[key(1)], ET_UWR_ABOVE_CAPACITY);

    // The proposal is drafted while the community still has a seed, because
    // after the withdrawal nobody can WRITE either: the write floor reads the
    // seed's reach, and a community whose seed has gone reaches nobody. That
    // is the same fact one layer down, and it would otherwise hide the assent
    // refusal this probe is about behind a bond refusal.
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 1, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 } }, &[key(1)]);

    // The founder leaves the role. Nothing is committed through them, so §Stability's
    // floor is zero and the withdrawal is ordinary.
    c.ok(Tx::DeclareSupply { member: 0, supply: 0.0 }, &[key(0)]);
    assert_eq!(c.st.external_seed(), 0.0, "the seed is gone, and nothing internal was ever standing in for it");

    c.err(Tx::Assent { member: 1, proposal: pid }, &[key(1)], ET_GOV_NO_MANDATE);
    c.err(Tx::Assent { member: 0, proposal: pid }, &[key(0)], ET_GOV_NO_MANDATE);
    assert!(!c.st.proposals[&pid].enacted);
}

/// **A withdrawal takes the vote down with it, and the endorsement cannot be
/// topped up from inside.**
///
/// The old form of this probe pinned an ORDERING: a declaration was part
/// endorsed and part declared against the member's own capacity, and a
/// withdrawal retracted the internal half first, so a member who trimmed a
/// 2,600 declaration to 500 kept the 100 the ceremony had seated. There is no
/// internal half any longer — a raise is refused, so the whole of a
/// declaration is what a ceremony put there — which retires the ordering and
/// leaves the property it was protecting: the electorate is exactly the roll,
/// and it follows a withdrawal down to nothing.
#[test]
fn a_withdrawal_takes_the_endorsed_vote_with_it() {
    let mut c = Chain::founded(4, 1);
    let m = 4;
    c.back(0, m, SUPPLY);
    let amount = 100.0;
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: m, kind: ProposalKind::SeedAmendment { amount } }, &[key(m as usize), key(0)]);
    for a in [0u64, 1] {
        c.ok(Tx::Assent { member: a, proposal: pid }, &[key(a as usize)]);
    }
    assert_eq!(external_of(&c, m), amount, "the ceremony seated it, and it is the whole of their supply");
    // Their own capacity is 2,500 and buys them not one unit more of it.
    assert_eq!(c.cap(m), SUPPLY);
    c.err(Tx::DeclareSupply { member: m, supply: SUPPLY + amount }, &[key(m as usize)], ET_UWR_ABOVE_CAPACITY);
    assert_eq!(external_of(&c, m), amount);

    let electorate: BTreeSet<MemberId> = c.st.underwriters.keys().copied().collect();
    assert_eq!(electorate, [0, 1, 2, 3, m].into_iter().collect());

    // Down, and the vote goes with it.
    c.ok(Tx::DeclareSupply { member: m, supply: 40.0 }, &[key(m as usize)]);
    assert_eq!(external_of(&c, m), 40.0);
    c.ok(Tx::DeclareSupply { member: m, supply: 0.0 }, &[key(m as usize)]);
    assert_eq!(external_of(&c, m), 0.0, "and leaving the role leaves the electorate");
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 } }, &[key(0)]);
    c.err(Tx::Assent { member: m, proposal: pid }, &[key(m as usize)], ET_GOV_NO_MANDATE);
}

// ------------------------------------------- who orders the ledger --

/// **Half the seed changes a constant; it does not change who orders the
/// ledger.**
///
/// At one threshold for every kind, whoever holds half the external seed can —
/// alone, in one epoch — remove every other validator down to the ledger's own
/// floor of one and suspend anybody who objects. What makes that different in
/// kind from an over-far parameter is recoverability: a constant is moved back through the same door, and every
/// value inside its constitutional range is one the ledger keeps working at,
/// while the coalition that holds the validator set decides which blocks
/// exist, including the ones that would undo it.
///
/// Six founders at 2,500 apiece. Three of them are exactly half the seed, which
/// is Θ on the nose — and is measured here rather than assumed, by carrying an
/// ordinary `ParamChange` with the same three.
#[test]
fn half_the_seed_moves_a_constant_and_not_the_validator_set() {
    let mut c = Chain::founded(6, 0);
    for v in [0u64, 4, 5] {
        c.validator(v);
    }
    let half = [0u64, 1, 2];

    // The bar, measured on a kind that is not about the order.
    let param = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 0.6 } }, &[key(0)]);
    for &a in &half {
        c.ok(Tx::Assent { member: a, proposal: param }, &[key(a as usize)]);
    }
    assert!(c.st.proposals[&param].enacted, "PROVEN: three of six is Θ_adopt exactly");
    assert_eq!(c.st.params.risk_k, 0.6);

    // The same three, on the validator set. Refused for want of mass rather
    // than refused outright — the assents are recorded and simply do not
    // reach the bar, which is what makes this a threshold and not a veto.
    let removal = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::ValidatorPower { member: 4, power: 0 } }, &[key(0)]);
    for &a in &half {
        c.ok(Tx::Assent { member: a, proposal: removal }, &[key(a as usize)]);
    }
    assert!(!c.st.proposals[&removal].enacted, "half the seed must not remove a validator");
    assert!(c.st.validators.contains_key(&4), "and the set is untouched");

    // Suspension is the same act one name over, and it is in the same class:
    // suspension removes a validator, and a rule keyed on the proposal KIND alone
    // would have left this door open.
    let suspend = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::Suspend { member: 4 } }, &[key(0)]);
    for &a in &half {
        c.ok(Tx::Assent { member: a, proposal: suspend }, &[key(a as usize)]);
    }
    assert!(!c.st.proposals[&suspend].enacted, "nor suspend one");
    assert_eq!(c.status(4), MemberStatus::Active);

    // Two thirds does both. The fourth founder is the whole of the difference.
    c.ok(Tx::Assent { member: 3, proposal: removal }, &[key(3)]);
    assert!(c.st.proposals[&removal].enacted, "four of six clears the validator bar");
    assert!(!c.st.validators.contains_key(&4));
    c.ok(Tx::Assent { member: 3, proposal: suspend }, &[key(3)]);
    assert!(c.st.proposals[&suspend].enacted);
    assert_eq!(c.status(4), MemberStatus::Suspended);
}

/// **Suspending a member who orders nothing is an ordinary proposal**, so the
/// higher bar is a rule about the ACT rather than about the word: it reads the
/// target's voting power off committed state, which every node agrees on.
#[test]
fn suspending_a_member_who_is_not_a_validator_takes_the_ordinary_bar() {
    let mut c = Chain::founded(6, 1);
    c.validator(0);
    let ordinary = 6u64;
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::Suspend { member: ordinary } }, &[key(0)]);
    for a in [0u64, 1, 2] {
        c.ok(Tx::Assent { member: a, proposal: pid }, &[key(a as usize)]);
    }
    assert!(c.st.proposals[&pid].enacted, "half the seed suspends a member who runs no validator");
    assert_eq!(c.status(ordinary), MemberStatus::Suspended);
}

/// **A validator whose signing key the ledger cannot name cannot be seated**,
/// in either direction: no power without a registered consensus key, and no
/// retiring the key while the power stands.
///
/// One key for both roles makes a validator host compromise hand the attacker
/// the operator's economic identity. The set fails CLOSED on a validator it
/// cannot resolve, which is a halt, so the ledger must never reach a state with
/// one in it.
#[test]
fn voting_power_needs_a_registered_consensus_key() {
    let mut c = Chain::founded(6, 1);
    c.validator(0);
    let operator = 6u64;
    c.back(0, operator, SUPPLY);

    // Proposed and refused at the door: the operator has registered nothing.
    c.err(
        Tx::Propose { author: 0, kind: ProposalKind::ValidatorPower { member: operator, power: 1 } },
        &[key(0)],
        ET_VAL_NO_CONSENSUS_KEY,
    );

    // The key is the member's own act, it is not one of `keys`, and it can
    // never sign a transaction.
    let ck = common::consensus_key(operator as usize);
    c.ok(Tx::SetConsensusKey { member: operator, key: Some(ck) }, &[key(operator as usize)]);
    assert_eq!(c.st.members[&operator].consensus_key, Some(ck));
    assert!(!c.st.members[&operator].keys.contains(&ck), "a consensus key is not a member key");
    assert_eq!(c.st.member_of_key(&ck), None, "and it resolves to nobody as a signer");
    c.err(Tx::Exit { member: operator }, &[ck], ET_MEM_NOT_SIGNER);

    // Nobody else may take it, and it may not be somebody's member key.
    c.err(Tx::SetConsensusKey { member: 1, key: Some(ck) }, &[key(1)], ET_VAL_KEY_IN_USE);
    c.err(Tx::SetConsensusKey { member: 1, key: Some(key(0)) }, &[key(1)], ET_VAL_KEY_IN_USE);

    // Now the community can seat them. Four of six, because seating a
    // validator is a change to who orders the ledger.
    let pid = c.st.next_proposal;
    c.ok(Tx::Propose { author: 0, kind: ProposalKind::ValidatorPower { member: operator, power: 1 } }, &[key(0)]);
    for a in [0u64, 1, 2, 3] {
        c.ok(Tx::Assent { member: a, proposal: pid }, &[key(a as usize)]);
    }
    assert_eq!(c.st.validators.get(&operator), Some(&1));

    // And the key cannot be retired out from under the power.
    c.err(Tx::SetConsensusKey { member: operator, key: None }, &[key(operator as usize)], ET_VAL_NO_CONSENSUS_KEY);
    assert_eq!(c.st.members[&operator].consensus_key, Some(ck));
}

// ------------------------------------------------------- the review's probes --

mod final_review_governance {
    use super::*;

    fn enact_by(c: &mut Chain, author: MemberId, kind: ProposalKind) -> ProposalId {
        let pid = c.st.next_proposal;
        c.ok(Tx::Propose { author, kind }, &[key(author as usize)]);
        c.ok(Tx::Assent { member: author, proposal: pid }, &[key(author as usize)]);
        assert!(c.st.proposals[&pid].enacted, "the author carries the bar alone in this fixture");
        pid
    }

    /// **The second assent refuses.** The assent set absorbed a duplicate
    /// silently, so a re-assent was admitted without limit — free, and a
    /// durable replay id each time.
    ///
    /// Mutation that bites: drop the `contains` check in `assent`.
    #[test]
    fn a_second_assent_is_refused() {
        let mut c = Chain::founded(3, 0);
        let pid = c.st.next_proposal;
        c.ok(
            Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 } },
            &[key(0)],
        );
        c.ok(Tx::Assent { member: 0, proposal: pid }, &[key(0)]);
        for _ in 0..8 {
            c.err(Tx::Assent { member: 0, proposal: pid }, &[key(0)], ET_GOV_ALREADY_ASSENTED);
        }
        assert_eq!(c.st.proposals[&pid].assents.len(), 1);
        assert!(!c.st.proposals[&pid].enacted, "one of three is below the bar");
        c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
        assert!(c.st.proposals[&pid].enacted, "and a second voter still carries it");
    }

    /// **The sanctioned may propose and vote on reinstatement, and on nothing
    /// else.** Half the seed suspended everyone else and withdrew, and no
    /// proposal of any kind could ever enact again: the suspended could not
    /// vote, the withdrawer held no seed, and the electorate had sanctioned
    /// itself out of existence. A vote on reinstatement alone undoes that
    /// without moving anybody's weight — a majority of the seed cannot be held
    /// suspended, and a minority still cannot reinstate itself.
    ///
    /// Mutation that bites: `require_active` back in `assent` and `propose`;
    /// the first `Propose` below is `ET-MEM-002` and nothing enacts again.
    #[test]
    fn the_sanctioned_may_vote_on_reinstatement_and_on_nothing_else() {
        let mut c = Chain::founded_with(&[5_000.0, 3_000.0, 2_000.0], 1);
        // The ordinary member's standing comes from a seed that stays: 0 is
        // about to withdraw, and reach through a withdrawn supply is nothing.
        c.back(1, 3, 500.0);
        enact_by(&mut c, 0, ProposalKind::Suspend { member: 1 });
        enact_by(&mut c, 0, ProposalKind::Suspend { member: 2 });
        c.ok(Tx::DeclareSupply { member: 0, supply: 0.0 }, &[key(0)]);
        assert!(!c.st.underwriters.contains_key(&0), "the withdrawer holds no seed");
        assert_eq!(c.st.external_seed(), 5_000.0, "and every unit left is a suspended member's");

        // Suspended, 1 may put reinstatement on the record — paid for by 3, an
        // ordinary member with standing, since a suspended budget is zero —
        // and nothing else.
        let pid = c.st.next_proposal;
        c.ok(Tx::Propose { author: 1, kind: ProposalKind::Unsuspend { member: 1 } }, &[key(1), key(3)]);
        c.err(
            Tx::Propose { author: 1, kind: ProposalKind::Suspend { member: 3 } },
            &[key(1), key(3)],
            ET_MEM_NOT_ACTIVE,
        );
        c.err(
            Tx::Propose { author: 1, kind: ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 } },
            &[key(1), key(3)],
            ET_MEM_NOT_ACTIVE,
        );
        // Three fifths of the seed, suspended, carries its own reinstatement.
        c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
        assert!(c.st.proposals[&pid].enacted);
        assert_eq!(c.status(1), MemberStatus::Active);

        // Two fifths cannot; once 1 is back, 1 carries it.
        let pid = c.st.next_proposal;
        c.ok(Tx::Propose { author: 2, kind: ProposalKind::Unsuspend { member: 2 } }, &[key(2), key(3)]);
        c.ok(Tx::Assent { member: 2, proposal: pid }, &[key(2)]);
        assert!(!c.st.proposals[&pid].enacted, "a minority cannot reinstate itself");
        c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
        assert!(c.st.proposals[&pid].enacted);
        assert_eq!(c.status(2), MemberStatus::Active);
    }

    /// **The validator floor is genesis data, and every removal path holds
    /// it.** Held only at the ceremony, a four was a three on one member's
    /// free `Exit` and a one after three, on no vote at all.
    ///
    /// Mutation that bites: read `k::MIN_VALIDATORS` again in `exit`; the
    /// founder leaves and the set is three.
    #[test]
    fn the_validator_floor_holds_every_removal_path() {
        let mut c = Chain::founded(4, 1);
        for i in 0..5 {
            c.validator(i);
        }
        c.st.params.min_validators = 4;
        assert_eq!(State::default().params.min_validators, 1, "the dev chain founds at one");

        // Five validators: the ordinary member may leave.
        c.ok(Tx::Exit { member: 4 }, &[key(4)]);
        assert_eq!(c.st.validators.len(), 4);
        // At the floor nobody may — not by exit, not by vote, not by suspension.
        c.ok(Tx::DeclareSupply { member: 0, supply: 0.0 }, &[key(0)]);
        c.err(Tx::Exit { member: 0 }, &[key(0)], ET_VAL_LAST_VALIDATOR);

        let pid = c.st.next_proposal;
        c.ok(Tx::Propose { author: 1, kind: ProposalKind::ValidatorPower { member: 2, power: 0 } }, &[key(1)]);
        c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
        c.err(Tx::Assent { member: 3, proposal: pid }, &[key(3)], ET_VAL_LAST_VALIDATOR);
        assert!(!c.st.proposals[&pid].enacted);

        let pid = c.st.next_proposal;
        c.ok(Tx::Propose { author: 1, kind: ProposalKind::Suspend { member: 2 } }, &[key(1)]);
        c.ok(Tx::Assent { member: 1, proposal: pid }, &[key(1)]);
        c.err(Tx::Assent { member: 3, proposal: pid }, &[key(3)], ET_VAL_LAST_VALIDATOR);
        assert_eq!(c.st.validators.len(), 4);
        assert_eq!(c.status(2), MemberStatus::Active);
    }

    /// **A unit that would round to nothing is refused at both doors.** Zero
    /// is absorbing everywhere else; here it would open — every bonded class
    /// free, every seat free — after enough downward re-denomination with the
    /// fraction at its floor.
    ///
    /// Mutation that bites: drop `unit_survives`; the proposals below are
    /// admitted and the assent enacts a free ledger.
    #[test]
    fn a_unit_that_would_round_to_nothing_is_refused_at_both_doors() {
        let mut c = Chain::founded(1, 0);
        c.st.params.gov_cooldown_epochs = 0;
        c.st.params.v_base = 5.0;
        c.err(
            Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::BondFraction, value: 0.001 } },
            &[key(0)],
            ET_GOV_OUT_OF_RANGE,
        );
        c.ok(
            Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::BondFraction, value: 0.002 } },
            &[key(0)],
        );
        c.st.params.v_base = 0.6;
        c.err(Tx::Propose { author: 0, kind: ProposalKind::Redenominate { num: 2, den: 3 } }, &[key(0)], ET_GOV_BAND);
        c.ok(Tx::Propose { author: 0, kind: ProposalKind::Redenominate { num: 3, den: 2 } }, &[key(0)]);

        // And at enactment, when the base moved after the proposal was written.
        c.st.params.v_base = 10.0;
        let pid = c.st.next_proposal;
        c.ok(
            Tx::Propose { author: 0, kind: ProposalKind::ParamChange { key: ParamKey::BondFraction, value: 0.001 } },
            &[key(0)],
        );
        c.st.params.v_base = 5.0;
        c.err(Tx::Assent { member: 0, proposal: pid }, &[key(0)], ET_GOV_OUT_OF_RANGE);
        assert!(!c.st.proposals[&pid].enacted);
        assert!(c.st.proposals[&pid].assents.is_empty(), "refused before the first write");
    }
}
