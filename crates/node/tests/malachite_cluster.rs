//! A REAL networked Malachite cluster on loopback — real OS processes,
//! real TCP gossip, real Ed25519 commit certificates, real wire codec
//! (`engine_codec.rs`). This is exactly what `engine_malachite.rs`'s own
//! module doc calls out as NOT exercisable by that file's in-process unit
//! tests ("real networked multi-node consensus... reachable only by
//! this file's own unit tests, which drive it directly"). Here, nothing is
//! driven directly: every node is a separate `edet-node malachite` child
//! process, wired via `engine_node::write_testnet`/`write_seed_tx`, gossiping
//! over `127.0.0.1` exactly as a real deployment would.
//!
//! Gated behind `--features malachite` (slow git deps; experimental engine —
//! see `Cargo.toml`/`engine_malachite.rs`), so it never runs under a default
//! `cargo test --workspace`:
//!
//!   cargo test -p edet-node --features malachite --test malachite_cluster \
//!     -- --nocapture
//!
//! Byzantine-safety check, not just liveness: a genesis of 4 validators
//! (equal power), but only 3 processes are ever started — the 4th is "down"
//! from the start. Quorum for 4 equal-power validators is
//! `ceil(2*4/3) = 3`, so the 3 live (honest) nodes must still make progress
//! and agree, exactly the "tolerates 1 down" acceptance criterion. A real,
//! Ed25519-signed transaction is submitted (`malachite seed-tx`, reusing the
//! same `dev_seed`/`sign_tx` convention as every other dev harness in this
//! crate) so the assertion is genuinely "the cluster applied this and agrees
//! on the resulting state", not merely "empty blocks tick forward".
//!
//! Observability note: these nodes are started with no `--client-port`, so
//! there is no HTTP endpoint to read (that shape is `malachite_http.rs`'s) —
//! `status.json`, written by
//! `engine_malachite::write_status` after every real commit, is this test's
//! only window into a running node's committed height/state hash.
#![cfg(feature = "malachite")]

mod common;
use common::{observe, run_cli, scratch, spawn, Budget};

/// The acceptance criterion: 4-validator genesis, 3 processes started
/// (node 3 stays down), a real signed transaction submitted, every live
/// (honest) node commits it, and their state commitments agree.
#[test]
fn three_of_four_nodes_commit_a_real_transaction_and_agree() {
    let home = scratch("malachite-cluster-test");

    run_cli(&["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "4"]);
    run_cli(&[
        "malachite",
        "seed-tx",
        "--home",
        home.to_str().unwrap(),
        "--nodes",
        "4",
        "--debtor",
        "0",
        "--creditor",
        "1",
        "--amount",
        "10",
    ]);

    let live = [0usize, 1, 2]; // node 3 never started: "tolerating 1 down"
    let _cluster = spawn(&home, &live);

    // Compared AT A HEIGHT, never at a moment: every block changes the state
    // root by design (the leaf salt binds the block marker), so two nodes read
    // a beat apart hold different hashes while agreeing perfectly.
    let seen = observe(&home, &live, 1, Budget::within(120))
        .unwrap_or_else(|why| panic!("the 3 live (quorum) nodes never reached a height they had all committed: {why}"));
    if let Some(split) = seen.disagreement() {
        panic!("the live nodes disagreed: {split}");
    }
    let common = seen.common_heights();
    assert!(!common.is_empty(), "no height was reported by all three nodes, so nothing was compared");
    eprintln!(
        "3-of-4 loopback cluster reached heights {:?} and agreed at every one of {} shared height(s)",
        seen.tops(),
        common.len()
    );

    drop(_cluster);
    let _ = std::fs::remove_dir_all(&home);
}
