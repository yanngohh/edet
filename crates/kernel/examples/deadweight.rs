//! Does a capacity query still cost what the ID COUNTER says, or what the
//! graph says?
//!
//! `cargo run --release -p edet-kernel --example deadweight`
//!
//! Sizing the network by `next_member` gives every account ever created a node
//! in every query — including the ones nobody ever staked on. Account creation
//! is unbillable by construction (a key nobody knows has no headroom to
//! charge), so that term is unbounded in something an attacker gets for free.
//!
//! Measured on this graph, id-space sizing against building the network over
//! the accounts that actually take part:
//!
//! | dead accounts | id-space sized | participant sized |
//! |--------------:|---------------:|------------------:|
//! |             0 |      20.5 ms   |          34 ms    |
//! |        10,000 |      27.4 ms   |          32 ms    |
//! |       100,000 |      32.9 ms   |          31 ms    |
//! |     1,000,000 |     266.6 ms   |          29 ms    |
//!
//! The trade is explicit: about 1.65x on a community with nothing dead in it,
//! to make the cost FLAT in a quantity an attacker controls at no cost. A
//! constant factor for the removal of an unbounded attacker-driven term is
//! the right way round, and the checksum column is what says the answers did
//! not move while the cost did.

use edet_kernel::flow::*;
use std::time::Instant;

fn main() {
    let live = 2000usize;
    let mut edges = Edges::new();
    let uw: Vec<(usize, u64)> = (0..6).map(|i| (i, 2500u64)).collect();
    // Each live account backed by an underwriter and by one other member, so
    // the graph has real depth rather than a single hop everywhere.
    for d in 6..live {
        stake(&mut edges, d % 6, d, u64::MAX, 2500);
        stake(&mut edges, (d * 7) % live, d, u64::MAX, 2500);
    }
    let (reserved, committed) = (Reservations::new(), Committed::new());

    println!("{live} live accounts, {} stake edges, 100 capacity queries each:\n", edges.len());
    for dead in [0usize, 10_000, 100_000, 1_000_000] {
        let n = live + dead;
        let started = Instant::now();
        let mut checksum = 0u64;
        for target in 6..106 {
            checksum += capacity(&edges, &reserved, &committed, &uw, &[target], n, u64::MAX);
        }
        println!(
            "  {dead:>9} dead (n = {n:>9}):  {:>7.1} ms   checksum {checksum}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    println!("\nThe checksum is the point: the cost moves, the answer does not.");
}
