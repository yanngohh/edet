//! **A community whose members are model sessions, over the real ledger.**
//!
//! Every person is its own session with the whole of its life in context, and
//! acts through tools shaped like an MCP server's; the runner makes each model
//! call and dispatches each tool call on behalf of the one member that session
//! is. The ledger is `edet_swarm::Driver` — `edet_state::apply` runs and
//! `invariants::audit` decides — what a person may read is
//! `edet_view::disclose`, and an offer waits in `edet_view::pending`, the node's
//! own pool. Money that is not credit lives in [`economy`].
//!
//! **A run is its tape.** Nothing a person decides can be re-derived from a
//! seed, so everything that happened is written as it happens: typed events,
//! the raw exchange beside them, and a manifest ([`tape`]). Resuming a run
//! replays the tape into a world and rebuilds every session from it; the
//! player, the violation report and the compile queue are all read off it.
//!
//! **The model generates and the kernel judges; no figure here is a
//! measurement.** What the agents did is a sample of one generator. What the
//! ledger did is arithmetic over this run's own state. Nothing in this crate
//! claims anything, and nothing in it runs in a gate.

pub mod acts;
pub mod cards;
pub mod config;
pub mod economy;
pub mod event;
pub mod index;
pub mod model;
pub mod prompt;
pub mod report;
pub mod run;
pub mod schedule;
pub mod session;
pub mod social;
pub mod tape;
pub mod tools;
pub mod world;

/// Every seeded stream the world draws from, one constant each, so no two
/// share a stream and none depends on how often another drew.
pub mod streams {
    /// Which card each person on the first day plays.
    pub const CARDS: u64 = 0xA6E7_C1C0_0000_0001;
    /// The weather.
    pub const MACRO: u64 = 0xA6E7_C1C0_0000_0002;
    /// The schedule's tie-breaks.
    pub const SCHEDULE: u64 = 0xA6E7_C1C0_0000_0003;
    /// Who knows whom on the first day.
    pub const SOCIAL: u64 = 0xA6E7_C1C0_0000_0004;
    /// Whether a person who does not use edet takes it up today.
    pub const ADOPTION: u64 = 0xA6E7_C1C0_0000_0005;
    /// Household `i`'s money is `HOUSEHOLD + i`.
    pub const HOUSEHOLD: u64 = 0xA6E7_C1C1_0000_0000;
}

/// Minor units per unit, the ledger's and the money's alike.
pub const MINOR: u64 = 100;

/// `12345` minor units as `"123.45"`.
pub fn fmt_minor(x: u64) -> String {
    format!("{}.{:02}", x / MINOR, x % MINOR)
}

/// A decimal amount as a person types it — `"120"`, `"120.5"`, `"120.50"` — in
/// minor units. No float is ever built, so nothing is rounded on the way in.
pub fn parse_amount(s: &str) -> Option<u64> {
    let s = s.trim().trim_start_matches('+');
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) || frac.len() > 2 {
        return None;
    }
    let whole: u64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    let frac: u64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<u64>().ok()? * 10,
        _ => frac.parse().ok()?,
    };
    whole.checked_mul(MINOR)?.checked_add(frac)
}

/// Lowercase hex of any bytes.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The inverse of [`hex`] for a fixed width.
pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim().trim_start_matches("0x");
    if s.len() != 2 * N {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}
