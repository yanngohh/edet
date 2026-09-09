//! Deterministic economic kernel.
//!
//! Pure functions only: no I/O, no clock, no state. Iteration orders are
//! fixed and every function is a deterministic map of its arguments, safe to
//! evaluate inside consensus. The capacity path (`flow`) is integer
//! throughout; nothing in it touches floating point.

pub mod cascade;
pub mod constants;
pub mod flow;
pub mod risk;
