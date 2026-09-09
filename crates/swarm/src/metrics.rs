//! What a run measures, and what the corpus pins.
//!
//! **No `f64` anywhere.** Amounts are minor units, ratios are
//! `(numerator, denominator)` pairs, and every map is a `BTreeMap`, so the
//! JSON is byte-stable and a diff in review reads as a change of BEHAVIOUR
//! rather than of rounding.
//!
//! Refusal codes by kind is the field that earns its keep. A population being
//! refused for a reason nobody predicted is the finding, and it is also what
//! turns the corpus into a mutation detector: a `refused` count that went to
//! zero is a rule that stopped firing.

use std::collections::BTreeMap;

use edet_state::state::State;
use edet_state::types::*;

/// The version of the pinned shape. A change to it is a deliberate
/// re-pinning, and `just swarm` says so instead of printing a field diff
/// nobody can read.
pub const FORMAT_VERSION: u32 = 2;

/// The capacity distribution, in minor units and in exact ratios.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Distribution {
    pub p50: u64,
    pub p90: u64,
    pub max: u64,
    /// What share of total member capacity the top decile holds.
    pub top_decile: (u64, u64),
    /// Members with no capacity at all, over members.
    pub zero_share: (u64, u64),
    /// **The share of capacity held by members with no out-edge** — the
    /// failure mode that looks healthy in every aggregate. Standing that
    /// confers nothing is standing the community cannot circulate.
    pub non_conferrer_share: (u64, u64),
}

impl Distribution {
    pub fn of(st: &State) -> Self {
        let ids: Vec<MemberId> = st.members.keys().copied().collect();
        let mut caps: Vec<(MemberId, u64)> = ids.iter().map(|&id| (id, st.capacity_raw_minor(id))).collect();
        let total: u64 = caps.iter().map(|&(_, c)| c).sum();
        let zero = caps.iter().filter(|&&(_, c)| c == 0).count() as u64;
        let confers: std::collections::BTreeSet<MemberId> = st
            .edges
            .iter()
            .filter(|&(_, &a)| a > 0)
            .map(|(&(c, _), _)| c as MemberId)
            .collect();
        let hoarded: u64 = caps.iter().filter(|(id, _)| !confers.contains(id)).map(|&(_, c)| c).sum();
        caps.sort_by_key(|&(_, c)| c);
        let n = caps.len();
        let at = |q: usize| if n == 0 { 0 } else { caps[(n * q / 100).min(n - 1)].1 };
        let decile_from = n.saturating_sub(n / 10);
        let top: u64 = caps[decile_from..].iter().map(|&(_, c)| c).sum();
        Distribution {
            p50: at(50),
            p90: at(90),
            max: caps.last().map(|&(_, c)| c).unwrap_or(0),
            top_decile: (top, total),
            zero_share: (zero, n as u64),
            non_conferrer_share: (hoarded, total),
        }
    }
}

/// One tick's reading.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Metrics {
    pub tick: u64,
    pub epoch: u64,
    pub members: u64,
    /// Booked this tick, and the part of it the community insured.
    pub rows_accepted: u64,
    pub accepted_minor: u64,
    pub insured_minor: u64,
    /// Outstanding debt across the whole book.
    pub drawn_minor: u64,
    /// Insured credit standing at once — the supply arcs' own reading.
    pub committed_minor: u64,
    pub refused: BTreeMap<String, u64>,
    pub declined: u64,
    pub unbuilt: u64,
    /// Rows seated this tick.
    pub seated: u64,
    /// Rows in `Expired` at the end of the tick.
    pub expired_rows: u64,
    pub open_default_minor: u64,
    pub forfeited_minor: u64,
    pub exits: u64,
    pub enacted: u64,
    /// Members the gate denied during this epoch.
    pub denied_members: u64,
    /// **A tick in which lends were attempted and none applied.** The
    /// distributional reading a per-member figure cannot show.
    pub deadlocked: bool,
    /// Rows the ceiling pushed uninsured: the pristine cut would have carried
    /// them and the live reservations did not.
    pub ceiling_uninsured: u64,
    /// Present only every `metrics_every` ticks and at the end, because it
    /// costs one cut per member.
    pub capacity: Option<Distribution>,
}

/// One archetype's row in the summary.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArchetypeRow {
    pub agents: u64,
    pub emitted: u64,
    pub applied: u64,
    pub refused: BTreeMap<String, u64>,
    pub declined: u64,
    pub unbuilt: u64,
    /// Intents dropped because the agent went past
    /// [`crate::intent::MAX_INTENTS_PER_TICK`]. Reported rather than silent:
    /// a bound mistaken for the population's own behaviour is a figure about
    /// the harness.
    pub over_budget: u64,
    pub probe_reached: bool,
    pub probe_what: String,
    pub probe_detail: String,
}

/// What the corpus pins: the whole of a run, in a shape a diff can be read
/// off.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Summary {
    pub format_version: u32,
    pub ticks: u64,
    pub members_final: u64,
    pub seated: u64,
    pub rows_accepted: u64,
    pub accepted_minor: u64,
    pub insured_minor: u64,
    pub committed_final: u64,
    pub external_seed: u64,
    pub refused: BTreeMap<String, u64>,
    pub declined: u64,
    pub unbuilt: u64,
    pub expired_rows: u64,
    pub open_default_final: u64,
    pub forfeited_final: u64,
    pub exits: u64,
    pub enacted: u64,
    pub denied_members: u64,
    pub deadlocked_ticks: u64,
    pub ceiling_uninsured: u64,
    pub capacity: Distribution,
    /// What the live seats hold on the underwriters' supply arcs at the end of
    /// the run — one bond unit per row a trade seated, and the quantity whose
    /// ceiling is the external seed. A run whose `seat_committed_final` equals
    /// `external_seed` has seated every row the community can.
    pub seat_committed_final: u64,
    pub archetypes: BTreeMap<String, ArchetypeRow>,
    /// Hex of `edet_state::root::state_root` — what makes the pin a
    /// bit-identity claim as well as a behavioural one.
    pub state_root: String,
}

/// Lowercase hex, so a pinned root is comparable by eye.
pub fn hex32(b: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}
