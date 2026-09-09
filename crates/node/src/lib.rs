//! Node: the driver layer around the engine-agnostic state machine.
//!
//! One binary serves every role — validator (always-on device), full, light —
//! phone or headless server alike. This crate owns what consensus needs and
//! the state machine deliberately doesn't: blocks and their codec, the
//! durable store (WAL + snapshots + recovery), the mempool, and the replica
//! that turns decided blocks into state transitions deterministically.
//!
//! Consensus is the embedded Malachite engine, behind the `malachite`
//! feature (`engine_malachite`, `engine_node`) — one binary, one consensus.
//! `SoloDriver` below is a single-process, instant-finality driver for
//! state-machine development and tests: it is NOT consensus and must never
//! be deployed as one.

pub mod block;
pub mod mempool;
pub mod replica;
pub mod store;

#[cfg(feature = "malachite")]
pub mod engine_codec;
#[cfg(feature = "malachite")]
pub mod engine_context;
#[cfg(feature = "malachite")]
pub mod engine_malachite;
#[cfg(feature = "malachite")]
pub mod engine_node;

#[cfg(feature = "serve")]
pub mod serve;

use edet_state::{apply, invariants, types::Key, State};

/// Re-exported for embedders (the Tauri client) that speak transactions
/// without depending on edet-state directly.
pub use edet_state::Tx;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Validator,
    Full,
    Light,
}

/// Driver-level failure: either the transaction was rejected by the state
/// machine (normal), or a whole-state invariant broke afterward (a bug — the
/// driver surfaces it instead of continuing on corrupt state).
#[derive(Debug)]
pub enum DriverError {
    Tx(edet_state::Error),
    Invariant(String),
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriverError::Tx(e) => write!(f, "rejected: {e}"),
            DriverError::Invariant(v) => write!(f, "invariant violated: {v}"),
        }
    }
}
impl std::error::Error for DriverError {}

/// Development driver: applies transactions in submission order at a caller-
/// controlled clock, auditing invariants after every transition.
pub struct SoloDriver {
    pub state: State,
    pub now_secs: u64,
    pub audit_every_tx: bool,
    /// Monotonic submission counter, mixed into each synthesised transaction
    /// id so two identical transactions in the same second stay distinct.
    tx_counter: u64,
}

impl SoloDriver {
    pub fn new(state: State) -> Self {
        SoloDriver { state, now_secs: 0, audit_every_tx: true, tx_counter: 0 }
    }

    pub fn advance(&mut self, secs: u64) {
        self.now_secs += secs;
        self.state.begin_block(self.now_secs);
    }

    /// Submit one transaction. `apply` needs a unique transaction id and
    /// a validity window expressed in EPOCHS; this driver has no `SignedTx`
    /// envelope to draw either from, so it synthesises both:
    ///
    /// - the id mixes a monotonic per-driver counter into the digest. Hashing
    ///   only the transaction and the current time would collide for two
    ///   genuinely distinct submissions of identical content within the same
    ///   second, and a collision here is not a near-miss: the second one is
    ///   refused as `ET_TX_REPLAY`, so a test would silently lose a
    ///   transaction it believes it sent.
    /// - the window is `state.epoch + MAX_TX_LIFETIME_EPOCHS`, in epochs. It
    ///   must not be derived from `now_secs`, which counts SECONDS: once the
    ///   driver has advanced ~30 simulated seconds, a seconds-valued window
    ///   exceeds `state.epoch + MAX_TX_LIFETIME_EPOCHS` and every subsequent
    ///   submission fails `ET_TX_WINDOW_TOO_LONG`.
    pub fn submit(&mut self, tx: Tx, signers: &[Key]) -> Result<(), DriverError> {
        self.tx_counter += 1;
        let tx_id = crate::block::sha256(format!("solo:{}:{}:{:?}", self.tx_counter, self.now_secs, tx).as_bytes());
        let not_after_epoch = self.state.epoch + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS;
        let r = apply(&mut self.state, tx, tx_id, not_after_epoch, signers, self.now_secs).map_err(DriverError::Tx);
        if self.audit_every_tx {
            if let Err(v) = invariants::audit(&self.state) {
                return Err(DriverError::Invariant(v.0));
            }
        }
        r
    }
}
