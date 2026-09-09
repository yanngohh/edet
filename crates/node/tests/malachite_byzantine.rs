//! A REAL misbehaving proposer against a real cluster: node 0 runs with
//! `--allow-unsigned`, so its ingress accepts a transaction whose signers are
//! claimed but never signed, and it proposes that forged transaction to its
//! honest peer over live consensus.
//!
//! This is the end-to-end form of the pre-vote authentication screen
//! (`engine_malachite::screen`). Three claims, in increasing order of what
//! they cost to get wrong:
//!
//! - **Soundness** — the forged transaction never commits, on either node.
//!   Already true before the screen existed (`Replica::commit_block` refuses
//!   it), so this half is a regression guard, not the news.
//! - **Liveness** — the cluster keeps committing heights. Also true without
//!   the screen, as it happens: a decided-but-uncommittable value makes every
//!   node answer `restart`, the round changes, and an honest proposer's block
//!   gets through. Slower, not stuck.
//! - **One decided value per height** — THIS is what the screen buys, and the
//!   only one of the three that fails without it. A validator that votes
//!   `Valid` on a block it will later refuse to commit signs a quorum
//!   certificate for a value that never becomes the ledger. Measured on this
//!   exact scenario, the unscreened engine decided two separate heights TWICE,
//!   with different values each time: two conflicting, fully-valid 2/3+
//!   certificates per height, signed by honest validators, neither matching
//!   what they committed. Anything trusting certificates rather than replaying
//!   the chain — `verify_decided`, the `GetDecidedValue` sync path, a light
//!   client — can be handed one of those.
//!
//! The third assertion reads the engine's own `Decided` log lines, so it is
//! only as good as that log format. It therefore fails loudly if it cannot
//! parse a plausible number of them, rather than passing vacuously on a
//! format change. It is also probabilistic in the failure direction: correct
//! code satisfies it deterministically (a screened value is never decided at
//! all), while broken code violates it only on the rounds the misbehaving
//! proposer actually wins — reliably within this window, but not on every
//! single height.
//!
//! Two equal-power validators, so quorum is 2: node 1 alone cannot commit
//! anything, which means every height observed here is one the honest node
//! signed. Node 0 keeps its forged transaction in its mempool forever (it can
//! never commit), so it re-proposes it on every turn.
//!
//!   cargo test -p edet-node --features malachite --test malachite_byzantine \
//!     -- --nocapture
#![cfg(feature = "malachite")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use edet_node::block::{dev_seed, hex32, pubkey_of, SignedTx};
use edet_state::tx::Tx;
use edet_state::types::Party;

/// Distinct from every other test/harness port range in this crate.
/// Distinct from every other engine test's base — see `write the testnet` below.
const CONSENSUS_BASE: usize = 26800;

const PORTS: [u16; 2] = [7431, 7432];

/// How far the cluster must get before the run is judged. Enough heights that
/// the misbehaving node has had several proposer turns.
const HEIGHTS: u64 = 6;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_edet-node"))
}

fn url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

struct Nodes(Vec<Child>);

impl Drop for Nodes {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

async fn get_json(client: &reqwest::Client, port: u16, path: &str) -> serde_json::Value {
    client
        .get(url(port, path))
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {path} on :{port}: {e}"))
        .json()
        .await
        .unwrap_or_else(|e| panic!("GET {path} on :{port} returned non-JSON: {e}"))
}

async fn await_health(client: &reqwest::Client, ports: &[u16], timeout: Duration) {
    let start = Instant::now();
    loop {
        let mut all = true;
        for &p in ports {
            all &= matches!(client.get(url(p, "/health")).send().await, Ok(r) if r.status().is_success());
        }
        if all {
            return;
        }
        assert!(start.elapsed() <= timeout, "client API on {ports:?} never came up");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn height(client: &reqwest::Client, port: u16) -> u64 {
    get_json(client, port, "/network")
        .await
        .get("height")
        .and_then(|h| h.as_u64())
        .unwrap_or(0)
}

/// Poll until `port` reports a committed height of at least `target`.
async fn await_height(client: &reqwest::Client, port: u16, target: u64, timeout: Duration) -> Option<u64> {
    let start = Instant::now();
    loop {
        let h = height(client, port).await;
        if h >= target {
            return Some(h);
        }
        if start.elapsed() > timeout {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Every distinct value the engine logged as `Decided`, per height, from one
/// node's own log. The engine prints
/// `Decided round=<r> height=<h> value=<hex>` at INFO
/// (`malachitebft_test_cli::logging`, which `main.rs` initialises).
fn decided_values_by_height(log: &Path) -> BTreeMap<u64, BTreeSet<String>> {
    let raw = std::fs::read_to_string(log).unwrap_or_default();
    let mut out: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
    for line in raw.lines() {
        let Some(rest) = line.split("Decided ").nth(1) else { continue };
        let field =
            |name: &str| -> Option<&str> { rest.split(name).nth(1).map(|v| v.split_whitespace().next().unwrap_or("")) };
        let (Some(h), Some(v)) = (field("height="), field("value=")) else { continue };
        let Ok(h) = h.parse::<u64>() else { continue };
        out.entry(h).or_default().insert(v.to_string());
    }
    out
}

#[tokio::test]
async fn a_forged_proposal_neither_commits_nor_gets_a_certificate() {
    let home = std::env::temp_dir().join(format!("edet-malachite-byz-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    // Its own consensus base, so this binary can never wait on the previous
    // one's nodes releasing theirs. `just engine-test` runs the four engine
    // binaries back to back, and `malachite_cluster` generates a testnet from
    // the same `write_testnet` default — a slow reap there surfaces here as a
    // cluster that simply never reaches quorum, with nothing to say why. The
    // base is read when the testnet is GENERATED, since it is written into
    // each node's config.toml along with its peers' addresses.
    let status = Command::new(bin())
        .args(["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "2"])
        .env("EDET_CONSENSUS_BASE_PORT", CONSENSUS_BASE.to_string())
        .status()
        .expect("write the testnet");
    assert!(status.success());

    let peers = PORTS
        .iter()
        .map(|p| format!("http://127.0.0.1:{p}"))
        .collect::<Vec<_>>()
        .join(",");
    let honest_log = home.join("honest.log");
    let children = (0..2usize)
        .map(|i| {
            let mut cmd = Command::new(bin());
            cmd.args(["malachite", "--home", home.join(i.to_string()).to_str().unwrap()]);
            cmd.args(["--client-port", &PORTS[i].to_string(), "--client-peers", &peers]);
            if i == 0 {
                // Node 0 only: its ingress waves the forgery below through,
                // and it then proposes it. Node 1 is honest and is the node
                // whose behaviour is actually under test.
                cmd.arg("--allow-unsigned");
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            } else {
                let f = std::fs::File::create(&honest_log).expect("create the honest node's log");
                let f2 = f.try_clone().expect("clone the log handle");
                cmd.stdout(Stdio::from(f)).stderr(Stdio::from(f2));
            }
            cmd.spawn().expect("spawn node")
        })
        .collect();
    let _nodes = Nodes(children);

    let client = reqwest::Client::new();
    await_health(&client, &PORTS, Duration::from_secs(30)).await;

    // A forgery: real member ids, real public keys claimed as signers, and no
    // signature behind either of them.
    let forged = SignedTx {
        tx: Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        },
        nonce: edet_node::block::counter_nonce(0),
        not_after_epoch: edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS,
        signers: vec![pubkey_of(&dev_seed(0)), pubkey_of(&dev_seed(1))],
        signatures: vec![],
    };
    let forged_hash = hex32(&forged.hash().expect("hash"));

    // The honest node must refuse it outright at the ingress...
    let honest: serde_json::Value = client
        .post(url(PORTS[1], "/tx"))
        .json(&forged)
        .send()
        .await
        .expect("POST /tx to node 1")
        .json()
        .await
        .expect("JSON");
    assert_eq!(
        honest.get("queued").and_then(|q| q.as_bool()),
        Some(false),
        "an honest node's ingress must reject an unsigned transaction",
    );

    // ...while node 0, running the harness hatch, accepts and will propose it.
    let byzantine: serde_json::Value = client
        .post(url(PORTS[0], "/tx"))
        .json(&forged)
        .send()
        .await
        .expect("POST /tx to node 0")
        .json()
        .await
        .expect("JSON");
    assert_eq!(
        byzantine.get("queued").and_then(|q| q.as_bool()),
        Some(true),
        "--allow-unsigned must actually accept it, or this test proves nothing",
    );

    // LIVENESS: the cluster keeps deciding heights despite node 0 spending its
    // proposer turns on a block nobody can commit. Quorum is 2, so every
    // height here carries the honest node's signature.
    // Generous on purpose. Half the heights here belong to the misbehaving
    // proposer and only advance once the round changes, so wall-clock cost is
    // dominated by real consensus timeouts rather than by work — and on a
    // loaded machine this timed out at 120s while passing in ~50s when idle.
    // A liveness regression still fails, just later; a flaky gate that cries
    // wolf on a busy CI box is worse than a slow one, because the response to
    // it is to stop believing the gate.
    let reached = await_height(&client, PORTS[1], HEIGHTS, Duration::from_secs(300))
        .await
        .expect("the cluster made no progress at all under a misbehaving proposer");

    // SOUNDNESS: the forgery is nowhere in the ledger, on either node.
    for (i, &port) in PORTS.iter().enumerate() {
        let outcome = get_json(&client, port, &format!("/tx/outcome/{forged_hash}")).await;
        assert_ne!(
            outcome.get("status").and_then(|s| s.as_str()),
            Some("ok"),
            "node {i} committed a transaction nobody signed: {outcome}",
        );
        let member = get_json(&client, port, "/member/0").await;
        assert_eq!(
            member.get("debt").and_then(|d| d.as_f64()),
            Some(0.0),
            "node {i} booked an obligation from the forged Accept: {member}",
        );
    }

    // NO CONFLICTING CERTIFICATES: the honest node must never have signed off
    // on two different values for one height. This is the assertion the screen
    // exists for — see this file's header.
    let decided = decided_values_by_height(&honest_log);
    assert!(
        decided.len() >= 3,
        "parsed only {} Decided heights from the honest node's log — the engine's log format has probably changed \
         and this assertion has stopped testing anything; fix the parser rather than deleting the check",
        decided.len(),
    );
    let conflicting: Vec<_> = decided.iter().filter(|(_, vs)| vs.len() > 1).collect();
    assert!(
        conflicting.is_empty(),
        "the honest validator decided {} height(s) more than once, with different values each time — it signed \
         quorum certificates for blocks it then refused to commit: {conflicting:?}",
        conflicting.len(),
    );

    eprintln!(
        "cluster reached height {reached} under a forging proposer; {} heights decided, none twice, nothing booked",
        decided.len(),
    );

    drop(_nodes);
    let _ = std::fs::remove_dir_all(&home);
}
