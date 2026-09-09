//! **A population of behaving agents over the real transition function,
//! judged by the real audit.**
//!
//! Not a model of the design: `edet_state::apply` is what runs, and
//! `invariants::audit` is what decides. The three questions the crate exists
//! for are separated on purpose — a search for where the mechanism breaks, a
//! distribution over plausible behaviour, and a generator of hypotheses that
//! reports no numbers of its own.
//!
//! The generator lives in `personas/`, run by hand and never by a gate: a
//! language model shows twenty to three hundred times LESS behavioural
//! variance than people do, and adoption and attack are both tail phenomena,
//! so a figure it produced would describe the generator. **The model
//! generates, the kernel judges.**

pub mod archetypes;
pub mod corpus;
pub mod driver;
pub mod intent;
pub mod keys;
pub mod metrics;
pub mod population;
pub mod rng;
pub mod run;
pub mod scenario;
pub mod strategy;

pub use driver::{bonded_dud, epochs, Audit, Driver, Envelope};
pub use intent::{AgentRef, Intent, Outcome};
pub use metrics::Summary;
pub use population::Population;
pub use run::{run, Report, Run, Violation};
