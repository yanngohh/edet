//! A dedup-aware FIFO mempool. Transactions are peeked (not consumed) when a
//! block is built, and removed only when a block commits — so a proposal that
//! fails to reach quorum loses nothing. Admission (membership gate, per-account
//! rate limits) belongs to the networking layer's CheckTx-equivalent; this
//! structure orders and de-duplicates.

use std::collections::{HashSet, VecDeque};

use crate::block::{Block, SignedTx};

#[derive(Default)]
pub struct Mempool {
    queue: VecDeque<(SignedTx, [u8; 32])>,
    queued: HashSet<[u8; 32]>,
    /// Recently committed hashes, bounded, to reject re-gossip of applied txs.
    seen: VecDeque<[u8; 32]>,
    seen_set: HashSet<[u8; 32]>,
}

const SEEN_CAP: usize = 4096;

/// Hard cap on queued (uncommitted) transactions: bounds this node's
/// memory and the batch a malicious or careless flood could force it to
/// carry, independent of any per-signer rate limiting upstream. A cluster
/// that is genuinely this backed up needs faster blocks or more capacity —
/// not an unbounded queue.
const MAX_QUEUE_LEN: usize = 8192;

impl Mempool {
    /// Queue a transaction. No-op if already queued, recently committed, or
    /// the mempool is at capacity. Returns true if newly accepted (worth
    /// gossiping to peers).
    pub fn push(&mut self, tx: SignedTx) -> bool {
        let h = match tx.hash() {
            Ok(h) => h,
            Err(_) => return false,
        };
        if self.queued.contains(&h) || self.seen_set.contains(&h) {
            return false;
        }
        if self.queue.len() >= MAX_QUEUE_LEN {
            return false;
        }
        self.queue.push_back((tx, h));
        self.queued.insert(h);
        true
    }

    /// Is this hash currently queued (not yet committed)? Used by the
    /// `/tx/outcome` view to distinguish "still pending" from "never
    /// seen".
    pub fn contains(&self, h: &[u8; 32]) -> bool {
        self.queued.contains(h)
    }

    /// Peek up to `max` transactions to build a block, without removing them.
    pub fn batch(&self, max: usize) -> Vec<SignedTx> {
        self.queue.iter().take(max).map(|(tx, _)| tx.clone()).collect()
    }

    /// Drop every transaction that a committed block applied, and remember it
    /// so re-gossip does not re-queue it.
    pub fn remove_committed(&mut self, block: &Block) {
        let mut committed: HashSet<[u8; 32]> = HashSet::new();
        for stx in &block.txs {
            if let Ok(h) = stx.hash() {
                committed.insert(h);
            }
        }
        self.queue.retain(|(_, h)| !committed.contains(h));
        for h in committed {
            self.queued.remove(&h);
            if self.seen_set.insert(h) {
                self.seen.push_back(h);
                if self.seen.len() > SEEN_CAP {
                    if let Some(old) = self.seen.pop_front() {
                        self.seen_set.remove(&old);
                    }
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edet_state::Tx;

    fn tx(contract: u64) -> SignedTx {
        SignedTx {
            tx: Tx::MarkExpired { contract },
            nonce: crate::block::counter_nonce(contract),
            not_after_epoch: 30,
            signers: vec![],
            signatures: vec![],
        }
    }

    #[test]
    fn the_queue_stops_growing_past_its_cap() {
        let mut mp = Mempool::default();
        for i in 0..MAX_QUEUE_LEN as u64 {
            assert!(mp.push(tx(i)), "distinct tx {i} should be admitted below the cap");
        }
        assert_eq!(mp.len(), MAX_QUEUE_LEN);
        assert!(!mp.push(tx(MAX_QUEUE_LEN as u64)), "a queue at capacity must reject a new distinct tx");
        assert_eq!(mp.len(), MAX_QUEUE_LEN, "the rejected tx must not have been queued");
    }
}
