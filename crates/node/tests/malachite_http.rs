//! The browser-UI path on a real BFT node: an `edet-node malachite` process
//! started with `--client-port` serves the client HTTP API
//! (`serve::client_router`) against state the Malachite engine decides.
//!
//! Real OS processes over loopback, like `malachite_cluster.rs`, and for the
//! same reason: nothing here is driven in-process, so the client API, the
//! engine, the wire codec and the HTTP surface are all exercised exactly as a
//! deployment would. Everything a client can claim about COMMITTED state
//! belongs here, because this is the only harness in the crate that commits
//! anything — `serve::tests` keeps the half that needs no consensus.
//!
//! Five tests, five distinct claims:
//!
//! 1. `a_browser_client_reads_and_submits_over_http_against_a_malachite_node`
//!    (solo validator) — the end-to-end UI claim: `GET /health`, `POST /tx`
//!    with a real Ed25519-signed transaction, then the committed outcome and
//!    the resulting state read back over HTTP. Plus the two router-shape
//!    assertions that keep `serve::http`'s surface honest: `/p2p/tx` IS
//!    served (the mempool-gossip ingress) and `/p2p/propose` is NOT. That
//!    second one is a tripwire on a mistake this crate must not make again —
//!    a build that could
//!    carry a dev consensus whose propose/vote gossip rode the same HTTP
//!    surface, and a node serving both would have had two proposers at one
//!    height, exactly the disagreement BFT exists to prevent.
//!
//! 2. `a_transaction_submitted_to_one_node_is_committed_and_agreed_by_both`
//!    (2 validators, quorum 2 — so BOTH must sign every commit) — the
//!    cross-node claim: a tx enters through ONE node's HTTP API and ends up
//!    committed, with both nodes agreeing on the resulting state hash. Note
//!    what this does and does not pin down: with 2 equal-power validators
//!    either eventually gets a proposer turn, so this asserts the OUTCOME
//!    (committed everywhere, agreed) and NOT that gossip carried it — it
//!    passes with `--client-peers` omitted too (checked). Test 3 is the one
//!    that pins the hop down.
//!
//! 3. `a_submission_is_gossiped_to_every_configured_client_peer` — the
//!    mempool-gossip claim, deterministically: the node's peer list points at
//!    a plain TCP socket THIS TEST owns, so a `POST /tx` must show up there as
//!    a `POST /p2p/tx` carrying that exact transaction, with consensus unable
//!    to have delivered it by any other route. Without this hop a transaction
//!    could only ever be proposed by the one node it was submitted to —
//!    Malachite gossips consensus messages, never the mempool.
//!
//! 4. `a_co_signed_multi_party_proposal_commits` — the pending-signature
//!    pool's far end. Two parties co-sign a proposal through the client API
//!    and the assembled transaction is carried into a real committed block.
//!    `serve::tests` covers the pool's own rules (gossip between nodes,
//!    thresholds, decline tombstones) without consensus; this is the part
//!    that needs a chain.
//!
//! 5. `a_commit_time_rejection_is_reported_at_tx_outcome_with_its_et_code` —
//!    the outcome claim, on the engine: a transaction can pass the ingress and
//!    still be refused by `apply` inside the block that commits it, which is
//!    silent to the submitter unless `/tx/outcome/:hash` says so.
//!
//! Gated behind `--features malachite` (slow git deps; experimental engine),
//! so a default `cargo test --workspace` never runs it:
//!
//!   cargo test -p edet-node --features malachite --test malachite_http \
//!     -- --nocapture --test-threads 1
//!
//! The tests are safe to run in parallel with each other (each owns a distinct
//! consensus port and client port), but NOT alongside another live cluster on
//! this machine: consensus ports are fixed per node home, not negotiated
//! (`engine_node::make_config`, `write_solo_home_on`).
#![cfg(feature = "malachite")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use edet_node::block::{dev_seed, sign_tx, SignedTx};

mod common;
use common::{await_epoch_settled, await_health};
use edet_state::tx::Tx;
use edet_state::types::Party;

/// Client-API ports, deliberately outside the local dev cluster's 7301..
/// range so a developer running `just dev` alongside this test never
/// collides.
const SOLO_PORT: u16 = 7401;
const CLUSTER_PORTS: [u16; 2] = [7411, 7412];
const GOSSIP_PORT: u16 = 7421;
/// 7431 is `malachite_byzantine`'s, so these start above it. Client ports
/// have to be distinct across BINARIES too, not just within one: the four
/// engine binaries run back to back, and a port still held by the previous
/// one's node fails the next in a way that reads like a consensus fault.
const PENDING_PORT: u16 = 7451;
const OUTCOME_PORT: u16 = 7461;

/// Consensus (libp2p) ports for the solo homes below — distinct from each
/// other and from `write_solo_home`'s default, so these tests may run in
/// parallel (see this file's header).
const SOLO_CONSENSUS_PORT: usize = 28911;
const GOSSIP_CONSENSUS_PORT: usize = 28912;
const PENDING_CONSENSUS_PORT: usize = 28913;
const OUTCOME_CONSENSUS_PORT: usize = 28914;

/// The cluster test below generates a testnet, and so does
/// `malachite_cluster.rs` — from the same `write_testnet`, therefore from the
/// same default consensus base. `just engine-test` runs those two binaries
/// back to back, so whether this one can bind depended on how quickly the
/// previous binary's node processes were reaped: a race that surfaces as this
/// test timing out with no explanation, since a node that cannot bind is
/// simply a cluster that never reaches quorum.
///
/// Moving this testnet's base out of the way makes the two independent. Set
/// when GENERATING the testnet, not when running it: the port is written into
/// each node's `config.toml` along with its peers' addresses.
const CLUSTER_CONSENSUS_BASE: usize = 26700;

/// Liveness budget for anything that waits on a real commit here.
///
/// Generous on purpose. These tests run in parallel with each other — five
/// of them now, each driving its own engine node, after the pending-pool and
/// commit-outcome tests joined the file — so a loaded machine multiplies
/// every consensus round by whatever else is running. Measured: the 2-node
/// cluster test takes ~27 s alone and exceeded a 90 s budget with the other
/// four alongside it, on the same commit that passes comfortably in
/// isolation.
///
/// A timeout here says "consensus did not make progress", so it must not
/// fire for "the machine was busy" — the same lesson `malachite_byzantine`'s
/// bound already carries (120 s -> 300 s): a gate that cries wolf gets
/// ignored, and the failure it cries about is one nobody can distinguish
/// from a real stall without re-running it.
const COMMIT_BUDGET: Duration = Duration::from_secs(240);

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_edet-node"))
}

fn url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

/// Owns the child processes for the test's duration — killed and reaped on
/// drop, so a failing assertion never leaks a node holding the fixed loopback
/// ports (same reason as `malachite_cluster.rs`'s `Cluster`).
struct Nodes(Vec<Child>);

impl Drop for Nodes {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Spawn `edet-node malachite` for each index, each serving its client API on
/// `ports[i]`. `homes[i]` is that node's own home directory (the solo layout
/// puts config directly under the home; a testnet puts it under `home/<i>`);
/// `peers` is the slot-indexed tx-gossip list every node is given (empty for
/// no gossip at all).
fn spawn(homes: &[PathBuf], ports: &[u16], indices: &[usize], peers: &[String]) -> Nodes {
    let joined = peers.join(",");
    let children = indices
        .iter()
        .map(|&i| {
            let mut cmd = Command::new(bin());
            cmd.args(["malachite", "--home", homes[i].to_str().unwrap()]);
            cmd.args(["--client-port", &ports[i].to_string()]);
            if !peers.is_empty() {
                cmd.args(["--client-peers", &joined]);
            }
            cmd.stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn `edet-node malachite` child process")
        })
        .collect();
    Nodes(children)
}

/// Every node's client-API base URL, in slot order.
fn peer_urls(ports: &[u16]) -> Vec<String> {
    ports.iter().map(|p| format!("http://127.0.0.1:{p}")).collect()
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

/// Poll `GET /tx/outcome/<hash>` on `port` until the node reports a recorded
/// COMMIT outcome (`ok`/`rejected` — `pending` is still only mempool), and
/// return that status. `None` on timeout.
async fn await_committed(client: &reqwest::Client, port: u16, hash: &str, timeout: Duration) -> Option<String> {
    let start = Instant::now();
    loop {
        let v = get_json(client, port, &format!("/tx/outcome/{hash}")).await;
        match v.get("status").and_then(|s| s.as_str()) {
            Some("ok") => return Some("ok".into()),
            Some("rejected") => {
                return Some(format!("rejected: {}", v.get("code").map(|c| c.to_string()).unwrap_or_default()))
            }
            _ => {}
        }
        if start.elapsed() > timeout {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A fresh nonce per call.
fn next_accept_nonce() -> [u8; 16] {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    edet_node::block::counter_nonce(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// A real Ed25519-signed trial-sized Accept between two dev founders — the
/// same `dev_seed`/`sign_tx` convention every other harness in this crate
/// uses, so the node's verified ingress (`serve::submit`) accepts it.
/// `not_after_epoch` is the caller's to pick — see
/// `submit_accept_and_await_commit`'s doc comment for why a wall-clock
/// PREDICTION of it is unsafe on this node.
fn signed_accept(debtor: u64, creditor: u64, amount: f64, not_after_epoch: u64) -> SignedTx {
    let tx = Tx::Accept {
        debtor: Party::Member(debtor),
        creditor: Party::Member(creditor),
        amount,
        maturity_epochs: 30,
        arb: None,
    };
    sign_tx(
        edet_node::block::DEV_CHAIN_ID,
        tx,
        next_accept_nonce(),
        not_after_epoch,
        &[dev_seed(debtor as u8), dev_seed(creditor as u8)],
    )
    .expect("sign the Accept")
}

/// (see `malachite_app.rs`'s identical comment): this
/// node's genesis starts at economic epoch 0 but runs on real wall-clock
/// time, so its first committed block(s) must close the gap to "today",
/// capped at `MAX_EPOCH_ADVANCE_PER_BLOCK` per block — a window computed
/// from wall-clock time BEFORE that catch-up finishes can be stale by the
/// time the tx actually lands (`ET_TX_WINDOW_TOO_LONG`). This reads the
/// node's OWN reported epoch (`/network`) fresh before every attempt and
/// resubmits with a new nonce on rejection, converging once the node's
/// epoch stabilizes — the "re-sign and retry" behaviour `edet_state::apply`'s
/// doc comment prescribes for a legitimately-failed-then-retried
/// transaction. Returns the terminal status string (`"ok"` or
/// `"rejected: ET-..."`, matching `await_committed`) or `"timeout"`,
/// together with the hex hash of whichever attempt that status belongs to
/// (nonces differ across retries, so a caller that also wants to check a
/// DIFFERENT node for the same commit needs the hash of the one that
/// actually succeeded, not the first attempt's).
async fn submit_accept_and_await_commit(
    client: &reqwest::Client,
    port: u16,
    debtor: u64,
    creditor: u64,
    amount: f64,
    timeout: Duration,
) -> (String, String) {
    let start = Instant::now();
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let epoch = get_json(client, port, "/network").await["epoch"].as_u64().unwrap_or(0);
        let not_after_epoch = epoch + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS;
        let stx = signed_accept(debtor, creditor, amount, not_after_epoch);
        let hash = edet_node::block::hex32(&stx.hash().expect("tx hash"));
        let queued: serde_json::Value = client
            .post(url(port, "/tx"))
            .json(&stx)
            .send()
            .await
            .expect("POST /tx")
            .json()
            .await
            .expect("POST /tx returned non-JSON");
        // Retrying is not free, and what it spends is the submitter's own
        // operation-bond allowance: every attempt reserves a bond, `A` of
        // them are free per epoch, and a reservation is held for
        // `bond_release_epochs`. Past that the ingress SHOULD refuse — that
        // is the bond working — so a refused RETRY is not an ingress defect
        // and must not be reported as one. It says the transaction never
        // committed and this loop ran out of allowance trying, which is the
        // real failure and the one worth printing.
        //
        // (Read as an ingress defect once already: ~30 five-second attempts
        // on a loaded machine walked straight through the free allowance,
        // and the test then failed on `queued: false` — a symptom, three
        // steps from its cause. Hence a per-attempt wait long enough that
        // the whole budget is a handful of attempts, well under `A`.)
        let queued = queued.get("queued").and_then(|q| q.as_bool());
        if queued != Some(true) {
            assert!(attempt > 1, "the signed Accept must pass the ingress on the first attempt (got {queued:?})");
            return (format!("ingress refused retry {attempt} — allowance spent, and it never committed"), hash);
        }

        let remaining = timeout.saturating_sub(start.elapsed());
        let per_attempt = Duration::from_secs(30).min(remaining);
        match await_committed(client, port, &hash, per_attempt).await {
            Some(status) if status == "ok" => return (status, hash),
            _ if start.elapsed() >= timeout => return ("timeout".to_string(), hash),
            _ => {} // rejected (stale window) or this attempt's budget ran out — retry with a fresh one
        }
    }
}

fn run_cli(args: &[&str], env: &[(&str, String)]) {
    let mut cmd = Command::new(bin());
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = cmd.status().expect("run edet-node CLI helper");
    assert!(status.success(), "`edet-node {}` failed", args.join(" "));
}

/// The UI claim: a browser client reads and writes over HTTP against a node
/// whose blocks the Malachite engine decides.
#[tokio::test]
async fn a_browser_client_reads_and_submits_over_http_against_a_malachite_node() {
    let home = std::env::temp_dir().join(format!("edet-malachite-http-solo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    // 1 sole validator (reaches quorum alone) + 1 extra member, so an
    // Accept(0 -> 1) has two real member ids as parties.
    edet_node::engine_node::write_solo_home_on(&home, 1, SOLO_CONSENSUS_PORT).expect("write the solo node home");

    // No `--client-peers`: a solo validator has nowhere to gossip to, which
    // also keeps this test's claims about the router shape uncontaminated.
    let _nodes = spawn(std::slice::from_ref(&home), &[SOLO_PORT], &[0], &[]);
    let client = reqwest::Client::new();
    await_health(&client, &[SOLO_PORT], Duration::from_secs(30)).await;

    // --- submit over HTTP, exactly as the UI does ---------------------------
    let (status, _hash) = submit_accept_and_await_commit(&client, SOLO_PORT, 0, 1, 40.0, COMMIT_BUDGET).await;
    assert_eq!(status, "ok", "the submitted Accept must commit cleanly");

    // The committed effect is readable over the same HTTP API. An anonymous
    // read gets the pow2-bucketed amount (authenticated reads §Standing), not the
    // exact 40 — so the assertion is on the booked obligation existing.
    let member = get_json(&client, SOLO_PORT, "/member/0").await;
    let debt = member.get("debt").and_then(|d| d.as_f64()).unwrap_or(0.0);
    assert!(debt > 0.0, "member 0's booked debt must be visible over HTTP after the commit (got {member})");
    let network = get_json(&client, SOLO_PORT, "/network").await;
    assert!(
        network.get("height").and_then(|h| h.as_u64()).unwrap_or(0) > 0,
        "the client API must report the Malachite-committed height (got {network})"
    );

    // The engine must commit through `NodeCore::commit_decided`, not around
    // it. `committed` is incremented there and nowhere else, so a `Decided`
    // handler that applied the block directly against the replica would leave
    // this at 0 while the height climbed — which is exactly the state this
    // node shipped in, and the reason a rotated-out device kept its read
    // access until its session token's TTL ran out.
    //
    // This assertion is half of that gate: it pins down that the engine goes
    // THROUGH `commit_decided`;
    // `serve::core::tests::the_decided_commit_path_runs_the_post_commit_bookkeeping`
    // pins down what `commit_decided` then does (the session sweep).
    // Neither alone is enough; together they cover the path.
    let head = get_json(&client, SOLO_PORT, "/head").await;
    assert!(
        head.get("committed").and_then(|c| c.as_u64()).unwrap_or(0) > 0,
        "the engine must commit through NodeCore::commit_decided, which is what counts commits (got {head})"
    );

    // --- router shape: the gossip ingress IS served, the dev consensus's
    // propose/vote surface is NOT (see `serve::http::client_router`) ---------
    // Anchored AFTER `await_epoch_settled`, not on a raw `/network` read.
    //
    // "The epoch has long since stabilized"
    // because the first Accept had committed, and that is not what a commit
    // proves: `submit_accept_and_await_commit` RETRIES past a stale window, so
    // it can succeed while the epoch is still climbing its 10,000-per-block
    // catch-up. This half of the test does not retry, so it inherited the race
    // — measured, once, as `ET-TX-002` on the gossiped transaction, a failure
    // about clocks reported against the router. There is a helper for exactly
    // this and the only fix needed was to call it.
    let epoch = await_epoch_settled(&client, SOLO_PORT, COMMIT_BUDGET).await;
    let gossiped = signed_accept(1, 0, 20.0, epoch + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS);
    let gossip_hash = edet_node::block::hex32(&gossiped.hash().expect("tx hash"));
    let wire = serde_json::json!({ "kind": "Tx", "tx": gossiped });
    let resp = client
        .post(url(SOLO_PORT, "/p2p/tx"))
        .json(&wire)
        .send()
        .await
        .expect("POST /p2p/tx");
    assert!(resp.status().is_success(), "/p2p/tx must be served: it is how a peer's submission reaches this mempool");
    assert_eq!(
        await_committed(&client, SOLO_PORT, &gossip_hash, COMMIT_BUDGET)
            .await
            .as_deref(),
        Some("ok"),
        "a transaction arriving by tx gossip must be committed like any other",
    );

    let propose = client
        .post(url(SOLO_PORT, "/p2p/propose"))
        .json(&serde_json::json!({ "kind": "Propose", "height": 1, "block": null }))
        .send()
        .await
        .expect("POST /p2p/propose");
    assert!(
        propose.status() == reqwest::StatusCode::NOT_FOUND
            || propose.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED,
        "a Malachite node must NOT serve the dev consensus's propose surface (got {})",
        propose.status()
    );

    drop(_nodes);
    let _ = std::fs::remove_dir_all(&home);
}

/// The cross-node claim: submitted to ONE node's HTTP API, committed by the
/// cluster, agreed by both.
#[tokio::test]
async fn a_transaction_submitted_to_one_node_is_committed_and_agreed_by_both() {
    let home = std::env::temp_dir().join(format!("edet-malachite-http-cluster-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    // 2 equal-power validators: quorum is ceil(2*2/3) = 2, so every commit
    // carries BOTH nodes' signatures — agreement here is real consensus
    // agreement, not one node deciding alone.
    run_cli(
        &["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "2"],
        &[("EDET_CONSENSUS_BASE_PORT", CLUSTER_CONSENSUS_BASE.to_string())],
    );

    let homes: Vec<PathBuf> = (0..2).map(|i| home.join(i.to_string())).collect();
    let _nodes = spawn(&homes, &CLUSTER_PORTS, &[0, 1], &peer_urls(&CLUSTER_PORTS));
    let client = reqwest::Client::new();
    await_health(&client, &CLUSTER_PORTS, Duration::from_secs(30)).await;

    // Submit to node 1 only. Malachite gossips consensus messages, not the
    // mempool, so `--client-peers` (the `/p2p/tx` hop) is what lets whichever
    // node proposes include it — though see this file's header for why THIS
    // test cannot, by itself, distinguish that hop from node 1's own
    // proposer turn.
    let (status1, hash) = submit_accept_and_await_commit(&client, CLUSTER_PORTS[1], 0, 1, 40.0, COMMIT_BUDGET).await;
    assert_eq!(status1, "ok", "node 1 must commit the transaction it was submitted to cleanly");

    for (i, &port) in CLUSTER_PORTS.iter().enumerate() {
        let status = await_committed(&client, port, &hash, COMMIT_BUDGET)
            .await
            .unwrap_or_else(|| panic!("node {i} (:{port}) never committed the transaction submitted to node 1"));
        assert_eq!(status, "ok", "node {i} must commit the transaction cleanly");
    }

    // Both nodes' committed state must be identical — the transaction landed
    // in one agreed block, not two divergent histories.
    let n0 = get_json(&client, CLUSTER_PORTS[0], "/network").await;
    let n1 = get_json(&client, CLUSTER_PORTS[1], "/network").await;
    let (h0, h1) = (n0["state_hash"].as_str().unwrap_or(""), n1["state_hash"].as_str().unwrap_or(""));
    assert!(!h0.is_empty(), "node 0 reported no state hash: {n0}");
    // Heights advance independently of this test's reads, so compare the hash
    // at a height both have reached rather than whatever each is at right now.
    if n0["height"] == n1["height"] {
        assert_eq!(h0, h1, "the two nodes diverged at the same height ({n0} vs {n1})");
    }
    let m0 = get_json(&client, CLUSTER_PORTS[0], "/member/0").await;
    assert!(
        m0.get("debt").and_then(|d| d.as_f64()).unwrap_or(0.0) > 0.0,
        "node 0 must show the booked obligation from the tx submitted to node 1 (got {m0})",
    );

    drop(_nodes);
    let _ = std::fs::remove_dir_all(&home);
}

/// Accept ONE connection on a bound listener and return the raw HTTP request
/// (head + body), answering `200` so the sender's client sees a clean
/// exchange. Runs on its own thread: it must be listening while the async test
/// makes the call that triggers it.
fn capture_one_request(listener: std::net::TcpListener) -> std::sync::mpsc::Receiver<String> {
    use std::io::{Read, Write};
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else { return };
        let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
        // Read until the body is complete per Content-Length (the sender is
        // reqwest with a JSON body, so the header is always present).
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
            }
            let text = String::from_utf8_lossy(&raw).to_string();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let len: usize = head
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length: ")
                            .or_else(|| l.strip_prefix("Content-Length: "))
                    })
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if body.len() >= len {
                    break;
                }
            }
        }
        let _ = sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}");
        let _ = tx.send(String::from_utf8_lossy(&raw).to_string());
    });
    rx
}

/// The mempool-gossip claim, with consensus deliberately taken out of the
/// picture: this node's ONLY peer is a plain socket the test owns, so anything
/// arriving there was put on the wire by `serve::submit`'s broadcast and by
/// nothing else. Without it, a transaction submitted to one validator could
/// only ever be proposed by that validator — Malachite's own gossip carries
/// consensus messages, never the mempool.
#[tokio::test]
async fn a_submission_is_gossiped_to_every_configured_client_peer() {
    let home = std::env::temp_dir().join(format!("edet-malachite-http-gossip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    edet_node::engine_node::write_solo_home_on(&home, 1, GOSSIP_CONSENSUS_PORT).expect("write the solo node home");

    // Slot 0 is this node's own client API (its own slot is skipped when
    // gossiping — `serve::Config::peers`); slot 1 is the test's socket.
    let sink = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the peer sink");
    let sink_port = sink.local_addr().expect("sink addr").port();
    let captured = capture_one_request(sink);

    let peers = vec![format!("http://127.0.0.1:{GOSSIP_PORT}"), format!("http://127.0.0.1:{sink_port}")];
    let _nodes = spawn(std::slice::from_ref(&home), &[GOSSIP_PORT], &[0], &peers);
    let client = reqwest::Client::new();
    await_health(&client, &[GOSSIP_PORT], Duration::from_secs(30)).await;

    // This test asserts only that the tx is GOSSIPED, never that it commits
    // (the sink is a plain socket, not a real peer) — the exact
    // `not_after_epoch` chosen has no bearing on that, so a fixed, generous
    // value is fine here.
    let stx = signed_accept(0, 1, 40.0, edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS);
    let hash = edet_node::block::hex32(&stx.hash().expect("tx hash"));
    let queued: serde_json::Value = client
        .post(url(GOSSIP_PORT, "/tx"))
        .json(&stx)
        .send()
        .await
        .expect("POST /tx")
        .json()
        .await
        .expect("JSON");
    assert_eq!(queued.get("queued").and_then(|q| q.as_bool()), Some(true));

    let request = tokio::task::spawn_blocking(move || captured.recv_timeout(Duration::from_secs(20)))
        .await
        .expect("join the capture thread")
        .expect("the node never gossiped the submitted transaction to its configured peer");

    let (head, body) = request.split_once("\r\n\r\n").expect("a well-formed HTTP request");
    assert!(
        head.starts_with("POST /p2p/tx "),
        "the gossip must go to the peer's tx endpoint, got request head:\n{head}"
    );
    let wire: serde_json::Value = serde_json::from_str(body).expect("the gossip body must be JSON");
    assert_eq!(wire.get("kind").and_then(|k| k.as_str()), Some("Tx"), "gossip body: {wire}");
    let gossiped: SignedTx = serde_json::from_value(wire["tx"].clone()).expect("the gossiped tx must deserialize");
    assert_eq!(
        edet_node::block::hex32(&gossiped.hash().expect("tx hash")),
        hash,
        "the peer must receive the very transaction that was submitted",
    );

    drop(_nodes);
    let _ = std::fs::remove_dir_all(&home);
}

/// The pending-signature pool's far end: a proposal co-signed by both parties
/// through the client API becomes a real committed contract.
///
/// The pool's own behaviour — gossip to another node, thresholds, decline
/// tombstones — is covered in-process (`serve::tests`), because none of it
/// needs consensus. What needs a chain is this last hop, and it is
/// asserted against the dev consensus; on the engine the same flow has to
/// survive the epoch race a fresh chain starts in, which is why the window
/// is anchored AFTER `await_epoch_settled` rather than guessed.
#[tokio::test]
async fn a_co_signed_multi_party_proposal_commits() {
    let home = std::env::temp_dir().join(format!("edet-malachite-http-pending-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    edet_node::engine_node::write_solo_home_on(&home, 1, PENDING_CONSENSUS_PORT).expect("write the solo node home");

    let _nodes = spawn(std::slice::from_ref(&home), &[PENDING_PORT], &[0], &[]);
    let client = reqwest::Client::new();
    await_health(&client, &[PENDING_PORT], Duration::from_secs(30)).await;
    let epoch = await_epoch_settled(&client, PENDING_PORT, COMMIT_BUDGET).await;

    let tx = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 40.0,
        maturity_epochs: 30,
        arb: None,
    };
    let nonce = next_accept_nonce();
    let not_after_epoch = epoch + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS;

    // Member 0 opens the proposal with their signature alone.
    let opened = post_json(&client, PENDING_PORT, "/pending/sign", &sign_pending(0, &tx, nonce, not_after_epoch)).await;
    assert_eq!(opened.get("ok").and_then(|v| v.as_bool()), Some(true), "opening the proposal: {opened}");
    assert_eq!(opened.get("completed").and_then(|v| v.as_bool()), Some(false), "one signature is not the threshold");

    // Member 1 co-signs: the threshold is reached, the node assembles the
    // SignedTx and pushes it through the same verified ingress `/tx` uses.
    let done = post_json(&client, PENDING_PORT, "/pending/sign", &sign_pending(1, &tx, nonce, not_after_epoch)).await;
    assert_eq!(done.get("completed").and_then(|v| v.as_bool()), Some(true), "the co-sign must complete it: {done}");
    assert_eq!(done.get("queued").and_then(|v| v.as_bool()), Some(true), "and it must pass the ingress: {done}");

    // ...and the engine must commit it. Asserted on the booked contract
    // rather than on a tx hash: the assembled transaction's signer order is
    // the pool's (`BTreeMap` over keys), so the submitter cannot predict the
    // hash — which is exactly the position a real second device is in.
    let start = Instant::now();
    loop {
        let contracts = get_json(&client, PENDING_PORT, "/contracts").await;
        let n = contracts.as_array().map(|a| a.len()).unwrap_or(0);
        if n == 1 {
            break;
        }
        assert!(
            start.elapsed() <= COMMIT_BUDGET,
            "the co-signed transaction never became a committed contract (contracts: {contracts})"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    drop(_nodes);
    let _ = std::fs::remove_dir_all(&home);
}

/// On the engine: passing the ingress is not the same as being applied. A
/// self-deal Accept is validly signed by its one party, so ingress cannot
/// refuse it, and `apply` rejects it inside the block that commits it —
/// invisible to the submitter unless `/tx/outcome/:hash` reports it, with the
/// exact ET code.
#[tokio::test]
async fn a_commit_time_rejection_is_reported_at_tx_outcome_with_its_et_code() {
    let home = std::env::temp_dir().join(format!("edet-malachite-http-outcome-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    edet_node::engine_node::write_solo_home_on(&home, 1, OUTCOME_CONSENSUS_PORT).expect("write the solo node home");

    let _nodes = spawn(std::slice::from_ref(&home), &[OUTCOME_PORT], &[0], &[]);
    let client = reqwest::Client::new();
    await_health(&client, &[OUTCOME_PORT], Duration::from_secs(30)).await;

    // A hash nothing was ever submitted under reads as unknown, not as some
    // default outcome.
    let unknown = get_json(&client, OUTCOME_PORT, &format!("/tx/outcome/{}", "0".repeat(64))).await;
    assert_eq!(
        unknown.get("status").and_then(|s| s.as_str()),
        Some("unknown"),
        "a never-seen hash must report unknown: {unknown}"
    );

    let epoch = await_epoch_settled(&client, OUTCOME_PORT, COMMIT_BUDGET).await;
    let self_deal = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(0),
        amount: 10.0,
        maturity_epochs: 30,
        arb: None,
    };
    let stx = sign_tx(
        edet_node::block::DEV_CHAIN_ID,
        self_deal,
        next_accept_nonce(),
        epoch + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS,
        &[dev_seed(0)],
    )
    .expect("sign the self-deal");
    let hash = edet_node::block::hex32(&stx.hash().expect("tx hash"));

    let queued: serde_json::Value = client
        .post(url(OUTCOME_PORT, "/tx"))
        .json(&stx)
        .send()
        .await
        .expect("POST /tx")
        .json()
        .await
        .expect("POST /tx returned non-JSON");
    assert_eq!(
        queued.get("queued").and_then(|q| q.as_bool()),
        Some(true),
        "the ingress cannot know a self-deal will fail at apply-time: {queued}"
    );

    let start = Instant::now();
    loop {
        let v = get_json(&client, OUTCOME_PORT, &format!("/tx/outcome/{hash}")).await;
        match v.get("status").and_then(|s| s.as_str()) {
            Some("rejected") => {
                assert_eq!(
                    v.get("code").and_then(|c| c.as_str()),
                    Some("ET-CTR-007"),
                    "the exact ET code must be reported: {v}"
                );
                break;
            }
            Some("ok") => panic!("a self-deal Accept must not apply cleanly: {v}"),
            _ => {}
        }
        assert!(
            start.elapsed() <= COMMIT_BUDGET,
            "the commit-time outcome never became visible at /tx/outcome (last: {v})"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    drop(_nodes);
    let _ = std::fs::remove_dir_all(&home);
}

/// A co-sign request for `member` over a proposal between members 0 and 1.
/// Every co-signer must reproduce the EXACT digest the proposal was
/// opened under — pool entries are keyed by digest — so `nonce` and
/// `not_after_epoch` are fixed per proposal and threaded through each call,
/// exactly as a second device reads them off the `pending` view.
fn sign_pending(member: u8, tx: &Tx, nonce: [u8; 16], not_after_epoch: u64) -> serde_json::Value {
    use ed25519_dalek::{Signer, SigningKey};
    let digest = edet_node::block::tx_digest(edet_node::block::DEV_CHAIN_ID, tx, &nonce, not_after_epoch)
        .expect("digest the proposal");
    let sk = SigningKey::from_bytes(&dev_seed(member));
    serde_json::json!({
        "tx": tx,
        "nonce": nonce,
        "not_after_epoch": not_after_epoch,
        "required": [{"Member": 0u64}, {"Member": 1u64}],
        "min_sigs": 2,
        "signer": sk.verifying_key().to_bytes(),
        "signature": sk.sign(&digest).to_bytes().to_vec(),
    })
}

async fn post_json(client: &reqwest::Client, port: u16, path: &str, body: &serde_json::Value) -> serde_json::Value {
    client
        .post(url(port, path))
        .json(body)
        .send()
        .await
        .unwrap_or_else(|e| panic!("POST {path} on :{port}: {e}"))
        .json()
        .await
        .unwrap_or_else(|e| panic!("POST {path} on :{port} returned non-JSON: {e}"))
}
