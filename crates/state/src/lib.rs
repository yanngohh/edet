//! The edet state machine: objects, transitions, and invariants.
//!
//! Engine-agnostic by design: `apply(state, tx, signers, time)` is a pure
//! transition function over owned state — no I/O, no clock, no network. A
//! consensus driver (or a test) owns ordering and signature verification.

pub mod apply;
pub mod bond;
mod cascade;
pub mod codec;
pub mod errors;
pub mod invariants;
pub mod journal;
mod loss;
pub mod params;
pub mod root;
pub mod seed;
pub mod state;
pub mod tx;
pub mod types;

pub use apply::{apply, apply_with_cache, authorises};
pub use errors::{Error, Res};
pub use state::State;
pub use tx::Tx;
