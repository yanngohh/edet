//! Kernel cross-pin: the Rust capacity model against an independent oracle.
//!
//! `sim/fixtures/kernel.json` is produced by `sim/edet_ref.py`, which computes
//! capacity with `scipy.sparse.csgraph.maximum_flow` — an implementation of
//! maximum flow written by people who had never heard of this project. So
//! agreement says the kernel computes the right MATHEMATICS, rather than that
//! two copies of one algorithm agree with each other.
//!
//! Agreement is demanded **bit-identical**. The whole capacity path is
//! integer, so two exact algorithms over the same integers have nothing to
//! disagree about, and a tolerance here would be hiding something rather than
//! accommodating anything.
//!
//! Regenerate with `python3 sim/gen_fixtures.py`.

use std::collections::BTreeMap;

use edet_kernel::flow::{capacity, capacity_with_own_supply, Committed, Edges, Reservations};
use serde_json::Value;

fn load() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sim/fixtures/kernel.json");
    let text = std::fs::read_to_string(path).expect("fixtures present — run python3 sim/gen_fixtures.py");
    serde_json::from_str(&text).expect("valid fixture json")
}

fn pairs(v: &Value) -> Vec<((usize, usize), u64)> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|row| {
            let k = row[0].as_array().expect("key pair");
            ((k[0].as_u64().expect("c") as usize, k[1].as_u64().expect("d") as usize), row[1].as_u64().expect("v"))
        })
        .collect()
}

/// The two readings of the same scene: `own_supply` says whether the target's
/// own supply arc takes part, and both sides of the pin carry the flag.
#[allow(clippy::too_many_arguments)]
fn cut(
    case: &Value,
    edges: &Edges,
    reserved: &Reservations,
    committed: &Committed,
    uw: &[(usize, u64)],
    targets: &[usize],
    n: usize,
    limit: u64,
) -> u64 {
    if case["own_supply"].as_bool().expect("own_supply") {
        capacity_with_own_supply(edges, reserved, committed, uw, targets, n, limit)
    } else {
        capacity(edges, reserved, committed, uw, targets, n, limit)
    }
}

fn singles(v: &Value) -> Vec<(usize, u64)> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|row| (row[0].as_u64().expect("k") as usize, row[1].as_u64().expect("v")))
        .collect()
}

#[test]
fn the_kernel_agrees_with_an_independent_max_flow() {
    let fixtures = load();
    let cases = fixtures["cases"].as_array().expect("cases");
    assert!(!cases.is_empty(), "an empty fixture file would pass vacuously");

    for case in cases {
        let name = case["name"].as_str().expect("name");
        let why = case["why"].as_str().unwrap_or("");
        let n = case["n"].as_u64().expect("n") as usize;

        let edges: Edges = pairs(&case["edges"]).into_iter().collect();
        let reserved: Reservations = pairs(&case["reserved"]).into_iter().collect();
        let committed: Committed = singles(&case["committed"]).into_iter().collect::<BTreeMap<_, _>>();
        let uw: Vec<(usize, u64)> = singles(&case["underwriters"]);
        let targets: Vec<usize> = case["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .map(|t| t.as_u64().expect("t") as usize)
            .collect();
        let expected = case["capacity"].as_u64().expect("capacity");

        let got = cut(case, &edges, &reserved, &committed, &uw, &targets, n, u64::MAX);
        assert_eq!(got, expected, "{name} ({why}): kernel {got} vs reference {expected}");

        // The early-exit path must answer the same question. It is an
        // optimisation, and an optimisation that changed the answer would be a
        // consensus fault rather than a slow path.
        for want in [1u64, expected / 2, expected, expected + 1] {
            let capped = cut(case, &edges, &reserved, &committed, &uw, &targets, n, want);
            assert_eq!(capped, want.min(expected), "{name}: early exit at {want} disagrees with the full answer");
        }
    }
}

/// Determinism, against the same oracle: the answer is a function of the
/// ledger and not of the order records happened to be inserted. Two replicas
/// that agreed on the value while taking different augmenting paths would still
/// be a consensus fault, which is why the capacity path is integer throughout.
#[test]
fn the_kernel_is_permutation_invariant_on_every_fixture() {
    for case in load()["cases"].as_array().expect("cases") {
        let n = case["n"].as_u64().expect("n") as usize;
        let forward: Edges = pairs(&case["edges"]).into_iter().collect();
        let mut reversed = Edges::new();
        for (&k, &v) in forward.iter().rev() {
            reversed.insert(k, v);
        }
        let reserved: Reservations = pairs(&case["reserved"]).into_iter().collect();
        let committed: Committed = singles(&case["committed"]).into_iter().collect::<BTreeMap<_, _>>();
        let mut uw: Vec<(usize, u64)> = singles(&case["underwriters"]);
        let targets: Vec<usize> = case["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .map(|t| t.as_u64().expect("t") as usize)
            .collect();

        let a = cut(case, &forward, &reserved, &committed, &uw, &targets, n, u64::MAX);
        uw.reverse();
        let b = cut(case, &reversed, &reserved, &committed, &uw, &targets, n, u64::MAX);
        assert_eq!(a, b, "{}: permuting the inputs changed the answer", case["name"]);
    }
}
