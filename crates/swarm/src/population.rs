//! Who is in the community, what they are, and the genesis they start from.
//!
//! **Standing is earned, never written.** The genesis backing goes in through
//! real `Accept`/`Settle` pairs, because a stake is only ever placed by a
//! transition the creditor signed — a fixture that reached into `edges` would
//! be measuring a community that cannot exist.

use std::collections::BTreeMap;

use edet_state::state::State;
use edet_state::types::*;

use crate::archetypes::{
    coalition, deadbeat, exiter, griefer, hoarder, honest, late_defaulter, sleeper, sybil_farm, wash_ring,
};
use crate::archetypes::{Archetype, Seating};
use crate::driver::{Audit, Driver};
use crate::keys::{consensus_key, member_key};
use crate::strategy::Strategy;

/// Who controls what. Extended when a `Fresh` key is seated.
#[derive(Clone, Debug, Default)]
pub struct World {
    controller: BTreeMap<MemberId, usize>,
    seats: Vec<Vec<MemberId>>,
}

impl World {
    pub fn with_agents(n: usize) -> Self {
        World { controller: BTreeMap::new(), seats: vec![Vec::new(); n] }
    }

    /// The agent that controls this member, or `usize::MAX` for a member
    /// nobody in this population seated.
    pub fn agent_of(&self, id: MemberId) -> usize {
        self.controller.get(&id).copied().unwrap_or(usize::MAX)
    }

    pub fn seats_of(&self, agent: usize) -> &[MemberId] {
        self.seats.get(agent).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn agents(&self) -> usize {
        self.seats.len()
    }

    pub fn claim(&mut self, id: MemberId, agent: usize) {
        if self.controller.insert(id, agent).is_none() {
            if let Some(s) = self.seats.get_mut(agent) {
                s.push(id);
                s.sort_unstable();
            }
        }
    }
}

/// Governed constants a preset moves before anybody trades.
#[derive(Clone, Debug, Default)]
pub struct ParamsOverride {
    /// The hostile-but-legal bond regime: no allowance, the constitutional
    /// ceiling on the bond unit, and bonds that do not release during the run.
    pub tighten: bool,
    pub bond_free_allowance: Option<u32>,
    pub v_base: Option<f64>,
}

impl ParamsOverride {
    fn apply(&self, st: &mut State) {
        if self.tighten {
            st.params.bond_free_allowance = 0;
            st.params.bond_fraction = 0.10;
            st.params.bond_release_epochs = 100;
        }
        if let Some(a) = self.bond_free_allowance {
            st.params.bond_free_allowance = a;
        }
        if let Some(v) = self.v_base {
            st.params.v_base = v;
        }
    }
}

/// A community, its behaviours, and the one honest edge or twenty that reach
/// it.
#[derive(Clone, Debug)]
pub struct Population {
    pub name: String,
    /// Seats `0..k`, each declaring a supply in minor units.
    pub underwriters: Vec<(Archetype, u64)>,
    /// Seats `k..n`, in order.
    pub ordinary: Vec<Archetype>,
    /// Genesis stakes, written through real trades: `(creditor, debtor,
    /// amount)` in minor units.
    pub backing: Vec<(MemberId, MemberId, u64)>,
    /// Members seated with a consensus key and genesis voting power.
    pub validators: Vec<MemberId>,
    pub params: ParamsOverride,
    /// What the control replaces every treatment seat with.
    pub honest: honest::Params,
}

impl Population {
    pub fn agents(&self) -> usize {
        self.underwriters.len() + self.ordinary.len()
    }

    fn archetypes(&self) -> Vec<Archetype> {
        self.underwriters
            .iter()
            .map(|(a, _)| a.clone())
            .chain(self.ordinary.iter().cloned())
            .collect()
    }

    /// **The control: the same seeds, the same seats, every treatment seat
    /// replaced by an honest trader with this preset's honest parameters,
    /// aged the same ticks.**
    ///
    /// Anything else credits the clock with the treatment's work, because
    /// advancing an epoch decays every stake.
    /// Whether any seat is a treatment — the population differs from its
    /// own control. A Q2 over a population with no treatment compares a run
    /// with itself and prints 1.000 on every line, which reads as "no effect"
    /// and means nothing was compared.
    pub fn has_treatment(&self) -> bool {
        self.underwriters.iter().any(|(a, _)| a.is_treatment()) || self.ordinary.iter().any(|a| a.is_treatment())
    }

    pub fn control(&self) -> Population {
        let honest = || Archetype::Honest(self.honest.clone());
        Population {
            name: format!("{}-control", self.name),
            underwriters: self
                .underwriters
                .iter()
                .map(|(a, s)| (if a.is_treatment() { honest() } else { a.clone() }, *s))
                .collect(),
            ordinary: self
                .ordinary
                .iter()
                .map(|a| if a.is_treatment() { honest() } else { a.clone() })
                .collect(),
            backing: self.backing.clone(),
            validators: self.validators.clone(),
            params: self.params.clone(),
            honest: self.honest.clone(),
        }
    }

    /// Found the community and write its genesis stakes.
    pub fn genesis(&self, audit: Audit) -> (Driver, World, Vec<Box<dyn Strategy>>) {
        let mut st = State::default();
        self.params.apply(&mut st);
        for (i, (_, supply)) in self.underwriters.iter().enumerate() {
            st.add_underwriter(vec![member_key(i)], State::from_minor(*supply))
                .expect("founding underwriter");
        }
        let k = self.underwriters.len();
        for i in k..k + self.ordinary.len() {
            st.new_account(vec![member_key(i)]);
        }
        for &v in &self.validators {
            st.set_consensus_key(v, consensus_key(v as usize)).expect("consensus key");
            st.set_genesis_validator(v, 1).expect("genesis validator");
        }

        // The genesis stakes, earned. `back` is `lend` then `settle`, which is
        // the only thing that writes an edge.
        let mut d = Driver::new(st).audit_mode(audit);
        for &(creditor, debtor, amount) in &self.backing {
            let cid = d.st.next_contract;
            let amount = State::from_minor(amount);
            d.ok(
                edet_state::tx::Tx::Accept {
                    debtor: Party::Member(debtor),
                    creditor: Party::Member(creditor),
                    amount,
                    maturity_epochs: d.st.params.min_maturity_epochs,
                    arb: None,
                },
                &[member_key(creditor as usize), member_key(debtor as usize)],
            );
            d.ok(
                edet_state::tx::Tx::Settle { contract: cid, amount },
                &[member_key(creditor as usize), member_key(debtor as usize)],
            );
        }

        let archetypes = self.archetypes();
        let mut world = World::with_agents(archetypes.len());
        for (agent, _) in archetypes.iter().enumerate() {
            world.claim(agent as MemberId, agent);
        }
        // A cohort is every agent carrying the same archetype: a ring's peers,
        // a coalition's members. Seats are assigned in order, so it is known
        // at construction.
        let mut strategies: Vec<Box<dyn Strategy>> = Vec::with_capacity(archetypes.len());
        for (agent, a) in archetypes.iter().enumerate() {
            let cohort: Vec<MemberId> = archetypes
                .iter()
                .enumerate()
                .filter(|(_, other)| other.name() == a.name())
                .map(|(i, _)| i as MemberId)
                .collect();
            let rank = cohort.iter().position(|&m| m == agent as MemberId).unwrap_or(0);
            strategies.push(a.build(&Seating { agent, seat: agent as MemberId, cohort, rank }));
        }
        (d, world, strategies)
    }
}

// ------------------------------------------------------------- the presets --

/// What one founding underwriter declares, minor units.
const SUPPLY: u64 = 250_000;
/// What one honest edge carries, minor units — an ordinary trade, not a gift.
const EDGE: u64 = 50_000;

fn honest_seat() -> Archetype {
    Archetype::Honest(honest::Params::default())
}

/// Underwriters back the ordinary seats round-robin, which is the only edge a
/// community with no graph can write.
fn round_robin(uw: usize, first: MemberId, count: usize, amount: u64) -> Vec<(MemberId, MemberId, u64)> {
    (0..count)
        .map(|i| ((i % uw) as MemberId, first + i as MemberId, amount))
        .collect()
}

impl Population {
    /// The named presets, each small enough for the corpus and each seating
    /// exactly what its archetype's probe needs.
    pub fn named(name: &str) -> Option<Population> {
        if let Some(rest) = name.strip_prefix("sleeper-debtor-") {
            return rest.parse().ok().map(|e| sleeper_preset(name, sleeper::Mode::Debtor, e));
        }
        if let Some(rest) = name.strip_prefix("sleeper-underwriter-") {
            return rest.parse().ok().map(|e| sleeper_preset(name, sleeper::Mode::Underwriter, e));
        }
        Some(match name {
            "honest" => honest_preset(),
            "deadbeats" => deadbeats_preset(),
            "wash-ring" => wash_ring_preset(),
            "sybil-farm" => sybil_farm_preset(EDGE, 8),
            "sybil-farm-x10" => sybil_farm_preset(10 * EDGE, 8),
            "sybil-farm-long" => sybil_farm_preset(EDGE, 300),
            "honest-open" => honest_open_preset(),
            "honest-full" => honest_full_preset(),
            "griefer" => griefer_preset(),
            "coalition-third" => coalition_preset("coalition-third", 200_000),
            "coalition-half" => coalition_preset("coalition-half", 330_000),
            "coalition-two-thirds" => coalition_preset("coalition-two-thirds", 420_000),
            "exit-under-suspension" => exit_preset(),
            "late-defaulter" => late_defaulter_preset(),
            "hoarders" => hoarders_preset(),
            "everything" => everything_preset(),
            _ => return None,
        })
    }

    /// Every preset name the corpus and the search may use.
    pub fn all_names() -> Vec<String> {
        let mut v: Vec<String> = [
            "honest",
            "deadbeats",
            "wash-ring",
            "sybil-farm",
            "sybil-farm-x10",
            "sybil-farm-long",
            "honest-open",
            "honest-full",
            "griefer",
            "coalition-third",
            "coalition-half",
            "coalition-two-thirds",
            "exit-under-suspension",
            "late-defaulter",
            "hoarders",
            "everything",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for e in [30, 90, 180, 365] {
            v.push(format!("sleeper-debtor-{e}"));
            v.push(format!("sleeper-underwriter-{e}"));
        }
        v
    }

    /// **Grow the population with honest traders, at the END.**
    ///
    /// Every seat a preset already named keeps its index and the genesis edges
    /// written to it, because a probe that names a seat — a ring's peers, a
    /// coalition's members, the one honest edge that reaches a farm — is a
    /// probe about WHICH seats are reached and not only how many. Inserting
    /// anywhere but the end renumbers them, and then the preset is a different
    /// scene wearing the same name.
    ///
    /// A size at or below what the preset already seats is a no-op: a preset
    /// shrunk past its treatment seats is not that preset.
    pub fn resize(&mut self, members: usize) {
        let seated = self.underwriters.len() + self.ordinary.len();
        let want = members.saturating_sub(seated);
        let first = seated as MemberId;
        for _ in 0..want {
            self.ordinary.push(Archetype::Honest(self.honest.clone()));
        }
        self.backing.extend(round_robin(self.underwriters.len(), first, want, EDGE));
    }
}

fn honest_preset() -> Population {
    let uw = 3;
    let ordinary = 12;
    Population {
        name: "honest".into(),
        underwriters: (0..uw).map(|_| (honest_seat(), SUPPLY)).collect(),
        ordinary: (0..ordinary).map(|_| honest_seat()).collect(),
        backing: round_robin(uw, uw as MemberId, ordinary, EDGE),
        validators: vec![0],
        params: Default::default(),
        honest: Default::default(),
    }
}

fn deadbeats_preset() -> Population {
    let uw = 2;
    let honest_n = 8;
    let dead = 2;
    let first = uw as MemberId;
    Population {
        name: "deadbeats".into(),
        underwriters: (0..uw).map(|_| (honest_seat(), SUPPLY)).collect(),
        ordinary: (0..honest_n)
            .map(|_| honest_seat())
            .chain((0..dead).map(|_| Archetype::Deadbeat(deadbeat::Params::default())))
            .collect(),
        backing: round_robin(uw, first, honest_n + dead, EDGE),
        validators: vec![0],
        params: Default::default(),
        honest: Default::default(),
    }
}

fn wash_ring_preset() -> Population {
    let uw = 2;
    let honest_n = 4;
    let ring = 5;
    let first = uw as MemberId;
    let ring_first = first + honest_n as MemberId;
    // **Exactly one honest edge reaches the ring.** A ring nobody reaches
    // cannot write at all, and would be scenery.
    let mut backing = round_robin(uw, first, honest_n, EDGE);
    backing.push((0, ring_first, EDGE));
    Population {
        name: "wash-ring".into(),
        underwriters: (0..uw).map(|_| (honest_seat(), SUPPLY)).collect(),
        ordinary: (0..honest_n)
            .map(|_| honest_seat())
            .chain((0..ring).map(|_| Archetype::WashRing(wash_ring::Params::default())))
            .collect(),
        backing,
        validators: vec![0],
        params: Default::default(),
        honest: Default::default(),
    }
}

/// One founder, one operator the community really backs, and `edge` of honest
/// backing behind that operator. The wash is a tenth of `v_base`, which is
/// twice the establishment floor.
///
/// **The backing is a parameter because the claim is linear in it.** A rule
/// that bounded the farm at one size and re-scaled at another would look
/// identical at a single size, which is what the `-x10` entry is for; a rule
/// that bounded it for a few ticks and not for many would look identical over
/// a short run, which is what `-long` is for. `name` carries the size so the
/// corpus entries read as three measurements of one claim.
fn sybil_farm_preset(edge: u64, ticks: u64) -> Population {
    let name = if edge != EDGE {
        "sybil-farm-x10"
    } else if ticks > 8 {
        "sybil-farm-long"
    } else {
        "sybil-farm"
    };
    Population {
        name: name.into(),
        // The founder's own supply has to carry the edge it writes.
        underwriters: vec![(honest_seat(), SUPPLY.max(5 * edge))],
        ordinary: vec![Archetype::SybilFarm(sybil_farm::Params::default())],
        backing: vec![(0, 1, edge)],
        validators: vec![0],
        params: Default::default(),
        honest: Default::default(),
    }
}

/// **An ordinary community with a newcomer stream**, well below the ceiling
/// its seed carries.
///
/// The falsifier it exists for: an honest seat refused for want of reach while
/// the community still has seed to spend. A ceiling nobody honest ever meets
/// is a ceiling that costs onboarding nothing, and this is where that is
/// measured rather than asserted.
fn honest_open_preset() -> Population {
    let mut p = honest_preset();
    p.name = "honest-open".into();
    p.honest.greet = 0.1;
    p.ordinary = (0..p.ordinary.len()).map(|_| Archetype::Honest(p.honest.clone())).collect();
    p.underwriters = p
        .underwriters
        .iter()
        .map(|&(_, s)| (Archetype::Honest(p.honest.clone()), s))
        .collect();
    p
}

/// **The same community, greeting on every tick, until the seed is spent.**
///
/// The other half of the falsifier: the ceiling must BIND, and it must bind at
/// `Σ supply / unit` and nowhere else. A bound that never binds is not one.
fn honest_full_preset() -> Population {
    let mut p = honest_open_preset();
    p.name = "honest-full".into();
    p.honest.greet = 1.0;
    p.ordinary = (0..p.ordinary.len()).map(|_| Archetype::Honest(p.honest.clone())).collect();
    p.underwriters = p
        .underwriters
        .iter()
        .map(|&(_, s)| (Archetype::Honest(p.honest.clone()), s))
        .collect();
    p
}

fn griefer_preset() -> Population {
    let uw = 2;
    let honest_n = 4;
    let first = uw as MemberId;
    let grief = first + honest_n as MemberId;
    let mut backing = round_robin(uw, first, honest_n, EDGE);
    backing.push((0, grief, EDGE));
    Population {
        name: "griefer".into(),
        underwriters: (0..uw).map(|_| (honest_seat(), SUPPLY)).collect(),
        ordinary: (0..honest_n)
            .map(|_| honest_seat())
            .chain(std::iter::once(Archetype::Griefer(griefer::Params::default())))
            .collect(),
        backing,
        validators: vec![0],
        params: Default::default(),
        honest: Default::default(),
    }
}

/// A coalition holding `share_minor` of a seed of 6,000.00, two honest
/// underwriters holding the rest, two accomplices holding none, and two
/// validators — because removing one of two is what `MIN_VALIDATORS` admits
/// and removing the only one is not.
fn coalition_preset(name: &str, share_minor: u64) -> Population {
    let total: u64 = 600_000;
    let coalition_seats = 2;
    let each = share_minor / coalition_seats;
    let rest = total - each * coalition_seats;
    let honest_uw = 2;
    let mut underwriters: Vec<(Archetype, u64)> = (0..coalition_seats)
        .map(|_| (Archetype::Coalition(coalition::Params::default()), each))
        .collect();
    for _ in 0..honest_uw {
        underwriters.push((honest_seat(), rest / honest_uw));
    }
    let k = underwriters.len();
    // Two accomplices with no supply at all, then honest members to trade
    // with and to suspend.
    let ordinary: Vec<Archetype> = (0..2)
        .map(|_| Archetype::Coalition(coalition::Params::default()))
        .chain((0..4).map(|_| honest_seat()))
        .collect();
    let first = k as MemberId;
    Population {
        name: name.into(),
        underwriters,
        ordinary: ordinary.clone(),
        backing: round_robin(k, first, ordinary.len(), EDGE),
        // The two honest underwriters order the chain, so the coalition's own
        // seat is a change to the set rather than a seat it already holds.
        validators: vec![coalition_seats as MemberId, coalition_seats as MemberId + 1],
        params: Default::default(),
        honest: Default::default(),
    }
}

/// A coalition strong enough to suspend, and the member it suspends walking
/// out anyway. **A Suspended member may still `Exit`** — winding down under
/// sanction is the recovery path.
fn exit_preset() -> Population {
    let coalition_seats = 2;
    let underwriters: Vec<(Archetype, u64)> = (0..coalition_seats)
        .map(|_| {
            (
                Archetype::Coalition(coalition::Params {
                    also_the_order: false,
                    suspend_outsider: true,
                    ..Default::default()
                }),
                SUPPLY,
            )
        })
        .collect();
    let k = underwriters.len();
    let exiter_seat = k as MemberId;
    Population {
        name: "exit-under-suspension".into(),
        underwriters,
        ordinary: vec![Archetype::Exiter(exiter::Params {
            trigger: exiter::Trigger::Tick(10),
            honest: Default::default(),
        })],
        backing: vec![(0, exiter_seat, EDGE)],
        validators: vec![0],
        params: Default::default(),
        honest: Default::default(),
    }
}

/// A debtor that defaults with a panel it holds a MINORITY of: two colluders
/// against three honest arbiters, quorum three. The median of the whole panel
/// is what the parties consented to, so nothing mints.
fn late_defaulter_preset() -> Population {
    let uw = 2;
    let honest_n = 6;
    let cohort = 3;
    let first = uw as MemberId;
    // **Nobody backs the cohort**, and that is what the probe needs. An
    // insured row that expires is SUBSTITUTED, and substitution drops the
    // consented panel — so a window that outlives a default outlives the
    // remedy, and the archetype would be measuring that instead of the
    // median. The colluders need no standing either: an attestation is a free
    // class, and what it costs is being on a panel both parties named.
    let backing = round_robin(uw, first, honest_n, EDGE);
    Population {
        name: "late-defaulter".into(),
        underwriters: (0..uw).map(|_| (honest_seat(), SUPPLY)).collect(),
        ordinary: (0..honest_n)
            .map(|_| honest_seat())
            .chain((0..cohort).map(|_| Archetype::LateDefaulter(late_defaulter::Params::default())))
            .collect(),
        backing,
        validators: vec![0],
        params: Default::default(),
        honest: Default::default(),
    }
}

fn hoarders_preset() -> Population {
    let uw = 2;
    let honest_n = 6;
    let hoard = 4;
    let first = uw as MemberId;
    Population {
        name: "hoarders".into(),
        underwriters: (0..uw).map(|_| (honest_seat(), SUPPLY)).collect(),
        ordinary: (0..honest_n)
            .map(|_| honest_seat())
            .chain((0..hoard).map(|_| Archetype::Hoarder(hoarder::Params::default())))
            .collect(),
        backing: round_robin(uw, first, honest_n + hoard, EDGE),
        validators: vec![0],
        params: Default::default(),
        honest: Default::default(),
    }
}

fn sleeper_preset(name: &str, mode: sleeper::Mode, e: u64) -> Population {
    let p = sleeper::Params { honest_ticks: e, mode, honest: Default::default() };
    let honest_n = 8;
    match mode {
        // Seated as a founding underwriter: its supply insures others for `E`
        // ticks, and then it stops honouring anything.
        sleeper::Mode::Underwriter => {
            let underwriters = vec![(honest_seat(), SUPPLY), (Archetype::Sleeper(p), SUPPLY)];
            let k = underwriters.len();
            Population {
                name: name.into(),
                underwriters,
                ordinary: (0..honest_n).map(|_| honest_seat()).collect(),
                backing: round_robin(k, k as MemberId, honest_n, EDGE),
                validators: vec![0],
                params: Default::default(),
                honest: Default::default(),
            }
        }
        sleeper::Mode::Debtor => {
            let uw = 2;
            let first = uw as MemberId;
            let sleeper_seat = first + honest_n as MemberId;
            let mut backing = round_robin(uw, first, honest_n, EDGE);
            backing.push((0, sleeper_seat, EDGE));
            Population {
                name: name.into(),
                underwriters: (0..uw).map(|_| (honest_seat(), SUPPLY)).collect(),
                ordinary: (0..honest_n)
                    .map(|_| honest_seat())
                    .chain(std::iter::once(Archetype::Sleeper(p)))
                    .collect(),
                backing,
                validators: vec![0],
                params: Default::default(),
                honest: Default::default(),
            }
        }
    }
}

/// **Everything at once**, which is the only run in which one archetype's
/// behaviour is another's environment.
fn everything_preset() -> Population {
    let uw = 4;
    let mut underwriters: Vec<(Archetype, u64)> = vec![
        (Archetype::Coalition(coalition::Params::default()), 100_000),
        (
            Archetype::Sleeper(sleeper::Params {
                honest_ticks: 40,
                mode: sleeper::Mode::Underwriter,
                honest: Default::default(),
            }),
            SUPPLY,
        ),
    ];
    while underwriters.len() < uw {
        underwriters.push((honest_seat(), SUPPLY));
    }
    let honest_n = 20;
    let ordinary: Vec<Archetype> = (0..honest_n)
        .map(|_| honest_seat())
        .chain([
            Archetype::Coalition(coalition::Params::default()),
            Archetype::Deadbeat(deadbeat::Params::default()),
            Archetype::Deadbeat(deadbeat::Params::default()),
            Archetype::Griefer(griefer::Params::default()),
            Archetype::Hoarder(hoarder::Params::default()),
            Archetype::Hoarder(hoarder::Params::default()),
            Archetype::Exiter(exiter::Params { trigger: exiter::Trigger::DefaultsSeen(3), honest: Default::default() }),
            Archetype::Sleeper(sleeper::Params {
                honest_ticks: 40,
                mode: sleeper::Mode::Debtor,
                honest: Default::default(),
            }),
            // **Washed BELOW the establishment floor here**, where the
            // dedicated preset washes above it. The difference is the whole
            // finding: below the floor the seated rows qualify for nothing and
            // the farm is linear in its backing; above it every row is
            // established and seats more.
            Archetype::SybilFarm(sybil_farm::Params { wash_minor: 100, ..Default::default() }),
            Archetype::WashRing(wash_ring::Params::default()),
            Archetype::WashRing(wash_ring::Params::default()),
            Archetype::WashRing(wash_ring::Params::default()),
            Archetype::LateDefaulter(late_defaulter::Params::default()),
            Archetype::LateDefaulter(late_defaulter::Params::default()),
            Archetype::LateDefaulter(late_defaulter::Params::default()),
        ])
        .collect();
    let first = uw as MemberId;
    // The honest seats greet, so the scene the search covers has ordinary
    // onboarding running beside the farm rather than only the farm's.
    let honest = honest::Params { greet: 0.05, ..Default::default() };
    let ordinary: Vec<Archetype> = ordinary
        .into_iter()
        .map(|a| if matches!(a, Archetype::Honest(_)) { Archetype::Honest(honest.clone()) } else { a })
        .collect();
    Population {
        name: "everything".into(),
        underwriters,
        backing: round_robin(uw, first, ordinary.len(), EDGE),
        ordinary,
        validators: vec![2, 3],
        params: Default::default(),
        honest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The populations with no treatment seat are exactly the honest ones,
    /// and `q2` refuses them — a treatment that is its own control measures
    /// nothing.
    #[test]
    fn only_the_honest_populations_have_no_treatment() {
        let untreated: Vec<String> = Population::all_names()
            .into_iter()
            .filter(|n| !Population::named(n).expect("a named population").has_treatment())
            .collect();
        assert_eq!(untreated, vec!["honest", "honest-open", "honest-full"]);
    }
}
