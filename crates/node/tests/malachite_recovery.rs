//! **What happens to a validator that stops, and to one that stops for too
//! long.**
//!
//! Everything else in this crate tests a cluster that stays up. Three things a
//! real deployment meets constantly were tested by nothing: a node paused, a
//! node paused past the point where its peers give up on the connection, and a
//! node paused past its peers' history floor — the one case value sync cannot
//! answer, because nobody holds the blocks it needs any more.
//!
//! `SIGSTOP` rather than a kill, because the two are different failures and
//! the pause is the one this file is about: the process keeps its sockets and
//! its memory and simply stops being scheduled, which is what a machine under
//! load, a paused VM or a long GC looks like to its peers.
//!
//! **What holds a validator to its peers.** This tree runs peer discovery
//! OFF, so that no name ever reaches the transport (`resolve_peer_names`, and
//! the advisory `just audit` carries on that argument). With discovery off,
//! upstream keeps an inbound connection for the life of the process and
//! dials a configured peer again whenever it is not connected, on a
//! one-second timer, reading the configured literal and nothing learned on
//! the wire. Every pair keeps ONE connection because only one side dials it
//! (`engine_node::split_by_dialer`: the lower peer id dials), since a message
//! to a peer with two connections is handed to whichever is ready and keeps
//! no order between them. The probes below hold the re-dial from both ends:
//! a validator whose peers dropped it, and a peer missed at boot, each the
//! validator that dials NOBODY by the rule, so that only its peers' dials can
//! reach it. Below the history floor a re-dial is not enough either, and the
//! last probe walks the snapshot an operator carries instead.
//!
//! Consensus base ports 27200, 27240, 27280, 27320, 27360 and 27400, clear of the
//! loopback testnet's 26600, `malachite_byzantine`'s 26800,
//! `malachite_federation`'s 27000 and Fedora's `passim` at 27500.
//!
//!   cargo test -p edet-node --features malachite --test malachite_recovery \
//!     -- --nocapture --test-threads=1
#![cfg(feature = "malachite")]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

mod common;
use common::{
    await_height, bin, heights, node_log, observe, read_status, run_cli, run_cli_env, scratch, spawn_env, Budget,
    Cluster, Wait,
};

/// This file's own consensus range, one per probe: a testnet's ports are fixed
/// per validator index rather than negotiated.
fn base_port(port: u16) -> [(&'static str, String); 1] {
    [("EDET_CONSENSUS_BASE_PORT", port.to_string())]
}

/// Stop and resume a child by pid. Unix only, as every engine test here is.
fn signal(cluster: &Cluster, slot: usize, sig: &str) {
    let status = Command::new("kill")
        .args([sig, &cluster.pid(slot).to_string()])
        .status()
        .expect("send a signal to the child");
    assert!(status.success(), "kill {sig} on slot {slot}");
}

fn height_of(home: &Path, index: usize) -> u64 {
    read_status(home, index).map(|s| s.height).unwrap_or(0)
}

/// **Three of four keep committing while the fourth is paused, and the
/// fourth rejoins on its own.**
///
/// Quorum for four equal-power validators is three, so the live three are
/// exactly quorum and must make progress without the fourth — which is the
/// whole of what "tolerates one down" means.
///
/// What the fourth does when it resumes is no longer a race with the pause's
/// length. Its peers may have timed the connections out; upstream's re-dial,
/// from whichever side of each pair the rule gives the dial, is what brings
/// it back either way, so the claim is the ROUTE: it rejoins without an
/// operator, and then agrees at every height it and the others share.
///
/// Mutation that bites: pin Malachite back to `v0.5.0`, whose network
/// repairs a dropped connection only under discovery. A pause
/// long enough for the peers to drop the connections leaves the resumed
/// validator alive, rebroadcasting votes nobody receives, at the height it
/// was paused at.
#[test]
fn a_paused_validator_rejoins_without_a_restart() {
    let home = scratch("malachite-recovery-pause");
    run_cli_env(&["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "4"], &base_port(27200));

    let live: Vec<usize> = (0..4).collect();
    let all = spawn_env(&home, &live, |_| Vec::new(), &[]);
    await_height(&home, &live, 3, Budget::within(120))
        .unwrap_or_else(|why| panic!("the cluster never reached height 3: {why}"));

    signal(&all, 3, "-STOP");
    let paused_at = height_of(&home, 3);
    let others = [0usize, 1, 2];
    await_height(&home, &others, paused_at + 10, Budget::within(120))
        .unwrap_or_else(|why| panic!("three of four must keep committing with one paused: {why}"));
    assert_eq!(height_of(&home, 3), paused_at, "a paused node commits nothing");

    signal(&all, 3, "-CONT");
    let target = height_of(&home, 0);
    await_height(&home, &[3], target, Budget::within(120))
        .unwrap_or_else(|why| panic!("the resumed validator must rejoin on its own: {why}"));

    let seen =
        observe(&home, &live, 1, Budget::within(60)).unwrap_or_else(|why| panic!("a height they all reported: {why}"));
    if let Some(split) = seen.disagreement() {
        panic!("a validator that rejoined disagreed with the ones that stayed up: {split}");
    }

    drop(all);
    let _ = std::fs::remove_dir_all(&home);
}

/// The validator the rule leaves nothing to dial: the greatest peer id of
/// the testnet's `nodes`. Its config lists every peer and `load_config`
/// keeps none of them, so its peers' dials are the only way to reach it.
fn dialed_by_everyone(nodes: usize) -> usize {
    (0..nodes)
        .max_by_key(|&i| edet_node::engine_node::peer_id_of_seed(&edet_node::block::dev_consensus_seed(i as u8)))
        .expect("a testnet has nodes")
}

/// **A validator its peers dropped is dialed again by them.** The validator
/// that dials nobody by the rule is killed, so every survivor sees its
/// connection close; it comes back listening, with nothing of its own to
/// dial, so only the survivors' re-dial can reach it — and it must catch up.
///
/// Mutation that bites: pin Malachite back to `v0.5.0`, whose network
/// repairs a dropped connection only under discovery. The restarted node
/// listens and nobody calls.
#[test]
fn a_validator_dropped_by_its_peers_is_re_dialed_by_them() {
    let home = scratch("malachite-recovery-redial");
    run_cli_env(&["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "4"], &base_port(27320));

    let live: Vec<usize> = (0..4).collect();
    let quiet = dialed_by_everyone(4);
    let others: Vec<usize> = live.iter().copied().filter(|&i| i != quiet).collect();
    let mut all = spawn_env(&home, &live, |_| Vec::new(), &[]);
    await_height(&home, &live, 3, Budget::within(120))
        .unwrap_or_else(|why| panic!("the cluster never reached height 3: {why}"));

    signal(&all, quiet, "-KILL");
    let _ = all.0[quiet].wait();
    let dropped_at = height_of(&home, others[0]);
    await_height(&home, &others, dropped_at + 5, Budget::within(120))
        .unwrap_or_else(|why| panic!("three of four must keep committing with one dead: {why}"));

    let back = spawn_env(&home, &[quiet], |_| Vec::new(), &[]);
    let target = height_of(&home, others[0]);
    await_height(&home, &[quiet], target, Budget::within(120))
        .unwrap_or_else(|why| panic!("a validator with nobody to dial must be dialed by its peers: {why}"));

    drop(back);
    drop(all);
    let _ = std::fs::remove_dir_all(&home);
}

/// **A peer missed at boot is dialed until it answers.** Three validators
/// start, and the one that dials nobody by the rule appears half a minute
/// later — past five retries on a Fibonacci backoff — so it must be reached,
/// which only an unbounded retry of a configured address does.
///
/// Mutation that bites: pin Malachite back to `v0.5.0`, whose network has no
/// persistent-peer timer. The three give up on the address after five
/// retries, at about twelve seconds, and the late one never sees a block.
#[test]
fn a_peer_missed_at_boot_is_dialed_until_it_answers() {
    let home = scratch("malachite-recovery-late");
    run_cli_env(&["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "4"], &base_port(27360));

    let late = dialed_by_everyone(4);
    let early: Vec<usize> = (0..4).filter(|&i| i != late).collect();
    let first = spawn_env(&home, &early, |_| Vec::new(), &[]);
    await_height(&home, &early, 3, Budget::within(120))
        .unwrap_or_else(|why| panic!("three of four never reached height 3: {why}"));
    std::thread::sleep(Duration::from_secs(30));

    let last = spawn_env(&home, &[late], |_| Vec::new(), &[]);
    let target = height_of(&home, early[0]);
    await_height(&home, &[late], target, Budget::within(120))
        .unwrap_or_else(|why| panic!("a validator that starts late must be dialed until it answers: {why}"));

    drop(last);
    drop(first);
    let _ = std::fs::remove_dir_all(&home);
}

/// **No WAL entry is dropped at the start of a height.** A validator appends
/// every input it processes to its WAL under the height it is at, and the
/// WAL actor drops an append for any other height; an engine that stamped
/// the inputs it replays at the start of a height with the previous one lost
/// the node's own first votes of every height it began with something
/// buffered, and a crash inside such a height replayed less than the node
/// had done. The engine logs a dropped append at `warn`, so a cluster that
/// commits ten heights without one is the claim.
///
/// Mutation that bites: pin Malachite back to `v0.5.0`, whose engine stamps
/// the replayed inputs with a height snapshot taken before the input — the
/// log carries `Ignoring append` sixteen times in one height.
#[test]
fn no_wal_entry_is_dropped_at_the_start_of_a_height() {
    let home = scratch("malachite-recovery-wal");
    run_cli_env(&["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "4"], &base_port(27400));

    let live: Vec<usize> = (0..4).collect();
    let cluster = spawn_env(&home, &live, |_| Vec::new(), &[]);
    await_height(&home, &live, 10, Budget::within(180))
        .unwrap_or_else(|why| panic!("the cluster never reached height 10: {why}"));
    drop(cluster);

    for &i in &live {
        let log = node_log(&home, i);
        let dropped = log.lines().filter(|l| l.contains("Ignoring append")).count();
        assert_eq!(dropped, 0, "node {i} dropped {dropped} WAL append(s) for a height mismatch");
    }
    let _ = std::fs::remove_dir_all(&home);
}

/// **The wait tells a stall from a slow box.** Every budget in these harnesses
/// reads through `common::await_height`, which fails on two different
/// sentences: nothing advanced for `stall`, or the target unmet at the
/// ceiling while heights still moved. A loaded box produces the second and
/// never the first; a cluster that has lost quorum produces the first long
/// before the second. Here all four validators are paused at once, so the
/// verdict has to be the stall, and it has to arrive on the stall's own clock
/// rather than the ceiling's.
///
/// Mutation that bites: make `Progress::judge` ignore `last_move`. The verdict
/// becomes `Slow` at the ceiling, and the promptness assertion fails with it.
#[test]
fn the_wait_reports_a_stall_and_not_a_slow_box() {
    let home = scratch("malachite-recovery-stall");
    run_cli_env(&["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "4"], &base_port(27280));

    let live: Vec<usize> = (0..4).collect();
    let all = spawn_env(&home, &live, |_| Vec::new(), &[]);
    await_height(&home, &live, 3, Budget::within(120))
        .unwrap_or_else(|why| panic!("the cluster never reached height 3: {why}"));

    for slot in 0..4 {
        signal(&all, slot, "-STOP");
    }
    let quiet = Duration::from_secs(8);
    let asked = Instant::now();
    let verdict = await_height(&home, &live, 1_000, Budget { stall: quiet, ceiling: Duration::from_secs(120) });
    assert!(
        matches!(verdict, Err(Wait::Stalled { .. })),
        "four paused validators are a stall, and the wait must say so: {verdict:?}"
    );
    assert!(
        asked.elapsed() < quiet + Duration::from_secs(10),
        "a stall is reported on the stall's clock, not the ceiling's: {:?}",
        asked.elapsed()
    );

    // Resumed together, with every connection intact, they carry on.
    for slot in 0..4 {
        signal(&all, slot, "-CONT");
    }
    let top = heights(&home, &live).into_iter().max().unwrap_or(0);
    await_height(&home, &live, top + 3, Budget::within(120))
        .unwrap_or_else(|why| panic!("the cluster did not resume after the pause: {why}"));

    drop(all);
    let _ = std::fs::remove_dir_all(&home);
}

/// **A snapshot carries a certified state between homes**, which is the way
/// back for a node below every peer's history floor.
///
/// Value sync serves what its peers still hold. Past their
/// `--prune-margin-blocks` nobody holds it, and no restart helps: an operator
/// exports a state from a healthy node and imports it here.
///
/// What the import checks is walked in both directions — a bundle whose bytes
/// have been altered is refused, because the state's commitment is no longer
/// what the certified block above it claims; a home already past that height
/// is refused, because an import may never rewind a node; and the untouched
/// bundle installs into a home that holds nothing.
#[test]
fn a_snapshot_carries_a_certified_state_between_homes() {
    let home = scratch("malachite-recovery-snapshot");
    run_cli_env(&["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "4"], &base_port(27240));

    let live: Vec<usize> = (0..4).collect();
    // A snapshot every eight blocks rather than every 128: a probe about
    // snapshots cannot wait for the default. Harness-only, and documented as
    // such beside `EDET_CONSENSUS_BASE_PORT`.
    let tight = [("EDET_SNAPSHOT_INTERVAL", "8".to_string())];
    let cluster = spawn_env(&home, &live, |_| Vec::new(), &tight);
    await_height(&home, &live, 10, Budget::within(180))
        .unwrap_or_else(|why| panic!("the cluster never reached a height past its first snapshot: {why}"));

    // Everything stopped: an operator exports from a node that is not running,
    // and imports into one that is not running either.
    drop(cluster);
    std::thread::sleep(Duration::from_secs(1));

    let bundle = home.join("bundle.bin");
    run_cli(&[
        "malachite",
        "export-snapshot",
        "--home",
        home.join("0").to_str().unwrap(),
        "--out",
        bundle.to_str().unwrap(),
    ]);

    // The mutation: a bundle whose bytes have been altered must be refused, or
    // the import is a way to install any state at all.
    let corrupt = home.join("corrupt.bin");
    let mut bytes = std::fs::read(&bundle).expect("the exported bundle");
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    std::fs::write(&corrupt, &bytes).expect("write the corrupt bundle");
    let refused = Command::new(bin())
        .args([
            "malachite",
            "import-snapshot",
            "--home",
            home.join("3").to_str().unwrap(),
            "--from",
            corrupt.to_str().unwrap(),
        ])
        .output()
        .expect("run import-snapshot");
    assert!(!refused.status.success(), "an altered snapshot bundle must be refused");

    // A home already past the exported height is refused too.
    let ahead = Command::new(bin())
        .args([
            "malachite",
            "import-snapshot",
            "--home",
            home.join("3").to_str().unwrap(),
            "--from",
            bundle.to_str().unwrap(),
        ])
        .output()
        .expect("run import-snapshot");
    let said = String::from_utf8_lossy(&ahead.stderr).into_owned() + &String::from_utf8_lossy(&ahead.stdout);
    assert!(
        !ahead.status.success() && said.contains("rewind"),
        "importing into a home that is already ahead must be refused, and say why: {said}"
    );

    // Into a home that holds nothing, it installs.
    let fresh = home.join("fresh");
    std::fs::create_dir_all(fresh.join("config")).expect("fresh home");
    for f in ["config.toml", "genesis.json", "priv_validator_key.json"] {
        std::fs::copy(home.join("3/config").join(f), fresh.join("config").join(f)).expect("copy the home's config");
    }
    run_cli(&["malachite", "import-snapshot", "--home", fresh.to_str().unwrap(), "--from", bundle.to_str().unwrap()]);

    let store = edet_node::store::Store::open(fresh.join("edet-store")).expect("the imported store");
    let (height, state) = store.read_snapshot().expect("read").expect("a snapshot was installed");
    assert!(height >= 8, "the exported snapshot is at a multiple of the interval: {height}");
    edet_state::invariants::audit(&state).expect("and it is a legal state");

    let _ = std::fs::remove_dir_all(&home);
}
