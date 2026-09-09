//! A bounded in-memory token-bucket rate limiter. Ingress admission
//! (empty-signer gating, membership checks) is a separate concern — this is
//! purely "how fast may this identity act", so a single misbehaving client
//! (or a flood of permissionless crank submissions) cannot swamp the mempool
//! or the pending-signature pool. Not a substitute for network-level DoS
//! protection; a stopgap sized for a dev/small-cluster deployment.

use std::collections::HashMap;
use std::time::Instant;

struct Bucket {
    tokens: f64,
    last: Instant,
}

pub struct RateLimiter {
    capacity: f64,
    refill_per_sec: f64,
    /// Bound on distinct tracked identities, so an attacker flooding with
    /// ever-new keys cannot grow this table without bound.
    ///
    /// When it fills, buckets that have refilled to capacity are swept before
    /// anything is refused — see `allow`. Refusing outright instead (the
    /// original rule) was fail-closed in the wrong direction: the table was
    /// never pruned, so 10 000 junk keys — needing no credential on the
    /// `/pending/sign` path, and measured at 244 ms of traffic — locked out
    /// every identity the node had not already seen, for the lifetime of the
    /// process rather than for a refill window.
    max_keys: usize,
    buckets: HashMap<Vec<u8>, Bucket>,
}

impl RateLimiter {
    pub fn new(capacity: f64, refill_per_sec: f64, max_keys: usize) -> RateLimiter {
        RateLimiter { capacity, refill_per_sec, max_keys, buckets: HashMap::new() }
    }

    /// Drop every bucket that has refilled to capacity.
    ///
    /// Evicting these is free, which is what makes a bounded table safe to
    /// prune at all. A never-seen key is created at full capacity, so a
    /// full bucket and an absent one grant exactly the same thing — dropping
    /// one resets nobody's limit, which was the whole objection to eviction.
    /// What cannot be evicted is a bucket still below capacity, because that
    /// is the only kind whose removal would hand someone back spent tokens.
    ///
    /// The attack this defeats therefore costs what it should: to keep the
    /// table full an attacker must hold `max_keys` buckets BELOW capacity,
    /// which means spending tokens on each one continuously rather than in a
    /// single burst — and the table self-heals the moment they stop.
    fn sweep_full(&mut self, now: Instant) {
        let capacity = self.capacity;
        let refill = self.refill_per_sec;
        self.buckets.retain(|_, b| {
            let elapsed = now.duration_since(b.last).as_secs_f64();
            (b.tokens + elapsed * refill).min(capacity) < capacity
        });
    }

    /// True iff `key` may act now; consumes one token when it does.
    /// Take one token. See `allow_cost` — a request whose worst case is one
    /// unit of work.
    pub fn allow(&mut self, key: &[u8]) -> bool {
        self.allow_cost(key, 1.0)
    }

    /// Take `cost` tokens, so a budget can bound WORK rather than request
    /// count.
    ///
    /// The two are the same thing only where every request costs the same, and
    /// on the read surface they do not: a members listing answers a max-flow
    /// per member it serves, while a params read answers from a struct. A
    /// count-based budget sized for the cheap ones lets the expensive ones hold
    /// the node lock continuously, and one sized for the expensive ones refuses
    /// ordinary polling.
    ///
    /// A request is admitted when the bucket holds at least ONE token, and then
    /// charged `cost` — which may take the bucket negative. Refusing an
    /// expensive read outright until a full `cost` had accumulated would starve
    /// a caller whose budget is smaller than the price of one read; going
    /// negative instead makes them wait exactly as long as the work they did.
    pub fn allow_cost(&mut self, key: &[u8], cost: f64) -> bool {
        let now = Instant::now();
        if !self.buckets.contains_key(key) {
            if self.buckets.len() >= self.max_keys {
                self.sweep_full(now);
            }
            // Still full: every tracked identity is genuinely mid-throttle, so
            // this is real overload rather than a table stuffed with keys that
            // cost nothing to mint. Refusing here is the honest answer.
            if self.buckets.len() >= self.max_keys {
                return false;
            }
            self.buckets.insert(key.to_vec(), Bucket { tokens: self.capacity, last: now });
        }
        let bucket = self.buckets.get_mut(key).expect("just inserted or already present");
        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= cost.max(1.0);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refill of one token a second decides this test on a loaded box: the
    /// four calls are consecutive and in-process, but a second of deschedule
    /// between any two of them hands back the very token the last assertion
    /// says is spent. Nothing here is about refilling, so the rate is set
    /// where a stall would have to last a fortnight to matter — the idiom
    /// `sweeping_does_not_reset_a_bucket_that_is_still_throttled` already uses.
    #[test]
    fn a_burst_within_capacity_passes_then_throttles() {
        let mut rl = RateLimiter::new(3.0, 0.000_001, 10);
        assert!(rl.allow(b"a"));
        assert!(rl.allow(b"a"));
        assert!(rl.allow(b"a"));
        assert!(!rl.allow(b"a"), "a fourth immediate call must be throttled");
        assert!(rl.allow(b"b"), "a different key has its own, unspent bucket");
    }

    #[test]
    fn the_key_table_fails_closed_while_every_tracked_identity_is_throttled() {
        // Capacity 1, so `a`'s single token is spent and its bucket is below
        // capacity — the one kind that may not be evicted. The refill is set
        // near zero for the reason above: at one a second, a stalled thread
        // returns `a`'s token, the sweep then finds a full bucket to evict,
        // and `b` is admitted — the opposite of what this asserts, decided by
        // the machine.
        let mut rl = RateLimiter::new(1.0, 0.000_001, 1);
        assert!(rl.allow(b"a"));
        assert!(!rl.allow(b"b"), "a distinct identity is refused while the table is genuinely saturated");
    }

    /// The table has to be swept, or keys that cost an
    /// attacker nothing to mint wedged out every unseen identity for the
    /// life of the process. Buckets that have refilled to capacity grant
    /// exactly what an absent key grants, so sweeping them frees the table
    /// without returning a spent token to anybody.
    #[test]
    fn a_flood_of_idle_keys_does_not_wedge_out_a_genuine_one() {
        // Refill fast enough that the flood's buckets are back at capacity
        // by the time the genuine caller arrives.
        let mut rl = RateLimiter::new(2.0, 10_000.0, 16);
        for i in 0..16u32 {
            assert!(rl.allow(&i.to_le_bytes()), "the flood itself is served");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(
            rl.allow(b"genuine-co-signer"),
            "PROVEN CLOSED: an identity the node has never seen is still admitted after the flood"
        );
    }

    /// The sweep must never hand back a token somebody already spent.
    #[test]
    fn sweeping_does_not_reset_a_bucket_that_is_still_throttled() {
        let mut rl = RateLimiter::new(2.0, 0.000_001, 2);
        assert!(rl.allow(b"victim"));
        assert!(rl.allow(b"victim"));
        assert!(!rl.allow(b"victim"), "spent");
        // Fill the table so the next distinct key triggers a sweep.
        assert!(rl.allow(b"other"));
        let _ = rl.allow(b"newcomer");
        assert!(!rl.allow(b"victim"), "the victim's spent bucket survived the sweep");
    }
}
