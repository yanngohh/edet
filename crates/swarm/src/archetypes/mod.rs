//! The behaviours, each a probe for one property of the mechanism.
//!
//! Every archetype names, in the doc comment of its test in
//! `tests/archetypes.rs`, the mutation to the state machine that must turn
//! that test red — and every one of those mutations has been applied by hand
//! once. A green suite that only asserts a gate PASSES says nothing about what
//! it DETECTS.

pub mod coalition;
pub mod deadbeat;
pub mod exiter;
pub mod griefer;
pub mod hoarder;
pub mod honest;
pub mod late_defaulter;
pub mod sleeper;
pub mod sybil_farm;
pub mod wash_ring;

use edet_state::types::MemberId;

use crate::strategy::Strategy;

/// What one agent is built from, and where it sits.
#[derive(Clone, Debug)]
pub struct Seating {
    pub agent: usize,
    pub seat: MemberId,
    /// The seats of every agent carrying this same archetype — a ring's peers,
    /// a coalition's members. Known at construction because seats are
    /// assigned in order.
    pub cohort: Vec<MemberId>,
    /// Where this agent sits inside `cohort`.
    pub rank: usize,
}

/// The parameterised behaviours, and the one place a name maps to one.
#[derive(Clone, Debug)]
pub enum Archetype {
    Honest(honest::Params),
    Deadbeat(deadbeat::Params),
    WashRing(wash_ring::Params),
    SybilFarm(sybil_farm::Params),
    Griefer(griefer::Params),
    Coalition(coalition::Params),
    Exiter(exiter::Params),
    Sleeper(sleeper::Params),
    LateDefaulter(late_defaulter::Params),
    Hoarder(hoarder::Params),
}

impl Archetype {
    pub fn name(&self) -> &'static str {
        match self {
            Archetype::Honest(_) => "honest",
            Archetype::Deadbeat(_) => "deadbeat",
            Archetype::WashRing(_) => "wash-ring",
            Archetype::SybilFarm(_) => "sybil-farm",
            Archetype::Griefer(_) => "griefer",
            Archetype::Coalition(_) => "coalition",
            Archetype::Exiter(_) => "exiter",
            Archetype::Sleeper(_) => "sleeper",
            Archetype::LateDefaulter(_) => "late-defaulter",
            Archetype::Hoarder(_) => "hoarder",
        }
    }

    /// Is this a seat the control replaces with an honest trader?
    ///
    /// **A control is the same seed, the same seats, every treatment seat
    /// honest, aged the same ticks.** Anything else credits the clock with the
    /// treatment's work, because advancing an epoch decays every stake.
    pub fn is_treatment(&self) -> bool {
        !matches!(self, Archetype::Honest(_))
    }

    pub fn build(&self, at: &Seating) -> Box<dyn Strategy> {
        match self {
            Archetype::Honest(p) => Box::new(honest::Honest::new(p.clone(), at)),
            Archetype::Deadbeat(p) => Box::new(deadbeat::Deadbeat::new(p.clone(), at)),
            Archetype::WashRing(p) => Box::new(wash_ring::WashRing::new(p.clone(), at)),
            Archetype::SybilFarm(p) => Box::new(sybil_farm::SybilFarm::new(p.clone(), at)),
            Archetype::Griefer(p) => Box::new(griefer::Griefer::new(p.clone(), at)),
            Archetype::Coalition(p) => Box::new(coalition::Coalition::new(p.clone(), at)),
            Archetype::Exiter(p) => Box::new(exiter::Exiter::new(p.clone(), at)),
            Archetype::Sleeper(p) => Box::new(sleeper::Sleeper::new(p.clone(), at)),
            Archetype::LateDefaulter(p) => Box::new(late_defaulter::LateDefaulter::new(p.clone(), at)),
            Archetype::Hoarder(p) => Box::new(hoarder::Hoarder::new(p.clone(), at)),
        }
    }
}
