//! What every engine test needs to drive real `edet-node` processes.
//!
//! One copy rather than one per file: these harnesses all start OS processes,
//! poll `status.json` and reap children on drop, and when each carried its own
//! `spawn` they drifted — a timeout raised in one, an env var threaded through
//! another, a `Drop` that reaped in one and leaked in the next. A leaked node
//! holds a fixed loopback consensus port, and the next test fails for a reason
//! that has nothing to do with the code.
//!
//! Nothing here asserts anything about the ledger. These are the mechanics; the
//! claims live in the test files.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The binary under test — the one cargo just built, never one on `PATH`.
pub fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_edet-node"))
}

/// A node's own window onto what it has committed, written by
/// `engine_malachite::write_status` after every real commit.
///
/// Nodes started with no `--client-port` serve no HTTP at all, so for those
/// this file is the only observation available.
#[derive(Debug, Clone)]
pub struct Status {
    pub height: u64,
    pub state_hash: String,
}

pub fn read_status(home: &Path, index: usize) -> Option<Status> {
    let raw = std::fs::read_to_string(home.join(index.to_string()).join("status.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(Status { height: v.get("height")?.as_u64()?, state_hash: v.get("state_hash")?.as_str()?.to_string() })
}

/// How long a wait may go on, in the two units a loaded box tells apart.
///
/// **A budget on wall clock alone goes red for two unrelated reasons**, and a
/// gate that does that teaches everybody to ignore it. A loaded box commits
/// slowly and still commits; a cluster that has lost quorum, or a validator
/// nobody dials any more, commits nothing at all. So a wait fails on `stall` —
/// no live node's height moved for that long, once any node had committed
/// anything — and separately on `ceiling`, the target unmet while progress
/// continued. The two are different sentences in the failure, because they
/// are different findings: the first is the code, the second is the box.
///
/// Forty-five seconds of nothing is a stall on any box this tree has run on:
/// an empty block is paced to one a second, a round at the tenth retry is
/// eight seconds, and a box slower by a factor of three is still under both.
/// Before the first commit only the ceiling applies, since a fresh cluster's
/// first block waits on process spawn, the genesis and the mesh forming.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub stall: Duration,
    pub ceiling: Duration,
}

impl Budget {
    pub const STALL: Duration = Duration::from_secs(45);

    /// The default stall rule under a ceiling of `secs`.
    pub fn within(secs: u64) -> Self {
        Budget { stall: Self::STALL, ceiling: Duration::from_secs(secs) }
    }
}

impl Default for Budget {
    fn default() -> Self {
        Budget::within(600)
    }
}

/// Why a wait ended before its condition held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wait {
    /// No node had committed anything when the ceiling ran out.
    NeverStarted { after: Duration },
    /// Some node had committed, and then no node's height moved for `quiet`.
    Stalled { heights: Vec<u64>, quiet: Duration },
    /// Heights kept moving and the target was still unmet at the ceiling.
    Slow { heights: Vec<u64>, after: Duration },
}

impl std::fmt::Display for Wait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Wait::NeverStarted { after } => write!(f, "never started: no node committed anything in {after:?}"),
            Wait::Stalled { heights, quiet } => {
                write!(f, "stalled at heights {heights:?}: nothing advanced in {quiet:?}")
            }
            Wait::Slow { heights, after } => {
                write!(f, "slow: at heights {heights:?} after {after:?}, still advancing")
            }
        }
    }
}

/// The progress rule, shared by every wait below.
struct Progress {
    start: Instant,
    best: Vec<u64>,
    last_move: Option<Instant>,
}

impl Progress {
    fn new(n: usize) -> Self {
        Progress { start: Instant::now(), best: vec![0; n], last_move: None }
    }

    /// Record this poll's heights and answer whether the wait must end.
    fn judge(&mut self, heights: &[u64], budget: Budget) -> Option<Wait> {
        let now = Instant::now();
        for (best, &h) in self.best.iter_mut().zip(heights) {
            if h > *best {
                *best = h;
                self.last_move = Some(now);
            }
        }
        match self.last_move {
            None if now.duration_since(self.start) > budget.ceiling => {
                Some(Wait::NeverStarted { after: now.duration_since(self.start) })
            }
            None => None,
            Some(moved) if now.duration_since(moved) > budget.stall => {
                Some(Wait::Stalled { heights: self.best.clone(), quiet: now.duration_since(moved) })
            }
            Some(_) if now.duration_since(self.start) > budget.ceiling => {
                Some(Wait::Slow { heights: self.best.clone(), after: now.duration_since(self.start) })
            }
            Some(_) => None,
        }
    }
}

/// Poll every named node's status file until each has committed at least
/// `height` with a non-empty hash. Returns each node's status in `live`'s
/// order, or why it stopped waiting.
pub fn await_height(home: &Path, live: &[usize], height: u64, budget: Budget) -> Result<Vec<Status>, Wait> {
    let mut progress = Progress::new(live.len());
    loop {
        let statuses: Vec<Option<Status>> = live.iter().map(|&i| read_status(home, i)).collect();
        if statuses
            .iter()
            .all(|s| matches!(s, Some(st) if st.height >= height && !st.state_hash.is_empty()))
        {
            return Ok(statuses.into_iter().map(|s| s.unwrap()).collect());
        }
        let heights: Vec<u64> = statuses.iter().map(|s| s.as_ref().map(|st| st.height).unwrap_or(0)).collect();
        if let Some(why) = progress.judge(&heights, budget) {
            return Err(why);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The committed height of every named node, zero for one that has written
/// nothing yet — what a failure message prints.
pub fn heights(home: &Path, live: &[usize]) -> Vec<u64> {
    live.iter()
        .map(|&i| read_status(home, i).map(|s| s.height).unwrap_or(0))
        .collect()
}

/// Every `(height, state_hash)` pair each node was seen at, indexed the way
/// `live` was given.
///
/// **A state hash is only comparable AT A HEIGHT.** Two nodes polled a moment
/// apart are legitimately at different heights, and every block changes the
/// root — the leaf salt binds the block marker, by design, so that two proofs
/// at two heights cannot reveal that a neighbour stayed the same. Comparing
/// the hashes off two status files as they happen to be read therefore fails
/// on healthy nodes and says "diverged": measured, node 3 at height 4 against
/// node 0 at height 5 on a cluster that agreed at every height either of them
/// had reached.
pub struct Seen(pub Vec<std::collections::BTreeMap<u64, String>>);

impl Seen {
    /// The first height at which two nodes reported different hashes — a
    /// consensus fault, and the only thing these files can prove.
    pub fn disagreement(&self) -> Option<String> {
        let mut by_height: std::collections::BTreeMap<u64, (usize, &str)> = Default::default();
        for (node, seen) in self.0.iter().enumerate() {
            for (h, hash) in seen {
                match by_height.get(h) {
                    Some((other, first)) if first != hash => {
                        return Some(format!(
                            "height {h}: node {other} committed {first}, node {node} committed {hash}"
                        ));
                    }
                    Some(_) => {}
                    None => {
                        by_height.insert(*h, (node, hash));
                    }
                }
            }
        }
        None
    }

    /// Heights every node reported. An empty list means nothing was actually
    /// compared, whatever `disagreement` says.
    pub fn common_heights(&self) -> Vec<u64> {
        let Some(first) = self.0.first() else { return Vec::new() };
        first
            .keys()
            .copied()
            .filter(|h| self.0.iter().all(|s| s.contains_key(h)))
            .collect()
    }

    /// The highest height each node reached.
    pub fn tops(&self) -> Vec<u64> {
        self.0.iter().map(|s| s.keys().next_back().copied().unwrap_or(0)).collect()
    }
}

/// Watch every named node until they have been observed at `min_common`
/// heights IN COMMON, recording every pair each passes through.
///
/// The condition is common heights rather than "each reached height N",
/// because a node that starts late or falls behind is caught up by value sync
/// and the others do not wait for it: measured on four healthy loopback
/// validators, one sat at height 3 reporting `SYNC REQUIRED` while the others
/// were at 7, and the two windows did not overlap at all. A comparison with no
/// shared height compares nothing, so that is the thing to wait for.
///
/// **`min_common` is how many heights are COMPARED, not how many the cluster
/// must reach.** Asking for several turns an agreement test into a liveness
/// test over several blocks, and that is a different claim with a different
/// failure: measured under `just ci` on a loaded box, three healthy nodes
/// committed heights 1 and 2 and then spent ninety seconds at height 3 with
/// every round prevoting Nil, agreeing perfectly throughout. Liveness over
/// many blocks belongs to a probe that says so and budgets for it.
pub fn observe(home: &Path, live: &[usize], min_common: usize, budget: Budget) -> Result<Seen, Wait> {
    let mut seen = Seen(vec![Default::default(); live.len()]);
    let mut progress = Progress::new(live.len());
    loop {
        for (slot, &i) in live.iter().enumerate() {
            if let Some(st) = read_status(home, i) {
                if !st.state_hash.is_empty() {
                    seen.0[slot].insert(st.height, st.state_hash);
                }
            }
        }
        if seen.common_heights().len() >= min_common || seen.disagreement().is_some() {
            return Ok(seen);
        }
        if let Some(why) = progress.judge(&seen.tops(), budget) {
            return Err(why);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Owns the child processes for the duration of a test — killed and reaped on
/// drop, so a failing assertion never leaves a node holding a fixed loopback
/// consensus port into the next test.
pub struct Cluster(pub Vec<Child>);

impl Drop for Cluster {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Cluster {
    /// The OS pid of the child at `slot`, for a test that pauses one.
    pub fn pid(&self, slot: usize) -> u32 {
        self.0[slot].id()
    }
}

/// Start one `edet-node malachite --home H --index i` process per entry of
/// `live`, with `extra(i)` appended to each command line.
///
/// Each child's output lands in `home/<i>/node.log` rather than `/dev/null`.
/// A node that stalls says why — it is the engine's only voice — and a harness
/// that discards it leaves "one node stopped at height 1" as the whole of the
/// evidence. `node_log` reads one back.
pub fn spawn_with(home: &Path, live: &[usize], extra: impl Fn(usize) -> Vec<String>) -> Cluster {
    spawn_env(home, live, extra, &[])
}

/// The same, with environment for the children — the harness-only overrides
/// (`EDET_SNAPSHOT_INTERVAL`, `EDET_PRUNE_MARGIN_BLOCKS`) a probe about the
/// history floor needs, passed per child rather than set process-wide, since
/// cargo runs tests in one binary in parallel.
pub fn spawn_env(home: &Path, live: &[usize], extra: impl Fn(usize) -> Vec<String>, env: &[(&str, String)]) -> Cluster {
    let children = live
        .iter()
        .map(|&i| {
            let dir = home.join(i.to_string());
            std::fs::create_dir_all(&dir).expect("node home");
            let log = std::fs::File::options()
                .create(true)
                .append(true)
                .open(dir.join("node.log"))
                .expect("node log");
            let errs = log.try_clone().expect("node log");
            let mut cmd = Command::new(bin());
            cmd.args(["malachite", "--home", home.to_str().unwrap(), "--index", &i.to_string()])
                .args(extra(i))
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(errs));
            for (k, v) in env {
                cmd.env(k, v);
            }
            cmd.spawn().expect("spawn `edet-node malachite` child process")
        })
        .collect();
    Cluster(children)
}

/// Run one `edet-node` subcommand with extra environment.
pub fn run_cli_env(args: &[&str], env: &[(&str, String)]) {
    let mut cmd = Command::new(bin());
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = cmd.status().expect("run edet-node CLI helper");
    assert!(status.success(), "`edet-node {}` failed", args.join(" "));
}

/// One node's log, for a probe that asserts what it SAID as well as what it
/// committed.
pub fn node_log(home: &Path, index: usize) -> String {
    std::fs::read_to_string(home.join(index.to_string()).join("node.log")).unwrap_or_default()
}

/// `spawn_with` and no extra arguments: consensus only, observable through
/// `status.json` alone.
pub fn spawn(home: &Path, live: &[usize]) -> Cluster {
    spawn_with(home, live, |_| Vec::new())
}

/// Run one `edet-node` subcommand to completion, asserting it succeeded.
pub fn run_cli(args: &[&str]) {
    let status = Command::new(bin()).args(args).status().expect("run edet-node CLI helper");
    assert!(status.success(), "`edet-node {}` failed", args.join(" "));
}

/// The same, returning stdout — for the subcommands whose OUTPUT is the point
/// (`keygen` prints the public half the genesis author needs).
pub fn run_cli_out(args: &[&str]) -> String {
    let out = Command::new(bin()).args(args).output().expect("run edet-node CLI helper");
    assert!(out.status.success(), "`edet-node {}` failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("edet-node prints UTF-8")
}

/// A fresh directory, removed if a previous run left one behind.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("edet-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// ---------------------------------------------------------- the HTTP waits --
//
// Behind `serve`, because that is the feature that carries `reqwest`: a test
// binary built without it includes this module too, and a helper naming an
// optional dependency would stop the whole crate compiling.

/// Block until every port answers `GET /health`, or fail.
///
/// The client API is bound before the engine's own startup completes
/// (`EdetApp::start`), so this only waits out process spawn and the TCP bind.
#[cfg(feature = "serve")]
pub async fn await_health(client: &reqwest::Client, ports: &[u16], timeout: Duration) {
    let start = Instant::now();
    loop {
        let mut all = true;
        for &p in ports {
            let url = format!("http://127.0.0.1:{p}/health");
            all &= matches!(client.get(url).send().await, Ok(r) if r.status().is_success());
        }
        if all {
            return;
        }
        assert!(start.elapsed() <= timeout, "client API on {ports:?} never came up within {timeout:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// **Wait for the chain's epoch to reach wall-clock before computing a
/// window.**
///
/// A fresh chain climbs `MAX_EPOCH_ADVANCE_PER_BLOCK` epochs per block until
/// its clock catches up, so an expiry computed before that is already in the
/// past when the proposer includes the transaction — and the failure arrives
/// as `ET-TX-002` on a transaction that was about something else entirely.
/// A commit does not prove the catch-up is over: a submitter that RETRIES past
/// a stale window succeeds while the epoch is still climbing.
#[cfg(feature = "serve")]
pub async fn await_epoch_settled(client: &reqwest::Client, port: u16, timeout: Duration) -> u64 {
    let want = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall clock after the unix epoch")
        .as_secs()
        / edet_kernel::constants::EPOCH_SECS;
    let start = Instant::now();
    loop {
        let url = format!("http://127.0.0.1:{port}/network");
        let v: serde_json::Value = client
            .get(url)
            .send()
            .await
            .expect("GET /network")
            .json()
            .await
            .expect("GET /network returned non-JSON");
        let epoch = v["epoch"].as_u64().unwrap_or(0);
        if epoch >= want {
            return epoch;
        }
        assert!(start.elapsed() <= timeout, "the chain's epoch never reached wall-clock ({epoch} < {want})");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
