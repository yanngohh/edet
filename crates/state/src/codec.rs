//! **The consensus codec: one module, one dependency, one place a migration
//! happens.**
//!
//! Every durable or transmitted byte string in this ledger is produced here —
//! the transaction digest a wallet signs, the leaf bytes the state root
//! commits to, the block on the wire, the snapshot and the write-ahead log.
//! They are not five encodings that happen to agree; they are one, and this
//! module is where it is stated.
//!
//! # What the encoding IS
//!
//! `bincode` in its 1.x default configuration, which is a fixed set of
//! choices and not a version-tolerant format:
//!
//! - integers little-endian and FIXED width (`u64` is eight bytes, never a
//!   varint),
//! - an enum as its variant index in a `u32`, in DECLARATION order,
//! - a sequence, map or string prefixed with its length as a `u64`,
//! - a fixed-size array with NO prefix,
//! - no field names, no type tags, no self-description of any kind.
//!
//! # Why it is pinned here rather than chosen at each call site
//!
//! **Changing any of those choices forks the chain**, and it does so silently
//! in the direction that matters least visibly: an old node and a new one
//! agree that a block is well-formed and disagree about what it says. The
//! transaction digest is the sharpest case — the wallet reimplements this
//! encoding in TypeScript (`ui/src/lib/txdigest.ts`) so that it never signs a
//! digest a node handed it, and the two are cross-pinned by generated vectors
//! (`just tx-digest-check`). A codec change that reached the tree through one
//! call site and not the others would leave that gate green while the wallet
//! signed for a different chain.
//!
//! So the dependency is named in exactly one file. Migrating off `bincode`
//! (1.3 is unmaintained, RUSTSEC-2025-0141, carried with its reason in
//! `justfile`) is then a change to two functions with the whole format written
//! down beside them, rather than a search across three crates — and the gates
//! that would catch a mistake are the fixture checks, which compare bytes
//! rather than behaviour.
//!
//! # What does NOT belong here
//!
//! Anything a human reads or a browser parses. The served views are JSON and
//! deliberately so: they are the API edge, they carry amounts in major units
//! (see [`crate::state::State::to_minor`]), and nothing about them is
//! consensus-critical.

use serde::de::DeserializeOwned;
use serde::Serialize;

/// A value could not be encoded or decoded.
///
/// Encoding failure is unreachable for every type in this tree — they are
/// finite in-memory structs, and a `Vec` sink cannot run out of room in a way
/// `serde` reports — but it is returned rather than unwrapped because a
/// validator may not panic on any input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError(pub String);

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CodecError {}

/// Encode a value in the consensus encoding.
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CodecError> {
    bincode::serialize(value).map_err(|e| CodecError(e.to_string()))
}

/// Decode a value from the consensus encoding.
///
/// **Trailing bytes are tolerated**, which is a property of the format rather
/// than a choice made here: a decode that succeeds does NOT prove the whole
/// buffer was consumed. Anything hashing a received buffer must hash the bytes
/// as they arrived, never a re-encoding of what came back out — see the
/// reassembly path in `engine_malachite`, which is where that matters.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    bincode::deserialize(bytes).map_err(|e| CodecError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The encoding is a set of byte-level commitments, not "whatever the
    /// library does today". Each of these is a choice that a chain's history
    /// depends on, so each is asserted against literal bytes: a dependency
    /// bump that changed one would come back as this test rather than as a
    /// fork.
    #[test]
    fn the_encoding_is_the_one_the_wallet_reimplements() {
        // Fixed-width little-endian integers, no varints.
        assert_eq!(encode(&1u64).unwrap(), vec![1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(encode(&(-2i32)).unwrap(), vec![254, 255, 255, 255]);
        // f64 as IEEE-754 little-endian.
        assert_eq!(encode(&1.0f64).unwrap(), vec![0, 0, 0, 0, 0, 0, 240, 63]);

        // A sequence carries its length as a u64 first.
        assert_eq!(encode(&vec![7u8, 8]).unwrap(), vec![2, 0, 0, 0, 0, 0, 0, 0, 7, 8]);
        // A fixed-size array carries no prefix at all.
        assert_eq!(encode(&[7u8, 8]).unwrap(), vec![7, 8]);
        // A string is length-prefixed the same way, and is not NUL-terminated.
        assert_eq!(encode("ab").unwrap(), vec![2, 0, 0, 0, 0, 0, 0, 0, b'a', b'b']);

        // An enum is its DECLARATION index in a u32 — which is why adding a
        // variant anywhere but the end renumbers every one after it.
        #[derive(serde::Serialize)]
        enum E {
            #[allow(dead_code)]
            First,
            Second(u8),
        }
        assert_eq!(encode(&E::Second(9)).unwrap(), vec![1, 0, 0, 0, 9]);

        // `Option` is a single tag byte, not a u32 variant index.
        assert_eq!(encode(&None::<u8>).unwrap(), vec![0]);
        assert_eq!(encode(&Some(5u8)).unwrap(), vec![1, 5]);
    }

    /// A successful decode is not a claim that the buffer was fully consumed.
    /// Stated as a test because callers that hash what they received depend on
    /// knowing it.
    #[test]
    fn a_decode_tolerates_trailing_bytes() {
        let mut bytes = encode(&7u64).unwrap();
        bytes.extend_from_slice(b"tampered");
        assert_eq!(decode::<u64>(&bytes).unwrap(), 7);
    }
}
