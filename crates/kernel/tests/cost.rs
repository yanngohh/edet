//! **The cost table, measured.** The paper's §Implementation quotes a
//! millisecond figure per capacity query at four community sizes, and a cited
//! figure with no probe behind it is not a measurement.
//!
//! This is that measurement. It is `#[ignore]`d rather than gated, because a
//! wall-clock assertion in CI is a flake generator and the shape of the answer
//! (bounded by COMMUNITY size, never by network size) is the claim that
//! matters; what a gate can honestly hold is that the harness still compiles
//! and still runs, which `cargo test` does on every pass.
//!
//! Run it with `just cost`, on an otherwise idle machine, and record the
//! hardware beside any figure quoted from it. **A/B, never absolute**: the
//! same probe on the same tree reads 81.7 ms for a 20,000-account query on a
//! busy box and 30.2 ms on a quiet one, so a figure compared against one taken
//! at another time measures the machine.

use std::collections::BTreeMap;
use std::time::Instant;

use edet_kernel::flow::{capacity, Committed, Edges, Reservations};

/// A community of `n` accounts: `u` founding underwriters, and a stake graph
/// where each account backs `deg` others drawn deterministically from a
/// multiplicative walk — dense enough to exercise the level graph, and fixed so
/// the figure is re-measurable rather than re-rolled.
fn community(n: usize, deg: usize) -> (Edges, Vec<(usize, u64)>) {
    let u = (n / 100).max(1);
    let uw: Vec<(usize, u64)> = (0..u).map(|i| (i, 1_000_000u64)).collect();
    let mut edges: Edges = BTreeMap::new();
    // Underwriters reach the first slice of ordinary members directly.
    for i in 0..u {
        for k in 0..deg {
            let d = u + (i * deg + k) % (n - u);
            edges.insert((i, d), 50_000);
        }
    }
    // Everybody else backs `deg` others, chosen by a coprime stride so the
    // graph is connected without being a ring.
    for c in u..n {
        for k in 0..deg {
            let d = u + ((c * 7919 + k * 104_729) % (n - u));
            if d != c {
                edges.insert((c, d), 10_000);
            }
        }
    }
    (edges, uw)
}

#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn one_capacity_query_at_four_community_sizes() {
    let reserved = Reservations::new();
    let committed = Committed::new();
    println!("\n  accounts     edges    max-flow query   one acceptance   early-exit   network build");
    for n in [1_000usize, 5_000, 10_000, 20_000, 50_000, 100_000] {
        let (edges, uw) = community(n, 8);
        let target = n - 1;
        // One untimed pass so the measurement is of the query rather than of
        // the first touch of a freshly built map.
        let _ = capacity(&edges, &reserved, &committed, &uw, &[target], n, u64::MAX);
        let reps = 20;
        let t0 = Instant::now();
        for _ in 0..reps {
            let _ = capacity(&edges, &reserved, &committed, &uw, &[target], n, u64::MAX);
        }
        let per = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        // §Implementation also claims early termination is worth "only 1-2x", which is a
        // second cited figure with nothing behind it. One ordinary acceptance
        // asks whether room exists for ONE amount, never for the maximum.
        let want = 10_000u64;
        let _ = capacity(&edges, &reserved, &committed, &uw, &[target], n, want);
        let t1 = Instant::now();
        for _ in 0..reps {
            let _ = capacity(&edges, &reserved, &committed, &uw, &[target], n, want);
        }
        let early = t1.elapsed().as_secs_f64() * 1e3 / reps as f64;

        // How much of a query is the BUILD rather than the search. `flow`'s
        // loop is `while total < limit`, so a limit of zero returns before the
        // first breadth-first pass and this times `Network::build` alone —
        // which is the term §Implementation says dominates, and which is identical across
        // every set of one invariant audit because invariant 1 measures each
        // on a pristine residual. A claim about where the cost goes is a claim
        // too, and this is its measurement.
        let _ = capacity(&edges, &reserved, &committed, &uw, &[target], n, 0);
        let t2 = Instant::now();
        for _ in 0..reps {
            let _ = capacity(&edges, &reserved, &committed, &uw, &[target], n, 0);
        }
        let build = t2.elapsed().as_secs_f64() * 1e3 / reps as f64;

        println!(
            "{n:10}  {:8}   {per:8.2} ms   {early:8.3} ms   {:5.1}x   {build:8.2} ms",
            edges.len(),
            per / early.max(1e-9)
        );
    }
    println!();
}

/// **The build is read in the walk's own order.** The probe above measures
/// with nothing reserved, and a build that looked its reservation up once per
/// edge reads the same there and grows with how much of the graph is reserved
/// — that axis is what this probe varies. The two conditions are interleaved
/// and the least of five rounds kept, so drift in the machine's load hits both
/// alike; the ratio is the claim, and it should sit near one.
#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn the_build_does_not_grow_with_what_is_reserved() {
    let committed = Committed::new();
    println!("\n  accounts     edges   build, none reserved   build, all reserved   ratio");
    for n in [20_000usize, 100_000] {
        let (edges, uw) = community(n, 8);
        let target = n - 1;
        let none = Reservations::new();
        let all: Reservations = edges.iter().map(|(&k, &w)| (k, w / 2)).collect();
        let (mut b_none, mut b_all) = (f64::MAX, f64::MAX);
        for _ in 0..5 {
            for (r, best) in [(&none, &mut b_none), (&all, &mut b_all)] {
                let _ = capacity(&edges, r, &committed, &uw, &[target], n, 0);
                let t = Instant::now();
                for _ in 0..5 {
                    let _ = capacity(&edges, r, &committed, &uw, &[target], n, 0);
                }
                *best = best.min(t.elapsed().as_secs_f64() * 1e3 / 5.0);
            }
        }
        println!("{n:10}  {:8}   {b_none:17.2} ms   {b_all:16.2} ms   {:5.2}x", edges.len(), b_all / b_none);
    }
    println!();
}
