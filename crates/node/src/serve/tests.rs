//! The client API, over real HTTP against real nodes: the verified ingress,
//! the `/p2p/*` perimeter, transaction gossip, the pending-signature pool,
//! authenticated reads, sessions, rate limiting and view truncation.
//!
//! None of it needs consensus, and that is why it is still in-process. These
//! tests do not start `serve::serve` against a dev
//! consensus — and several of them asserted on committed state, which meant
//! the surface a browser talks to was gated against a consensus we do not
//! ship. The dev consensus is gone; what remains here is the half that is
//! genuinely independent of who decides blocks, driven against
//! `serve::spawn_client`, the very router a Malachite validator serves.
//!
//! Everything that needs a block to actually COMMIT moved out of process, to
//! `tests/malachite_http.rs`, where it runs against the engine.
//!
//! Runs only under `--features serve`.

use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};

use edet_state::tx::Tx;
use edet_state::types::Party;

use super::auth::viewer_auth_message;
use super::pending::decline_message;
use super::{dev_genesis, Config, Node};
use crate::block::{counter_nonce, dev_seed, pubkey_of, sign_tx, tx_digest, SignedTx, DEV_CHAIN_ID};

/// A fresh nonce per call, unique across this whole test binary.
fn next_nonce() -> [u8; 16] {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    counter_nonce(COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Every genesis here is `dev_genesis` -> `State::default()`, whose epoch is
/// 0 and never advances (nothing commits in these tests), so a fixed generous
/// window is valid everywhere in this file. A node on a live chain needs the
/// window anchored to the chain's CURRENT epoch instead — see
/// `tests/malachite_http.rs::submit_accept_and_await_commit`.
const TEST_NOT_AFTER_EPOCH: u64 = 30;

/// One node's client-API config on `port`, with `peers` as its tx-gossip
/// targets (slot-indexed, own slot included and ignored).
fn cfg(index: usize, port: u16, peers: Vec<String>) -> Config {
    Config {
        index,
        n: peers.len().max(1),
        listen_port: port,
        peers,
        allow_unsigned: false,
        data_dir: None,
        snapshot_interval: 0,
        prune_margin_blocks: crate::replica::DEFAULT_PRUNE_MARGIN_BLOCKS,
        bind_all: false,
        cors_ports: vec![],
        cluster_token: None,
        trust_forwarded_for: false,
    }
}

fn solo_cfg(port: u16) -> Config {
    cfg(0, port, vec![format!("http://127.0.0.1:{port}")])
}

/// A listener on a port the OS chose. Every test binds its own, so no two
/// tests contend for a port and no server leaked by an earlier run can fail
/// one for a reason that has nothing to do with the code — the engine
/// harnesses need fixed ports, because a testnet's addresses are written into
/// every node's config; nothing here does.
fn free_listener() -> (std::net::TcpListener, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    listener.set_nonblocking(true).expect("a non-blocking listener");
    let port = listener.local_addr().expect("the bound address").port();
    (listener, port)
}

/// Build a node and serve its client API on `listener`, returning once it is
/// served — the listener is already bound, so unlike the old dev-cluster
/// harness this needs no "let the listener come up" sleep, and a test that
/// races startup fails loudly instead of flaking.
async fn start_on(listener: std::net::TcpListener, cfg: Config, genesis: edet_state::State) -> Arc<Node> {
    let node = super::build(cfg, genesis).expect("build the node");
    let listener = tokio::net::TcpListener::from_std(listener).expect("adopt the listener");
    super::spawn_client_on(node.clone(), listener).expect("serve the client API");
    node
}

/// A single node on a free port, with the port it got.
async fn start_solo(genesis: edet_state::State) -> (Arc<Node>, u16) {
    let (listener, port) = free_listener();
    (start_on(listener, solo_cfg(port), genesis).await, port)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A fresh 16-byte nonce, hex. Every credential needs one: Ed25519 is
/// deterministic, so without it two identical requests carry one signature and
/// the node's replay cache refuses the honest second.
fn viewer_nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    format!("{:032x}", N.fetch_add(1, Ordering::Relaxed))
}

/// Sign a §Standing header set for founder `member` (dev seed convention) over
/// `method`/`path`/`ts`, ready to attach to a `reqwest::RequestBuilder`.
fn viewer_headers(member: u64, seed_id: u8, method: &str, path: &str, ts: u64) -> Vec<(&'static str, String)> {
    viewer_headers_with(member, seed_id, method, path, ts, &viewer_nonce())
}

fn viewer_headers_with(
    member: u64,
    seed_id: u8,
    method: &str,
    path: &str,
    ts: u64,
    nonce: &str,
) -> Vec<(&'static str, String)> {
    let sk = SigningKey::from_bytes(&dev_seed(seed_id));
    let sig = sk.sign(&viewer_auth_message(crate::block::DEV_CHAIN_ID, method, path, ts, nonce));
    let sig_hex: String = sig.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    vec![
        ("x-edet-viewer", member.to_string()),
        ("x-edet-viewer-sig", sig_hex),
        ("x-edet-viewer-ts", ts.to_string()),
        ("x-edet-viewer-nonce", nonce.to_string()),
    ]
}

async fn get_json(client: &reqwest::Client, port: u16, path: &str) -> serde_json::Value {
    client
        .get(format!("http://127.0.0.1:{port}{path}"))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json")
}

async fn post_json(client: &reqwest::Client, port: u16, path: &str, body: &serde_json::Value) -> serde_json::Value {
    client
        .post(format!("http://127.0.0.1:{port}{path}"))
        .json(body)
        .send()
        .await
        .expect("post")
        .json()
        .await
        .expect("json")
}

async fn submit_tx(client: &reqwest::Client, port: u16, stx: &SignedTx) -> serde_json::Value {
    let res = client
        .post(format!("http://127.0.0.1:{port}/tx"))
        .json(stx)
        .send()
        .await
        .expect("post /tx");
    // The per-source limiter answers ahead of the handler with a plain 429:
    // not queued, and not a JSON body to parse. A flood from one address is
    // refused there before the per-signer bucket ever sees it.
    if res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return serde_json::json!({ "queued": false, "throttled": true });
    }
    res.json().await.expect("json")
}

/// `submit_tx` from a named source address; only meaningful against a node
/// with `trust_forwarded_for` set.
///
/// **The per-IP bucket in front of `/tx` is tighter than the per-signer one**,
/// so a flood from one address never reaches the per-signer bucket at all:
/// 120 read tokens against `1.0 + body_len / 768` — about 2.4 for a
/// two-signature envelope — is ~50 requests of burst, where the per-signer
/// bucket carries 64. Spreading the flood across sources is what leaves the
/// per-signer bucket as the thing under test.
async fn submit_tx_from(client: &reqwest::Client, port: u16, stx: &SignedTx, source: &str) -> serde_json::Value {
    let res = client
        .post(format!("http://127.0.0.1:{port}/tx"))
        .header("x-forwarded-for", source)
        .json(stx)
        .send()
        .await
        .expect("post /tx");
    if res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return serde_json::json!({ "queued": false, "throttled": true });
    }
    res.json().await.expect("json")
}

// --- the verified ingress ---------------------------------------------------

#[tokio::test]
async fn forged_and_unsigned_submissions_are_rejected() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();
    let tx = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 30.0,
        maturity_epochs: 30,
        arb: None,
    };

    // A bare claim (correct keys, no signatures) must not queue.
    let claim = SignedTx {
        tx: tx.clone(),
        nonce: next_nonce(),
        not_after_epoch: TEST_NOT_AFTER_EPOCH,
        signers: vec![pubkey_of(&dev_seed(0)), pubkey_of(&dev_seed(1))],
        signatures: vec![],
    };
    assert_eq!(submit_tx(&client, port, &claim).await["queued"], false, "unsigned claim was accepted");

    // Signatures over a DIFFERENT transaction must not queue either.
    let other = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 999.0,
        maturity_epochs: 30,
        arb: None,
    };
    let mut forged =
        sign_tx(DEV_CHAIN_ID, other, next_nonce(), TEST_NOT_AFTER_EPOCH, &[dev_seed(0), dev_seed(1)]).expect("sign");
    forged.tx = tx.clone();
    assert_eq!(submit_tx(&client, port, &forged).await["queued"], false, "signature over a different tx was accepted");

    // The properly signed transaction goes through.
    let good =
        sign_tx(DEV_CHAIN_ID, tx, next_nonce(), TEST_NOT_AFTER_EPOCH, &[dev_seed(0), dev_seed(1)]).expect("sign");
    assert_eq!(submit_tx(&client, port, &good).await["queued"], true, "valid signature was rejected");
}

#[tokio::test]
async fn empty_signer_non_crank_txs_and_floods_are_rejected() {
    // An empty signer set only passes `SignedTx::verify` trivially — the
    // ingress gate (`driver::submit`) must still refuse it unless the
    // transaction is one of the deliberately permissionless cranks.
    // `trust_forwarded_for`, so the flood at the end of this test can carry a
    // source of its own per request and be refused by the per-signer bucket
    // rather than by the per-IP one in front of it. Nothing else in the node
    // reads that header — `reader_ip` is the rate limiter's alone.
    let (listener, port) = free_listener();
    let mut config = solo_cfg(port);
    config.trust_forwarded_for = true;
    let _node = start_on(listener, config, dev_genesis(5)).await;
    let client = reqwest::Client::new();

    // A bare, unsigned Accept (no signers claimed at all) must not queue,
    // even though `verify()` would pass an empty signer/signature pair.
    let bare = SignedTx {
        tx: Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 30.0,
            maturity_epochs: 30,
            arb: None,
        },
        nonce: next_nonce(),
        not_after_epoch: TEST_NOT_AFTER_EPOCH,
        signers: vec![],
        signatures: vec![],
    };
    assert_eq!(submit_tx(&client, port, &bare).await["queued"], false, "an empty-signer non-crank tx must be refused");

    // An empty-signer crank is legitimate and must queue. `RotateFinalize` on
    // a member that exists rather than `MarkExpired` on a contract that does
    // not: an envelope naming an unissued id is kept out of the mempool on its
    // own account, and this probe is about the signer rule.
    let crank = SignedTx {
        tx: Tx::RotateFinalize { member: 0 },
        nonce: next_nonce(),
        not_after_epoch: TEST_NOT_AFTER_EPOCH,
        signers: vec![],
        signatures: vec![],
    };
    assert_eq!(
        submit_tx(&client, port, &crank).await["queued"],
        true,
        "an empty-signer permissionless crank must still be accepted"
    );

    // Flood ONE signer identity with distinct, validly signed transactions:
    // the per-signer token bucket must refuse some of them.
    //
    // **What this probe measures has to be the node's limiter and not this
    // machine's speed.** Two things used to decide the outcome instead:
    //
    //   * *The refill outran the flood.* The bucket admits
    //     `TX_BUCKET_CAPACITY` at once plus `TX_BUCKET_REFILL_PER_SEC` for
    //     every second the flood takes, so a fixed 200-request loop only saw a
    //     refusal while it stayed under (200 - 64) / 16 = 8.5 seconds — 23
    //     requests a second with two Ed25519 signatures each, in a debug
    //     build. A loaded CI runner does not manage that: every request is
    //     admitted and the probe fails with `queued=200`, having proved
    //     nothing. So the signing happens BEFORE the timed flood, the requests
    //     go out in overlapping waves, and it stops at the first refusal.
    //   * *The wrong bucket answered.* From one address the per-IP limiter is
    //     the tighter of the two (see `submit_tx_from`), so it refused first
    //     and a `rejected > 0` counted its 429s — the per-signer bucket this
    //     test names could have been absent and it would still have passed.
    //     A source per request leaves the per-signer bucket alone in front,
    //     which is also the shape the bucket is FOR: one identity flooding
    //     from many addresses is exactly what a per-IP limit cannot see.
    //
    // The ceiling assertion is then the bucket's own guarantee rather than a
    // count — admissions never exceed capacity plus refill for the time taken
    // — which is true at any speed and, wherever the flood outran the refill,
    // forces the refusal the second assertion demands.
    const FLOOD: usize = 640;
    const WAVE: usize = 32;
    let flood: Vec<SignedTx> = (0..FLOOD as u64)
        .map(|i| {
            let tx = Tx::Accept {
                debtor: Party::Member(2),
                creditor: Party::Member(3),
                amount: 1.0 + i as f64,
                maturity_epochs: 30,
                arb: None,
            };
            sign_tx(DEV_CHAIN_ID, tx, next_nonce(), TEST_NOT_AFTER_EPOCH, &[dev_seed(2), dev_seed(3)]).expect("sign")
        })
        .collect();

    let mut queued = 0usize;
    let mut refused = 0usize;
    let mut throttled = 0usize;
    let started = std::time::Instant::now();
    for (w, wave) in flood.chunks(WAVE).enumerate() {
        let mut set = tokio::task::JoinSet::new();
        for (i, stx) in wave.iter().enumerate() {
            let (client, stx) = (client.clone(), stx.clone());
            // A source of its own per request: a fresh bucket every time, so
            // the per-IP limiter admits the whole flood and never stands in.
            let n = w * WAVE + i;
            let source = format!("10.0.{}.{}", n / 256, n % 256);
            set.spawn(async move { submit_tx_from(&client, port, &stx, &source).await });
        }
        while let Some(res) = set.join_next().await {
            let body = res.expect("submit task");
            match (body["queued"] == true, body["throttled"] == true) {
                (true, _) => queued += 1,
                (false, true) => throttled += 1,
                (false, false) => refused += 1,
            }
        }
        if refused > 0 {
            break;
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    let sent = queued + refused + throttled;

    assert_eq!(throttled, 0, "the flood was spread over {sent} sources and must never reach the per-IP limiter");
    let ceiling = super::core::TX_BUCKET_CAPACITY + super::core::TX_BUCKET_REFILL_PER_SEC * elapsed + 1.0;
    assert!(
        queued as f64 <= ceiling,
        "the per-signer bucket admitted {queued} in {elapsed:.2}s, over its own ceiling of {ceiling:.0}"
    );
    assert!(
        refused > 0,
        "a flood from one signer was never refused by the per-signer bucket (queued={queued}, \
         {sent} requests in {elapsed:.2}s = {rate:.0}/s against a refill of {refill}/s)",
        rate = sent as f64 / elapsed.max(1e-9),
        refill = super::core::TX_BUCKET_REFILL_PER_SEC,
    );
}

// --- the gossip perimeter ---------------------------------------------------

#[tokio::test]
async fn p2p_requires_the_configured_cluster_token() {
    // Once a cluster_token is set, it is required on every /p2p/* call —
    // even from loopback, which the no-token default would otherwise trust.
    let (listener, port) = free_listener();
    let mut c = solo_cfg(port);
    c.cluster_token = Some("s3cr3t".into());
    let _node = start_on(listener, c, dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let tx = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 30.0,
        maturity_epochs: 30,
        arb: None,
    };
    let stx = sign_tx(DEV_CHAIN_ID, tx, next_nonce(), TEST_NOT_AFTER_EPOCH, &[dev_seed(0), dev_seed(1)]).expect("sign");
    let wire = serde_json::json!({ "kind": "Tx", "tx": stx });
    let url = format!("http://127.0.0.1:{port}/p2p/tx");

    // No token: rejected.
    let r = client.post(&url).json(&wire).send().await.expect("post");
    assert_eq!(r.status(), reqwest::StatusCode::FORBIDDEN, "p2p call without the cluster token must be rejected");

    // Wrong token: rejected.
    let r = client
        .post(&url)
        .header("x-edet-cluster-token", "wrong")
        .json(&wire)
        .send()
        .await
        .expect("post");
    assert_eq!(r.status(), reqwest::StatusCode::FORBIDDEN, "p2p call with the wrong cluster token must be rejected");

    // Right token: accepted.
    let r = client
        .post(&url)
        .header("x-edet-cluster-token", "s3cr3t")
        .json(&wire)
        .send()
        .await
        .expect("post");
    assert!(r.status().is_success(), "p2p call with the correct cluster token must be accepted");
}

/// **The token guards a surface, not a capability**, and knowing which is what
/// decides whether `--bind-all` without one may be a warning.
///
/// `/p2p/tx` and `/tx` call one `driver::submit`; `/p2p/pending-sign` and
/// `/pending/sign` call one `driver::pending_sign`. So an untokened `/p2p/*`
/// admits exactly what the client routes on the same open port admit with no
/// token at all, under the same signature check, the same empty-signer rule
/// and the same per-signer bucket. Refusing to start without a token would
/// close a door standing open beside it and take a legitimate deployment — a
/// validator whose perimeter is its network segment — with it.
///
/// What the token DOES buy is a perimeter: with one set, the source-address
/// fallback is gone and every gossip call must present it (the probe above).
/// Without one the fallback is loopback-or-a-configured-peer, which is a
/// weaker claim than it sounds on a node bound to every interface — hence the
/// warning, which is the honest shape.
#[tokio::test]
async fn the_peer_ingress_admits_exactly_what_the_client_ingress_admits() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();
    let accept = |d, c| Tx::Accept {
        debtor: Party::Member(d),
        creditor: Party::Member(c),
        amount: 30.0,
        maturity_epochs: 30,
        arb: None,
    };

    // An envelope nobody signed for: refused on both, without a token on
    // either. The peer route is not a way past the ingress rule.
    let forged = sign_tx(DEV_CHAIN_ID, accept(0, 1), next_nonce(), TEST_NOT_AFTER_EPOCH, &[dev_seed(2)]).expect("sign");
    assert_eq!(submit_tx(&client, port, &forged).await["queued"], false, "the client route refuses it");
    let before = client.get(format!("http://127.0.0.1:{port}/head")).send().await.expect("head");
    assert!(before.status().is_success());
    let r = client
        .post(format!("http://127.0.0.1:{port}/p2p/tx"))
        .json(&serde_json::json!({ "kind": "Tx", "tx": forged }))
        .send()
        .await
        .expect("post");
    assert!(r.status().is_success(), "the peer route answers, and drops it in the same driver");

    // A properly signed one: admitted on both. The peer route is not a
    // privileged one either — it is the client route with a different name.
    let good = sign_tx(DEV_CHAIN_ID, accept(0, 1), next_nonce(), TEST_NOT_AFTER_EPOCH, &[dev_seed(0), dev_seed(1)])
        .expect("sign");
    assert_eq!(submit_tx(&client, port, &good).await["queued"], true, "and admits a signed one");
}

#[tokio::test]
async fn a_submitted_transaction_reaches_its_peers_mempool() {
    // Malachite gossips consensus messages, never the mempool, so this hop is
    // the only thing that lets a transaction submitted on one device be
    // proposed by whichever node's turn it is. Two client APIs peered with
    // each other, no consensus anywhere: what arrives at node 1 arrived by
    // gossip and by nothing else.
    let (listener0, port0) = free_listener();
    let (listener1, port1) = free_listener();
    let ports = [port0, port1];
    let peers: Vec<String> = ports.iter().map(|p| format!("http://127.0.0.1:{p}")).collect();
    let _n0 = start_on(listener0, cfg(0, ports[0], peers.clone()), dev_genesis(5)).await;
    let n1 = start_on(listener1, cfg(1, ports[1], peers), dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let tx = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 30.0,
        maturity_epochs: 30,
        arb: None,
    };
    let stx = sign_tx(DEV_CHAIN_ID, tx, next_nonce(), TEST_NOT_AFTER_EPOCH, &[dev_seed(0), dev_seed(1)]).expect("sign");
    let hash = stx.hash().expect("tx hash");
    assert_eq!(submit_tx(&client, ports[0], &stx).await["queued"], true);

    // The send is fire-and-forget on a spawned task, so poll rather than
    // assume it has landed by the time the submit response comes back.
    let mut seen = false;
    for _ in 0..100 {
        if n1.lock().tx_pending(&hash) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(seen, "a transaction submitted to node 0 never reached node 1's mempool");
}

#[tokio::test]
async fn outbound_gossip_carries_the_cluster_token_its_peers_require() {
    // The other half of `p2p_requires_the_configured_cluster_token`: a node
    // that requires the token must also PRESENT it, or a token-guarded
    // cluster would refuse its own members' gossip and quietly stop sharing
    // a mempool. Asserted on the wire rather than through a second node, so
    // the header is checked directly instead of inferred from an outcome
    // that a single-node cluster could have produced on its own.
    let (listener, port) = free_listener();
    let sink = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the peer sink");
    let sink_port = sink.local_addr().expect("sink addr").port();
    let captured = capture_one_request(sink);

    let mut c = cfg(0, port, vec![format!("http://127.0.0.1:{port}"), format!("http://127.0.0.1:{sink_port}")]);
    c.cluster_token = Some("herd-secret".into());
    let _node = start_on(listener, c, dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let tx = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 30.0,
        maturity_epochs: 30,
        arb: None,
    };
    let stx = sign_tx(DEV_CHAIN_ID, tx, next_nonce(), TEST_NOT_AFTER_EPOCH, &[dev_seed(0), dev_seed(1)]).expect("sign");
    assert_eq!(submit_tx(&client, port, &stx).await["queued"], true);

    let request = tokio::task::spawn_blocking(move || captured.recv_timeout(Duration::from_secs(10)))
        .await
        .expect("join the capture thread")
        .expect("the node never gossiped the submitted transaction to its configured peer");
    let head = request.split("\r\n\r\n").next().unwrap_or_default().to_lowercase();
    assert!(head.starts_with("post /p2p/tx "), "gossip must go to the peer's tx endpoint:\n{head}");
    assert!(
        head.contains("x-edet-cluster-token: herd-secret"),
        "outbound gossip must carry the token the peer's own guard requires:\n{head}"
    );
}

/// Accept ONE connection on a bound listener and return the raw HTTP request,
/// answering `200` so the sender sees a clean exchange. Runs on its own
/// thread: it must be listening while the async test triggers the call.
fn capture_one_request(listener: std::net::TcpListener) -> std::sync::mpsc::Receiver<String> {
    use std::io::{Read, Write};
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else { return };
        let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
            }
            // Read until the body is complete per Content-Length (the sender
            // is reqwest with a JSON body, so the header is always present).
            let text = String::from_utf8_lossy(&raw).to_string();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let len: usize = head
                    .lines()
                    .find_map(|l| l.to_lowercase().strip_prefix("content-length: ").map(|v| v.trim().to_string()))
                    .and_then(|v| v.parse().ok())
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

// --- the pending-signature pool --------------------------------------------

/// A co-sign request for `member` over a proposal. A co-sign must
/// reproduce the EXACT digest the proposal was opened under — entries are
/// keyed by digest — so `nonce`/`not_after_epoch` are fixed per logical
/// proposal and threaded through every call for it, exactly like a real
/// second device reading them off the `pending` view.
fn sign_pending(member: u8, tx: &Tx, nonce: [u8; 16], not_after_epoch: u64) -> serde_json::Value {
    let digest = tx_digest(DEV_CHAIN_ID, tx, &nonce, not_after_epoch).expect("digest");
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

#[tokio::test]
async fn pending_pool_collects_signatures_across_nodes() {
    // A pair split across two devices converges: the proposal is opened on
    // one node, becomes visible on the other by gossip, and the co-signature
    // arriving THERE completes it and pushes the assembled transaction
    // through the verified ingress.
    //
    // Where this test stops is deliberate: it asserts the completed
    // transaction was ACCEPTED (`queued`), not that it committed. Nothing
    // here decides blocks. `tests/malachite_http.rs` carries the same
    // proposal all the way to a committed contract on a real engine node.
    let (listener0, port0) = free_listener();
    let (listener1, port1) = free_listener();
    let ports = [port0, port1];
    let peers: Vec<String> = ports.iter().map(|p| format!("http://127.0.0.1:{p}")).collect();
    let _n0 = start_on(listener0, cfg(0, ports[0], peers.clone()), dev_genesis(5)).await;
    let _n1 = start_on(listener1, cfg(1, ports[1], peers), dev_genesis(5)).await;
    let client = reqwest::Client::new();

    // Member 0 (on node 0) proposes; only their signature so far.
    let tx = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 30.0,
        maturity_epochs: 30,
        arb: None,
    };
    let nonce1 = next_nonce();
    let r = post_json(&client, ports[0], "/pending/sign", &sign_pending(0, &tx, nonce1, TEST_NOT_AFTER_EPOCH)).await;
    assert_eq!(r["ok"], true);
    assert_eq!(r["completed"], false);

    // Gossip must surface it on node 1 as awaiting member 1's signature.
    // `/pending/:member` is gated to `viewer == member` (or a validator) —
    // authenticate as member 1 asking about their own queue.
    let mut seen = false;
    for _ in 0..100 {
        let ts = now_secs();
        let mut req = client.get(format!("http://127.0.0.1:{}/pending/1", ports[1]));
        for (k, v) in viewer_headers(1, 1, "GET", "/pending/1", ts) {
            req = req.header(k, v);
        }
        let p: serde_json::Value = req.send().await.expect("get").json().await.expect("json");
        if p["awaiting_me"].as_array().map(|a| a.len()) == Some(1) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(seen, "pending proposal did not gossip to node 1");

    // Member 1 co-signs on that OTHER node; threshold reached → submitted.
    let r = post_json(&client, ports[1], "/pending/sign", &sign_pending(1, &tx, nonce1, TEST_NOT_AFTER_EPOCH)).await;
    assert_eq!(r["ok"], true);
    assert_eq!(r["completed"], true, "co-sign should complete the proposal");
    assert_eq!(r["queued"], true, "completed tx should pass the verified ingress");

    // A second proposal, declined by the counterparty, disappears.
    let tx2 = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 12.0,
        maturity_epochs: 31,
        arb: None,
    };
    let nonce2 = next_nonce();
    post_json(&client, ports[0], "/pending/sign", &sign_pending(0, &tx2, nonce2, TEST_NOT_AFTER_EPOCH)).await;
    let digest2 = tx_digest(DEV_CHAIN_ID, &tx2, &nonce2, TEST_NOT_AFTER_EPOCH).expect("digest");
    let sk1 = SigningKey::from_bytes(&dev_seed(1));
    let decline = serde_json::json!({
        "digest": crate::block::hex32(&digest2),
        "signer": sk1.verifying_key().to_bytes(),
        "signature": sk1.sign(&decline_message(&digest2)).to_bytes().to_vec(),
    });
    let r = post_json(&client, ports[1], "/pending/decline", &decline).await;
    assert_eq!(r["ok"], true, "decline by the counterparty must be accepted");

    // Tombstoned: re-signing the declined tx is refused.
    let r = post_json(&client, ports[1], "/pending/sign", &sign_pending(1, &tx2, nonce2, TEST_NOT_AFTER_EPOCH)).await;
    assert_eq!(r["ok"], false, "declined proposal must not be resurrectable");
}

/// **The newcomer's first trade, assembled across two devices.**
///
/// This is the flow that replaced `OpenAccount`, and it is the one the deleted
/// transition would make trivial: the newcomer has no member id, so the
/// established member opens the proposal naming them by KEY, and the newcomer's
/// device finds it — and co-signs it — addressed by that same key.
///
/// Three properties, each of which the old shape got for free and this one has
/// to earn. The newcomer can SEE the proposal (`/pending/key/:hex`, which
/// `Viewer` alone cannot serve because it resolves an unattributed key to
/// anonymous). They can SIGN it, which means the pool's required set and its
/// rate-limiter door both had to learn about keys. And a stranger cannot read
/// somebody else's queue by naming their key, which is the gate that comes with
/// the new door.
#[tokio::test]
async fn a_newcomer_co_signs_the_trade_that_seats_them() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    // A key the ledger has never seen. Deliberately not a dev founder seed:
    // the whole point is that nobody knows this one.
    let newcomer = SigningKey::from_bytes(&[0xAB; 32]);
    let newcomer_key = newcomer.verifying_key().to_bytes();
    let key_hex: String = newcomer_key.iter().map(|b| format!("{b:02x}")).collect();

    let tx = Tx::Accept {
        debtor: Party::Key(newcomer_key),
        creditor: Party::Member(0),
        amount: 400.0,
        maturity_epochs: 30,
        arb: None,
    };
    let nonce = next_nonce();
    let digest = tx_digest(DEV_CHAIN_ID, &tx, &nonce, TEST_NOT_AFTER_EPOCH).expect("digest");
    let required = serde_json::json!([{ "Key": newcomer_key }, { "Member": 0u64 }]);

    // The established member opens it, naming the newcomer by key.
    let sk0 = SigningKey::from_bytes(&dev_seed(0));
    let open = serde_json::json!({
        "tx": tx, "nonce": nonce, "not_after_epoch": TEST_NOT_AFTER_EPOCH,
        "required": required, "min_sigs": 2,
        "signer": sk0.verifying_key().to_bytes(),
        "signature": sk0.sign(&digest).to_bytes().to_vec(),
    });
    let r = post_json(&client, port, "/pending/sign", &open).await;
    assert_eq!(r["ok"], true, "an established member may open a proposal for a key: {r}");
    assert_eq!(r["completed"], false);

    // The newcomer's device finds it, addressed by its own key.
    let path = format!("/pending/key/{key_hex}");
    let ts = now_secs();
    let cred_nonce = viewer_nonce();
    let sig: String = newcomer
        .sign(&viewer_auth_message(crate::block::DEV_CHAIN_ID, "GET", &path, ts, &cred_nonce))
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mine: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}{path}"))
        .header("x-edet-viewer-key", &key_hex)
        .header("x-edet-viewer-sig", &sig)
        .header("x-edet-viewer-ts", ts.to_string())
        .header("x-edet-viewer-nonce", &cred_nonce)
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    assert_eq!(mine["awaiting_me"].as_array().map(|a| a.len()), Some(1), "the newcomer must see it: {mine}");

    // A stranger naming that key gets nothing, credential or not. The second
    // caller is another unattributed KEY rather than a member, because in
    // `dev_genesis` every founder is a validator and would read it under the
    // The validator carve-out — the scar this whole test file exists to avoid.
    let anon: serde_json::Value = get_json(&client, port, &path).await;
    assert!(anon.get("error").is_some(), "an unauthenticated caller must not read it: {anon}");
    let other_key = SigningKey::from_bytes(&[0xEF; 32]);
    let other_hex: String = other_key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let ts = now_secs();
    let other_nonce = viewer_nonce();
    let other_sig: String = other_key
        .sign(&viewer_auth_message(crate::block::DEV_CHAIN_ID, "GET", &path, ts, &other_nonce))
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let other: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}{path}"))
        .header("x-edet-viewer-key", &other_hex)
        .header("x-edet-viewer-sig", &other_sig)
        .header("x-edet-viewer-ts", ts.to_string())
        .header("x-edet-viewer-nonce", &other_nonce)
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    assert!(other.get("error").is_some(), "another key must not read this one's queue: {other}");

    // And the newcomer's own signature completes it.
    let cosign = serde_json::json!({
        "tx": tx, "nonce": nonce, "not_after_epoch": TEST_NOT_AFTER_EPOCH,
        "required": required, "min_sigs": 2,
        "signer": newcomer_key,
        "signature": newcomer.sign(&digest).to_bytes().to_vec(),
    });
    let r = post_json(&client, port, "/pending/sign", &cosign).await;
    assert_eq!(r["ok"], true, "the newcomer must be able to co-sign: {r}");
    assert_eq!(r["completed"], true, "and their signature completes it");
    assert_eq!(r["queued"], true, "which the verified ingress accepts");
}

/// A key with no account may CO-SIGN a proposal that names it and may never
/// OPEN one.
///
/// The pool's occupancy bound is per initiator, and keys are free — so an
/// initiator the ledger cannot count is the free-signature bound one layer out,
/// exactly the shape the alphabet has no transition for. Refused in the pool, and
/// refused again at the rate-limiter door, which is where a stranger would
/// otherwise claim a bucket for nothing.
#[tokio::test]
async fn a_key_with_no_account_cannot_open_a_pending_proposal() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let stranger = SigningKey::from_bytes(&[0xCD; 32]);
    let stranger_key = stranger.verifying_key().to_bytes();
    let tx = Tx::Accept {
        debtor: Party::Key(stranger_key),
        creditor: Party::Member(0),
        amount: 400.0,
        maturity_epochs: 30,
        arb: None,
    };
    let nonce = next_nonce();
    let digest = tx_digest(DEV_CHAIN_ID, &tx, &nonce, TEST_NOT_AFTER_EPOCH).expect("digest");
    let req = serde_json::json!({
        "tx": tx, "nonce": nonce, "not_after_epoch": TEST_NOT_AFTER_EPOCH,
        "required": [{ "Key": stranger_key }, { "Member": 0u64 }],
        "min_sigs": 2,
        "signer": stranger_key,
        "signature": stranger.sign(&digest).to_bytes().to_vec(),
    });
    let r = post_json(&client, port, "/pending/sign", &req).await;
    assert_eq!(r["ok"], false, "a key nobody knows must not be able to open an entry: {r}");
}

// --- one read surface, two transports ---------------------------------------

/// `serve::Read` names the reads the embedded (Tauri IPC) client makes, and
/// its `path()` is what an IPC viewer credential is signed over. That is only
/// sound while those paths are the ones this node actually serves: a
/// credential signed over `/member/3` authenticates a read of `/member/3`,
/// and if the IPC side ever named it something else the two transports would
/// have quietly grown separate credential namespaces — with the IPC one
/// verifying against a path no route matches, i.e. authenticating nothing.
///
/// Asserted by asking the router, not by comparing two lists of strings.
#[tokio::test]
async fn every_read_path_names_a_route_this_node_serves() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let reads = [
        super::Read::Network,
        super::Read::Members,
        super::Read::Member(0),
        super::Read::Contracts,
        super::Read::Params,
        super::Read::Proposals,
        super::Read::Pending(0),
        super::Read::PendingByKey("00".repeat(32)),
        super::Read::Whois("00".repeat(32)),
        super::Read::TxOutcome("00".repeat(32)),
    ];
    for read in reads {
        let path = read.path();
        let r = client.get(format!("http://127.0.0.1:{port}{path}")).send().await.expect("get");
        assert_ne!(r.status(), reqwest::StatusCode::NOT_FOUND, "{path} is not a route this node serves");
        assert_ne!(r.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED, "{path} is not served as a GET");
    }
}

// --- authenticated reads and sessions ---------------------------------------

/// Authenticated reads, step 1 (the paper's §Implementation
/// a validly signed header set resolves; an absent header set
/// still reads fine (anonymous, unchanged from today); a claimed-but-invalid
/// one (bad sig / unknown member / stale timestamp) is a hard 401 — none of
/// this changes what `/members` returns (that's step 2), only whether the
/// request is accepted at all.
#[tokio::test]
async fn signed_header_reads_are_resolved_or_rejected_by_validity() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();
    let ts = now_secs();
    let url = format!("http://127.0.0.1:{port}/members");

    let signed_get = |headers: Vec<(&'static str, String)>| {
        let mut req = client.get(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        req
    };

    // No viewer headers at all: today's anonymous read, unchanged.
    let r = client.get(&url).send().await.expect("get");
    assert_eq!(r.status(), reqwest::StatusCode::OK, "an anonymous read must keep working exactly as before");

    // A validly signed request (member 0's dev seed) resolves.
    let r = signed_get(viewer_headers(0, 0, "GET", "/members", ts))
        .send()
        .await
        .expect("get");
    assert_eq!(r.status(), reqwest::StatusCode::OK, "a validly signed viewer read must be accepted");

    // Bad signature: signed by member 1's key but CLAIMED as member 0.
    let r = signed_get(viewer_headers(0, 1, "GET", "/members", ts))
        .send()
        .await
        .expect("get");
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED, "a signature by the wrong key must be rejected");

    // Unknown member id (99 was never admitted).
    let r = signed_get(viewer_headers(99, 0, "GET", "/members", ts))
        .send()
        .await
        .expect("get");
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED, "an unknown member id must be rejected");

    // Stale timestamp: signed a long time ago.
    let r = signed_get(viewer_headers(0, 0, "GET", "/members", ts.saturating_sub(3600)))
        .send()
        .await
        .expect("get");
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED, "a stale timestamp must be rejected");

    // A signature over a different path must not authenticate this one
    // (the path is inside the signed payload — a captured `/member/3` auth
    // must not replay against `/members`).
    let r = signed_get(viewer_headers(0, 0, "GET", "/member/3", ts))
        .send()
        .await
        .expect("get");
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED, "a signature over a different path must not verify here");
}

/// `POST /session` mints a bearer token from a valid signed-request-style
/// signed request; that token then authenticates subsequent reads as a
/// cheap `Authorization: Bearer` header instead of a fresh signature each
/// time. An invalid mint request is rejected the same way a direct read
/// would be; an unknown/garbage bearer token never authenticates.
#[tokio::test]
async fn session_token_mints_from_a_signed_request_and_authenticates_reads() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();
    let ts = now_secs();

    // An invalid mint request (bad signature) is rejected outright.
    let mut req = client.post(format!("http://127.0.0.1:{port}/session"));
    for (k, v) in viewer_headers(0, 1, "POST", "/session", ts) {
        req = req.header(k, v);
    }
    let r = req.send().await.expect("post");
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED, "an invalid signed mint request must be rejected");

    // A valid mint request for member 0.
    let mut req = client.post(format!("http://127.0.0.1:{port}/session"));
    for (k, v) in viewer_headers(0, 0, "POST", "/session", ts) {
        req = req.header(k, v);
    }
    let r = req.send().await.expect("post");
    assert_eq!(r.status(), reqwest::StatusCode::OK, "a validly signed mint request must succeed");
    let body: serde_json::Value = r.json().await.expect("json");
    assert_eq!(body["member_id"].as_u64(), Some(0));
    let token = body["token"].as_str().expect("token present").to_string();
    assert!(!token.is_empty());
    assert!(body["expires_secs"].as_u64().unwrap_or(0) > ts, "expiry must be in the future");

    // The minted token authenticates a subsequent read as a bearer token.
    let r = client
        .get(format!("http://127.0.0.1:{port}/members"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("get");
    assert_eq!(r.status(), reqwest::StatusCode::OK, "a freshly minted session token must authenticate a read");

    // A garbage bearer token never authenticates.
    let r = client
        .get(format!("http://127.0.0.1:{port}/members"))
        .header("authorization", "Bearer not-a-real-token")
        .send()
        .await
        .expect("get");
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED, "an unknown bearer token must be rejected");
}

// --- read-surface rate limiting and view truncation --------------------

/// A genesis of `n` members with no relation to the dev-seed convention —
/// enough to exercise `/members`' truncation without the u8 ceiling
/// `dev_genesis`'s `members: u8` parameter would impose. No transaction ever
/// signs against these members, so the fake key/attestation bytes never need
/// to be real Ed25519 material.
fn many_members_genesis(n: u32) -> edet_state::State {
    let mut st = edet_state::State::default();
    for i in 0..n {
        let mut key = [0u8; 32];
        key[..4].copy_from_slice(&i.to_le_bytes());
        let mut attestation = [0u8; 32];
        attestation[4..8].copy_from_slice(&i.to_le_bytes());
        let _ = st.add_underwriter(vec![key], 25_000.0);
    }
    st
}

#[tokio::test]
async fn read_burst_past_capacity_gets_429_but_a_different_source_ip_still_succeeds() {
    use std::net::{IpAddr, Ipv4Addr};

    let (_node, port) = start_solo(dev_genesis(5)).await;

    // Two clients, bound to two distinct loopback source addresses (the
    // whole 127.0.0.0/8 block is loopback on Linux) — this is what makes
    // `ConnectInfo` see them as different callers, the same mechanism
    // `read_rate_limit` keys on.
    let client_a = reqwest::Client::builder()
        .local_address(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)))
        .build()
        .expect("client a");
    let client_b = reqwest::Client::builder()
        .local_address(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)))
        .build()
        .expect("client b");

    // Burst well past READ_BUCKET_CAPACITY (120) from client A alone.
    let mut saw_429 = false;
    for _ in 0..150 {
        let r = client_a
            .get(format!("http://127.0.0.1:{port}/members"))
            .send()
            .await
            .expect("get");
        if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            saw_429 = true;
            break;
        }
    }
    assert!(saw_429, "a burst past READ_BUCKET_CAPACITY on /members must eventually be throttled");

    // Client B, a different source IP, has its own unspent bucket and is
    // unaffected by A's burst.
    let r = client_b
        .get(format!("http://127.0.0.1:{port}/members"))
        .send()
        .await
        .expect("get");
    assert_eq!(
        r.status(),
        reqwest::StatusCode::OK,
        "a different source IP must not be throttled by another IP's burst"
    );
}

#[tokio::test]
async fn health_is_never_rate_limited() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    // Well past READ_BUCKET_CAPACITY: every single call must still be OK.
    for _ in 0..150 {
        let r = client.get(format!("http://127.0.0.1:{port}/health")).send().await.expect("get");
        assert_eq!(r.status(), reqwest::StatusCode::OK, "/health must never be rate-limited, even under a flood");
    }
}

#[tokio::test]
async fn members_view_truncates_at_max_view_items_with_total_and_flag() {
    let n = (super::views::MAX_VIEW_ITEMS + 50) as u32;
    let (_node, port) = start_solo(many_members_genesis(n)).await;
    let client = reqwest::Client::new();

    let r: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/members"))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    assert!(r.is_object(), "a truncated response becomes an object carrying the array, not a bare array");
    let arr = r["members"].as_array().expect("members array present under the object");
    assert_eq!(arr.len(), super::views::MAX_VIEW_ITEMS, "must cap at exactly MAX_VIEW_ITEMS entries");
    assert_eq!(r["truncated"], true);
    assert_eq!(r["total"].as_u64(), Some(n as u64), "total must report the FULL member count, not the capped one");
}

#[tokio::test]
async fn members_view_stays_a_bare_untruncated_array_under_the_cap() {
    // Today's shape (a bare JSON array, no sibling fields) must be
    // byte-for-byte unchanged for any community under MAX_VIEW_ITEMS.
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let r: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/members"))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    assert!(r.is_array(), "under the cap, /members must stay a bare array exactly as before");
    assert_eq!(r.as_array().unwrap().len(), 5);
}

/// **A listing is walkable, and the walk is exact.** 1,200 members over three
/// pages: every id exactly once, in order, and the last page says there is no
/// next.
///
/// Mutation that bites: make `Page::range` start at `Included(after)` instead
/// of `Excluded(after)`. Every page then repeats its predecessor's last row and
/// the union has duplicates — the defect an offset walk has structurally, and
/// the reason the cursor is an id rather than a count.
#[tokio::test]
async fn members_view_pages_past_the_cap_with_a_cursor() {
    let cap = super::views::MAX_VIEW_ITEMS;
    let n = (cap * 2 + 200) as u32;
    let (_node, port) = start_solo(many_members_genesis(n)).await;
    let client = reqwest::Client::new();

    let mut seen: Vec<u64> = Vec::new();
    let mut after: Option<u64> = None;
    let mut pages = 0;
    loop {
        let url = match after {
            None => format!("http://127.0.0.1:{port}/members?limit={cap}"),
            Some(a) => format!("http://127.0.0.1:{port}/members?after={a}&limit={cap}"),
        };
        let r: serde_json::Value = client.get(url).send().await.expect("get").json().await.expect("json");
        let rows = r["members"].as_array().expect("a paged listing carries the envelope").clone();
        assert_eq!(r["total"].as_u64(), Some(n as u64), "total is the whole set, never the page");
        seen.extend(rows.iter().map(|m| m["id"].as_u64().expect("id")));
        pages += 1;
        match r.get("next").and_then(|v| v.as_u64()) {
            Some(next) => {
                assert_eq!(rows.len(), cap, "only the last page may be short");
                assert_eq!(Some(next), rows.last().and_then(|m| m["id"].as_u64()), "next is the last id SERVED");
                after = Some(next);
            }
            None => {
                assert_eq!(rows.len(), 200, "the last page holds the remainder");
                break;
            }
        }
        assert!(pages < 10, "the walk must terminate");
    }
    assert_eq!(pages, 3);
    let mut expected: Vec<u64> = (0..n as u64).collect();
    expected.sort_unstable();
    assert_eq!(seen, expected, "the union of the pages is every id exactly once, in order");
}

/// A cursor past the last row is not an error: it is an empty page, which is
/// what a client polling for new rows sends.
#[tokio::test]
async fn a_cursor_past_the_end_serves_an_empty_page() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/members?after=9999"))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    assert_eq!(r["members"].as_array().map(Vec::len), Some(0));
    assert!(r.get("next").is_none(), "an empty page has no next");
    assert_eq!(r["total"].as_u64(), Some(5), "total still reports the whole set");
}

/// The cap is the node's, not the caller's: `?limit=10000` serves
/// `MAX_VIEW_ITEMS`, because the price of this endpoint is set against that
/// number.
#[tokio::test]
async fn a_limit_above_the_cap_is_clamped() {
    let cap = super::views::MAX_VIEW_ITEMS;
    let (_node, port) = start_solo(many_members_genesis((cap + 50) as u32)).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/members?limit=10000"))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    assert_eq!(r["members"].as_array().map(Vec::len), Some(cap));
    assert_eq!(r["next"].as_u64(), Some(cap as u64 - 1));
}

/// The same envelope on the other listing, so a client walks both the same
/// way.
#[tokio::test]
async fn contracts_view_pages_with_a_cursor() {
    let (_node, port) = start_solo(contracts_genesis(7)).await;
    let client = reqwest::Client::new();

    let first: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/contracts?limit=3"))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    let ids: Vec<u64> = first["contracts"]
        .as_array()
        .expect("envelope")
        .iter()
        .map(|c| c["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, vec![0, 1, 2]);
    assert_eq!(first["next"].as_u64(), Some(2));
    assert_eq!(first["total"].as_u64(), Some(7));

    let last: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/contracts?after=5&limit=3"))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    let ids: Vec<u64> = last["contracts"]
        .as_array()
        .expect("envelope")
        .iter()
        .map(|c| c["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, vec![6]);
    assert!(last.get("next").is_none(), "the last page says so");
}

/// A community of `n` obligations between two founders, for the listing
/// probes. Booked straight onto the state: what these measure is the VIEW.
fn contracts_genesis(n: u64) -> edet_state::State {
    let mut st = edet_state::State::default();
    let a = st.add_underwriter(vec![[1u8; 32]], 25_000.0).expect("underwriter");
    let b = st.add_underwriter(vec![[2u8; 32]], 25_000.0).expect("underwriter");
    for i in 0..n {
        st.contracts.insert(
            i,
            edet_state::types::Contract {
                id: i,
                debtor: b,
                creditor: a,
                outstanding: 1_000,
                original: 1_000,
                maturity_epoch: 100,
                status: edet_state::types::ContractStatus::Active,
                created_epoch: 0,
                accepted_epoch: 0,
                insured: false,
                held: Default::default(),
                arb: None,
                arb_attestations: Default::default(),
                arb_awarded: false,
            },
        );
    }
    st.next_contract = n;
    st
}

// --- the credential is single-use inside its window -------------------------

/// **A viewer credential read off the wire is worthless.** The same headers
/// twice are 200 then 401: the signature verified both times, and the second
/// presentation is a replay.
///
/// Mutation that bites: drop the `seen.admit` call in `verify_signed_headers`.
/// Both reads then answer 200 and a captured credential is good for the rest
/// of its 60-second window.
#[tokio::test]
async fn a_replayed_viewer_signature_is_refused_inside_its_window() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let headers = viewer_headers(0, 0, "GET", "/members", now_secs());
    let send = || {
        let mut req = client.get(format!("http://127.0.0.1:{port}/members"));
        for (k, v) in &headers {
            req = req.header(*k, v);
        }
        req.send()
    };
    assert_eq!(send().await.expect("get").status(), reqwest::StatusCode::OK);
    let second = send().await.expect("get");
    assert_eq!(second.status(), reqwest::StatusCode::UNAUTHORIZED, "the same credential a second time is a replay");
    assert!(second.text().await.expect("body").contains("replayed"), "and the refusal says which one it is");
}

/// The honest half of the same rule. **Ed25519 is deterministic**, so two
/// reads of one path in one second would carry the identical signature — and
/// the cache above would refuse the second, which is a wallet that stops
/// working. The nonce is what separates them.
///
/// Mutation that bites: drop the nonce from `viewer_auth_message` (and from
/// the helper), and this fails on the second read.
#[tokio::test]
async fn two_reads_in_one_second_carry_different_signatures() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let ts = now_secs();
    let a = viewer_headers(0, 0, "GET", "/members", ts);
    let b = viewer_headers(0, 0, "GET", "/members", ts);
    let sig = |h: &Vec<(&'static str, String)>| {
        h.iter()
            .find(|(k, _)| *k == "x-edet-viewer-sig")
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_ne!(sig(&a), sig(&b), "a fresh nonce is what makes two identical requests two credentials");

    for headers in [&a, &b] {
        let mut req = client.get(format!("http://127.0.0.1:{port}/members"));
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        assert_eq!(req.send().await.expect("get").status(), reqwest::StatusCode::OK, "both honest reads are served");
    }
}

/// A credential with no nonce is malformed rather than accepted — otherwise
/// the version bump would be optional and an old client would keep minting
/// replayable credentials.
#[tokio::test]
async fn a_credential_without_a_nonce_is_refused() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    for nonce in ["", "not-hex-at-all-not-hex-at-all-xx", "00"] {
        let headers = viewer_headers_with(0, 0, "GET", "/members", now_secs(), nonce);
        let mut req = client.get(format!("http://127.0.0.1:{port}/members"));
        for (k, v) in &headers {
            if k == &"x-edet-viewer-nonce" && nonce.is_empty() {
                continue;
            }
            req = req.header(*k, v);
        }
        assert_eq!(
            req.send().await.expect("get").status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "a nonce of {nonce:?} must not authenticate"
        );
    }
}

/// `/session` mints a bearer token from one signed request, so its credential
/// has to be single-use too — otherwise a captured `POST /session` is worth a
/// whole session TTL rather than the skew window.
#[tokio::test]
async fn a_session_mint_is_single_use_per_signature() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let headers = viewer_headers(0, 0, "POST", "/session", now_secs());
    let send = || {
        let mut req = client.post(format!("http://127.0.0.1:{port}/session"));
        for (k, v) in &headers {
            req = req.header(*k, v);
        }
        req.send()
    };
    let first = send().await.expect("post");
    assert_eq!(first.status(), reqwest::StatusCode::OK, "the first mint succeeds");
    assert_eq!(
        send().await.expect("post").status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "and the same signed request cannot mint a second token"
    );
}

// --- attacks the read surface has to refuse ---------------------------------
//
// Each test below is an attack that succeeded at `131d767` and must now be
// refused. See the defect history in `git log`.

/// H-11: `/tx/check` dry-runs `apply` against real state, and `apply` returns
/// `ET-CAP-001` exactly when the amount exceeds the debtor's headroom — so the
/// reply code was a one-bit probe of a private figure, binary-searchable to
/// the last decimal by an anonymous caller in ~60 queries. The bond quote is
/// genuinely public and still returned; the outcome now needs a party.
#[tokio::test]
async fn tx_check_does_not_disclose_an_outcome_to_an_anonymous_caller() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let tx = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 1_000_000.0,
        maturity_epochs: 30,
        arb: None,
    };
    let stx = SignedTx {
        tx,
        nonce: next_nonce(),
        not_after_epoch: TEST_NOT_AFTER_EPOCH,
        signers: vec![pubkey_of(&dev_seed(0)), pubkey_of(&dev_seed(1))],
        signatures: vec![],
    };
    let anon = post_json(&client, port, "/tx/check", &serde_json::to_value(&stx).expect("encode")).await;
    assert!(anon.get("ok").is_none(), "no verdict for an anonymous caller: {anon}");
    assert!(anon.get("code").is_none(), "and no rejection code — that IS the oracle: {anon}");
    assert!(anon["bond"]["amount"].as_f64().is_some(), "the bond quote stays public");

    // A party pre-flighting its own draft is unaffected — the whole purpose.
    let ts = now_secs();
    let mut req = client.post(format!("http://127.0.0.1:{port}/tx/check")).json(&stx);
    for (k, v) in viewer_headers(0, 0, "POST", "/tx/check", ts) {
        req = req.header(k, v);
    }
    let party: serde_json::Value = req.send().await.expect("post").json().await.expect("json");
    assert!(party.get("ok").is_some(), "a party still gets its verdict: {party}");
}

/// H-12: `contract_view` correctly drops `debtor`/`creditor` for a non-party,
/// and was then placed inside arrays named `owes`/`owed` — so the position of
/// an entry disclosed the edge the redaction had removed. Enumerating
/// `/member/:id` anonymously reconstructed the whole debt graph.
#[tokio::test]
async fn an_anonymous_member_read_does_not_disclose_the_debt_graph() {
    let (listener, port) = free_listener();
    let mut genesis = dev_genesis(5);
    // One edge, booked directly: this file is the consensus-FREE half of the
    // surface, and what is under test is who may read an edge, not how one
    // comes to exist (`tests/malachite_http.rs` covers that against a real
    // engine). Written through `apply` so the caches and the contract book
    // agree — the commit path audits the invariants now.
    // Member 2 has carried and paid a debt to member 0, so member 0 has real
    // standing behind them — the only way capacity ever comes to exist.
    genesis.place_stake(2, 0, 5_000.0);
    edet_state::apply(
        &mut genesis,
        Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        },
        [7u8; 32],
        TEST_NOT_AFTER_EPOCH,
        &[pubkey_of(&dev_seed(0)), pubkey_of(&dev_seed(1))],
        0,
    )
    .expect("book one obligation");
    let _node = start_on(listener, solo_cfg(port), genesis).await;
    let client = reqwest::Client::new();

    let anon = get_json(&client, port, "/member/0").await;
    assert_eq!(anon["owes"].as_array().map(|a| a.len()), Some(0), "an anonymous caller sees no edges at all: {anon}");

    // The debtor sees their own row.
    let ts = now_secs();
    let mut req = client.get(format!("http://127.0.0.1:{port}/member/0"));
    for (k, v) in viewer_headers(0, 0, "GET", "/member/0", ts) {
        req = req.header(k, v);
    }
    let mine: serde_json::Value = req.send().await.expect("get").json().await.expect("json");
    assert_eq!(mine["owes"].as_array().map(|a| a.len()), Some(1), "a party still sees its own: {mine}");
}

/// M-17: §Standing's public row is id/address/status. Publishing keys to anyone is
/// what let an anonymous caller name a victim as a claimed signer, which is
/// the primitive H-11's oracle was built on.
#[tokio::test]
async fn public_keys_are_not_published_to_an_anonymous_caller() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let anon = get_json(&client, port, "/members").await;
    let first = anon.as_array().and_then(|a| a.first()).cloned().expect("a member row");
    assert!(first.get("keys").is_none(), "keys are not public: {first}");
    assert!(first.get("id").is_some() && first.get("status").is_some(), "the public row still is");

    let ts = now_secs();
    let mut req = client.get(format!("http://127.0.0.1:{port}/members"));
    for (k, v) in viewer_headers(0, 0, "GET", "/members", ts) {
        req = req.header(k, v);
    }
    let seen: serde_json::Value = req.send().await.expect("get").json().await.expect("json");
    let first = seen.as_array().and_then(|a| a.first()).cloned().expect("a member row");
    assert!(first.get("keys").is_some(), "an authenticated member still resolves counterparty keys");
}

/// M-16: `/tx` verified one signature per CLAIMED signer before any rate
/// limit, with `signers` caller-supplied and unbounded.
#[tokio::test]
async fn an_envelope_claiming_absurdly_many_signers_is_refused_before_verifying_them() {
    let (node, _port) = start_solo(dev_genesis(5)).await;
    let tx = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 30.0,
        maturity_epochs: 30,
        arb: None,
    };
    let stx = SignedTx {
        tx,
        nonce: next_nonce(),
        not_after_epoch: TEST_NOT_AFTER_EPOCH,
        signers: vec![pubkey_of(&dev_seed(0)); super::http::MAX_SIGNERS + 1],
        signatures: vec![vec![0u8; 64]; super::http::MAX_SIGNERS + 1],
    };
    assert!(!super::driver::submit(&node, stx).await, "refused on the claimed count alone");
}

/// P-4: a member can obtain evidence, and it verifies against the root the
/// same reply carries. Without this the member-held-commitment mitigation for a
/// `>2/3` federation rewriting history — member-held commitments — has no
/// mechanism behind it.
#[tokio::test]
async fn a_member_can_obtain_a_proof_of_its_own_record_and_verify_it() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let ts = now_secs();
    let mut req = client.get(format!("http://127.0.0.1:{port}/proof/member/0"));
    for (k, v) in viewer_headers(0, 0, "GET", "/proof/member/0", ts) {
        req = req.header(k, v);
    }
    let body: serde_json::Value = req.send().await.expect("get").json().await.expect("json");
    assert!(body.get("error").is_none(), "a member may prove its own record: {body}");

    // The root travels WITH the proof, so a client never pairs a proof with a
    // root it raced against.
    let root_hex = body["app_hash"].as_str().expect("the reply carries its root").to_string();
    let root = crate::block::unhex32(&root_hex).expect("32-byte root");

    let proof: edet_state::root::InclusionProof =
        serde_json::from_value(rehydrate_proof(&body["proof"])).expect("decode the wire proof");
    assert!(edet_state::root::verify(&root, &proof), "the proof must verify against the root it was served with");

    // And it is evidence about a specific ledger: tamper with the value and
    // the same root refuses it.
    let mut tampered = proof.clone();
    tampered.value[0] ^= 0xFF;
    assert!(!edet_state::root::verify(&root, &tampered), "a tampered leaf must not verify");
}

/// The leaf IS the record — amounts unbucketed, counterparties named — so an
/// ungated proof would hand back through a side door exactly what
/// `/member/:id` and `contract_view` withhold (H-11, H-12).
#[tokio::test]
async fn a_proof_is_refused_to_anyone_who_may_not_read_the_record() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let anon = get_json(&client, port, "/proof/member/0").await;
    assert!(anon.get("error").is_some(), "anonymous callers get no proof: {anon}");
    assert!(anon.get("proof").is_none(), "and nothing that could be verified into one");

    // A member is not entitled to a proof of SOMEONE ELSE's record, even
    // though they are a member in good standing. `dev_genesis` makes every
    // founder a validator, so member 4 is dropped to a plain member first —
    // otherwise the blanket validator visibility would make this vacuous.
    let mut genesis = dev_genesis(5);
    genesis.validators.remove(&4);
    let (_node2, port) = start_solo(genesis).await;
    let ts = now_secs();
    let mut req = client.get(format!("http://127.0.0.1:{port}/proof/member/0"));
    for (k, v) in viewer_headers(4, 4, "GET", "/proof/member/0", ts) {
        req = req.header(k, v);
    }
    let other: serde_json::Value = req.send().await.expect("get").json().await.expect("json");
    assert!(other.get("error").is_some(), "a non-validator member may not prove another's record: {other}");
}

/// The wire encoding is hex; `InclusionProof`'s serde shape is byte arrays.
/// The client verifies from the hex directly (`ui/src/lib/proof.ts`); this
/// test only needs to get back to the Rust type to reuse `root::verify`.
fn rehydrate_proof(wire: &serde_json::Value) -> serde_json::Value {
    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .filter_map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
            .collect()
    }
    let bytes = |h: &serde_json::Value| {
        serde_json::Value::Array(
            unhex(h.as_str().unwrap_or_default())
                .into_iter()
                .map(|b| serde_json::json!(b))
                .collect(),
        )
    };
    let hashes = |a: &serde_json::Value| {
        serde_json::Value::Array(a.as_array().cloned().unwrap_or_default().iter().map(bytes).collect())
    };
    let section = match wire["section"].as_str().unwrap_or_default() {
        "members" => "Members",
        "contracts" => "Contracts",
        "proposals" => "Proposals",
        "validators" => "Validators",
        _ => "Ledger",
    };
    serde_json::json!({
        "section": section,
        "index": wire["index"],
        "leaf_count": wire["leaf_count"],
        "key": bytes(&wire["key"]),
        "value": bytes(&wire["value"]),
        "leaf_salt": bytes(&wire["leaf_salt"]),
        "path": hashes(&wire["path"]),
        "section_path": hashes(&wire["section_path"]),
    })
}

/// Every origin a client can actually have must be allowed through CORS.
///
/// **The probe that was missing when the desktop and mobile app stopped
/// embedding a node.** While the app read its own node over IPC, this layer
/// only had to serve dev UIs and nothing checked it. The app is an ordinary
/// cross-origin browser client now, and its origin is not `localhost:5173`:
/// wry serves it from `tauri://localhost` on Linux and macOS, and from
/// `http://tauri.localhost` on Android with `useHttpsScheme` at its default.
/// The allowlist carried only the `https://` form, which is the one Android
/// does NOT use — read off a device, where `location.href` is
/// `http://tauri.localhost/`.
///
/// This fails closed in the worst way: the browser refuses to send the
/// request at all, so the app reports an unreachable node against one that is
/// answering every other client perfectly well, and no log on either side
/// names the origin. Hence a test rather than a comment.
#[tokio::test]
async fn every_client_origin_is_allowed_through_cors() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    for origin in [
        // Browser dev server (`just dev`).
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        // The app, on every platform wry serves it from.
        "tauri://localhost",
        "http://tauri.localhost",
        "https://tauri.localhost",
    ] {
        let res = client
            .request(reqwest::Method::OPTIONS, format!("http://127.0.0.1:{port}/network"))
            .header("Origin", origin)
            .header("Access-Control-Request-Method", "GET")
            .header("Access-Control-Request-Headers", "authorization")
            .send()
            .await
            .expect("preflight");
        let allowed = res
            .headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap_or_default().to_string());
        assert_eq!(
            allowed.as_deref(),
            Some(origin),
            "preflight from {origin} was not allowed — that client cannot read anything at all"
        );
    }

    // Non-vacuity: the allowlist is an ALLOWLIST. If this ever passes, the
    // layer has gone permissive and the assertions above prove nothing.
    let res = client
        .request(reqwest::Method::OPTIONS, format!("http://127.0.0.1:{port}/network"))
        .header("Origin", "https://not-a-client.example")
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await
        .expect("preflight");
    assert!(
        res.headers().get("access-control-allow-origin").is_none(),
        "an unlisted origin was allowed — the allowlist is not one"
    );
}

/// **One read must not hold the node lock for long.**
///
/// Every read handler takes the node lock, and it is the same lock a commit
/// needs — so whatever one read costs is what a commit can be made to wait
/// for. A members listing answers a max-flow per member it serves, up to
/// `MAX_VIEW_ITEMS` of them: measured at 0.3 s, 2.3 s and 5.0 s per read at
/// 1,000, 5,000 and 10,000 accounts. At those figures a single source at the
/// allowed read rate keeps the lock permanently and the validator misses its
/// rounds; behind a NAT or a reverse proxy every member shares one bucket, so
/// it happens without an attacker too.
///
/// Three bounds are under test together, because each alone leaves the hole
/// open: an anonymous caller gets no capacity and so costs no queries; the
/// node's cache answers every read whose cut inputs have not moved; and a read
/// that does compute is bounded and charged for it.
///
/// **The assertion is on the WORK a read does, not on how long it took.** A
/// millisecond figure measures the machine: this asserted `< 250 ms` per read
/// and per commit, which is four times the margin on a runner already slow
/// enough to have gone red elsewhere, and CLAUDE.md's own rule is that a
/// measurement on this box is a ratio and never a figure. What makes the read
/// short is that the queries are BOUNDED — an anonymous caller costs none, a
/// cached cut costs none, and a cold read pays at most
/// `MAX_COLD_CAPACITY_PER_READ` of them — and every one of those three is a
/// count this test can read straight out of the response, at any speed, on any
/// box. The seconds belong to `just cost`, where measurements live.
#[tokio::test]
async fn one_read_does_not_hold_the_node_lock_for_long() {
    // Big enough that an unbounded pass over `MAX_VIEW_ITEMS` members is
    // measurably slow, and small enough to build in a unit test.
    let members = 1_500usize;
    let mut genesis = edet_state::State::default();
    for i in 0..members {
        let mut key = [0u8; 32];
        key[0..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
        // Member 0 carries a dev key, so the reads below can authenticate as
        // it. Capacity is served to an authenticated viewer and to nobody
        // else, which is the first of the three bounds — without a viewer
        // this probe would only ever exercise that one.
        let key = if i == 0 { pubkey_of(&dev_seed(0)) } else { key };
        if i < 8 {
            genesis.add_underwriter(vec![key], 25_000.0).expect("founder");
        } else {
            genesis.new_account(vec![key]);
        }
    }
    // A graph with something to solve: every ordinary member is staked on by a
    // founder, so a capacity query is a real max-flow rather than an immediate
    // zero.
    for i in 8..members {
        genesis.edges.insert((i % 8, i), 500_000);
    }
    // A port of this file's own, like every other test here: these bind fixed
    // loopback ports so a collision fails loudly rather than flaking.
    //
    // `trust_forwarded_for`, so each read below can carry a source of its own.
    // `/members` costs `MAX_COLD_CAPACITY_PER_READ` tokens of the per-IP read
    // budget, and three reads a block over twelve blocks is more than that
    // budget holds — the probe would be answered 429 by a limiter it is not
    // about, at a moment decided by how fast the loop ran. A source per read
    // takes that limiter out of the picture entirely; nothing else reads the
    // header, `reader_ip` is the rate limiter's alone.
    let (listener, port) = free_listener();
    let mut config = solo_cfg(port);
    config.trust_forwarded_for = true;
    let node = start_on(listener, config, genesis).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // Rows of one page carrying a capacity at all: none for an anonymous
    // caller, and for a viewer the cached ones plus what this read paid for.
    // 1,500 members is more than one page, so the view answers the truncated
    // envelope rather than a bare list — the same two shapes `members_finish`
    // patches capacities into.
    let served = |v: &serde_json::Value| -> usize {
        let rows = match v {
            serde_json::Value::Array(rows) => rows,
            serde_json::Value::Object(map) => map["members"].as_array().expect("a page of members"),
            other => panic!("neither a list of members nor a page of them: {other}"),
        };
        rows.iter().filter(|row| row.get("capacity").is_some()).count()
    };

    // What each read in turn served, in order, starting from nothing served.
    let mut sequence = vec![0usize];
    for h in 1..=12u64 {
        // A block between the reads: the shape of the attack the bounds are
        // for, which is a member trading once a block and reading from a few
        // addresses. (An EMPTY block moves none of the cut's inputs, so the
        // cache legitimately still answers across it — that is bound two doing
        // its job, and the sequence below shows it: what one read computed,
        // the next one is served for free.)
        {
            let mut core = node.lock();
            let block =
                crate::block::Block { height: h, time_secs: h * 2, app_hash: core.replica.app_hash(), txs: Vec::new() };
            core.replica.commit_block_unchecked(&block).expect("commit");
        }

        // ONE. An anonymous flood costs no queries whatever, because it is
        // served no capacity to compute.
        let anon = client
            .get(format!("{base}/members"))
            .header("x-forwarded-for", format!("10.1.{h}.0"))
            .send()
            .await
            .expect("read");
        assert!(anon.status().is_success(), "the read has to succeed to have cost anything: {}", anon.status());
        let anon: serde_json::Value = anon.json().await.expect("json");
        assert_eq!(served(&anon), 0, "an anonymous read must serve no capacity at all, and so cost no max-flow");

        // A fresh credential per request: the same one twice is a replay, and
        // `viewer_headers` mints a new nonce on every call.
        let read_as_member_0 = |source: String| {
            let mut req = client.get(format!("{base}/members")).header("x-forwarded-for", source);
            for (k, v) in viewer_headers(0, 0, "GET", "/members", now_secs()) {
                req = req.header(k, v);
            }
            req.send()
        };
        for source in [format!("10.1.{h}.1"), format!("10.1.{h}.2")] {
            let res = read_as_member_0(source).await.expect("read");
            assert!(res.status().is_success(), "the read has to succeed to have cost anything: {}", res.status());
            sequence.push(served(&res.json::<serde_json::Value>().await.expect("json")));
        }
    }

    // TWO and THREE, over the whole sequence. A read may only serve MORE than
    // the one before it — what one computed, the cache answers for the next —
    // and it may serve at most `MAX_COLD_CAPACITY_PER_READ` more, out of the
    // ~490 rows of this page still waiting for a max-flow. That ceiling is
    // what bounds the lock, and it is a count: true at any speed, on any box.
    for (i, pair) in sequence.windows(2).enumerate() {
        let (before, after) = (pair[0], pair[1]);
        // Strictly more, not merely no fewer: ~490 rows of this page are still
        // waiting for a max-flow after all 24 reads, so every read has a full
        // budget's worth left to spend. A cache that answered nothing would
        // recompute the same first rows for ever and hold this flat.
        assert!(
            after > before,
            "read {i} served no more than the read before it ({before}, then {after}): the cache is not \
             answering what an earlier read computed, so every read pays for the same rows again"
        );
        assert!(
            after - before <= super::core::MAX_COLD_CAPACITY_PER_READ,
            "read {i} served {} capacities more than the read before it, over its budget of {} — one read is \
             computing max-flows without a bound, and a commit waits behind every one of them",
            after - before,
            super::core::MAX_COLD_CAPACITY_PER_READ
        );
    }

    // The strict growth above is also what keeps the ceiling from being
    // vacuous: a `/members` serving no capacity to anybody would hold the
    // sequence at zero and fail on its first window.
}

// --- a halted node answers nothing but its halt ------------------------------

/// **A node whose audit refused a block serves that refusal, not the ledger.**
///
/// A block applies in place, so a refused one leaves the ledger the invariants
/// said cannot exist in memory, and the engine's answer — returning out of
/// `run` — takes a moment during which this surface is still bound. Every
/// route answers 503 with the height it stopped at and why, including the
/// health check: a node that has stopped is not healthy, and a client's right
/// move is to read one of the other nodes its network names.
///
/// Mutation that bites: drop the `halt_guard` layer. `/head` answers 200 with
/// the state the audit refused.
#[tokio::test]
async fn a_halted_node_answers_every_route_with_its_halt() {
    // Distinct from every other port in this file: two tests binding one port
    // fail with `AddrInUse` on whichever loses the schedule, which reads as a
    // flake in whatever was changed rather than as the collision it is.
    let (node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    // Healthy first, so the refusal below is about the halt and not about the
    // node never having worked.
    assert!(client
        .get(format!("http://127.0.0.1:{port}/head"))
        .send()
        .await
        .expect("get")
        .status()
        .is_success());

    {
        let mut core = node.lock();
        // The conservation clause, broken by hand: the cached debt no longer
        // agrees with the contract book.
        core.replica.state.members.get_mut(&0).expect("member 0").debt_out = 99_900;
        let block =
            crate::block::Block { height: 1, time_secs: 86_400, app_hash: core.replica.app_hash(), txs: Vec::new() };
        core.replica
            .commit_block_unchecked(&block)
            .expect_err("the audit must refuse it");
        assert!(core.replica.halted().is_some(), "fixture: the replica must have stopped");
    }

    for path in ["/head", "/health", "/members", "/member/0", "/params", "/proof/member/0"] {
        let res = client.get(format!("http://127.0.0.1:{port}{path}")).send().await.expect("get");
        assert_eq!(res.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE, "{path} must answer 503");
        let body: serde_json::Value = res.json().await.expect("json");
        assert_eq!(body["halted_at_height"], 1, "{path} must name the height");
        assert!(body["reason"].as_str().is_some_and(|r| !r.is_empty()), "{path} must name the reason");
    }
}

// --- the seat commitment on /head ------------------------------------------

/// **`/head` says how many rows the seed carries and how many it has
/// seated.** A founder with a supply of 2,500 seats two strangers; the seats
/// hold two bond units of the seed, the ceiling is the seed over the unit,
/// and the room is what is left — the figure an operator reads when nobody
/// can onboard.
///
/// Mutation that bites: serve `committed` (the insured credit) in place of
/// `seat_committed`. Nothing here is insured, so the field reads 0.00
/// against the 40.00 two seats hold.
#[tokio::test]
async fn head_reports_the_seat_commitment_against_the_seed() {
    use edet_swarm::driver::Driver;
    use edet_swarm::keys::{fresh_key, member_key};

    let mut st = edet_state::State::default();
    let founder = st
        .add_underwriter(vec![member_key(0)], 2_500.0)
        .expect("a founding underwriter");
    let mut d = Driver::new(st);
    for n in 0..2 {
        let k = fresh_key(7, n);
        d.apply(
            Tx::Accept {
                debtor: Party::Key(k),
                creditor: Party::Member(founder),
                amount: 1.0,
                maturity_epochs: 30,
                arb: None,
            },
            &[k, member_key(0)],
        )
        .expect("a founder seats a stranger");
    }
    let unit = d.st.params.bond_unit();

    let (_node, port) = start_solo(d.st).await;
    let client = reqwest::Client::new();
    let head = get_json(&client, port, "/head").await;
    assert_eq!(head["seed"], 2_500.0);
    assert_eq!(head["seats"], 2);
    assert_eq!(head["seat_committed"], 2.0 * unit);
    assert_eq!(head["seat_ceiling_rows"], (2_500.0 / unit) as u64);
    assert_eq!(head["seat_room_rows"], (2_500.0 / unit) as u64 - 2);
}

/// **A queued transaction returns the hash its outcome is keyed on.** The
/// wallet cannot reproduce `SignedTx::hash()` — it is over the consensus
/// encoding of the signed envelope — so without this a member saw "sent" and
/// never learned that a queued transaction was refused at commit.
#[tokio::test]
async fn a_queued_transaction_returns_the_hash_its_outcome_is_keyed_on() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();
    let tx = Tx::Accept {
        debtor: Party::Member(0),
        creditor: Party::Member(1),
        amount: 30.0,
        maturity_epochs: 30,
        arb: None,
    };
    let good =
        sign_tx(DEV_CHAIN_ID, tx, next_nonce(), TEST_NOT_AFTER_EPOCH, &[dev_seed(0), dev_seed(1)]).expect("sign");
    let res = submit_tx(&client, port, &good).await;
    assert_eq!(res["queued"], true);
    assert_eq!(
        res["hash"].as_str().expect("the hash rides beside `queued`"),
        crate::block::hex32(&good.hash().expect("hash")),
        "the same key `/tx/outcome/:hash` answers under"
    );
}

// --- a key opens its own first purchase on a member's invitation ------------

/// The inviting member's signed invitation, as the seller's wallet mints it
/// beside its "pay me" QR and the buyer's wallet carries it back.
fn invitation(seed_id: u8, not_after_secs: u64, nonce: [u8; 16]) -> serde_json::Value {
    let sk = SigningKey::from_bytes(&dev_seed(seed_id));
    let key = sk.verifying_key().to_bytes();
    let sig = sk.sign(&super::pending::invite_message(DEV_CHAIN_ID, &key, not_after_secs, &nonce));
    serde_json::json!({
        "key": key,
        "not_after_secs": not_after_secs,
        "nonce": nonce,
        "signature": sig.to_bytes().to_vec(),
    })
}

/// A keyed buyer's request to open a purchase from `seller_id`, carrying
/// `invite`. The buyer signs the envelope digest like any co-signer.
fn keyed_open(
    buyer: &SigningKey,
    tx: &Tx,
    nonce: [u8; 16],
    required: serde_json::Value,
    invite: Option<serde_json::Value>,
) -> serde_json::Value {
    let digest = tx_digest(DEV_CHAIN_ID, tx, &nonce, TEST_NOT_AFTER_EPOCH).expect("digest");
    let mut req = serde_json::json!({
        "tx": tx, "nonce": nonce, "not_after_epoch": TEST_NOT_AFTER_EPOCH,
        "required": required, "min_sigs": 2,
        "signer": buyer.verifying_key().to_bytes(),
        "signature": buyer.sign(&digest).to_bytes().to_vec(),
    });
    if let Some(inv) = invite {
        req["invite"] = inv;
    }
    req
}

fn purchase_from(buyer_key: [u8; 32], seller: Party, amount: f64) -> Tx {
    Tx::Sale { seller, buyer: Party::Key(buyer_key), amount, maturity_epochs: 30 }
}

/// **The newcomer's first purchase reaches the seller's inbox through the
/// node**, on the seller's invitation, and the seller completes it from
/// there like any other request.
///
/// This is the pool path `ui/src/lib/offer.ts` tries before falling back to
/// a code shown by hand: the invitation is what lets a key — which cannot be
/// charged for occupancy — open an entry charged to the member it names.
#[tokio::test]
async fn an_invited_key_opens_its_first_purchase_and_the_seller_signs_it_from_their_queue() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();

    let newcomer = SigningKey::from_bytes(&[0xBC; 32]);
    let newcomer_key = newcomer.verifying_key().to_bytes();
    let invite = invitation(0, now_secs() + 3600, next_nonce());
    let tx = purchase_from(newcomer_key, Party::Member(0), 400.0);
    let nonce = next_nonce();
    let required = serde_json::json!([{ "Member": 0u64 }, { "Key": newcomer_key }]);

    let r =
        post_json(&client, port, "/pending/sign", &keyed_open(&newcomer, &tx, nonce, required.clone(), Some(invite)))
            .await;
    assert_eq!(r["ok"], true, "an invited key opens its purchase: {r}");
    assert_eq!(r["completed"], false);

    // The seller finds it in their own queue, opened by the key, charged to them.
    let ts = now_secs();
    let mut get = client.get(format!("http://127.0.0.1:{port}/pending/0"));
    for (k, v) in viewer_headers(0, 0, "GET", "/pending/0", ts) {
        get = get.header(k, v);
    }
    let queue: serde_json::Value = get.send().await.expect("get").json().await.expect("json");
    let awaiting = queue["awaiting_me"].as_array().cloned().unwrap_or_default();
    assert_eq!(awaiting.len(), 1, "the seller's queue must hold it: {queue}");
    assert_eq!(awaiting[0]["initiator"], 0, "occupancy is the inviter's");
    assert_eq!(awaiting[0]["opener"], serde_json::json!({ "Key": newcomer_key }), "and the inbox says who opened it");

    // The seller co-signs from that queue; the envelope is assembled and accepted.
    let digest = tx_digest(DEV_CHAIN_ID, &tx, &nonce, TEST_NOT_AFTER_EPOCH).expect("digest");
    let sk0 = SigningKey::from_bytes(&dev_seed(0));
    let cosign = serde_json::json!({
        "tx": tx, "nonce": nonce, "not_after_epoch": TEST_NOT_AFTER_EPOCH,
        "required": required, "min_sigs": 2,
        "signer": sk0.verifying_key().to_bytes(),
        "signature": sk0.sign(&digest).to_bytes().to_vec(),
    });
    let r = post_json(&client, port, "/pending/sign", &cosign).await;
    assert_eq!(r["ok"], true, "the seller completes it: {r}");
    assert_eq!(r["completed"], true);
    assert_eq!(r["queued"], true, "and the verified ingress accepts the assembled envelope");
}

/// What an invitation does NOT open: anything but a purchase from the
/// inviter, by the key it was handed to, inside its window — and no more than
/// `MAX_INVITED_PER_MEMBER` at once, so a captured invitation is bounded to a
/// few visible junk requests in one inbox.
#[tokio::test]
async fn an_invitation_opens_only_a_bounded_number_of_purchases_from_the_inviter() {
    let (_node, port) = start_solo(dev_genesis(5)).await;
    let client = reqwest::Client::new();
    let newcomer = SigningKey::from_bytes(&[0xBD; 32]);
    let key = newcomer.verifying_key().to_bytes();
    let req_for = |seller: u64| serde_json::json!([{ "Member": seller }, { "Key": key }]);
    let refused = |r: &serde_json::Value, why: &str| assert_eq!(r["ok"], false, "{why}: {r}");

    // Expired.
    let tx = purchase_from(key, Party::Member(0), 10.0);
    let r = post_json(
        &client,
        port,
        "/pending/sign",
        &keyed_open(&newcomer, &tx, next_nonce(), req_for(0), Some(invitation(0, now_secs() - 1, next_nonce()))),
    )
    .await;
    refused(&r, "an expired invitation opens nothing");

    // Signed by a key that is nobody's.
    let stranger = SigningKey::from_bytes(&[0xBE; 32]);
    let stranger_key = stranger.verifying_key().to_bytes();
    let n = next_nonce();
    let bad = serde_json::json!({
        "key": stranger_key, "not_after_secs": now_secs() + 3600, "nonce": n,
        "signature": stranger.sign(&super::pending::invite_message(DEV_CHAIN_ID, &stranger_key, now_secs() + 3600, &n)).to_bytes().to_vec(),
    });
    let r = post_json(&client, port, "/pending/sign", &keyed_open(&newcomer, &tx, next_nonce(), req_for(0), Some(bad)))
        .await;
    refused(&r, "an invitation from a non-member opens nothing");

    // Member 1's invitation does not open a purchase from member 0.
    let r = post_json(
        &client,
        port,
        "/pending/sign",
        &keyed_open(&newcomer, &tx, next_nonce(), req_for(0), Some(invitation(1, now_secs() + 3600, next_nonce()))),
    )
    .await;
    refused(&r, "an invitation names its own seller");

    // A tampered invitation (expiry moved) fails its signature.
    let mut moved = invitation(0, now_secs() + 3600, next_nonce());
    moved["not_after_secs"] = serde_json::json!(now_secs() + 7200);
    let r =
        post_json(&client, port, "/pending/sign", &keyed_open(&newcomer, &tx, next_nonce(), req_for(0), Some(moved)))
            .await;
    refused(&r, "an altered invitation fails its signature");

    // Not a purchase FROM the inviter: the key as the seller.
    let selling = Tx::Sale { seller: Party::Key(key), buyer: Party::Member(0), amount: 10.0, maturity_epochs: 30 };
    let r = post_json(
        &client,
        port,
        "/pending/sign",
        &keyed_open(
            &newcomer,
            &selling,
            next_nonce(),
            req_for(0),
            Some(invitation(0, now_secs() + 3600, next_nonce())),
        ),
    )
    .await;
    refused(&r, "an invitation is to buy from the inviter, not to sell to them");

    // Not a purchase at all.
    let other = Tx::Settle { contract: 0, amount: 1.0 };
    let r = post_json(
        &client,
        port,
        "/pending/sign",
        &keyed_open(&newcomer, &other, next_nonce(), req_for(0), Some(invitation(0, now_secs() + 3600, next_nonce()))),
    )
    .await;
    refused(&r, "an invitation opens purchases only");

    // A third required party is not what the inviter agreed to.
    let three = serde_json::json!([{ "Member": 0u64 }, { "Key": key }, { "Member": 2u64 }]);
    let r = post_json(
        &client,
        port,
        "/pending/sign",
        &keyed_open(&newcomer, &tx, next_nonce(), three, Some(invitation(0, now_secs() + 3600, next_nonce()))),
    )
    .await;
    refused(&r, "an invitation names exactly two parties");

    // Bounded: one invitation, reused, opens at most MAX_INVITED_PER_MEMBER
    // entries for the inviter at once; the next is refused while they stand.
    let invite = invitation(0, now_secs() + 3600, next_nonce());
    for i in 0..super::pending::MAX_INVITED_PER_MEMBER {
        let tx = purchase_from(key, Party::Member(0), 20.0 + i as f64);
        let r = post_json(
            &client,
            port,
            "/pending/sign",
            &keyed_open(&newcomer, &tx, next_nonce(), req_for(0), Some(invite.clone())),
        )
        .await;
        assert_eq!(r["ok"], true, "purchase {i} within the cap: {r}");
    }
    let tx = purchase_from(key, Party::Member(0), 99.0);
    let r =
        post_json(&client, port, "/pending/sign", &keyed_open(&newcomer, &tx, next_nonce(), req_for(0), Some(invite)))
            .await;
    refused(&r, "the cap on open invitations holds");
}
