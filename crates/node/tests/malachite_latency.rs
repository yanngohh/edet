//! **A federation a wide area apart.** Every other figure in this crate comes
//! from validators a loopback apart, and latency between real validators is
//! the term none of them contains. Here each validator reaches its peers
//! through a relay in this process that delays every byte by a fixed one-way
//! latency, so the same four processes commit across a wire that behaves
//! like a continent.
//!
//! The relay is also the link forwarder partition testing needs. Given one
//! relay per ORDERED pair of validators, every link is a switch of its own: a
//! partition is a validator's relays going dark — the connections through
//! them close, and a new one is dropped at the door — and healing is the
//! same relays forwarding again, after which upstream's re-dial brings the
//! link back with nobody restarted.
//!
//! Three probes. The ordinary two, in `just engine-test`, assert liveness at
//! 50 ms one way under the stall rule of `common::Budget`, and that a
//! partition heals. The measurement,
//! `#[ignore]`d and run by `just latency`, prints the block interval at 0,
//! 50 and 150 ms one way — a RATIO to read in one sitting, never a figure to
//! quote from another day. Both count the blocks a fresh chain commits while
//! its epoch catches up to the clock, which are unpaced, so what they measure
//! is the consensus round trips and not the one-second pacing of an empty
//! block on a live chain.
//!
//! Consensus base ports 27600, 27640 and 27680, relays at 27700, 27740 and
//! 27800–27815, clear of every other harness and of Fedora's `passim` at
//! 27500.
#![cfg(feature = "malachite")]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod common;
use common::{await_height, heights, observe, read_status, run_cli_env, scratch, spawn_env, Budget};

/// A relay that accepts on `listen`, connects each accepted stream to
/// `target`, and forwards every chunk in both directions `delay` after it
/// was read — constant latency, unbounded throughput, order preserved.
///
/// `up` is the link: while it is false a new connection is dropped at the
/// door and every connection through the relay closes at its next chunk,
/// which on a consensus link is within the second.
async fn relay(listen: u16, target: u16, delay: Duration, up: Arc<AtomicBool>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", listen))
        .await
        .expect("bind the relay");
    loop {
        let Ok((inbound, _)) = listener.accept().await else { continue };
        if !up.load(Ordering::SeqCst) {
            drop(inbound);
            continue;
        }
        let Ok(outbound) = tokio::net::TcpStream::connect(("127.0.0.1", target)).await else { continue };
        let (ri, wi) = inbound.into_split();
        let (ro, wo) = outbound.into_split();
        for (mut from, mut to) in [(ri, wo), (ro, wi)] {
            let up = up.clone();
            tokio::spawn(async move {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
                let link = up.clone();
                let writer = tokio::spawn(async move {
                    while let Some((due, bytes)) = rx.recv().await {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
                        if !link.load(Ordering::SeqCst) || to.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                });
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match from.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if !up.load(Ordering::SeqCst)
                                || tx.send((Instant::now() + delay, buf[..n].to_vec())).is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                drop(tx);
                let _ = writer.await;
            });
        }
    }
}

/// A link that is always up.
fn always_up() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(true))
}

/// Point every node's `persistent_peers` at its peers' relays rather than at
/// the peers themselves.
/// The peer id testnet node `j` dials with, which every peer entry names
/// so that `load_config` can say which side of the pair dials.
fn dev_peer_id(j: usize) -> multiaddr::PeerId {
    edet_node::engine_node::peer_id_of_seed(&edet_node::block::dev_consensus_seed(j as u8))
}

fn route_through_relays(home: &Path, nodes: usize, relay_base: u16) {
    for i in 0..nodes {
        let path = home.join(i.to_string()).join("config").join("config.toml");
        let raw = std::fs::read_to_string(&path).expect("the node's config.toml");
        let start = raw.find("persistent_peers = [").expect("a persistent_peers list");
        let end = start + raw[start..].find(']').expect("the list closes") + 1;
        let peers: Vec<String> = (0..nodes)
            .filter(|&j| j != i)
            .map(|j| format!("\"/ip4/127.0.0.1/tcp/{}/p2p/{}\"", relay_base as usize + j, dev_peer_id(j)))
            .collect();
        let out = format!("{}persistent_peers = [{}]{}", &raw[..start], peers.join(", "), &raw[end..]);
        std::fs::write(&path, out).expect("rewrite config.toml");
    }
}

/// A four-validator testnet whose every link carries `delay` one way; the
/// seconds per block it commits over `blocks` blocks after its first three.
fn interval_at(name: &str, consensus_base: u16, relay_base: u16, delay: Duration, blocks: u64) -> Result<f64, String> {
    let home = scratch(name);
    let env = [("EDET_CONSENSUS_BASE_PORT", consensus_base.to_string())];
    run_cli_env(&["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "4"], &env);
    route_through_relays(&home, 4, relay_base);

    let rt = tokio::runtime::Runtime::new().expect("a runtime for the relays");
    for i in 0..4u16 {
        rt.spawn(relay(relay_base + i, consensus_base + i, delay, always_up()));
    }
    let live: Vec<usize> = (0..4).collect();
    let cluster = spawn_env(&home, &live, |_| Vec::new(), &[]);
    await_height(&home, &live, 3, Budget::within(180)).map_err(|why| format!("never reached height 3: {why}"))?;
    let started = Instant::now();
    await_height(&home, &live, 3 + blocks, Budget::within(600))
        .map_err(|why| format!("stopped short of {} blocks: {why} (at {:?})", blocks, heights(&home, &live)))?;
    let per_block = started.elapsed().as_secs_f64() / blocks as f64;
    drop(cluster);
    rt.shutdown_background();
    let _ = std::fs::remove_dir_all(&home);
    Ok(per_block)
}

/// The relay for the link `from → to`: one per ordered pair, so that every
/// link can be cut on its own.
fn pair_port(relay_base: u16, nodes: u16, from: u16, to: u16) -> u16 {
    relay_base + from * nodes + to
}

/// Point every node's `persistent_peers` at its own relay toward each peer.
fn route_through_pair_relays(home: &Path, nodes: u16, relay_base: u16) {
    for i in 0..nodes {
        let path = home.join(i.to_string()).join("config").join("config.toml");
        let raw = std::fs::read_to_string(&path).expect("the node's config.toml");
        let start = raw.find("persistent_peers = [").expect("a persistent_peers list");
        let end = start + raw[start..].find(']').expect("the list closes") + 1;
        let peers: Vec<String> = (0..nodes)
            .filter(|&j| j != i)
            .map(|j| {
                format!("\"/ip4/127.0.0.1/tcp/{}/p2p/{}\"", pair_port(relay_base, nodes, i, j), dev_peer_id(j as usize))
            })
            .collect();
        let out = format!("{}persistent_peers = [{}]{}", &raw[..start], peers.join(", "), &raw[end..]);
        std::fs::write(&path, out).expect("rewrite config.toml");
    }
}

fn height_of(home: &Path, index: usize) -> u64 {
    read_status(home, index).map(|s| s.height).unwrap_or(0)
}

/// **A partition heals when the link returns.** Every link between two
/// validators runs through a relay of its own. The fourth validator's six
/// links go dark: the three others keep committing — three of four is
/// quorum, which is the whole of what "tolerates one down" means — and the
/// fourth commits nothing, cut off from every peer. The links return, and
/// the fourth catches up with nobody restarted: upstream's timer dials the
/// configured addresses again, from whichever side of each pair the rule
/// gives the dial, and value sync serves what it missed. Then every node
/// agrees at every height they share.
///
/// Mutation that bites: pin Malachite back to `v0.5.0`, whose network
/// repairs a dropped connection only under discovery. The links return and
/// nobody calls; the fourth stays at the height it was cut at.
#[test]
fn a_partition_heals_when_the_link_returns() {
    const NODES: u16 = 4;
    const CONSENSUS: u16 = 27680;
    const RELAYS: u16 = 27800;
    let home = scratch("malachite-partition");
    let env = [("EDET_CONSENSUS_BASE_PORT", CONSENSUS.to_string())];
    run_cli_env(&["malachite", "testnet", "--home", home.to_str().unwrap(), "--nodes", "4"], &env);
    route_through_pair_relays(&home, NODES, RELAYS);

    let rt = tokio::runtime::Runtime::new().expect("a runtime for the relays");
    let mut links: Vec<Vec<Arc<AtomicBool>>> = Vec::new();
    for from in 0..NODES {
        let mut row = Vec::new();
        for to in 0..NODES {
            let up = always_up();
            if from != to {
                rt.spawn(relay(pair_port(RELAYS, NODES, from, to), CONSENSUS + to, Duration::ZERO, up.clone()));
            }
            row.push(up);
        }
        links.push(row);
    }
    let set_links_of = |node: usize, up: bool| {
        for other in (0..NODES as usize).filter(|&other| other != node) {
            links[node][other].store(up, Ordering::SeqCst);
            links[other][node].store(up, Ordering::SeqCst);
        }
    };

    let live: Vec<usize> = (0..NODES as usize).collect();
    let cluster = spawn_env(&home, &live, |_| Vec::new(), &[]);
    await_height(&home, &live, 3, Budget::within(180))
        .unwrap_or_else(|why| panic!("the cluster never reached height 3: {why}"));

    // The partition: every link of node 3, both directions.
    set_links_of(3, false);
    let cut_at = height_of(&home, 3);
    let others = [0usize, 1, 2];
    await_height(&home, &others, cut_at + 5, Budget::within(120))
        .unwrap_or_else(|why| panic!("three of four must keep committing with the fourth cut off: {why}"));
    let alone = height_of(&home, 3);
    assert!(alone <= cut_at + 1, "a validator cut off from every peer commits nothing: it went {cut_at} → {alone}");

    // The heal.
    set_links_of(3, true);
    let target = height_of(&home, 0);
    await_height(&home, &[3], target, Budget::within(120))
        .unwrap_or_else(|why| panic!("the healed validator must catch up to {target} on its own: {why}"));

    let seen = observe(&home, &live, 1, Budget::within(60))
        .unwrap_or_else(|why| panic!("a height they all reported after the heal: {why}"));
    if let Some(split) = seen.disagreement() {
        panic!("a validator that rejoined after a partition disagreed with the ones that stayed up: {split}");
    }

    drop(cluster);
    rt.shutdown_background();
    let _ = std::fs::remove_dir_all(&home);
}

/// **Fifty milliseconds one way is a continent, and the federation still
/// commits.** Liveness under the stall rule, and nothing about the rate: a
/// loaded box makes it slow, and slow is not the finding this probe is for.
///
/// Mutation that bites: make the relay forward nothing, and the cluster never
/// reaches height 3 — `never started` under the stall rule.
#[test]
fn a_federation_fifty_milliseconds_apart_still_commits() {
    let per_block = interval_at("malachite-latency-50", 27600, 27700, Duration::from_millis(50), 10)
        .unwrap_or_else(|why| panic!("at 50 ms one way: {why}"));
    eprintln!("50 ms one way: {per_block:.2} s per block");
}

/// **The block interval across latencies**, three in one sitting so the
/// figure is a ratio: 0, 50 and 150 ms one way. `just latency`.
#[test]
#[ignore]
fn the_block_interval_across_latencies() {
    for (ms, base, relays) in [(0u64, 27640u16, 27740u16), (50, 27640, 27740), (150, 27640, 27740)] {
        let per_block = interval_at(&format!("malachite-latency-{ms}"), base, relays, Duration::from_millis(ms), 20)
            .unwrap_or_else(|why| panic!("at {ms} ms one way: {why}"));
        println!("{ms:>4} ms one way: {per_block:.2} s per block over 20 blocks");
    }
}
