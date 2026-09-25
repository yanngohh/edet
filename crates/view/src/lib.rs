//! **What a viewer may see, and the pool a multi-party transaction waits in.**
//!
//! Both are needed by the node's client surface and by something that is not a
//! node: a simulated member deciding on what a real member could read, and
//! meeting an offer the way a real member meets one. A second implementation
//! of either is the drift this tree already keeps a gate against (`just
//! view-shape-check`), so there is one, here, and the node calls it.
//!
//! Nothing in this crate knows about HTTP, a lock, a cache or a signature
//! scheme. A caller that has a capacity cache passes what it holds; a caller
//! that verifies Ed25519 passes a [`pending::Scheme`] that does.

pub mod disclose;
pub mod pending;

/// Lowercase hex of 32 bytes: a key, a digest, a hash.
pub fn hex32(h: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in h {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}
