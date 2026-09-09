//! **A viewer credential is single-use inside its window.**
//!
//! A signed read travels in clear unless an operator terminates TLS in front
//! of the node, and the signature covers the path and a timestamp — so anybody
//! who reads one off the wire can replay it, as that member, for the rest of
//! its 60-second skew window. TLS is still the answer for confidentiality; this
//! is the part that can be done inside the node, and what it buys is that the
//! captured credential is already spent.
//!
//! **Ed25519 is deterministic**, so the message had to gain a nonce before a
//! signature-keyed cache could exist at all: two honest reads of the same path
//! in the same second carry the *same* signature, and a cache without a nonce
//! refuses the second one. The nonce is 16 random bytes, part of the signed
//! bytes and carried in its own header, which is what makes two identical
//! requests two different credentials.
//!
//! Only VERIFIED signatures enter, which is what bounds the store: filling it
//! costs a member key and a signature per entry, and the read budget already
//! bounds how fast one source may present them.

use std::collections::{BTreeMap, BTreeSet};

/// How many verified signatures are remembered at once.
///
/// The arithmetic, so the number is a claim rather than a round figure: one
/// source may spend `READ_BUCKET_CAPACITY` (120) tokens and refills at
/// `READ_BUCKET_REFILL_PER_SEC` (30), so a window of `TIMESTAMP_SKEW_SECS`
/// (60) admits at most `120 + 60 x 30 = 1,920` reads from one IP. Ten sources
/// at once is 19,200, and this is that with a factor of two — 40,960 entries of
/// 64 bytes plus index overhead, a few megabytes.
///
/// It is a ceiling rather than a budget: entries leave when their window
/// closes, so a node at rest holds nothing. When it is reached the OLDEST
/// bucket is evicted, which can only ever let a replay of an about-to-expire
/// credential through, never refuse an honest first read.
pub(crate) const MAX_SEEN_SIGNATURES: usize = 40_960;

/// The verified viewer signatures still inside their window.
///
/// Two structures over the same set because the two questions differ: `seen`
/// answers "has this exact signature been presented", and `by_expiry` answers
/// "which of them may be dropped now". A single list would make the first
/// question linear in the second's size.
#[derive(Default)]
pub(crate) struct SeenSignatures {
    by_expiry: BTreeMap<u64, Vec<[u8; 64]>>,
    seen: BTreeSet<[u8; 64]>,
    /// Entries beyond which the oldest bucket is dropped. `0` means the
    /// default.
    cap: usize,
}

impl SeenSignatures {
    /// A store with a smaller ceiling, for the probe that has to reach it.
    #[cfg(test)]
    pub(crate) fn with_cap(cap: usize) -> Self {
        SeenSignatures { cap, ..Default::default() }
    }

    fn cap(&self) -> usize {
        if self.cap == 0 {
            MAX_SEEN_SIGNATURES
        } else {
            self.cap
        }
    }

    /// Record `sig`, or refuse it as a replay.
    ///
    /// `false` means this exact signature has already been presented inside
    /// its window. `ts` is the timestamp the signature covers, which is what
    /// decides when the entry may be forgotten — the credential is worthless
    /// past `ts + skew` whatever this store holds, so keeping it longer buys
    /// nothing.
    pub(crate) fn admit(&mut self, sig: [u8; 64], ts: u64, now: u64, skew: u64) -> bool {
        self.prune(now);
        if !self.seen.insert(sig) {
            return false;
        }
        self.by_expiry.entry(ts.saturating_add(skew)).or_default().push(sig);
        while self.seen.len() > self.cap() {
            // The oldest bucket first: a credential nearest its own expiry is
            // the one whose replay window is shortest, so evicting it costs
            // the least. Refusing an honest first read instead would be a
            // denial of service under load, which is the wrong direction for
            // a defence against replay.
            let Some((&oldest, _)) = self.by_expiry.iter().next() else { break };
            let Some(bucket) = self.by_expiry.remove(&oldest) else { break };
            for s in bucket {
                self.seen.remove(&s);
            }
        }
        true
    }

    /// Forget every signature whose window has closed. Called by `admit` and
    /// again after each committed block, beside the session sweep, so a node
    /// that stops being read stops holding anything.
    pub(crate) fn prune(&mut self, now: u64) {
        let live = self.by_expiry.split_off(&now);
        for (_, bucket) in std::mem::replace(&mut self.by_expiry, live) {
            for s in bucket {
                self.seen.remove(&s);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(n: u8) -> [u8; 64] {
        [n; 64]
    }

    #[test]
    fn a_signature_is_admitted_once_inside_its_window() {
        let mut seen = SeenSignatures::default();
        assert!(seen.admit(sig(1), 1000, 1000, 60));
        assert!(!seen.admit(sig(1), 1000, 1000, 60), "the same credential twice is a replay");
        assert!(seen.admit(sig(2), 1000, 1000, 60), "a different one is not");
    }

    /// A store that never forgot would grow without bound on a node nobody
    /// attacks. Past the window the credential is refused by the skew check
    /// anyway, so holding it buys nothing.
    #[test]
    fn a_closed_window_is_forgotten() {
        let mut seen = SeenSignatures::default();
        assert!(seen.admit(sig(1), 1000, 1000, 60));
        seen.prune(1061);
        assert_eq!(seen.len(), 0, "the window closed");
        assert!(seen.admit(sig(1), 1061, 1061, 60), "and a fresh credential is not a replay");
    }

    /// The ceiling evicts the OLDEST bucket rather than refusing the newest
    /// entry: a defence against replay that turns into a denial of service
    /// under load is the wrong trade.
    #[test]
    fn the_seen_set_is_bounded_and_evicts_the_oldest() {
        let mut seen = SeenSignatures::with_cap(4);
        for i in 0..4u8 {
            assert!(seen.admit(sig(i), 1000 + i as u64, 1000, 60));
        }
        assert_eq!(seen.len(), 4);
        assert!(seen.admit(sig(200), 1010, 1000, 60), "a new credential is always admitted");
        assert!(seen.len() <= 4, "the store stays inside its ceiling: {}", seen.len());
        assert!(seen.admit(sig(0), 1000, 1000, 60), "the oldest was evicted, so its replay is no longer caught");
        assert!(!seen.admit(sig(200), 1010, 1000, 60), "the newest is still remembered");
    }
}
