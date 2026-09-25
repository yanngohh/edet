//! **Who has a day today.**
//!
//! The run has one budget of days for everybody, over its whole length. Each
//! day the scheduler spends what is left spread evenly over the days left to
//! the target, gives those days to the people who have gone longest without
//! one — a person who has never had one first — and breaks ties by a draw from
//! the world's schedule stream. The order it returns is the order days are
//! lived and applied in, which matters: priority under a binding ceiling is
//! first-come.
//!
//! A person with no account is scheduled while an offer waits for them, and
//! otherwise only on a day something brings them to a wallet: bills they
//! cannot meet, or a community they can see settling what it owes
//! ([`World::wants_in`]). **Having had a day does not earn the next one.**
//! Under the older rule one such draw made a person a daily user for the rest
//! of the run, with nothing they could do on a ledger they had no account on:
//! pilot-4's 42 wallet-holding neighbours lived 2,656 of its 3,300 days, paid
//! cash, posted and wrote to each other. A real person without an account
//! opens the app when something asks them to, which is what the three doors
//! are. A member keeps having days, and a person whose life no longer fits the
//! model is not scheduled at all.

use edet_state::types::Party;
use edet_swarm::rng::{mix, Rng};

use crate::streams;
use crate::world::World;

pub fn today(world: &World, tick: u64) -> Vec<usize> {
    let cfg = &world.cfg;
    let left = cfg.turn_budget.saturating_sub(world.turns_used);
    if left == 0 {
        return Vec::new();
    }
    let st = world.st();
    let eligible: Vec<usize> = world
        .persons
        .iter()
        .filter(|p| p.retired.is_none())
        .filter(|p| {
            p.member.is_some()
                || !world.pool.for_party(st, Party::Key(p.key)).0.is_empty()
                || world.wants_in(p.index, tick).is_some()
        })
        .map(|p| p.index)
        .collect();
    let days_left = cfg.target_ticks.saturating_sub(tick).saturating_add(1).max(1);
    let n = (left.div_ceil(days_left) as usize).min(eligible.len());
    let mut rng = Rng::seeded(mix(mix(cfg.world_seed, streams::SCHEDULE), tick));
    let mut keyed: Vec<(u64, u64, usize)> = eligible
        .into_iter()
        .map(|i| {
            let since = world.persons[i].last_day.map(|t| t + 1).unwrap_or(0);
            (since, rng.next(), i)
        })
        .collect();
    keyed.sort_unstable();
    keyed.into_iter().take(n).map(|(_, _, i)| i).collect()
}
