//! **Conformance**: the same scenario, three times, and the three must agree.
//!
//! The swarm drives `State` in process, because a search that needs a hundred
//! thousand ticks cannot pay an OS process for each. That is a claim about
//! cost, and it silently assumes a second thing — that in-process application
//! and a real node's committed blocks are the same transition function. This
//! is where that assumption is checked, once, on one scenario, rather than
//! assumed by every run in the crate.
//!
//! Three plays of `edet_swarm::scenario::conformance()`:
//!
//!   - **the node**, over HTTP, one transaction per block, each outcome
//!     awaited before the next is submitted, so the committed order is the
//!     scenario's;
//!   - **path A**, the driver replaying the node's own envelopes at the node's
//!     own block times, which proves `Driver` is `Replica::apply_block_to` —
//!     bit-identically, since the state commitment it ends on must equal the
//!     `app_hash` the node published;
//!   - **path B**, the scenario through the swarm's own adapter at one fixed
//!     time inside one epoch, which proves the scenario's semantics do not
//!     depend on transaction ids or on where a block boundary fell. That is
//!     what lets every other run in the crate stay in process.
//!
//! Two steps are refused at a different DOOR on the two sides and the test
//! says so where it maps them. An envelope that authorises nothing never
//! reaches a block, because `deterministic_validity` and the submit screen
//! refuse it at the ingress — where in process it is `ET-MEM-003` with the id
//! released. An envelope naming an id the ledger has not issued is refused by
//! the same screen (`names_issued_ids`), with the code `apply` would have
//! given it — in process, the crank on contract 9,999 is `ET-CTR-001`. Same
//! verdict, different door, both times.
//!
//! **Ports**: client 7471, consensus 28921, metrics 29571. Outside 26600,
//! 26700, 26800, 27000, 27200, 27240, 27500 (Fedora's `passim`), 28911–28914,
//! 29500 and 7401–7461, all of which other harnesses in this crate bind. Check
//! `ss -ltn` before reading a red here as a code fault.
#![cfg(feature = "malachite")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use edet_node::block::{counter_nonce, dev_seed, hex32, pubkey_of, sign_tx, state_hash, SignedTx, DEV_CHAIN_ID};
use edet_node::engine_node::{genesis_state, write_solo_home_on, EdetGenesis};
use edet_node::store::Store;
use edet_state::types::Key;
use edet_swarm::driver::{Audit, Driver};
use edet_swarm::intent::Outcome;

mod common;
use common::{await_epoch_settled, await_health};

const CLIENT_PORT: u16 = 7471;
const CONSENSUS_PORT: usize = 28921;
const METRICS_PORT: &str = "29571";
/// Six dev members: one validator and five more, every one an underwriter at
/// `DEV_SUPPLY`.
const EXTRA_MEMBERS: usize = 5;

/// Liveness budgets, for the reason `malachite_http` gives: a loaded box
/// multiplies every consensus round, and a timeout must say "consensus did not
/// make progress" rather than "the machine was busy".
const CATCHUP_BUDGET: Duration = Duration::from_secs(240);
const COMMIT_BUDGET: Duration = Duration::from_secs(60);

struct Node(Option<Child>);

impl Node {
    fn kill(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The dev roster: every member's key and the seed that signs for it.
fn roster() -> Vec<(Key, [u8; 32])> {
    (0..=EXTRA_MEMBERS)
        .map(|i| (pubkey_of(&dev_seed(i as u8)), dev_seed(i as u8)))
        .collect()
}

fn seed_for(key: &Key) -> [u8; 32] {
    roster()
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, s)| s)
        .expect("a signer outside the dev roster")
}

async fn post_tx(client: &reqwest::Client, stx: &SignedTx) -> serde_json::Value {
    client
        .post(format!("http://127.0.0.1:{CLIENT_PORT}/tx"))
        .json(stx)
        .send()
        .await
        .expect("POST /tx")
        .json()
        .await
        .expect("POST /tx returned non-JSON")
}

/// Poll `/tx/outcome/<hash>` until the node reports a COMMIT outcome.
async fn await_outcome(client: &reqwest::Client, hash: &str) -> String {
    let start = Instant::now();
    loop {
        let v: serde_json::Value = client
            .get(format!("http://127.0.0.1:{CLIENT_PORT}/tx/outcome/{hash}"))
            .send()
            .await
            .expect("GET /tx/outcome")
            .json()
            .await
            .expect("GET /tx/outcome returned non-JSON");
        match v.get("status").and_then(|s| s.as_str()) {
            Some("ok") => return "ok".into(),
            Some("rejected") => return format!("rejected: {}", v.get("code").and_then(|c| c.as_str()).unwrap_or("?")),
            _ => {}
        }
        assert!(start.elapsed() <= COMMIT_BUDGET, "no committed outcome for {hash} within {COMMIT_BUDGET:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The outcome as this crate names it, so a node's answer and the driver's
/// are comparable at all.
fn as_node_says(o: &Outcome) -> String {
    match o {
        Outcome::Applied => "ok".into(),
        Outcome::Refused(c) => format!("rejected: {c}"),
        other => panic!("a scripted step cannot be {other:?} — a script IS the consent"),
    }
}

#[tokio::test]
async fn the_driver_and_a_real_node_apply_the_same_scenario_the_same_way() {
    let home = std::env::temp_dir().join(format!("edet-malachite-swarm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    write_solo_home_on(&home, EXTRA_MEMBERS, CONSENSUS_PORT).expect("write the solo node home");

    // The solo layout puts `config/` and `edet-store/` directly under the
    // home, so there is no `--index`.
    let log = std::fs::File::create(home.join("node.log")).expect("a log to read a failure out of");
    let mut node = Node(Some(
        Command::new(PathBuf::from(env!("CARGO_BIN_EXE_edet-node")))
            .args(["malachite", "--home", home.to_str().unwrap()])
            .args(["--client-port", &CLIENT_PORT.to_string()])
            .env("EDET_METRICS_BASE_PORT", METRICS_PORT)
            .stdout(Stdio::from(log.try_clone().expect("clone the log handle")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn `edet-node malachite`"),
    ));

    let client = reqwest::Client::new();
    await_health(&client, &[CLIENT_PORT], Duration::from_secs(30)).await;
    // **Before any window is computed.** A fresh chain climbs ten thousand
    // epochs a block until its clock catches up, and a window computed before
    // that is already expired when the proposer includes it.
    let epoch = await_epoch_settled(&client, CLIENT_PORT, CATCHUP_BUDGET).await;
    let not_after = epoch + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS;

    // --- the node -----------------------------------------------------------
    //
    // The scenario is resolved by the SWARM's own adapter against the state
    // the node is about to see, then signed with the dev seeds. That is the
    // point: if the adapter built a different envelope from the one the driver
    // applies below, the two sides would disagree for a reason that is about
    // neither.
    let steps = edet_swarm::scenario::conformance(epoch);
    let genesis: EdetGenesis =
        serde_json::from_str(&std::fs::read_to_string(home.join("config/genesis.json")).expect("genesis.json"))
            .expect("genesis.json parses");
    let mut shadow = Driver::new(genesis_state(&genesis).expect("the node's own genesis")).audit_mode(Audit::Off);
    let mut world = edet_swarm::population::World::with_agents(shadow.st.members.len());
    for id in shadow.st.members.keys() {
        world.claim(*id, *id as usize);
    }
    // **Catch the shadow up to the chain's epoch before composing anything.**
    // A fresh state climbs at most `MAX_EPOCH_ADVANCE_PER_BLOCK` epochs per
    // block — the structural backstop against an unbounded future timestamp —
    // so one `begin_block` at wall clock leaves it thousands of epochs short,
    // and every window computed against it is then `ET-TX-003`. The node pays
    // the same catch-up; this is the shadow paying it.
    let secs = epoch * edet_kernel::constants::EPOCH_SECS;
    for _ in 0..64 {
        if shadow.st.epoch >= epoch {
            break;
        }
        // The clamp is per BLOCK, and a repeated `begin_block` at one wall
        // clock is idempotent by design, so the catch-up has to advance the
        // CLOCK in steps — which is what a chain committing empty blocks does.
        let next = (shadow.st.epoch + edet_kernel::constants::MAX_EPOCH_ADVANCE_PER_BLOCK).min(epoch);
        shadow.begin(next * edet_kernel::constants::EPOCH_SECS);
    }
    assert_eq!(shadow.st.epoch, epoch, "the shadow never reached the chain's epoch");

    let mut submitted: Vec<(String, String)> = Vec::new();
    let mut node_says: Vec<String> = Vec::new();
    for (n, step) in steps.iter().enumerate() {
        let composed = edet_swarm::intent::compose(&shadow.st, &world, step.actor, &step.intent);
        let mut signers = composed.signers.clone();
        signers.extend(composed.asks.iter().map(|a| a.key));
        signers.sort_by_key(|k| shadow.st.member_of_key(k).unwrap_or(u64::MAX));
        signers.dedup();
        let seeds: Vec<[u8; 32]> = signers.iter().map(seed_for).collect();
        let stx = sign_tx(DEV_CHAIN_ID, composed.tx.clone(), counter_nonce(n as u64), not_after, &seeds)
            .expect("sign the step");
        let hash = hex32(&stx.hash().expect("tx hash"));

        let reply = post_tx(&client, &stx).await;
        if reply.get("queued").and_then(|q| q.as_bool()) == Some(false) {
            // **Refused at the INGRESS**, by one of two screens, and these are
            // the steps where the two sides are MAPPED rather than compared.
            // An envelope naming an id the ledger has not issued is kept out of
            // the mempool by `names_issued_ids`, the function the node ran,
            // asked here of the shadow the node's state is mirrored in — the
            // code it answers is the one `apply` gives the same envelope in
            // process. Otherwise the envelope authorised nothing:
            // `edet_state::authorises` is a query-free function of committed
            // state, applied at the ingress, in the pre-vote screen and at
            // commit, and in process the same envelope is `ET-MEM-003` with
            // its id released. Same verdict, different door.
            node_says.push(match edet_state::apply::names_issued_ids(&shadow.st, &composed.tx) {
                Err(e) => format!("rejected: {}", e.0),
                Ok(()) => "rejected: ET-MEM-003".into(),
            });
            // The shadow has to move with the node, and the node did nothing.
            continue;
        }
        node_says.push(await_outcome(&client, &hash).await);
        submitted.push((hash, node_says.last().cloned().unwrap_or_default()));
        // Keep the shadow in step, so the NEXT envelope is composed against
        // the state the node holds. Its own ids, from its own counter, at one
        // fixed time inside the settled epoch — none of which leaves this
        // process.
        let id = shadow.tx_id();
        let _ = shadow.apply_at(composed.tx, id, not_after, &signers, secs);
    }

    let head: serde_json::Value = client
        .get(format!("http://127.0.0.1:{CLIENT_PORT}/head"))
        .send()
        .await
        .expect("GET /head")
        .json()
        .await
        .expect("GET /head returned non-JSON");
    let height = head["height"].as_u64().expect("a committed height");
    let app_hash_at_read = head["app_hash"].as_str().expect("a state commitment").to_string();
    assert!(height > 0, "the node committed nothing at all");
    node.kill();

    // --- path A: the driver IS `apply_block_to` -----------------------------
    //
    // **Held to the block's own `app_hash`, not to a live `/head`.** An empty
    // block is paced to one a second and the node keeps committing them, so a
    // hash read over HTTP is a hash at a height that has already moved by the
    // time the store is opened. A block's `app_hash` is the commitment left by
    // its PARENT — the only claim a proposal can carry, and the same
    // convention every validator votes on — so replaying every block below the
    // top and landing on the top block's own field is the same assertion with
    // no race in it.
    let mut store = Store::open(home.join("edet-store")).expect("the node's own store");
    let top = store.max_block_height().expect("the node committed nothing at all");
    assert!(top >= height, "the store must hold at least what /head reported ({top} < {height})");

    let mut d = Driver::new(genesis_state(&genesis).expect("genesis")).audit_mode(Audit::EveryTransition);
    let mut replayed: Vec<(String, String)> = Vec::new();
    let mut parent_commitment = String::new();
    for h in 1..=top {
        let block = store
            .find_block(h)
            .expect("read a committed block")
            .expect("a gap in the committed chain");
        if h == top {
            parent_commitment = hex32(&state_hash(&d.st).expect("state hash"));
            assert_eq!(
                parent_commitment,
                hex32(&block.app_hash),
                "the driver applying the node's own blocks must land on the commitment the node's next block carries"
            );
        }
        d.begin(block.time_secs);
        for stx in &block.txs {
            let id = stx.id(DEV_CHAIN_ID).expect("the id the node applied under");
            let hash = hex32(&stx.hash().expect("tx hash"));
            let out = d.apply_at(stx.tx.clone(), id, stx.not_after_epoch, &stx.signers, block.time_secs);
            replayed.push((
                hash,
                match out {
                    Ok(()) => "ok".into(),
                    Err(e) => format!("rejected: {}", e.0),
                },
            ));
        }
    }
    assert_eq!(
        replayed, submitted,
        "the driver must reach the node's own verdict on the node's own envelopes, in the node's own order"
    );
    assert_ne!(parent_commitment, app_hash_at_read, "nothing was checked if the two reads are the same string");

    // --- path B: the scenario does not depend on ids or on block times ------
    let (_, in_process) = edet_swarm::scenario::play(genesis_state(&genesis).expect("genesis"), &steps, epoch);
    let mapped: Vec<String> = in_process.iter().map(as_node_says).collect();
    assert_eq!(
        mapped, node_says,
        "the scenario's per-step outcomes must not depend on transaction ids or on where a block boundary fell"
    );

    let _ = std::fs::remove_dir_all(&home);
}
