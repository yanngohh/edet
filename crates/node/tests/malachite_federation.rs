//! **A real genesis, booted.** Four operators mint their own consensus keys, an
//! author writes a genesis with a real chain id, each operator writes their node
//! home, and three of the four — exactly quorum, so every commit carries all of
//! them — agree on a transaction signed with a founder's MEMBER key.
//!
//! Every other engine test in this crate boots `write_testnet`'s homes, which
//! name edet's own published dev keys, the published state-root salt and the
//! reserved `edet-dev` chain id — so three gates that exist for real chains
//! were never once exercised by anything that boots:
//!
//!   * the **dev-key tripwire**, which refuses a published key off the harness
//!     chain, in `genesis init` and again at first boot;
//!   * the **state-root salt**, drawn from the OS CSPRNG by `genesis init` and
//!     required by `genesis_state` on any chain but the harness one — every
//!     validator must hold the SAME value or they compute different roots and
//!     cannot agree, and nothing had ever carried a generated one through a
//!     real commit;
//!   * the **ceremony tooling** itself (`keygen`, `init`), which is what an
//!     operator runs instead of hand-writing `config.toml` and
//!     `priv_validator_key.json`.
//!
//! Real OS processes over loopback, like `malachite_cluster.rs`, and node 0
//! also serves the client API so the claim can be "the ledger holds this row"
//! rather than only "the hashes match".
//!
//!   cargo test -p edet-node --features malachite --test malachite_federation \
//!     -- --nocapture
//!
//! Consensus base port 27000, deliberately clear of the loopback testnet's
//! 26600, `malachite_byzantine`'s 26800 and Fedora's `passim` at 27500.
#![cfg(feature = "malachite")]

use std::path::Path;
use std::time::Duration;

use edet_node::block::{counter_nonce, hex32, pubkey_of, sign_tx, unhex32};
use edet_node::engine_node::peer_id_of_public;
use edet_state::tx::Tx;
use edet_state::types::Party;

mod common;
use common::{bin, observe, run_cli, run_cli_out, scratch, spawn_with, Budget};

const CHAIN_ID: &str = "edet-federation-test";
const NODES: usize = 4;
const CONSENSUS_BASE: u16 = 27000;
const CLIENT_PORT: u16 = 7441;
const METRICS_BASE: u16 = 29900;
/// The founders' MEMBER keys. Not `block::dev_seed`: those are published, and
/// `genesis init` refuses a published key on a chain that is not the harness
/// one — which is one of the things this test is here to walk through.
fn member_seed(i: usize) -> [u8; 32] {
    let mut s = [0xA0u8; 32];
    s[0] = i as u8;
    s
}

/// The `keygen` line that carries the public half: the command prints a
/// sentence, then the hex on a line of its own, then the instruction. Reading
/// the 64-hex line rather than a fixed offset keeps the test from pinning the
/// prose.
fn public_hex(stdout: &str) -> String {
    stdout
        .lines()
        .map(str::trim)
        .find(|l| l.len() == 64 && l.chars().all(|c| c.is_ascii_hexdigit()))
        .expect("keygen prints the public half as a 64-hex line")
        .to_string()
}

#[tokio::test]
async fn a_federation_founded_by_the_ceremony_commits_and_agrees() {
    let home = scratch("malachite-federation");
    std::fs::create_dir_all(&home).expect("scratch home");
    let genesis_file = home.join("genesis.json");

    // 1. Every operator mints their own consensus key and reads off the
    //    public half. The private half never leaves the file.
    let mut consensus_hex = Vec::new();
    for i in 0..NODES {
        let key_file = home.join(format!("key{i}.json"));
        let out = run_cli_out(&["malachite", "keygen", "--out", key_file.to_str().unwrap()]);
        assert!(!out.contains("seed"), "keygen must never print the private half: {out}");
        consensus_hex.push(public_hex(&out));
        // The peer id it prints is the one the genesis's public key derives
        // to, or the entry an operator copies from it never matches a dial.
        let derived = peer_id_of_public(&unhex32(&consensus_hex[i]).expect("64 hex"))
            .expect("a key")
            .to_string();
        assert!(out.lines().any(|l| l.trim() == derived), "keygen prints the peer id {derived}: {out}");
    }

    // 2. The author writes the genesis. A real chain id, so the dev-key
    //    tripwire is live and a fresh root salt is drawn; four powered
    //    validators, which is the BFT floor `genesis init` enforces.
    let mut args: Vec<String> = vec![
        "genesis".into(),
        "init".into(),
        "--chain-id".into(),
        CHAIN_ID.into(),
        "--out".into(),
        genesis_file.to_str().unwrap().into(),
    ];
    for (i, ck) in consensus_hex.iter().enumerate() {
        args.push("--validator".into());
        args.push(format!("{i}:{}:{ck}:1", hex32(&pubkey_of(&member_seed(i)))));
    }
    // Two founding underwriters, so the community has a seed at all — with
    // none every capacity is zero for ever, and the members signing below
    // would hold no write allowance either.
    args.push("--underwriter".into());
    args.push("0:5000".into());
    args.push("--underwriter".into());
    args.push("1:5000".into());
    run_cli(&args.iter().map(String::as_str).collect::<Vec<_>>());

    let genesis: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&genesis_file).expect("genesis written")).expect("genesis json");
    assert!(!genesis["root_salt"].is_null(), "a real chain must carry its own state-root salt");

    // 3. Every operator writes their home from that genesis and their own key.
    //    Nothing here is hand-written and nothing is copied from a testnet.
    for i in 0..NODES {
        let peers: Vec<String> = (0..NODES)
            .filter(|&j| j != i)
            .map(|j| {
                let id = peer_id_of_public(&unhex32(&consensus_hex[j]).expect("64 hex")).expect("a key");
                format!("/ip4/127.0.0.1/tcp/{}/p2p/{id}", CONSENSUS_BASE as usize + j)
            })
            .collect();
        run_cli(&[
            "malachite",
            "init",
            "--home",
            home.join(i.to_string()).to_str().unwrap(),
            "--genesis",
            genesis_file.to_str().unwrap(),
            "--key",
            home.join(format!("key{i}.json")).to_str().unwrap(),
            "--listen",
            &format!("/ip4/127.0.0.1/tcp/{}", CONSENSUS_BASE as usize + i),
            "--peers",
            &peers.join(","),
            "--metrics-port",
            &(METRICS_BASE as usize + i).to_string(),
        ]);
    }

    // 4. Boot three of the four. Quorum for four equal-power validators is
    //    `ceil(2*4/3) = 3`, so every commit has to carry all three live nodes
    //    and none of them can quietly fall behind — which is what makes a
    //    height-by-height hash comparison a statement about agreement rather
    //    than about who happened to keep up. Node 0 also serves the client
    //    API, so the claim can be about the ledger and not only the hashes.
    let live: Vec<usize> = (0..NODES - 1).collect();
    let _cluster = spawn_with(&home, &live, |i| {
        if i == 0 {
            vec!["--client-port".to_string(), CLIENT_PORT.to_string()]
        } else {
            Vec::new()
        }
    });

    let client = reqwest::Client::new();
    await_health(&client, CLIENT_PORT, Duration::from_secs(90)).await;
    // A fresh genesis starts at epoch 0 while the wall clock is ~20,000 epochs
    // ahead, and `MAX_EPOCH_ADVANCE_PER_BLOCK` caps catch-up at 10,000 a
    // block — so a validity window computed before this point names an epoch
    // the chain has not reached, the transaction is refused as too far in the
    // future, and the commit never arrives.
    let epoch = await_epoch_settled(&client, CLIENT_PORT, Duration::from_secs(120)).await;

    // 5. A transaction signed with two founders' MEMBER keys against THIS
    //    chain id — the digest binds it, so one minted for the harness chain
    //    is refused here — entering through the client API a wallet uses.
    let tx = Tx::Accept {
        debtor: Party::Member(1),
        creditor: Party::Member(0),
        amount: 10.0,
        maturity_epochs: 30,
        arb: None,
    };
    let signed = sign_tx(
        CHAIN_ID,
        tx,
        counter_nonce(1),
        epoch + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS,
        &[member_seed(0), member_seed(1)],
    )
    .expect("sign with two member keys");
    let hash = hex32(&signed.hash().expect("tx hash"));
    let queued: serde_json::Value = client
        .post(format!("http://127.0.0.1:{CLIENT_PORT}/tx"))
        .json(&signed)
        .send()
        .await
        .expect("POST /tx")
        .json()
        .await
        .expect("POST /tx returned non-JSON");
    assert_eq!(queued["queued"].as_bool(), Some(true), "the ingress refused a well-formed signed Accept: {queued}");
    assert_eq!(
        await_committed(&client, CLIENT_PORT, &hash, Duration::from_secs(90))
            .await
            .as_deref(),
        Some("ok"),
        "the federation never committed the founders' obligation"
    );

    // 6. Every node agrees, and the row is on the book — the part
    //    `status.json` cannot say.
    let seen = observe(&home, &live, 1, Budget::within(120)).unwrap_or_else(|why| {
        panic!("the ceremony-founded validators never reached a height they had all committed: {why}")
    });
    if let Some(split) = seen.disagreement() {
        panic!("the federation disagreed: {split}");
    }
    let common = seen.common_heights();
    assert!(!common.is_empty(), "no height was reported by every live node, so nothing was compared");

    let rows = get_json(&client, CLIENT_PORT, "/contracts").await;
    let rows = rows.as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1, "exactly the one obligation submitted: {rows:?}");
    let row = &rows[0];
    assert_eq!(row["status"].as_str(), Some("active"));
    // Read as an anonymous viewer, which is what an unauthenticated `GET
    // /contracts` is: the parties are withheld from anyone who is not one of
    // them or a validator, so they are not what this asserts.
    assert!(row.get("debtor").is_none(), "an anonymous read must not name the parties: {row}");
    // A founder's first trade is uninsured, and on a fresh genesis every
    // trade is a first trade: founders are given no stakes in one another, so
    // the debtor's capacity is zero until a settlement writes one.
    assert_eq!(row["insured"].as_bool(), Some(false), "nobody has staked on this debtor yet: {row}");

    eprintln!(
        "a federation founded by keygen + genesis init + init reached heights {:?}, agreed at every one of \
         {} shared height(s), and carries the founders' obligation",
        seen.tops(),
        common.len()
    );
    drop(_cluster);
    let _ = std::fs::remove_dir_all(&home);
}

async fn await_health(client: &reqwest::Client, port: u16, timeout: Duration) {
    let start = std::time::Instant::now();
    loop {
        let up = matches!(
            client.get(format!("http://127.0.0.1:{port}/health")).send().await,
            Ok(r) if r.status().is_success()
        );
        if up {
            return;
        }
        assert!(start.elapsed() <= timeout, "the client API on :{port} never came up within {timeout:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn get_json(client: &reqwest::Client, port: u16, path: &str) -> serde_json::Value {
    client
        .get(format!("http://127.0.0.1:{port}{path}"))
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {path} on :{port}: {e}"))
        .json()
        .await
        .unwrap_or_else(|e| panic!("GET {path} on :{port} returned non-JSON: {e}"))
}

/// Wait until the chain's epoch has reached the one wall-clock implies. Not
/// "it looked stable for a moment": the clamp is idempotent within a wall-clock
/// second, so the epoch sits still for ~1 s at a time while still climbing.
async fn await_epoch_settled(client: &reqwest::Client, port: u16, timeout: Duration) -> u64 {
    let want = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall clock after the unix epoch")
        .as_secs()
        / edet_kernel::constants::EPOCH_SECS;
    let start = std::time::Instant::now();
    loop {
        let epoch = get_json(client, port, "/network").await["epoch"].as_u64().unwrap_or(0);
        if epoch >= want {
            return epoch;
        }
        assert!(start.elapsed() <= timeout, "the chain's epoch never reached wall-clock ({epoch} < {want})");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Poll `/tx/outcome/<hash>` until the node reports a COMMIT outcome —
/// `pending` is still only the mempool.
async fn await_committed(client: &reqwest::Client, port: u16, hash: &str, timeout: Duration) -> Option<String> {
    let start = std::time::Instant::now();
    loop {
        let v = get_json(client, port, &format!("/tx/outcome/{hash}")).await;
        match v.get("status").and_then(|s| s.as_str()) {
            Some("ok") => return Some("ok".into()),
            Some("rejected") => return Some(format!("rejected: {}", v.get("code").cloned().unwrap_or_default())),
            _ => {}
        }
        if start.elapsed() > timeout {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// `init` is the operator's last chance to be told the key file is the wrong
/// one — after it, the node asserts at boot and all it can do is panic.
///
/// Driven through the BINARY rather than the library, because a check that
/// only exists in a function nobody calls from the CLI is a check an operator
/// never meets.
#[test]
fn the_cli_refuses_a_home_whose_key_the_genesis_does_not_name() {
    let home = scratch("malachite-federation-wrongkey");
    std::fs::create_dir_all(&home).expect("scratch home");
    let genesis_file = home.join("genesis.json");

    let mut consensus_hex = Vec::new();
    for i in 0..NODES {
        consensus_hex.push(public_hex(&run_cli_out(&[
            "malachite",
            "keygen",
            "--out",
            home.join(format!("key{i}.json")).to_str().unwrap(),
        ])));
    }
    // A fifth key nobody named.
    let stranger = home.join("stranger.json");
    run_cli(&["malachite", "keygen", "--out", stranger.to_str().unwrap()]);

    let mut args: Vec<String> = vec![
        "genesis".into(),
        "init".into(),
        "--chain-id".into(),
        CHAIN_ID.into(),
        "--out".into(),
        genesis_file.to_str().unwrap().into(),
    ];
    for (i, ck) in consensus_hex.iter().enumerate() {
        args.push("--validator".into());
        args.push(format!("{i}:{}:{ck}:1", hex32(&pubkey_of(&member_seed(i)))));
    }
    args.push("--underwriter".into());
    args.push("0:5000".into());
    run_cli(&args.iter().map(String::as_str).collect::<Vec<_>>());

    let out = std::process::Command::new(bin())
        .args([
            "malachite",
            "init",
            "--home",
            home.join("0").to_str().unwrap(),
            "--genesis",
            genesis_file.to_str().unwrap(),
            "--key",
            stranger.to_str().unwrap(),
            "--listen",
            "/ip4/127.0.0.1/tcp/27010",
        ])
        .output()
        .expect("run init");
    assert!(!out.status.success(), "init must refuse a key the genesis does not name");
    let err = String::from_utf8_lossy(&out.stderr) + String::from_utf8_lossy(&out.stdout);
    assert!(err.contains("not one this genesis names"), "the refusal must say what is wrong: {err}");
    assert!(!home.join("0").join("config").exists(), "a refused init writes no home");

    let _ = std::fs::remove_dir_all(&home);
}

/// A home written by `init` is one `resolve_peer_names` accepts and the loaders
/// read — checked without booting anything, so it says so in a second.
#[test]
fn a_ceremony_home_is_readable_without_booting_it() {
    let home = scratch("malachite-federation-readback");
    std::fs::create_dir_all(&home).expect("scratch home");
    let key = home.join("key.json");
    let hex = public_hex(&run_cli_out(&["malachite", "keygen", "--out", key.to_str().unwrap()]));
    let genesis_file = home.join("genesis.json");
    let mut args: Vec<String> = vec![
        "genesis".into(),
        "init".into(),
        "--chain-id".into(),
        CHAIN_ID.into(),
        "--out".into(),
        genesis_file.to_str().unwrap().into(),
    ];
    // Four validators even though only one home is written: the BFT floor is
    // what `genesis init` checks, and this probe boots nothing.
    for i in 0..NODES {
        args.push("--validator".into());
        let ck = if i == 0 { hex.clone() } else { hex32(&pubkey_of(&[0xB0 + i as u8; 32])) };
        args.push(format!("{i}:{}:{ck}:1", hex32(&pubkey_of(&member_seed(i)))));
    }
    args.push("--underwriter".into());
    args.push("0:1000".into());
    run_cli(&args.iter().map(String::as_str).collect::<Vec<_>>());

    let id = |i: usize| peer_id_of_public(&pubkey_of(&[0xB0 + i as u8; 32])).expect("a key");
    let peers = format!("/ip4/127.0.0.1/tcp/27021/p2p/{},/dns4/localhost/tcp/27022/p2p/{}", id(1), id(2));
    run_cli(&[
        "malachite",
        "init",
        "--home",
        home.join("0").to_str().unwrap(),
        "--genesis",
        genesis_file.to_str().unwrap(),
        "--key",
        key.to_str().unwrap(),
        "--listen",
        "/ip4/127.0.0.1/tcp/27020",
        "--peers",
        &peers,
        "--moniker",
        "arno",
    ]);

    let raw = std::fs::read_to_string(home.join("0/config/config.toml")).expect("config written");
    assert!(raw.contains("arno"), "the moniker is the operator's: {raw}");
    assert!(
        raw.contains(&format!("/dns4/localhost/tcp/27022/p2p/{}", id(2))),
        "a named peer is written as given: {raw}"
    );

    let config = load_home(&home.join("0"));
    // The name is resolved, and the node dials exactly the peers above its own id.
    let own = peer_id_of_public(&unhex32(&hex).expect("64 hex")).expect("a key");
    let want: Vec<String> = [(1usize, 27021u16), (2, 27022)]
        .into_iter()
        .filter(|&(i, _)| id(i) > own)
        .map(|(i, port)| format!("/ip4/127.0.0.1/tcp/{port}/p2p/{}", id(i)))
        .collect();
    let got: Vec<String> = config.consensus.p2p.persistent_peers.iter().map(|p| p.to_string()).collect();
    assert_eq!(got, want, "resolved literals, one side of each pair");
    assert!(!config.consensus.p2p.discovery.enabled);

    let _ = std::fs::remove_dir_all(&home);
}

fn load_home(home: &Path) -> edet_node::engine_node::EdetConfig {
    use malachitebft_app_channel::app::node::Node;
    edet_node::engine_node::EdetApp::at(home.to_path_buf(), None, None)
        .load_config()
        .expect("config loads")
}
