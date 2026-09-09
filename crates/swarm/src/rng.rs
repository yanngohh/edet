//! Per-agent randomness, derived from the run's one seed.
//!
//! **Never a shared stream.** A stream every agent draws from makes what one
//! agent does a function of how many times its neighbours drew, so adding an
//! observer changes the run — and a violation reported against a seed then
//! fails to reproduce under a driver that logs one extra line. Each agent's
//! stream is `mix(seed, index)`, the population draw is `mix(seed, u64::MAX)`,
//! and nothing else exists.

/// SplitMix64's finalizer. Used both to derive a stream from `(seed, index)`
/// and as the generator's own step.
fn splitmix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The stream seed for one agent, or for the population draw.
pub fn mix(seed: u64, index: u64) -> u64 {
    splitmix(seed ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// SplitMix64.
///
/// In-tree and dependency-free for the reason `sizing.rs`'s generator is: a
/// run two people replay must produce the same figures, and a dependency
/// would put the process's shape outside the file anybody reading it would
/// look in.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn seeded(seed: u64) -> Self {
        Rng(seed)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        splitmix(self.0)
    }

    /// A value in `0..n`; zero for an empty range.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.next() % n
    }

    /// A value in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// True with probability `p`.
    ///
    /// **A behaviour nobody turned on draws nothing.** A parameter added at
    /// zero would otherwise move every agent's stream, so the corpus diff a
    /// re-pin produces would be dominated by the draw rather than by the rule
    /// that was changed — and a pin whose diff cannot be read is a pin nobody
    /// reads. It costs one comparison and changes no run that uses the
    /// behaviour at all.
    pub fn chance(&mut self, p: f64) -> bool {
        if p <= 0.0 || p.is_nan() {
            return false;
        }
        self.unit() < p
    }

    /// One of `xs`, or `None` when there is nothing to pick.
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> Option<&'a T> {
        if xs.is_empty() {
            return None;
        }
        let i = self.below(xs.len() as u64) as usize;
        xs.get(i)
    }

    /// An amount uniform in `[½, 3⁄2] x mean`, in minor units, never zero.
    ///
    /// Never zero because `to_minor(x) == 0` is a REFUSAL rather than a
    /// rounding, and a population that drew one by accident would spend a run
    /// measuring `ET-CTR-004`. The griefer draws one on purpose.
    pub fn amount_minor(&mut self, mean: u64) -> u64 {
        let lo = mean / 2;
        let span = mean.saturating_sub(lo) + mean / 2;
        (lo + self.below(span.max(1))).max(1)
    }
}
