//! **Who knows whom.**
//!
//! A town is not a market of strangers drawn evenly: people buy from the same
//! few they have always bought from, and the person they have never met is the
//! one they have no reason to trust. Without that distinction a run cannot say
//! anything about the thing the ledger is for — **an underwritten claim is how
//! you take the paper of somebody you have no reason to trust**, and a world
//! where everybody is equally a stranger, equally visible and equally scored
//! never puts anybody to that choice.
//!
//! The graph is seeded and built once from the cards: a ring of nearest
//! neighbours, a share of its edges rewired to somebody across the town, and a
//! degree per person that says what their card is like — a stallholder knows
//! the market, a teenager knows four people. It then GROWS: a trade that
//! completes writes an edge, and so does an introduction, so that dealing with
//! a stranger is what stops them being one.
//!
//! Nothing here is on the ledger, and the ledger cannot see it. It decides who
//! a person's day brings them and what their note calls that person; every
//! refusal is still the ledger's own.

use std::collections::BTreeSet;

use edet_swarm::rng::{mix, Rng};

use crate::config::SocialConfig;
use crate::streams;

/// The acquaintance graph. Absent from a configuration there is none, and a
/// town where nobody is a stranger is the town every run without one lives in.
#[derive(Clone, Debug, Default)]
pub struct Social {
    /// One sorted set of acquaintances per person, symmetric by construction.
    knows: Vec<BTreeSet<usize>>,
    on: bool,
}

impl Social {
    /// No graph at all: everybody knows everybody, which is what a world with
    /// no acquaintance in it amounts to.
    pub fn none() -> Self {
        Social { knows: Vec::new(), on: false }
    }

    /// Build the town's acquaintances from its cards. `degree_of` says how
    /// many people a person of that card knows; the ring gives everybody at
    /// least their share of near neighbours and `rewire_ppm` of those edges go
    /// to somebody drawn from the whole town instead, which is what makes a
    /// town small-world rather than a circle.
    pub fn found(cfg: &SocialConfig, world_seed: u64, cards: &[String]) -> Self {
        if cfg.knows_mean == 0 {
            return Social::none();
        }
        let n = cards.len();
        let mut knows = vec![BTreeSet::new(); n];
        if n < 2 {
            return Social { knows, on: true };
        }
        let mut rng = Rng::seeded(mix(world_seed, streams::SOCIAL));
        for i in 0..n {
            let want = cfg.degree_of(&cards[i]) as usize;
            // Half the degree forward on the ring: the other half arrives from
            // the people behind, since every edge is written both ways.
            for k in 1..=(want.div_ceil(2)).min(n.saturating_sub(1)) {
                let mut j = (i + k) % n;
                if rng.below(1_000_000) < cfg.rewire_ppm {
                    // Anybody but themselves, drawn evenly.
                    let mut r = rng.below(n as u64 - 1) as usize;
                    if r >= i {
                        r += 1;
                    }
                    j = r;
                }
                if j != i {
                    knows[i].insert(j);
                    knows[j].insert(i);
                }
            }
        }
        Social { knows, on: true }
    }

    /// Whether the graph decides anything at all.
    pub fn on(&self) -> bool {
        self.on
    }

    /// A person joining the town knowing nobody.
    pub fn grow(&mut self) {
        if self.on {
            self.knows.push(BTreeSet::new());
        }
    }

    /// Two people have now dealt with each other, which is what stops somebody
    /// being a stranger. Idempotent, and symmetric.
    pub fn meet(&mut self, a: usize, b: usize) {
        if !self.on || a == b {
            return;
        }
        let need = a.max(b) + 1;
        if self.knows.len() < need {
            self.knows.resize(need, BTreeSet::new());
        }
        self.knows[a].insert(b);
        self.knows[b].insert(a);
    }

    /// Whether `a` knows `b`. With no graph everybody does.
    pub fn knows(&self, a: usize, b: usize) -> bool {
        if !self.on {
            return true;
        }
        a == b || self.knows.get(a).is_some_and(|s| s.contains(&b))
    }

    /// Who `a` knows, in person order. Empty with no graph, where the question
    /// does not arise.
    pub fn neighbours(&self, a: usize) -> Vec<usize> {
        self.knows.get(a).map(|s| s.iter().copied().collect()).unwrap_or_default()
    }

    /// Somebody for `a` to trade with today: one of their acquaintances, or —
    /// with `strangers_ppm` — anybody in the town, which is the new customer
    /// nobody has met yet. `None` when the graph is on and this person knows
    /// nobody at all: a day brings them nothing, and that is the point.
    pub fn partner(&self, cfg: &SocialConfig, rng: &mut Rng, a: usize, people: usize) -> Option<usize> {
        let anybody = |rng: &mut Rng| {
            let mut j = rng.below(people as u64 - 1) as usize;
            if j >= a {
                j += 1;
            }
            j
        };
        if people < 2 {
            return None;
        }
        if !self.on {
            return Some(anybody(rng));
        }
        // Drawn whatever the graph says, so the stream does not depend on how
        // many people anybody happens to know.
        let stranger = rng.below(1_000_000) < cfg.strangers_ppm;
        let known = self.neighbours(a);
        let pick = anybody(rng);
        if stranger || known.is_empty() {
            return Some(pick);
        }
        Some(known[pick % known.len()])
    }
}
