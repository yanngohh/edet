//! The `serve` wiring (`NodeCore::open`), not just the underlying
//! `Replica`, must recover the ledger across a restart. `replication.rs`
//! already covers `Replica::open` directly (snapshot + WAL, torn tail); this
//! test exercises the same durability through the node core the server and
//! the Tauri app actually construct.

#![cfg(feature = "serve")]

use edet_kernel::constants::EPOCH_SECS;
use edet_node::block::{Block, SignedTx};
use edet_node::serve::NodeCore;
use edet_state::tx::Tx;
use edet_state::types::{Key, Party};
use edet_state::State;

fn key(n: u8) -> Key {
    [n; 32]
}

fn genesis(n: u8) -> State {
    let mut st = State::default();
    for i in 0..n {
        st.add_underwriter(vec![key(i + 1)], 25_000.0).expect("unique genesis");
    }
    st
}

#[test]
fn node_core_persists_across_reopen() {
    let dir = std::env::temp_dir().join(format!("edet-test-nodecore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dir_str = dir.to_str().expect("utf8 tmp path");

    // Snapshot every 3 blocks, so recovery below replays a mix of a
    // snapshot plus a WAL tail, like `recovery_from_snapshot_and_wal`.
    let mut core = NodeCore::open(0, 1, genesis(4), Some(dir_str), 3, edet_node::replica::DEFAULT_PRUNE_MARGIN_BLOCKS)
        .expect("open");
    for h in 1..=7u64 {
        let signers = vec![key(1), key(2)];
        let tx = SignedTx {
            tx: Tx::Accept {
                debtor: Party::Member(0),
                creditor: Party::Member(1),
                amount: 10.0 + h as f64,
                maturity_epochs: 30,
                arb: None,
            },
            nonce: edet_node::block::counter_nonce(h),
            not_after_epoch: h + 30,
            signers,
            signatures: vec![],
        };
        // Chained from the replica's OWN app_hash right before this
        // block — this test reopens the store later, and reopen's WAL replay
        // validates app_hash just like a live commit does, so a wrong value
        // here would break recovery, not just this call.
        let app_hash = core.replica.app_hash();
        let block = Block { height: h, time_secs: h * EPOCH_SECS, app_hash, txs: vec![tx] };
        core.replica.commit_block_unchecked(&block).expect("commit");
    }
    let want_height = core.replica.height;
    let want_hash = core.replica.app_hash();
    assert_eq!(want_height, 7);

    // Crash: drop the core (and its store) without any clean shutdown.
    drop(core);

    let reopened = NodeCore::open(0, 1, genesis(4), Some(dir_str), 3, edet_node::replica::DEFAULT_PRUNE_MARGIN_BLOCKS)
        .expect("reopen");
    assert_eq!(reopened.head(), want_height, "height did not survive restart");
    assert_eq!(reopened.replica.app_hash(), want_hash, "state diverged after restart");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn node_core_stays_in_memory_without_data_dir() {
    // No data_dir: behaves exactly like `NodeCore::new` — nothing written
    // to disk, nothing to recover (still available for
    // ad-hoc/test configs).
    let mut core =
        NodeCore::open(0, 1, genesis(4), None, 128, edet_node::replica::DEFAULT_PRUNE_MARGIN_BLOCKS).expect("open");
    let block = Block {
        height: 1,
        time_secs: EPOCH_SECS,
        app_hash: core.replica.app_hash(),
        txs: vec![SignedTx {
            tx: Tx::Accept {
                debtor: Party::Member(0),
                creditor: Party::Member(1),
                amount: 10.0,
                maturity_epochs: 30,
                arb: None,
            },
            nonce: edet_node::block::counter_nonce(0),
            not_after_epoch: 30,
            signers: vec![key(1), key(2)],
            signatures: vec![],
        }],
    };
    core.replica.commit_block_unchecked(&block).expect("commit");
    assert_eq!(core.head(), 1);
}

// ------------------------------------------ the fail-stop across a restart --

/// **A block the audit refuses is neither written nor replayed.**
///
/// The invariants are a fail-stop: a violated one means the transition
/// function did something no reading of the rules allows, every honest node
/// runs the same deterministic code on the same block, so they all halt at the
/// same height rather than diverging. That guarantee is only as deep as the
/// next restart, so two things have to hold together — a refused block leaves
/// no trace on disk, and a store containing one is refused rather than
/// absorbed. Writing the WAL before the audit would break the first; replaying
/// unaudited would break the second, and an operator restarting a node that has
/// stopped committing is the ordinary case rather than an unlikely one.
#[test]
fn a_block_that_breaks_an_invariant_is_neither_written_nor_replayed() {
    let dir = std::env::temp_dir().join(format!("edet-test-failstop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dir_str = dir.to_str().expect("utf8 tmp path");

    {
        let mut core =
            NodeCore::open(0, 1, genesis(4), Some(dir_str), 0, edet_node::replica::DEFAULT_PRUNE_MARGIN_BLOCKS)
                .expect("open");
        let block = Block { height: 1, time_secs: EPOCH_SECS, app_hash: core.replica.app_hash(), txs: Vec::new() };
        core.replica.commit_block_unchecked(&block).expect("an ordinary block commits");
        assert_eq!(core.head(), 1);

        // Doctor the ledger so the next block's audit fails: the cached debt no
        // longer agrees with the contract book, which is the conservation
        // clause and needs no exotic transaction to trip.
        core.replica.state.members.get_mut(&0).expect("member 0").debt_out = 99_900;
        let refused =
            Block { height: 2, time_secs: 2 * EPOCH_SECS, app_hash: core.replica.app_hash(), txs: Vec::new() };
        let err = core
            .replica
            .commit_block_unchecked(&refused)
            .expect_err("a block that leaves the ledger impossible must be refused");
        assert!(
            matches!(err, edet_node::replica::ReplicaError::InvariantViolated { height: 2, .. }),
            "unexpected error: {err:?}"
        );
        assert_eq!(core.head(), 1, "a refused block must not advance the height");
    }

    // Nothing was written: the WAL holds height 1 and nothing else, so a
    // restart cannot resurrect the refused block.
    {
        let mut store = edet_node::store::Store::open(&dir).expect("store");
        let heights: Vec<u64> = store.read_wal().expect("wal").iter().map(|b| b.height).collect();
        assert_eq!(heights, vec![1], "the refused block must not be in the WAL");
    }

    // A clean restart therefore comes back at height 1, on the state the audit
    // was happy with — the doctoring was in memory only.
    {
        let core = NodeCore::open(0, 1, genesis(4), Some(dir_str), 0, edet_node::replica::DEFAULT_PRUNE_MARGIN_BLOCKS)
            .expect("reopen");
        assert_eq!(core.head(), 1);
    }

    // And the second lock, for a store damaged in place or written by a build
    // with a defect the audit catches: a snapshot the invariants refuse stops
    // the node at open rather than being resumed from.
    {
        let mut store = edet_node::store::Store::open(&dir).expect("store");
        let (height, mut state) = store.read_snapshot().expect("snapshot read").unwrap_or((0, genesis(4)));
        state.members.get_mut(&0).expect("member 0").debt_out = 99_900;
        store.write_snapshot(height.max(1), &state).expect("write a poisoned snapshot");
    }
    let refused_to_open =
        NodeCore::open(0, 1, genesis(4), Some(dir_str), 0, edet_node::replica::DEFAULT_PRUNE_MARGIN_BLOCKS).is_err();
    assert!(refused_to_open, "a store the invariants refuse must not be resumed from");

    let _ = std::fs::remove_dir_all(&dir);
}

/// **A WAL that is never pruned grows with the chain's AGE, not its content.**
///
/// An empty block is paced at one a second, so an idle chain writes about 31.5
/// million frames a year — every one of them read and decoded at every start,
/// for heights nothing will ever replay. A snapshot is what makes them
/// redundant, so the sweep that writes one prunes below it, keeping a margin
/// of one interval so a peer slightly behind can still be served.
///
/// The other half is that a node which has pruned must SAY so. Answering "my
/// history starts at 1" when the log starts at 90,000 turns every sync request
/// below that into a silent failure the peer cannot tell from a broken node.
///
/// **How far back is an operator's choice**, because it is how long a
/// validator may be down and still rejoin from a peer rather than from
/// somebody carrying a snapshot. Eight blocks here so the probe can see the
/// sweep; `DEFAULT_PRUNE_MARGIN_BLOCKS` is a day of them, which is the
/// separate claim below.
#[test]
fn the_wal_is_pruned_below_the_snapshot_and_the_floor_is_reported() {
    let dir = std::env::temp_dir().join(format!("edet-test-walprune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dir_str = dir.to_str().expect("utf8 tmp path");

    let interval = 4u64;
    let blocks = 40u64;
    let margin = 2 * interval;
    {
        let mut core = NodeCore::open(0, 1, genesis(4), Some(dir_str), interval, margin).expect("open");
        for h in 1..=blocks {
            let block =
                Block { height: h, time_secs: h * EPOCH_SECS / 2, app_hash: core.replica.app_hash(), txs: Vec::new() };
            core.replica.commit_block_unchecked(&block).expect("commit");
        }
        assert_eq!(core.head(), blocks);
        // Bounded by the retention window rather than by the chain's length.
        assert!(
            core.replica.history_min_height() > 1,
            "nothing was pruned after {blocks} blocks at an interval of {interval}"
        );
        assert!(
            core.replica.history_min_height() <= blocks - 2 * interval + 1,
            "the margin is one interval either side of the snapshot, not more"
        );
    }

    // The frames are really gone, and what is left still opens and still
    // resumes at the same height on the same state.
    {
        let mut store = edet_node::store::Store::open(&dir).expect("store");
        let heights: Vec<u64> = store.read_wal().expect("wal").iter().map(|b| b.height).collect();
        assert!(!heights.is_empty(), "pruning must not empty the log");
        assert!(
            (heights.len() as u64) <= 2 * interval + 1,
            "the WAL holds {} frames, which is not a bounded window",
            heights.len()
        );
        assert!(heights.windows(2).all(|w| w[1] == w[0] + 1), "and what is left is contiguous: {heights:?}");
    }
    let reopened = NodeCore::open(0, 1, genesis(4), Some(dir_str), interval, margin).expect("reopen");
    assert_eq!(reopened.head(), blocks, "a pruned store still resumes at the height it reached");
    let _ = std::fs::remove_dir_all(&dir);

    // **And the DEFAULT keeps a day of blocks**, which is the point of making
    // the margin a setting: a validator down for an hour rejoins from a peer,
    // and one down for a week needs an operator with a snapshot. Forty blocks
    // is nowhere near it, so nothing is pruned and the floor is still 1.
    let dir = std::env::temp_dir().join(format!("edet-test-walkeep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dir_str = dir.to_str().expect("utf8 tmp path");
    let mut core =
        NodeCore::open(0, 1, genesis(4), Some(dir_str), interval, edet_node::replica::DEFAULT_PRUNE_MARGIN_BLOCKS)
            .expect("open");
    for h in 1..=blocks {
        let block =
            Block { height: h, time_secs: h * EPOCH_SECS / 2, app_hash: core.replica.app_hash(), txs: Vec::new() };
        core.replica.commit_block_unchecked(&block).expect("commit");
    }
    assert_eq!(core.replica.history_min_height(), 1, "a day of blocks is kept, and forty is not a day");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The margin can never cut inside the last snapshot's own interval.**
///
/// A syncing peer needs a snapshot AND every block above it, so a margin below
/// two intervals would leave a hole nothing can serve. The CLI refuses one
/// there; the replica floors it whatever it was handed, because the two are
/// different doors onto the same field.
#[test]
fn a_prune_margin_below_two_snapshot_intervals_is_floored_not_obeyed() {
    let dir = std::env::temp_dir().join(format!("edet-test-walfloor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dir_str = dir.to_str().expect("utf8 tmp path");

    let interval = 4u64;
    let mut core = NodeCore::open(0, 1, genesis(4), Some(dir_str), interval, 1).expect("open");
    for h in 1..=40u64 {
        let block =
            Block { height: h, time_secs: h * EPOCH_SECS / 2, app_hash: core.replica.app_hash(), txs: Vec::new() };
        core.replica.commit_block_unchecked(&block).expect("commit");
    }
    let floor = core.replica.history_min_height();
    assert!(
        floor <= 40 - 2 * interval + 1,
        "a margin of one block must still keep two intervals: the log starts at {floor}"
    );
    let mut store = edet_node::store::Store::open(&dir).expect("store");
    let heights: Vec<u64> = store.read_wal().expect("wal").iter().map(|b| b.height).collect();
    assert!(
        heights.len() as u64 >= 2 * interval,
        "and what is kept spans the last snapshot and everything above it: {heights:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
