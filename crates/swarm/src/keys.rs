//! Every key the swarm uses, derived from an index.
//!
//! Four namespaces, separated by their leading byte, because a collision
//! between two of them is not a test failure — it is a member resolving to
//! somebody else's account, and every figure downstream of that is about a
//! community that cannot exist.

use edet_state::types::Key;

/// Member `i`'s key. Derived from the index so a strategy can name a signer
/// without threading keys through every call.
pub fn member_key(i: usize) -> Key {
    let mut k = [0u8; 32];
    k[0..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
    k
}

/// Member `i`'s CONSENSUS key — what their validator signs blocks with, and
/// never a key that can sign a transaction. A different domain byte from
/// [`member_key`], so the two namespaces cannot collide by accident.
pub fn consensus_key(i: usize) -> Key {
    let mut k = [0xC0u8; 32];
    k[0..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
    k
}

/// A key belonging to nobody — for the cases that must be refused because the
/// signer resolves to no member at all.
pub fn stranger_key(n: u8) -> Key {
    [0xF0 | (n & 0x0F); 32]
}

/// The `n`-th key agent `owner` mints for itself: a sybil seat, a rotation
/// target, a key nobody has staked anything on.
///
/// `0x5A` leads, where [`member_key`] leads with the top byte of an index and
/// therefore with zero for every population this crate can seat. Both halves
/// of the pair are in the derivation, so two agents minting their `n`-th key
/// in the same tick mint two different keys.
pub fn fresh_key(owner: usize, n: u32) -> Key {
    let mut k = [0u8; 32];
    k[0] = 0x5A;
    k[1..9].copy_from_slice(&(owner as u64).to_be_bytes());
    k[9..13].copy_from_slice(&n.to_be_bytes());
    k
}
