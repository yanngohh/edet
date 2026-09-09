//! The edet `Node` binding and a loopback testnet-config generator
//! (`--features malachite`) — the bootstrap `start_engine` needs, wired to
//! the already-existing `EdetContext` (`engine_context.rs`), wire codec
//! (`engine_codec.rs`), and application-channel handler loop
//! (`engine_malachite::run`).
//!
//! Modeled directly on the vendored channel example
//! (`~/.cargo/git/checkouts/malachite-*/code/examples/channel/src/{node,config}.rs`,
//! the pinned commit): an `EdetApp` playing the same role as that example's `App`,
//! implementing `malachitebft_app_channel::app::node::Node` with
//! `Context = EdetContext`. It keeps that example's node-home layout
//! (`<home>/config/{config.toml, genesis.json, priv_validator_key.json}`),
//! but writes those files itself (`write_node_home`) rather than through
//! `malachitebft-test-cli` — the read side never used that crate anyway, and
//! its presence in the graph made the desktop client unbuildable; see
//! `write_node_home`'s own doc comment.
//!
//! One deliberate departure from the vendored example: `TestContext`'s
//! `Address::from_public_key` derives a validator's address FROM its public
//! key. `EdetAddress` is a stable `MemberId` instead (`engine_context.rs`'s
//! own doc comment: never key-derived, since edet members rotate keys) — so
//! `get_address` here resolves a public key to its member id by looking it
//! up in the loaded genesis file instead of computing it, and genesis
//! validators are edet's own dev founders (`block::dev_seed`/`pubkey_of`,
//! the same BIP39 derivation every other dev harness in this crate uses),
//! not Malachite's generic `CanGeneratePrivateKey`/`CanMakeGenesis` machinery
//! (which mints validator keys with no connection to edet identity at all).
//! `write_testnet` below is this file's own stand-in for the vendored
//! `testnet`/`init` commands, hand-rolled for exactly that reason.
//!
//! Status: run, not merely compiled. A single loopback validator commits;
//! `tests/malachite_cluster.rs` runs a real 4-validator genesis as separate OS
//! processes gossiping over loopback TCP (3 live, 1 down) and asserts they
//! commit a signed transaction and agree on the state hash; and with
//! `ClientApi` set, `tests/malachite_http.rs` drives the whole thing from a
//! browser-shaped HTTP client. Still single-host loopback with dev-key
//! genesis — not a geo-distributed deployment, and not audited.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use malachitebft_app_channel::app::config::{
    ConsensusConfig, DiscoveryConfig, LoggingConfig, MetricsConfig, P2pConfig, RuntimeConfig, TransportProtocol,
    ValuePayload, ValueSyncConfig,
};
use malachitebft_app_channel::app::events::{RxEvent, TxEvent};
use malachitebft_app_channel::app::node::{EngineHandle, Node, NodeConfig, NodeHandle};
use malachitebft_app_channel::app::types::Keypair;
use multiaddr::{Multiaddr, PeerId};

use edet_state::state::DEV_ROOT_SALT;
use edet_state::State;

use crate::block::SignedTx;
use crate::engine_codec::EdetCodec;
use crate::engine_context::{EdetAddress, EdetContext, EdetHeight, EdetSigningProvider, EdetValidatorSet};

// ---------------------------------------------------------------------------
// Config (mirrors the vendored channel example's `Config` byte-for-byte)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EdetConfig {
    pub moniker: String,
    pub logging: LoggingConfig,
    pub consensus: ConsensusConfig,
    pub value_sync: ValueSyncConfig,
    pub metrics: MetricsConfig,
    pub runtime: RuntimeConfig,
}

impl NodeConfig for EdetConfig {
    fn moniker(&self) -> &str {
        &self.moniker
    }
    fn consensus(&self) -> &ConsensusConfig {
        &self.consensus
    }
    fn consensus_mut(&mut self) -> &mut ConsensusConfig {
        &mut self.consensus
    }
    fn value_sync(&self) -> &ValueSyncConfig {
        &self.value_sync
    }
    fn value_sync_mut(&mut self) -> &mut ValueSyncConfig {
        &mut self.value_sync
    }
}

/// Loopback consensus ports for a locally generated testnet — distinct from
/// the vendored example's `27000`/`29000` bases so the two never collide if
/// ever run side by side on the same machine.
///
/// It is deliberately not `27500`, which is **passim**'s port — the Local
/// Caching Server that ships enabled on Fedora and starts on demand. The
/// collision is not a flake and not "another node left running": on such a
/// machine node 0 of every generated testnet silently fails to bind, the
/// cluster never reaches quorum, and `malachite_cluster`,
/// `malachite_byzantine`, `malachite_http`'s 2-validator case and `just e2e`
/// all fail together at their liveness timeouts, reading exactly like a
/// consensus fault. Observed on Fedora 44, mid-session, when passim happened
/// to start.
///
/// Any fixed port can be taken by something, so the base is also
/// overridable: set `EDET_CONSENSUS_BASE_PORT` (and `EDET_METRICS_BASE_PORT`)
/// before generating a testnet. The value is baked into the generated
/// `config.toml`, so it must be set for `malachite testnet`, not merely for
/// the run — which is the honest shape, since the peer addresses of the other
/// nodes are written into that file too.
fn base_port(var: &str, default: usize) -> usize {
    std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn consensus_base_port() -> usize {
    base_port("EDET_CONSENSUS_BASE_PORT", 26600)
}

/// A harness override read from the environment, or `None`.
fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn metrics_base_port() -> usize {
    base_port("EDET_METRICS_BASE_PORT", 29500)
}

// ---------------------------------------------------------------------------
// Peer names, resolved once at boot
// ---------------------------------------------------------------------------

/// **Rewrite every DNS peer address to a literal before the transport sees
/// one, and refuse the configurations that would make a name arrive later.**
///
/// `just audit` carries RUSTSEC-2026-0119 — quadratic name compression in
/// `hickory-proto`, reached through `libp2p-dns` under Malachite's network
/// crates, with no fixed version available under `libp2p-dns` 0.44 — on the
/// argument that nothing here resolves a name. Before this function that was a
/// property of the CONFIGURATIONS this repo writes, which are all `/ip4/`
/// literals, and not of the binary: a federation that named its validators by
/// DNS turned the advisory back on, and no gate would have said so. It is a
/// property of the code now, and `a_dns_peer_is_resolved_at_boot_and_never_
/// dialed_by_name` is what checks it.
///
/// Three refusals, because a rewrite alone would not be enough:
///
/// * **discovery**, which learns addresses from peers at run time — a
///   discovered address may be a name, and it arrives long past boot;
/// * **`/dnsaddr/`**, which is resolved by a TXT lookup that yields further
///   multiaddrs, so there is no single `(host, port)` to hand the system
///   resolver;
/// * **a name in the listen address**, which is not an endpoint to reach but
///   an interface to bind, and a name is never one.
///
/// Resolution is `std::net::ToSocketAddrs` — the system resolver, in the
/// standard library, not hickory — so the compiled advisory is unreachable
/// rather than merely unused. It happens ONCE, at boot: a DNS change needs a
/// node restart, which is the cost this buys the reachability argument with.
fn resolve_peer_names(config: &mut EdetConfig) -> eyre::Result<()> {
    use multiaddr::Protocol;

    let p2p = &mut config.consensus.p2p;
    if p2p.discovery.enabled {
        eyre::bail!(
            "peer discovery is off by design: a discovered address may be a name, and this node never \
             resolves one on the wire — set consensus.p2p.discovery.enabled = false and name the peers"
        );
    }
    if p2p.listen_addr.iter().any(|p| is_name(&p)) {
        eyre::bail!(
            "the listen address {} carries a DNS name: a listen address is the interface to bind, \
             which is always a literal",
            p2p.listen_addr
        );
    }
    let mut resolved = Vec::with_capacity(p2p.persistent_peers.len());
    for peer in &p2p.persistent_peers {
        if peer.iter().any(|p| matches!(p, Protocol::Dnsaddr(_))) {
            eyre::bail!(
                "the peer address {peer} is a /dnsaddr/: it resolves to further multiaddrs through a TXT \
                 lookup rather than to one host and port, so this node cannot rewrite it — name the \
                 validator's address directly"
            );
        }
        let rewritten = resolve_peer(peer)?;
        if &rewritten != peer {
            println!("resolved peer {peer} to {rewritten} — a name is read once, at boot");
        }
        resolved.push(rewritten);
    }
    p2p.persistent_peers = resolved;
    Ok(())
}

/// Does this component name a host rather than address one?
fn is_name(p: &multiaddr::Protocol<'_>) -> bool {
    use multiaddr::Protocol;
    matches!(p, Protocol::Dns(_) | Protocol::Dns4(_) | Protocol::Dns6(_) | Protocol::Dnsaddr(_))
}

/// One peer address with its `/dns*/` component replaced by the literal the
/// system resolver answers with, and everything else — the transport, the
/// port, a trailing `/p2p/<peer id>` — carried through untouched.
///
/// The family the component named is honoured: `/dns4/` takes the first IPv4
/// answer and `/dns6/` the first IPv6 one, while `/dns/` takes the first of
/// either. "First" is over an ORDER this function imposes rather than the
/// resolver's, because a resolver is free to rotate its answers and two
/// validators reading the same name must not disagree about which host they
/// are talking to.
fn resolve_peer(peer: &multiaddr::Multiaddr) -> eyre::Result<multiaddr::Multiaddr> {
    use multiaddr::{Multiaddr, Protocol};
    use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

    let components: Vec<Protocol<'_>> = peer.iter().collect();
    let Some(at) = components.iter().position(is_name) else {
        return Ok(peer.clone());
    };
    let host = match &components[at] {
        Protocol::Dns(h) | Protocol::Dns4(h) | Protocol::Dns6(h) => h.to_string(),
        // `Dnsaddr` is refused by the caller, and `is_name` names no other
        // variant, so this arm is unreachable by construction.
        other => eyre::bail!("{other} is not a resolvable name"),
    };
    // A port is needed to ask the resolver at all, and it is also the thing
    // that makes the address dialable: a peer entry without one is malformed
    // however it named its host.
    let Some(Protocol::Tcp(port)) = components.iter().find(|p| matches!(p, Protocol::Tcp(_))) else {
        eyre::bail!("the peer address {peer} names a host but no TCP port, so there is nothing to dial");
    };
    let mut answers: Vec<SocketAddr> = (host.as_str(), *port)
        .to_socket_addrs()
        .map_err(|e| eyre::eyre!("the peer name {host} did not resolve: {e}"))?
        .collect();
    answers.sort();
    let want_v4 = matches!(components[at], Protocol::Dns4(_));
    let want_v6 = matches!(components[at], Protocol::Dns6(_));
    let ip = answers
        .iter()
        .map(SocketAddr::ip)
        .find(|ip| match ip {
            IpAddr::V4(_) => !want_v6,
            IpAddr::V6(_) => !want_v4,
        })
        .ok_or_else(|| {
            eyre::eyre!("the peer name {host} resolved, but to no address of the family {} asks for", components[at])
        })?;
    let mut out = Multiaddr::empty();
    for (i, c) in components.iter().enumerate() {
        if i == at {
            out.push(match ip {
                IpAddr::V4(v4) => Protocol::Ip4(v4),
                IpAddr::V6(v6) => Protocol::Ip6(v6),
            });
        } else {
            out.push(c.clone());
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// One connection per pair: the lower peer id dials
// ---------------------------------------------------------------------------

/// **Every pair of validators keeps ONE connection, because only one side
/// dials it.** Two validators that name each other dial each other at boot
/// and after every repair and hold two TCP connections; gossipsub hands a
/// message to whichever is ready and keeps no order between them, so a
/// proposal's parts arrive with the close before the hash, the stream is
/// refused, and with three live validators of four one Nil prevote fails the
/// round. Measured with the fourth validator paused, 90 s: 69 and 70 blocks
/// on one connection per pair against 16 and 26 on two.
///
/// The rule is a function of the two peer ids alone, so both sides reach it
/// without a message and without a patch to the transport: a validator dials
/// the peers whose id is greater than its own and is dialed by the rest.
/// Upstream does everything else with discovery off — an inbound connection
/// is kept for the life of the process, a configured peer is dialed again
/// whenever it is not connected, a close clears the dial history — so the
/// second connection never exists and there is nothing to tie-break or to
/// close. A cap on connections per peer is not this: it closes the newer
/// connection on each side, the two sides need not agree which that is, and
/// a connection closed at identify time can be the one carrying gossipsub's
/// one-time subscription announcement.
///
/// So every peer entry says WHO it names, a trailing `/p2p/<peer id>`, which
/// also makes the dial an authenticated one; `config.toml` lists the whole
/// set, and what this node dials is decided here, at boot, from its own key.
/// An entry without an id is refused, and so is one naming this node.
///
/// The peer id is the transport's identity and follows the consensus key, so
/// rotating a consensus key changes what every peer's config names.
///
/// Answers `(dialed by this node, dialed by the peer)`.
fn split_by_dialer(local: &PeerId, peers: &[Multiaddr]) -> eyre::Result<(Vec<Multiaddr>, Vec<Multiaddr>)> {
    let mut dials = Vec::new();
    let mut dialed_by = Vec::new();
    for peer in peers {
        let id = peer_id_in(peer).ok_or_else(|| {
            eyre::eyre!(
                "the peer address {peer} carries no /p2p/<peer id>: a pair of validators keeps one connection, \
                 dialed by the lower peer id, so every peer entry must say who it names — `keygen` prints a \
                 validator's peer id beside its consensus key"
            )
        })?;
        match id.cmp(local) {
            std::cmp::Ordering::Equal => eyre::bail!("the peer address {peer} names this node's own peer id {local}"),
            std::cmp::Ordering::Greater => dials.push(peer.clone()),
            std::cmp::Ordering::Less => dialed_by.push(peer.clone()),
        }
    }
    Ok((dials, dialed_by))
}

/// The `/p2p/<peer id>` component of a peer address, if it carries one.
fn peer_id_in(peer: &Multiaddr) -> Option<PeerId> {
    peer.iter().find_map(|p| match p {
        multiaddr::Protocol::P2p(id) => Some(id),
        _ => None,
    })
}

/// The libp2p peer id a validator with this consensus seed dials with: the
/// identity `get_keypair` hands the transport, and what its peers name it by.
pub fn peer_id_of_seed(seed: &[u8; 32]) -> PeerId {
    Keypair::ed25519_from_bytes(*seed)
        .expect("a 32-byte Ed25519 seed is always a valid libp2p keypair")
        .public()
        .to_peer_id()
}

/// The libp2p peer id of a consensus PUBLIC key, which is what the genesis
/// carries, so an operator's `--peers` entry can be checked against the
/// ceremony and `keygen` can print what the peers will name.
pub fn peer_id_of_public(public: &[u8; 32]) -> eyre::Result<PeerId> {
    let key = libp2p_identity::ed25519::PublicKey::try_from_bytes(public)
        .map_err(|e| eyre::eyre!("{} is not an Ed25519 public key: {e}", crate::block::hex32(public)))?;
    Ok(libp2p_identity::PublicKey::from(key).to_peer_id())
}

/// **The shape of every node config this tree writes**, whatever wrote it: no
/// discovery (a fixed set names its peers directly, and a discovered address
/// may be a name — see `resolve_peer_names`), metrics off (kept out of the
/// boot proof's surface), value payload `ProposalAndParts` (this crate's
/// `GetValue`/`ReceivedProposalPart` handlers always stream and reassemble
/// chunked parts, so the parts stream must actually travel the wire), and a
/// single-threaded runtime.
///
/// One function, because there are two callers and they must not drift: a
/// loopback testnet and a real federation differ in their addresses and in
/// nothing else. A federation home written by hand — which is what an
/// operator had to do before `init` existed — is exactly where a difference
/// would appear and go unnoticed until the mesh failed to form.
fn node_config(
    moniker: String,
    listen_addr: Multiaddr,
    persistent_peers: Vec<Multiaddr>,
    metrics_port: usize,
) -> EdetConfig {
    EdetConfig {
        moniker,
        logging: LoggingConfig::default(),
        consensus: ConsensusConfig {
            value_payload: ValuePayload::ProposalAndParts,
            p2p: P2pConfig {
                listen_addr,
                persistent_peers,
                discovery: DiscoveryConfig { enabled: false, ..Default::default() },
                ..Default::default()
            },
            ..Default::default()
        },
        value_sync: ValueSyncConfig::default(),
        metrics: MetricsConfig { enabled: false, listen_addr: format!("127.0.0.1:{metrics_port}").parse().unwrap() },
        runtime: RuntimeConfig::SingleThreaded,
    }
}

/// One node's config out of `total` in a loopback testnet: TCP listen
/// address `127.0.0.1:{consensus base + index}`, explicit persistent
/// peers at every other index's address.
fn make_config(index: usize, total: usize) -> EdetConfig {
    let consensus_base = consensus_base_port();
    node_config(
        format!("edet-{index}"),
        TransportProtocol::Tcp.multiaddr("127.0.0.1", consensus_base + index),
        (0..total)
            .filter(|&j| j != index)
            .map(|j| {
                TransportProtocol::Tcp
                    .multiaddr("127.0.0.1", consensus_base + j)
                    .with_p2p(peer_id_of_seed(&crate::block::dev_consensus_seed(j as u8)))
                    .expect("a fresh address carries no peer id yet")
            })
            .collect(),
        metrics_base_port() + index,
    )
}

// ---------------------------------------------------------------------------
// Genesis + private-key-file wire shapes
// ---------------------------------------------------------------------------

/// One genesis entry: a stable edet `MemberId`-derived address, the member
/// key it holds, the key its validator signs consensus with, and its voting
/// power. Deliberately NOT `malachitebft_test::Validator` (which has no
/// address field at all, deriving one from the key instead) — see this file's
/// own top doc comment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EdetGenesisEntry {
    pub address: EdetAddress,
    /// The member key: what this founder signs `Accept`, `Settle` and
    /// `DeclareSupply` with. **It never goes on a server.**
    pub public_key: [u8; 32],
    /// The key this founder's VALIDATOR signs consensus with, if they run one.
    ///
    /// Separate from `public_key`, and the separation is the point: the
    /// consensus key lives unencrypted in `priv_validator_key.json` on a host
    /// that answers the internet. One key for both roles makes a validator host
    /// compromise hand the attacker the operator's economic identity, and makes
    /// restoring that founder's recovery phrase into the wallet turn a handset
    /// into a hot consensus key.
    ///
    /// `None` for a founder who runs no validator, which is how a community
    /// seats a funder with no infrastructure. A genesis entry with
    /// `voting_power > 0` and no consensus key is refused
    /// (`ET-VAL-NO-CONSENSUS-KEY`).
    #[serde(default)]
    pub consensus_key: Option<[u8; 32]>,
    pub voting_power: u64,
}

/// A founding underwriter and the supply they declared, in denomination
/// units (§Adoption).
///
/// Named by ADDRESS — which is a `MemberId` — so an underwriter must be one
/// of the genesis entries. Underwriting is an economic position and running a
/// validator is an operational one, so the two sets need not overlap: a
/// genesis entry with `voting_power: 0` is a member and an underwriter but
/// never a validator, which is exactly how a community seats a funder who
/// runs no infrastructure.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EdetGenesisUnderwriter {
    pub address: EdetAddress,
    /// The declared supply. **An accepted liability**: if the members this
    /// underwriter backs fail, this much of the loss is theirs. It should name
    /// no amount anybody cannot bear.
    pub supply: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EdetGenesis {
    pub validators: Vec<EdetGenesisEntry>,
    /// The founding underwriters and their supplies — **the only seed the
    /// community will ever have that it did not have to earn** (§Adoption).
    ///
    /// A community may leave this empty and it will work: every obligation is
    /// uninsured, borne bilaterally, which is an ordinary mutual-credit
    /// network. What it can never do is leave that state. With no supply there
    /// is no source arc, so no capacity, so nothing may be conferred, so no
    /// settlement stakes anything, so no member can ever declare a supply.
    /// **Zero is absorbing**, and the free-signature bound says no amount of
    /// subsequent trading substitutes for the seed.
    ///
    /// Size it to **peak simultaneous** insured credit, never annual volume:
    /// the seed is not consumed but reserved, released and reserved again, so
    /// it turns over once per settlement term — 12x a year at the maturity
    /// floor of 30 epochs, the fastest any chain admits, and 4x on 90-day
    /// terms. Err low — understating costs friction the uninsured tier
    /// absorbs, while overstating is the error with no later correction.
    ///
    /// `serde(default)` so a genesis file written before this field existed
    /// still loads, as a community that underwrites nothing.
    #[serde(default)]
    pub underwriters: Vec<EdetGenesisUnderwriter>,
    /// The chain id every transaction and consensus message signed
    /// against this genesis must bind to (`crate::block::tx_digest`,
    /// `engine_context::sign_bytes`) — genesis data, not something a running
    /// network can quietly change. `#[serde(default)]` so a genesis file
    /// written before this field existed still loads, falling back to the
    /// same default `edet_state::State::default()` carries — see
    /// `default_chain_id`.
    #[serde(default = "default_chain_id")]
    pub chain_id: String,
    /// The per-chain salt every state-root leaf is derived from
    /// (`edet_state::root`, the paper's §Implementation).
    /// Genesis data exactly like `chain_id`: validators holding different
    /// values compute different roots and cannot agree.
    ///
    /// `Option` + `serde(default)` so a genesis file written before the root
    /// existed still LOADS — and then `genesis_state` refuses it unless the
    /// chain is a dev one, rather than quietly substituting the published dev
    /// salt on a real network. Same shape as the dev-key tripwire below, and
    /// for the same reason: the safe fallback for a throwaway chain is the
    /// dangerous one for a real chain, so the chain id has to decide.
    #[serde(default)]
    pub root_salt: Option<[u8; 32]>,
    /// The validator floor the ledger holds every removal path to
    /// (`Params::min_validators`). The ceremony decides it because only the
    /// ceremony knows which kind of chain this is: `genesis init` writes
    /// `MIN_VALIDATORS_REAL_CHAIN` for a real chain and `MIN_VALIDATORS` for
    /// the dev one. Absent in a file written before the field existed, the
    /// chain id decides the same way (`genesis_state`).
    #[serde(default)]
    pub min_validators: Option<u64>,
    /// The `SealAmounts` charter policy at genesis: 1.0 seals precise amounts
    /// to the parties and validators, 0.0 serves them to every reader. A real
    /// chain seals unless the ceremony says otherwise (`--open-amounts`); the
    /// dev chain is open. Absent, the chain id decides.
    #[serde(default)]
    pub seal_amounts: Option<f64>,
}

/// `serde(default)` value for `EdetGenesis::chain_id` — mirrors
/// `crate::block::DEV_CHAIN_ID`/`State::default()`'s own default, so an
/// old genesis file missing this field loads onto the same chain a fresh
/// dev genesis would have used anyway.
fn default_chain_id() -> String {
    crate::block::DEV_CHAIN_ID.to_string()
}

/// Seat every entry of a DEV genesis as a founding underwriter.
///
/// Without a supply a chain is inert by arithmetic — no source arc means no
/// capacity, so nothing is conferrable, so no member has bond headroom and the
/// ingress screen refuses every bonded transition. A dev genesis with no
/// underwriter describes a community that cannot transact, which would make
/// every harness measure the same zero and pass.
///
/// Dev only, and it must stay that way: on a real chain the founding
/// underwriters are the most consequential decision of the whole ceremony
/// (§Adoption), agreed out of band and named explicitly with `--underwriter`. A
/// default supply is exactly the thing that must never exist there.
#[cfg(any(test, feature = "serve"))]
pub fn dev_underwriters(validators: &[EdetGenesisEntry]) -> Vec<EdetGenesisUnderwriter> {
    validators
        .iter()
        .map(|v| EdetGenesisUnderwriter { address: v.address, supply: crate::serve::DEV_SUPPLY })
        .collect()
}

/// The private-key file's on-disk shape: the raw 32-byte Ed25519 seed.
/// `ed25519_dalek::SigningKey` itself carries no `serde` impl (this crate's
/// `ed25519-dalek` dependency does not turn on that feature — see
/// `Cargo.toml`), so this is the wire shape `Node::PrivateKeyFile` needs.
#[derive(Clone, Serialize, Deserialize)]
pub struct EdetPrivateKeyFile {
    pub seed: [u8; 32],
}

/// The one test for "is this a throwaway harness chain", shared by every
/// exemption that turns on it — the root-salt fallback in `genesis_state` and
/// the published-dev-key tripwire in `EdetApp::start`. EXACT equality with
/// `block::DEV_CHAIN_ID`, never a prefix: `starts_with("edet-dev")` also
/// matches `edet-devon-mutual-credit` and `edet-development-fund`, which are
/// ordinary real chain ids that happen to begin with those eight characters
/// — and matching them silently disabled both protections for that chain's
/// whole life. Every dev genesis this crate generates (`write_testnet`,
/// `write_solo_home_on`) carries exactly `DEV_CHAIN_ID`, so the exact test
/// covers all of them; a `--dev` chain given some other id is treated as
/// real, which is the safe direction (it must then carry its own salt and
/// its own keys).
pub fn is_dev_chain(chain_id: &str) -> bool {
    chain_id == crate::block::DEV_CHAIN_ID
}

/// True iff `pk` equals `pubkey_of(dev_seed(i))` for some `i in 0..=255` —
/// the dev-key tripwire. `dev_seed`'s entropy (`block::dev_entropy`,
/// `[i+1; 16]`) is DELIBERATELY published — every dev harness in this crate
/// prints the resulting recovery phrases (`dev_phrases`) so a founder can be
/// restored from nothing but its index — so any key equal to one of these
/// 256 is, by construction, known to everyone and secures nothing. Checked
/// at `EdetApp::start`, not at `genesis_state`: `genesis_state` also builds
/// the harness's own dev genesis (`is_dev_chain`), which legitimately uses
/// these keys, so the chain-id exemption has to live where both pieces of
/// context — the key AND the chain id — are in hand together.
pub fn is_dev_key(pk: &[u8; 32]) -> bool {
    (0..=255u8).any(|i| {
        &crate::block::pubkey_of(&crate::block::dev_seed(i)) == pk
            || &crate::block::pubkey_of(&crate::block::dev_consensus_seed(i)) == pk
    })
}

/// the tripwire as a function of the genesis alone, so it is reachable
/// without standing up an engine: refuse a genesis that claims a real chain
/// id while any of its validators holds one of the published dev keys. Both
/// halves of the decision — the key and the chain id — are here, which is why
/// this lives beside `is_dev_chain` rather than inside `genesis_state` (which
/// legitimately builds the harness's own dev genesis).
fn refuse_published_dev_keys(genesis: &EdetGenesis) -> eyre::Result<()> {
    if is_dev_chain(&genesis.chain_id) {
        return Ok(());
    }
    // BOTH keys, because both are keys a real network's security rests on and
    // only one of them was checked while they were the same key.
    if let Some(offender) = genesis
        .validators
        .iter()
        .find(|v| is_dev_key(&v.public_key) || v.consensus_key.is_some_and(|k| is_dev_key(&k)))
    {
        return Err(eyre::eyre!(
            "genesis validator {} uses a dev key (derived from published entropy — block::dev_seed) on chain_id \"{}\", which is not a dev chain; generate a real key for every validator before this network can start",
            offender.address.0,
            genesis.chain_id
        ));
    }
    Ok(())
}

/// Build the `edet_state::State` a genesis file describes: every entry
/// becomes a genesis member (Active, at its recorded key) and a genesis
/// validator at its recorded power, added strictly in ascending `address`
/// order so `add_genesis_member`'s sequentially assigned `MemberId`s land
/// exactly on the addresses the genesis file itself declares (`EdetAddress`
/// IS a `MemberId` — `engine_context.rs`).
/// The founding roll, checked as a SUM.
///
/// Every ingress refuses an amount the boundary cannot name, one amount at a
/// time, and every one of them was written for a payload two parties signed.
/// A genesis has neither: it is a ceremony, and it seats the whole roll at
/// once. Ten founders each comfortably inside the ceiling still sum past it,
/// and the roll is what governance weight, the amendment rate and every
/// capacity in the community are a fraction OF — so this is where the total is
/// asked about, in `u128`, before a single account exists.
///
/// One function for two doors: `genesis init` asks it while authoring the file
/// and `genesis_state` asks it again at boot, because a file can be
/// hand-edited between the two.
pub fn check_seed_total(supplies: impl IntoIterator<Item = f64>) -> Result<(), String> {
    let total: u128 = supplies.into_iter().map(|s| State::to_minor(s) as u128).sum();
    let ceiling = edet_kernel::constants::MAX_AMOUNT_MINOR;
    if total > ceiling as u128 {
        return Err(format!(
            "the founding underwriters declare {} between them, and the ledger cannot name more than {} — \
             amounts cross the wire as major-unit floats, and past that figure the conversion back is no \
             longer exact. Size the seed to PEAK SIMULTANEOUS insured credit, not to annual volume.",
            State::from_minor((total.min(u64::MAX as u128)) as u64),
            State::from_minor(ceiling)
        ));
    }
    Ok(())
}

pub fn genesis_state(genesis: &EdetGenesis) -> eyre::Result<State> {
    if genesis.chain_id.is_empty() {
        return Err(eyre::eyre!(
            "genesis chain_id must not be empty — an unset chain id lets a signed transaction or vote replay onto any other network that also left it unset"
        ));
    }
    // The state-root salt (the paper's §Implementation). A real
    // chain must carry its own: the dev value is published, and a published
    // salt lets anyone holding one inclusion proof brute-force the sibling
    // hashes travelling with it and read the neighbouring records. A dev
    // chain may fall back to it, because a dev chain's privacy is already nil
    // (`block::dev_seed`) and reproducible roots across runs are worth more
    // there than a secret that is not one.
    //
    // The gate is on the salt's VALUE, not merely on the field being
    // present. A genesis adapted from a dev one — copy the file, change the
    // chain id, keep everything else — carries `Some(DEV_ROOT_SALT)`, which
    // a presence-only check waves through: the resulting chain agrees with
    // itself, every proof verifies, and every proof it ever issues is
    // unblinded to anyone who has read this source. Refusing the published
    // value by name is the only form of this check that catches that.
    let dev_chain = is_dev_chain(&genesis.chain_id);
    let root_salt = match genesis.root_salt {
        Some(salt) if salt == [0u8; 32] => {
            return Err(eyre::eyre!(
                "genesis for chain \"{}\" carries an all-zero root_salt — that is an unset field, not a secret; \
                 `edet-node genesis init` draws one from the OS RNG",
                genesis.chain_id
            ));
        }
        Some(salt) if !dev_chain && salt == DEV_ROOT_SALT => {
            return Err(eyre::eyre!(
                "genesis for chain \"{}\" carries the PUBLISHED dev root_salt (edet_state::state::DEV_ROOT_SALT) — \
                 it is a constant in this crate's source, so every inclusion proof this chain issues would let any \
                 reader brute-force the sibling leaves travelling with it. Draw a real one with \
                 `edet-node genesis init`; see the paper's §Implementation",
                genesis.chain_id
            ));
        }
        Some(salt) => salt,
        None if dev_chain => DEV_ROOT_SALT,
        None => {
            return Err(eyre::eyre!(
                "genesis for chain \"{}\" has no root_salt — a real chain must carry its own 32-byte state-root salt \
                 (`edet-node genesis init` writes one). Falling back to the published dev salt would make every \
                 inclusion proof leak the records of the leaves next to it; see the paper's §Implementation",
                genesis.chain_id
            ));
        }
    };
    // The founding supplies, indexed by address so the member loop can seat
    // each one as it creates the account. Validated first, before a single
    // account exists, because a genesis that names a supply for somebody who
    // is not a member is a file the operators mis-assembled and must fix — not
    // something to discover halfway through building the state.
    let mut supplies: std::collections::BTreeMap<u64, f64> = std::collections::BTreeMap::new();
    for uw in &genesis.underwriters {
        if !genesis.validators.iter().any(|v| v.address == uw.address) {
            return Err(eyre::eyre!(
                "genesis underwriter {} is not one of the genesis entries — an underwriter must be a member of \
                 the community it stands behind; add them with voting_power 0 if they run no validator",
                uw.address.0
            ));
        }
        if !State::amount_representable(uw.supply) {
            return Err(eyre::eyre!(
                "genesis underwriter {} declares a supply of {} — a declared supply is an accepted liability and \
                 must be a finite, non-negative amount the ledger can name (at most {})",
                uw.address.0,
                uw.supply,
                State::from_minor(edet_kernel::constants::MAX_AMOUNT_MINOR)
            ));
        }
        if supplies.insert(uw.address.0, uw.supply).is_some() {
            return Err(eyre::eyre!(
                "genesis names underwriter {} twice — which of the two supplies is the accepted liability is \
                 exactly the question a founding ceremony must not leave open",
                uw.address.0
            ));
        }
    }

    check_seed_total(supplies.values().copied()).map_err(|e| eyre::eyre!("{e}"))?;

    let mut state = State { chain_id: genesis.chain_id.clone(), root_salt, ..Default::default() };
    // The two policies the ceremony decides and the state machine cannot see.
    // A file that names neither is read the way `genesis init` would have
    // written it for this chain id.
    state.params.min_validators = genesis.min_validators.unwrap_or(if dev_chain {
        edet_kernel::constants::MIN_VALIDATORS as u64
    } else {
        edet_kernel::constants::MIN_VALIDATORS_REAL_CHAIN as u64
    });
    state.params.seal_amounts = genesis.seal_amounts.unwrap_or(if dev_chain { 0.0 } else { 1.0 });
    if !(0.0..=1.0).contains(&state.params.seal_amounts) {
        return Err(eyre::eyre!(
            "genesis for chain \"{}\" names seal_amounts {} — the charter policy is a value in [0, 1]",
            genesis.chain_id,
            state.params.seal_amounts
        ));
    }
    for entry in &genesis.validators {
        let supply = supplies.get(&entry.address.0).copied().unwrap_or(0.0);
        let id = state.add_underwriter(vec![entry.public_key], supply)?;
        if id != entry.address.0 {
            return Err(eyre::eyre!(
                "genesis members must be listed in ascending address order: expected id {} for address {}, got {id}",
                entry.address.0,
                entry.address.0
            ));
        }
        if let Some(ck) = entry.consensus_key {
            if ck == entry.public_key {
                return Err(eyre::eyre!(
                    "genesis entry {} uses one key for both its member identity and its validator: a consensus key \
                     lives unencrypted on a host that answers the internet, and a member key signs obligations",
                    entry.address.0
                ));
            }
            state.set_consensus_key(id, ck)?;
        }
        // A genesis entry with zero power is a member but NOT a validator —
        // the community's charter validators are a subset of its members.
        if entry.voting_power > 0 {
            if entry.consensus_key.is_none() {
                return Err(eyre::eyre!(
                    "genesis validator {} has voting power but no consensus_key — a validator whose signing key the \
                     ledger cannot name is one no certificate can be verified against",
                    entry.address.0
                ));
            }
            state.set_genesis_validator(id, entry.voting_power)?;
        }
    }
    Ok(state)
}

// ---------------------------------------------------------------------------
// The Node
// ---------------------------------------------------------------------------

/// The browser-facing HTTP API a Malachite validator optionally also serves
/// (`serve::spawn_client`). `None` on `EdetApp` means "consensus only": the
/// embedded Tauri app reads its in-process `Arc<Node>` over IPC and needs no
/// socket at all, and the cluster harness reads `status.json`.
#[derive(Clone, Debug, Default)]
pub struct ClientApi {
    /// TCP port for the client API.
    pub port: u16,
    /// One HTTP base URL per validator, in genesis order (`peers[i]` is the
    /// node whose `EdetAddress`/`MemberId` is `i`), this node's own slot
    /// included and ignored — the same slot-indexed shape `serve::Config::peers`
    /// documents. These are TRANSACTION-gossip targets, not consensus peers:
    /// Malachite gossips consensus messages over its own libp2p mesh
    /// (`EdetConfig::consensus.p2p`), but knows nothing about the mempool, so a
    /// tx submitted to one node would otherwise only ever be proposed by that
    /// node. Empty = no tx gossip (a solo validator needs none).
    pub peers: Vec<String>,
    /// Extra localhost UI dev-server ports to allow over CORS, beyond the
    /// default 5173 (`serve::Config::cors_ports`).
    pub cors_ports: Vec<u16>,
    /// Bind on every interface instead of loopback (`serve::Config::bind_all`).
    pub bind_all: bool,
    /// Shared secret for the tx-gossip `/p2p/*` surface
    /// (`serve::Config::cluster_token`) — set it whenever `bind_all` is.
    pub cluster_token: Option<String>,
    /// See `serve::Config::trust_forwarded_for`.
    pub trust_forwarded_for: bool,
    /// Accept transactions at THIS node's ingress without checking their
    /// signatures (`serve::Config::allow_unsigned`) — the harness hatch for
    /// shell-scripted smokes, and the way a test stands up a genuinely
    /// misbehaving proposer.
    ///
    /// It cannot spread: consensus screens every proposal it did not build
    /// (`engine_malachite::screen`), so a block carrying what this node
    /// waved through is voted invalid by every honest peer and never
    /// commits. The node only wastes its own proposer turns.
    pub allow_unsigned: bool,
}

/// The edet Malachite application. Paths are resolved once (by the caller —
/// `main.rs`'s `malachite` subcommand, or `write_testnet` below) rather than
/// derived from `home_dir` internally, matching the vendored `App`'s own
/// `config_file`/`genesis_file`/`private_key_file` fields.
#[derive(Clone)]
pub struct EdetApp {
    pub home_dir: PathBuf,
    /// **How far back this node keeps its WAL**, in blocks — how long it may
    /// be down and still rejoin from a peer rather than from an operator
    /// carrying a snapshot. `--prune-margin-blocks` sets it; see
    /// `replica::DEFAULT_PRUNE_MARGIN_BLOCKS`.
    pub prune_margin_blocks: u64,
    pub config_file: PathBuf,
    pub genesis_file: PathBuf,
    pub private_key_file: PathBuf,
    pub start_height: Option<EdetHeight>,
    /// Serve the browser-client HTTP API alongside consensus (`ClientApi`).
    pub client: Option<ClientApi>,
}

pub struct EdetNodeHandle {
    pub app: JoinHandle<()>,
    pub engine: EngineHandle,
    pub tx_event: TxEvent<EdetContext>,
    /// The shared node the engine drives: the embedder (the Tauri app, tests)
    /// reads Malachite-committed state through `node.lock()` and submits
    /// client transactions via `serve::submit(&node, tx)` — the same
    /// `Arc<serve::Node>` the IPC/view surface uses, so the browser UI, the
    /// desktop app and the engine all read one node.
    pub node: std::sync::Arc<crate::serve::Node>,
}

#[async_trait]
impl NodeHandle<EdetContext> for EdetNodeHandle {
    fn subscribe(&self) -> RxEvent<EdetContext> {
        self.tx_event.subscribe()
    }

    async fn kill(&self, _reason: Option<String>) -> eyre::Result<()> {
        self.engine.actor.kill_and_wait(None).await?;
        self.app.abort();
        self.engine.handle.abort();
        Ok(())
    }
}

impl EdetNodeHandle {
    /// Stop the node (kill the consensus actor + abort the app loop). An
    /// inherent wrapper so embedders/tests can shut down without importing the
    /// Malachite `NodeHandle` trait themselves.
    pub async fn shutdown(&self) -> eyre::Result<()> {
        <Self as NodeHandle<EdetContext>>::kill(self, None).await
    }
}

impl EdetApp {
    /// An app over the home at `home_dir`, with the three file paths a home
    /// holds derived from it.
    ///
    /// `node_home` already names those three for the WRITE side, and every
    /// reader spelling them out again is one more place for a rename to leave
    /// half the tree looking in the old spot.
    pub fn at(home_dir: PathBuf, start_height: Option<EdetHeight>, client: Option<ClientApi>) -> Self {
        let paths = node_home(&home_dir);
        EdetApp {
            config_file: paths.config,
            genesis_file: paths.genesis,
            private_key_file: paths.key,
            home_dir,
            prune_margin_blocks: crate::replica::DEFAULT_PRUNE_MARGIN_BLOCKS,
            start_height,
            client,
        }
    }

    /// Start the Malachite engine and return the handle (carrying the shared
    /// `serve::Node` for reads/submits). Inherent wrapper over the Malachite
    /// `Node::start` trait method so an embedder (the Tauri app) or a test can
    /// call it without importing the trait.
    pub async fn start_embedded(&self) -> eyre::Result<EdetNodeHandle> {
        <Self as Node>::start(self).await
    }
}

#[async_trait]
impl Node for EdetApp {
    type Context = EdetContext;
    type Config = EdetConfig;
    type Genesis = EdetGenesis;
    type PrivateKeyFile = EdetPrivateKeyFile;
    type SigningProvider = EdetSigningProvider;
    type NodeHandle = EdetNodeHandle;

    fn get_home_dir(&self) -> PathBuf {
        self.home_dir.clone()
    }

    fn load_config(&self) -> eyre::Result<EdetConfig> {
        let raw = fs::read_to_string(&self.config_file)?;
        let mut config: EdetConfig = toml::from_str(&raw)?;
        resolve_peer_names(&mut config)?;
        // What this node dials is a function of its own key and the ids the
        // config names, decided here so that the transport never holds two
        // connections to one peer (`split_by_dialer`).
        let local = peer_id_of_seed(&self.load_private_key_file()?.seed);
        let (dials, dialed_by) = split_by_dialer(&local, &config.consensus.p2p.persistent_peers)?;
        for peer in &dialed_by {
            println!("peer {peer} dials this node and is not dialed: a pair keeps one connection, dialed by the lower peer id");
        }
        config.consensus.p2p.persistent_peers = dials;
        Ok(config)
    }

    /// Resolve this node's CONSENSUS public key to its member id by looking
    /// it up in the genesis validator set — never derived from the key
    /// material itself (`EdetAddress` is a stable `MemberId`; see this file's
    /// top doc comment and `engine_context.rs`'s doc comment on
    /// `EdetAddress`).
    ///
    /// The key held in `priv_validator_key.json` is the consensus key, so this
    /// reads `consensus_key`. A node whose key
    /// matches some entry's MEMBER key is refused rather than resolved: that
    /// is an operator having put the money key on the server, which is the
    /// exact configuration this separation exists to make impossible.
    fn get_address(&self, pk: &VerifyingKey) -> EdetAddress {
        let genesis = self
            .load_genesis()
            .expect("genesis file must be readable to resolve this node's own address");
        let key = pk.to_bytes();
        assert!(
            !genesis.validators.iter().any(|v| v.public_key == key),
            "this node's private key file holds a MEMBER key: a validator signs with its consensus key, and a \
             member key on a server is an economic identity on a server"
        );
        genesis
            .validators
            .iter()
            .find(|v| v.consensus_key == Some(key))
            .map(|v| v.address)
            .expect("this node's consensus key must be one of the genesis validators'")
    }

    fn get_public_key(&self, pk: &SigningKey) -> VerifyingKey {
        pk.verifying_key()
    }

    fn get_keypair(&self, pk: SigningKey) -> Keypair {
        Keypair::ed25519_from_bytes(pk.to_bytes()).expect("a 32-byte Ed25519 seed is always a valid libp2p keypair")
    }

    fn load_private_key(&self, file: EdetPrivateKeyFile) -> SigningKey {
        SigningKey::from_bytes(&file.seed)
    }

    /// Read the consensus key file, **refusing one any other account on the
    /// host can read**.
    ///
    /// It was written with `fs::write`'s default mode — 0644 under a typical
    /// umask — and nothing ever looked at the mode again, so an unencrypted
    /// consensus key sat world-readable on a machine that answers the
    /// internet. Refused rather than silently repaired: a key that has been
    /// readable is a key that may already have been read, and `chmod`-ing it
    /// here would hide that from the operator who has to decide whether to
    /// rotate.
    fn load_private_key_file(&self) -> eyre::Result<EdetPrivateKeyFile> {
        refuse_permissive_key_file(&self.private_key_file)?;
        let raw = fs::read_to_string(&self.private_key_file)?;
        serde_json::from_str(&raw).map_err(Into::into)
    }

    fn get_signing_provider(&self, private_key: SigningKey) -> EdetSigningProvider {
        // Every consensus message this validator signs must bind to
        // its OWN genesis's chain id — read the same way `get_address`
        // above already reads the genesis file, rather than defaulting and
        // silently signing for the wrong network.
        let chain_id = self
            .load_genesis()
            .map(|g| g.chain_id)
            .unwrap_or_else(|_| crate::block::DEV_CHAIN_ID.to_string());
        EdetSigningProvider::new(private_key, chain_id)
    }

    fn load_genesis(&self) -> eyre::Result<EdetGenesis> {
        let raw = fs::read_to_string(&self.genesis_file)?;
        serde_json::from_str(&raw).map_err(Into::into)
    }

    async fn start(&self) -> eyre::Result<EdetNodeHandle> {
        let config = self.load_config()?;

        let private_key_file = self.load_private_key_file()?;
        let private_key = self.load_private_key(private_key_file);
        let public_key = self.get_public_key(&private_key);
        let own_address = self.get_address(&public_key);
        let ctx = EdetContext;

        let genesis = self.load_genesis()?;
        // The dev-key tripwire. `chain_id` exactly `DEV_CHAIN_ID` is this
        // crate's own marker for "a harness genesis" (every dev generator
        // below sets exactly that, and the test is exact — see
        // `is_dev_chain`) — anything else claims to be a real network, and a
        // real network signed by known, published entropy is not secured at
        // all: refuse to boot rather than let a testnet key end up
        // controlling real obligations.
        refuse_published_dev_keys(&genesis)?;
        let state = genesis_state(&genesis)?;
        let initial_validator_set = EdetValidatorSet::from_state(&state)
            .map_err(|e| eyre::eyre!("failed to build the initial validator set from genesis: {e}"))?;

        let (channels, engine_handle) = malachitebft_app_channel::start_engine(
            ctx,
            self.clone(),
            config,
            EdetCodec, // WAL codec
            EdetCodec, // Network codec
            self.start_height,
            initial_validator_set,
        )
        .await?;

        let tx_event = channels.events.clone();

        // The edet block store (WAL + snapshots), kept in its own
        // sub-directory: Malachite's own consensus WAL already lives at
        // `home_dir/wal` (`spawn_wal_actor`) — this is edet's separate,
        // higher-level durable log of decided blocks + state snapshots
        // (`store.rs`), never the same files.
        // The engine drives a shared `serve::Node` (durable block store at
        // `home_dir/edet-store`, kept separate from Malachite's own consensus
        // WAL at `home_dir/wal`). The embedder reads committed state and
        // submits client txs through this same handle — see `EdetNodeHandle`.
        let store_dir = self.home_dir.join("edet-store");
        let client = self.client.clone().unwrap_or_default();
        let cfg = crate::serve::Config {
            index: own_address.0 as usize,
            n: genesis.validators.len(),
            listen_port: client.port,
            peers: client.peers.clone(),
            allow_unsigned: client.allow_unsigned,
            data_dir: Some(store_dir.to_string_lossy().into_owned()),
            // Harness-only overrides, like `EDET_CONSENSUS_BASE_PORT`: an
            // engine test that has to reach the history floor cannot wait for
            // a day of blocks to be pruned. An operator sets
            // `--prune-margin-blocks`, which is what `client.prune_margin_
            // blocks` carries.
            snapshot_interval: env_u64("EDET_SNAPSHOT_INTERVAL").unwrap_or(128),
            prune_margin_blocks: env_u64("EDET_PRUNE_MARGIN_BLOCKS").unwrap_or(self.prune_margin_blocks),
            bind_all: client.bind_all,
            cors_ports: client.cors_ports.clone(),
            cluster_token: client.cluster_token.clone(),
            trust_forwarded_for: client.trust_forwarded_for,
        };
        let node = crate::serve::build(cfg, state)
            .map_err(|e| eyre::eyre!("failed to open the edet node store at {}: {e}", store_dir.display()))?;

        // The browser-client API, when this node serves one: client routes
        // plus tx gossip, and no block production of any kind — the only
        // writer into this core is the `Decided` handler below. Bound before
        // the engine loop starts so a UI that connects immediately is never
        // refused.
        if self.client.is_some() {
            crate::serve::spawn_client(node.clone())
                .await
                .map_err(|e| eyre::eyre!("failed to bind the client API on port {}: {e}", node.cfg.listen_port))?;
        }

        // Cluster harness: an externally-injected tx this node carries into
        // whichever height it next proposes. That harness runs its nodes with no
        // client API and so no tx gossip either (`ClientApi::peers` is what
        // carries a submission between mempools), so it seeds every node's
        // mempool with the SAME signed tx up front; `Mempool::push` dedups by
        // hash. Absent outside the loopback cluster harness. In the embedded
        // app, client submissions arrive live via `serve::submit`.
        // Held to the same authentication rule as any other ingress
        // (`serve::submit`, `Block::verify_txs`) even though the file is local
        // and trusted: this is the one path into the mempool that does not go
        // through `serve::submit`, and a mempool entry becomes a proposal. An
        // unauthenticated one would only ever produce blocks every peer votes
        // invalid — this node stalling itself, over and over.
        if let Ok(raw) = fs::read_to_string(self.home_dir.join("seed_tx.json")) {
            match serde_json::from_str::<SignedTx>(&raw) {
                Ok(tx) if tx.is_authenticated(&genesis.chain_id) => {
                    node.lock().mempool.push(tx);
                }
                Ok(_) => eprintln!("ignoring seed_tx.json: its transaction is not properly signed"),
                Err(_) => eprintln!("ignoring seed_tx.json: not a decodable transaction"),
            }
        }
        let signing = EdetSigningProvider::new(private_key, genesis.chain_id.clone());
        // Harness observability side channel (`just malachite-cluster` / the
        // cluster test): committed height + state hash after each commit,
        // never consulted by consensus itself.
        let status_path = self.home_dir.join("status.json");

        let engine_node = node.clone();
        let app_handle: JoinHandle<()> = tokio::spawn(async move {
            let time_source = || SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            if let Err(e) = crate::engine_malachite::run(
                engine_node,
                channels,
                time_source,
                own_address,
                signing,
                Some(status_path),
            )
            .await
            {
                eprintln!("edet consensus loop exited: {e}");
            }
        });

        Ok(EdetNodeHandle { app: app_handle, engine: engine_handle, tx_event, node })
    }

    async fn run(self) -> eyre::Result<()> {
        let handle = self.start().await?;
        handle.app.await.map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// Config generation: an N-node loopback testnet
// ---------------------------------------------------------------------------

/// The three files that make up a node home, at the layout Malachite's own
/// CLI resolves: `<home>/config/{config.toml, genesis.json,
/// priv_validator_key.json}`.
struct NodeHome {
    config: PathBuf,
    genesis: PathBuf,
    key: PathBuf,
}

fn node_home(home_dir: &Path) -> NodeHome {
    let dir = home_dir.join("config");
    NodeHome {
        config: dir.join("config.toml"),
        genesis: dir.join("genesis.json"),
        key: dir.join("priv_validator_key.json"),
    }
}

/// Write one node home: TOML for the config, JSON for the genesis and the
/// private key.
///
/// Deliberately edet's own, rather than the vendored
/// `malachitebft_test_cli::file::{save_config, save_genesis,
/// save_priv_validator_key}`. Two reasons, and the second is the sharper one:
///
///   - the READ side never used that crate anyway (`load_config` is a
///     `toml::from_str`, `load_genesis`/`load_private_key_file` are
///     `serde_json::from_str`), so half the pair was already edet-owned and
///     this makes the formats visibly one decision instead of two;
///   - `malachitebft-test-cli` is a TEST crate, and having it in the
///     dependency graph made the desktop/mobile client unbuildable: it
///     requires `toml >= 0.8.21`, while Tauri's GTK stack pins
///     `toml_datetime = "=0.6.3"` transitively, and the two cannot be
///     satisfied together. Dropping it resolves that, and a test CLI has no
///     business in a production graph regardless.
fn write_node_home(home_dir: &Path, config: &EdetConfig, genesis: &EdetGenesis, seed: [u8; 32]) -> eyre::Result<()> {
    let paths = node_home(home_dir);
    if let Some(dir) = paths.config.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&paths.config, toml::to_string_pretty(config)?)?;
    fs::write(&paths.genesis, serde_json::to_string_pretty(genesis)?)?;
    write_private_key(&paths.key, seed)?;
    Ok(())
}

/// Write the consensus key file at 0600 — owner read/write, nobody else.
///
/// The mode is set BEFORE the bytes land, by creating the file with it, rather
/// than written-then-chmod-ed: between those two calls the key is on disk at
/// whatever the umask allowed, and "briefly world-readable" is the same
/// security property as "world-readable" against anything watching the
/// directory.
fn write_private_key(path: &Path, seed: [u8; 32]) -> eyre::Result<()> {
    let json = serde_json::to_string_pretty(&EdetPrivateKeyFile { seed })?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(json.as_bytes())?;
        // An existing file keeps its old mode through `create(true)`, so say
        // it again for the overwrite case.
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    fs::write(path, json)?;
    Ok(())
}

/// Refuse to boot on a consensus key file any other account on the host can
/// read.
///
/// Unix only, because the mode bits are: on a platform where this cannot be
/// asked, it passes rather than pretending to have checked — and says nothing,
/// because a gate that claims to have checked what it could not is the failure
/// this whole tree is written against. Windows is not a validator platform
/// here.
pub fn refuse_permissive_key_file(path: &Path) -> eyre::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(eyre::eyre!(
                "{} is mode {:o}: the consensus key is readable by other accounts on this host. Rotate the key \
                 (it may already have been read), then `chmod 600` the file. edet writes it 0600; something else \
                 changed it.",
                path.display(),
                mode
            ));
        }
    }
    let _ = path;
    Ok(())
}

// ---------------------------------------------------------------------------
// The founding ceremony's node side: keygen, then init
// ---------------------------------------------------------------------------

/// **Mint a consensus key file and return its public half**, which is what the
/// operator hands the genesis author.
///
/// The private half never leaves this function or the file, and the file is
/// created 0600 by `write_private_key` — the mode set before the bytes land,
/// because "briefly world-readable" is the same security property as
/// world-readable.
///
/// Refuses to overwrite. A key file that already exists is a validator's
/// identity in the genesis of some chain, and silently replacing it makes that
/// node's certificates unverifiable against a validator set it can no longer
/// sign for. The operator moves the old file themselves, which is the moment
/// to notice.
pub fn keygen(path: &Path) -> eyre::Result<[u8; 32]> {
    if path.exists() {
        return Err(eyre::eyre!(
            "{} already exists. A consensus key file is a validator's identity in some chain's genesis, and \
             overwriting one makes every certificate it signed unverifiable — move it aside first if you really \
             mean to replace it.",
            path.display()
        ));
    }
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
        }
    }
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| eyre::eyre!("could not draw a consensus key from the OS RNG: {e}"))?;
    write_private_key(path, seed)?;
    Ok(crate::block::pubkey_of(&seed))
}

/// **Write a node home from a real genesis, this operator's own key and real
/// addresses** — the step between `genesis init` and running a validator, and
/// the one an operator used to have to do by hand.
///
/// Every check here is one the node would otherwise make at first boot, moved
/// to where the message can name what to fix:
///
///   - a **dev chain id** without `--dev`, mirroring `genesis init`. The dev
///     concessions (the published root salt, the dev-key tripwire) key on the
///     chain id, so a home written against a harness genesis is a home that
///     will run on published keys;
///   - a **key the genesis does not name**. `get_address` asserts this at
///     boot, where the only thing it can do is panic; here it can say that the
///     public half printed by `keygen` never reached the ceremony;
///   - the **member key on the host**, which `get_address` also refuses — a
///     validator signs with its consensus key, and a member key on a server is
///     an economic identity on a server;
///   - a **name in the listen address**, which is an interface to bind and so
///     always a literal;
///   - a **peer without a peer id, or with one the genesis does not carry**.
///     A pair of validators keeps one connection and which side dials it is
///     read off the two ids (`split_by_dialer`), so every entry ends in the
///     `/p2p/<peer id>` `keygen` printed for that validator; an id no
///     validator derives to is a dial the handshake refuses for ever, quietly.
///
/// `--peers` may be names: `resolve_peer_names` reads them once at boot.
#[allow(clippy::too_many_arguments)]
pub fn init_home(
    home_dir: &Path,
    genesis_file: &Path,
    key_file: &Path,
    listen: &str,
    peers: &[String],
    moniker: Option<&str>,
    metrics_port: usize,
    dev: bool,
) -> eyre::Result<()> {
    let genesis: EdetGenesis = serde_json::from_str(&fs::read_to_string(genesis_file)?)
        .map_err(|e| eyre::eyre!("{} is not a genesis file: {e}", genesis_file.display()))?;
    if genesis.chain_id.starts_with(crate::block::DEV_CHAIN_ID) && !dev {
        return Err(eyre::eyre!(
            "genesis chain-id \"{}\" is a harness chain: it boots on edet's own published keys and published \
             state-root salt, which secure nothing. Pass --dev if that is really what you want.",
            genesis.chain_id
        ));
    }

    refuse_permissive_key_file(key_file)?;
    let key: EdetPrivateKeyFile = serde_json::from_str(&fs::read_to_string(key_file)?)
        .map_err(|e| eyre::eyre!("{} is not a consensus key file: {e}", key_file.display()))?;
    let public = crate::block::pubkey_of(&key.seed);
    if genesis.validators.iter().any(|v| v.public_key == public) {
        return Err(eyre::eyre!(
            "{} holds a MEMBER key: the genesis names it as a founder's economic identity, not as a validator's \
             signing key. A validator signs with its consensus key; `keygen` mints one.",
            key_file.display()
        ));
    }
    if !genesis.validators.iter().any(|v| v.consensus_key == Some(public)) {
        return Err(eyre::eyre!(
            "the consensus key in {} is not one this genesis names (public half {}). The hex `keygen` printed has \
             to reach the genesis author as the CONSENSUSKEYHEX field of this operator's --validator entry.",
            key_file.display(),
            crate::block::hex32(&public)
        ));
    }

    let listen_addr: Multiaddr = listen
        .parse()
        .map_err(|e| eyre::eyre!("--listen {listen} is not a multiaddr: {e}"))?;
    if listen_addr.iter().any(|p| is_name(&p)) {
        return Err(eyre::eyre!(
            "--listen {listen} carries a DNS name: a listen address is the interface to bind, which is always a \
             literal. Peers may be named; this may not."
        ));
    }
    let persistent_peers: Vec<Multiaddr> = peers
        .iter()
        .map(|p| {
            p.parse::<Multiaddr>()
                .map_err(|e| eyre::eyre!("--peers entry {p} is not a multiaddr: {e}"))
        })
        .collect::<eyre::Result<_>>()?;
    let own = peer_id_of_seed(&key.seed);
    split_by_dialer(&own, &persistent_peers)?;
    let known: Vec<(EdetAddress, PeerId)> = genesis
        .validators
        .iter()
        .filter_map(|v| v.consensus_key.map(|k| (v.address, k)))
        .map(|(a, k)| peer_id_of_public(&k).map(|id| (a, id)))
        .collect::<eyre::Result<_>>()?;
    for peer in &persistent_peers {
        let id = peer_id_in(peer).expect("split_by_dialer refused every entry without an id");
        if !known.iter().any(|(_, k)| *k == id) {
            let table: Vec<String> = known.iter().map(|(a, k)| format!("{} is {k}", a.0)).collect();
            return Err(eyre::eyre!(
                "--peers entry {peer} names peer id {id}, which is no genesis validator's. A peer id is derived \
                 from the consensus key the ceremony recorded; by address, they are: {}",
                table.join("; ")
            ));
        }
    }

    let moniker = moniker.map(str::to_string).unwrap_or_else(|| {
        genesis
            .validators
            .iter()
            .find(|v| v.consensus_key == Some(public))
            .map(|v| format!("edet-{}", v.address.0))
            .unwrap_or_else(|| "edet".to_string())
    });
    let config = node_config(moniker, listen_addr, persistent_peers, metrics_port);
    write_node_home(home_dir, &config, &genesis, key.seed)
}

/// Write `n` self-contained node homes under `home_dir/0`, `home_dir/1`, ...
/// (`home_dir/<i>/config/{config.toml,genesis.json,priv_validator_key.json}`),
/// all sharing one genesis built from edet's own dev founders
/// (`block::dev_seed`/`pubkey_of`, seeds `0..n-1`) rather than freshly
/// generated random keys — this is the edet-specific stand-in for the
/// vendored `testnet`/`init` commands (see this file's top doc comment for
/// why those aren't invoked directly). Always overwrites — callers wanting
/// to preserve an existing home should regenerate elsewhere.
pub fn write_testnet(home_dir: &Path, n: usize) -> eyre::Result<()> {
    if n == 0 {
        return Err(eyre::eyre!("a testnet needs at least 1 node"));
    }

    let validators: Vec<EdetGenesisEntry> = (0..n)
        .map(|i| {
            let seed = crate::block::dev_seed(i as u8);
            EdetGenesisEntry {
                address: EdetAddress(i as u64),
                public_key: crate::block::pubkey_of(&seed),
                consensus_key: Some(crate::block::pubkey_of(&crate::block::dev_consensus_seed(i as u8))),
                voting_power: 1,
            }
        })
        .collect();
    let genesis = EdetGenesis {
        underwriters: dev_underwriters(&validators),
        validators,
        chain_id: crate::block::DEV_CHAIN_ID.to_string(),
        root_salt: Some(DEV_ROOT_SALT),
        min_validators: None,
        seal_amounts: None,
    };

    for i in 0..n {
        write_node_home(
            &home_dir.join(i.to_string()),
            &make_config(i, n),
            &genesis,
            crate::block::dev_consensus_seed(i as u8),
        )?;
    }

    Ok(())
}

/// Cluster-harness helper: write `tx` (already signed) as every node's
/// `seed_tx.json` under a testnet `write_testnet` already wrote — the "submit
/// a transaction" step of the loopback cluster test/`just malachite-cluster`,
/// read back by `EdetApp::start` above. Not part of `write_testnet` itself
/// (ordinary testnets boot with empty mempools); called separately, after it.
pub fn write_seed_tx(home_dir: &Path, n: usize, tx: &SignedTx) -> eyre::Result<()> {
    let json = serde_json::to_string(tx)?;
    for i in 0..n {
        fs::write(home_dir.join(i.to_string()).join("seed_tx.json"), &json)?;
    }
    Ok(())
}

/// The default solo consensus port — distinct from `write_testnet`'s cluster
/// range so a solo node and a cluster never collide on one machine.
pub const SOLO_CONSENSUS_PORT: usize = 28901;

/// `write_solo_home_on` at the default `SOLO_CONSENSUS_PORT`.
pub fn write_solo_home(home_dir: &Path, extra_members: usize) -> eyre::Result<()> {
    write_solo_home_on(home_dir, extra_members, SOLO_CONSENSUS_PORT)
}

/// Write a single node home directly under `home_dir` (not `home_dir/0`): one
/// sole validator (founder 0, power 1 — so it reaches quorum alone) plus
/// `extra_members` non-validator genesis members (founders 1..=extra_members,
/// power 0), so transactions between real member ids have valid parties while
/// the one validator still commits by itself. For the embedded-app / live
/// -submission path (a single in-process node) and its tests.
///
/// `consensus_port` is explicit because these ports are fixed per home, not
/// negotiated: two solo nodes alive at once (two tests running in parallel,
/// a dev instance beside a test) must be given different ones or the second
/// one's libp2p listener fails to bind.
pub fn write_solo_home_on(home_dir: &Path, extra_members: usize, consensus_port: usize) -> eyre::Result<()> {
    let mut validators = vec![EdetGenesisEntry {
        address: EdetAddress(0),
        public_key: crate::block::pubkey_of(&crate::block::dev_seed(0)),
        consensus_key: Some(crate::block::pubkey_of(&crate::block::dev_consensus_seed(0))),
        voting_power: 1,
    }];
    for i in 1..=extra_members {
        validators.push(EdetGenesisEntry {
            address: EdetAddress(i as u64),
            public_key: crate::block::pubkey_of(&crate::block::dev_seed(i as u8)),
            consensus_key: None,
            voting_power: 0,
        });
    }
    let genesis = EdetGenesis {
        underwriters: dev_underwriters(&validators),
        validators,
        chain_id: crate::block::DEV_CHAIN_ID.to_string(),
        root_salt: Some(DEV_ROOT_SALT),
        min_validators: None,
        seal_amounts: None,
    };

    let mut config = make_config(0, 1);
    config.consensus.p2p.listen_addr = TransportProtocol::Tcp.multiaddr("127.0.0.1", consensus_port);
    config.consensus.p2p.persistent_peers = Vec::new();

    write_node_home(home_dir, &config, &genesis, crate::block::dev_consensus_seed(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn genesis(chain_id: &str, root_salt: Option<[u8; 32]>) -> EdetGenesis {
        let validators: Vec<EdetGenesisEntry> = (0..3u64)
            .map(|i| EdetGenesisEntry {
                address: EdetAddress(i),
                public_key: [i as u8 + 1; 32],
                consensus_key: Some([0xC0 | (i as u8 + 1); 32]),
                voting_power: 1,
            })
            .collect();
        EdetGenesis {
            underwriters: dev_underwriters(&validators),
            validators,
            chain_id: chain_id.to_string(),
            root_salt,
            min_validators: None,
            seal_amounts: None,
        }
    }

    // ------------------------------------------------ the ceremony ----

    /// A fresh directory per call. Tests inside one binary run in PARALLEL by
    /// default, so a name derived from the process alone is one directory two
    /// probes write different key files into.
    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "edet-ceremony-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("tmp dir");
        dir
    }

    /// A genesis naming `n` validators whose consensus keys are the ones in
    /// `keys`, so a probe can `init` against a real chain id.
    fn federation_genesis(keys: &[[u8; 32]]) -> EdetGenesis {
        let validators: Vec<EdetGenesisEntry> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| EdetGenesisEntry {
                address: EdetAddress(i as u64),
                public_key: [i as u8 + 1; 32],
                consensus_key: Some(*k),
                voting_power: 1,
            })
            .collect();
        EdetGenesis {
            underwriters: dev_underwriters(&validators),
            validators,
            chain_id: "edet-federation-probe".to_string(),
            root_salt: Some([9u8; 32]),
            min_validators: None,
            seal_amounts: None,
        }
    }

    /// A key file is a validator's identity in some chain's genesis. Replacing
    /// one silently makes every certificate it signed unverifiable against a
    /// validator set it can no longer sign for.
    #[test]
    fn keygen_refuses_to_overwrite() {
        let dir = tempdir();
        let path = dir.join("priv_validator_key.json");
        keygen(&path).expect("the first one is written");
        let err = keygen(&path).expect_err("the second must refuse").to_string();
        assert!(err.contains("already exists"), "the refusal must say what it found: {err}");
    }

    /// Two `keygen`s are two identities. A constant seed here would make every
    /// operator in a federation the same validator, which the genesis would
    /// then refuse as a duplicate key — but only if somebody noticed.
    #[test]
    fn keygen_mints_a_fresh_key_each_time() {
        let dir = tempdir();
        let a = keygen(&dir.join("a.json")).expect("a");
        let b = keygen(&dir.join("b.json")).expect("b");
        assert_ne!(a, b);
    }

    /// **The whole ceremony, end to end**: two operators mint keys, an author
    /// writes a genesis naming both, each operator writes their home, and the
    /// node's own loaders read every file back. This is what an operator used
    /// to have to hand-write.
    #[test]
    fn init_writes_a_home_the_node_can_load() {
        let dir = tempdir();
        let keys = [keygen(&dir.join("k0.json")).expect("k0"), keygen(&dir.join("k1.json")).expect("k1")];
        let genesis_file = dir.join("genesis.json");
        fs::write(&genesis_file, serde_json::to_string_pretty(&federation_genesis(&keys)).unwrap()).unwrap();

        let ids = [peer_id_of_public(&keys[0]).unwrap(), peer_id_of_public(&keys[1]).unwrap()];
        for i in 0..2usize {
            init_home(
                &dir.join(format!("node{i}")),
                &genesis_file,
                &dir.join(format!("k{i}.json")),
                &format!("/ip4/0.0.0.0/tcp/{}", 27100 + i),
                &[format!("/ip4/127.0.0.1/tcp/{}/p2p/{}", 27100 + (1 - i), ids[1 - i])],
                None,
                29800 + i,
                false,
            )
            .expect("init writes a home");
        }

        // Every read the node makes at boot, made here.
        for (i, want) in keys.iter().enumerate() {
            let raw = fs::read_to_string(dir.join(format!("node{i}/config/config.toml"))).unwrap();
            assert!(raw.contains(&ids[1 - i].to_string()), "the config lists the peer whichever side dials: {raw}");
            let app = EdetApp::at(dir.join(format!("node{i}")), None, None);
            let config = app.load_config().expect("config loads, peers resolve");
            assert!(!config.consensus.p2p.discovery.enabled);
            // The pair keeps one connection: the lower id dials, the other lists nobody.
            let dials = ids[i] < ids[1 - i];
            assert_eq!(
                config.consensus.p2p.persistent_peers.len(),
                usize::from(dials),
                "node {i} dials exactly when its peer id is the lower"
            );
            let genesis = app.load_genesis().expect("genesis loads");
            assert_eq!(genesis.chain_id, "edet-federation-probe");
            let key = app.load_private_key_file().expect("key loads at 0600");
            let public = crate::block::pubkey_of(&key.seed);
            assert_eq!(&public, want, "each home holds its own operator's key");
            assert_eq!(app.get_address(&VerifyingKey::from_bytes(&public).unwrap()), EdetAddress(i as u64));
        }
    }

    /// The public half `keygen` printed has to actually reach the ceremony.
    /// `get_address` asserts this at boot, where all it can do is panic; here
    /// the message can say which file holds the wrong key.
    #[test]
    fn init_refuses_a_key_the_genesis_does_not_name() {
        let dir = tempdir();
        let named = keygen(&dir.join("named.json")).expect("named");
        let stranger = dir.join("stranger.json");
        keygen(&stranger).expect("stranger");
        let genesis_file = dir.join("genesis.json");
        fs::write(&genesis_file, serde_json::to_string_pretty(&federation_genesis(&[named])).unwrap()).unwrap();

        let err =
            init_home(&dir.join("node"), &genesis_file, &stranger, "/ip4/0.0.0.0/tcp/27110", &[], None, 29810, false)
                .expect_err("must refuse")
                .to_string();
        assert!(err.contains("not one this genesis names"), "the refusal must say what is wrong: {err}");
    }

    /// A member key on a server is an economic identity on a server, and this
    /// is the door it would come through: an operator who put the wrong file
    /// in `--key`.
    #[test]
    fn init_refuses_a_member_key_on_the_host() {
        let dir = tempdir();
        let consensus = keygen(&dir.join("c.json")).expect("c");
        let mut genesis = federation_genesis(&[consensus]);
        // The member key the genesis names, written where a consensus key
        // belongs.
        let member_seed = [42u8; 32];
        genesis.validators[0].public_key = crate::block::pubkey_of(&member_seed);
        let genesis_file = dir.join("genesis.json");
        fs::write(&genesis_file, serde_json::to_string_pretty(&genesis).unwrap()).unwrap();
        let member_file = dir.join("member.json");
        write_private_key(&member_file, member_seed).unwrap();

        let err = init_home(
            &dir.join("node"),
            &genesis_file,
            &member_file,
            "/ip4/0.0.0.0/tcp/27120",
            &[],
            None,
            29820,
            false,
        )
        .expect_err("must refuse")
        .to_string();
        assert!(err.contains("MEMBER key"), "the refusal must name what it found: {err}");
    }

    /// A listen address is the interface to bind, which is always a literal.
    /// Refused here as well as at boot, because here the message can say that
    /// peers may be names and this may not.
    #[test]
    fn init_refuses_a_dns_listen_address() {
        let dir = tempdir();
        let k = keygen(&dir.join("k.json")).expect("k");
        let genesis_file = dir.join("genesis.json");
        fs::write(&genesis_file, serde_json::to_string_pretty(&federation_genesis(&[k])).unwrap()).unwrap();

        let err = init_home(
            &dir.join("node"),
            &genesis_file,
            &dir.join("k.json"),
            "/dns4/validator.example/tcp/27130",
            &[],
            None,
            29830,
            false,
        )
        .expect_err("must refuse")
        .to_string();
        assert!(err.contains("listen"), "the refusal must say which address: {err}");
    }

    /// The dev concessions — the published root salt and the dev-key tripwire
    /// — key on the chain id, so a home written against a harness genesis is a
    /// home that runs on published keys.
    #[test]
    fn init_refuses_a_harness_genesis_without_dev() {
        let dir = tempdir();
        let k = keygen(&dir.join("k.json")).expect("k");
        let mut g = federation_genesis(&[k]);
        g.chain_id = crate::block::DEV_CHAIN_ID.to_string();
        let genesis_file = dir.join("genesis.json");
        fs::write(&genesis_file, serde_json::to_string_pretty(&g).unwrap()).unwrap();

        let err = init_home(
            &dir.join("node"),
            &genesis_file,
            &dir.join("k.json"),
            "/ip4/0.0.0.0/tcp/27140",
            &[],
            None,
            29840,
            false,
        )
        .expect_err("must refuse")
        .to_string();
        assert!(err.contains("harness chain"), "the refusal must say what kind of chain: {err}");
    }

    /// A peer named by DNS is written through unchanged, because
    /// `resolve_peer_names` reads it once at boot. `init` refusing one would
    /// make the whole boot-time resolution pointless.
    #[test]
    fn init_writes_a_named_peer_through_for_boot_to_resolve() {
        let dir = tempdir();
        // Two fixed seeds; the node takes the lower peer id so that it is the
        // side that dials, and the peer's id is the higher.
        let (mine, theirs) = {
            let (a, b) = ([0x51u8; 32], [0x52u8; 32]);
            if peer_id_of_seed(&a) < peer_id_of_seed(&b) {
                (a, b)
            } else {
                (b, a)
            }
        };
        write_private_key(&dir.join("k.json"), mine).expect("k");
        let genesis_file = dir.join("genesis.json");
        let g = federation_genesis(&[crate::block::pubkey_of(&mine), crate::block::pubkey_of(&theirs)]);
        fs::write(&genesis_file, serde_json::to_string_pretty(&g).unwrap()).unwrap();
        let peer = format!("/dns4/localhost/tcp/27151/p2p/{}", peer_id_of_seed(&theirs));
        init_home(
            &dir.join("node"),
            &genesis_file,
            &dir.join("k.json"),
            "/ip4/0.0.0.0/tcp/27150",
            std::slice::from_ref(&peer),
            Some("kept"),
            29850,
            false,
        )
        .expect("init");

        let raw = fs::read_to_string(dir.join("node/config/config.toml")).unwrap();
        assert!(raw.contains(&peer), "the name is written as given: {raw}");
        let config = EdetApp::at(dir.join("node"), None, None)
            .load_config()
            .expect("boot resolves it");
        assert_eq!(config.moniker, "kept");
        assert_eq!(
            config.consensus.p2p.persistent_peers[0].to_string(),
            format!("/ip4/127.0.0.1/tcp/27151/p2p/{}", peer_id_of_seed(&theirs))
        );
    }

    /// `init` refuses a peer id the genesis does not carry: a pasted id that
    /// belongs to nobody is a dial the handshake refuses for ever, quietly,
    /// and the refusal lists the ids the ceremony did produce.
    #[test]
    fn init_refuses_a_peer_the_genesis_does_not_name() {
        let dir = tempdir();
        let k = keygen(&dir.join("k.json")).expect("k");
        let genesis_file = dir.join("genesis.json");
        fs::write(&genesis_file, serde_json::to_string_pretty(&federation_genesis(&[k])).unwrap()).unwrap();
        let stranger = peer_id_of_seed(&[0x53u8; 32]);
        let err = init_home(
            &dir.join("node"),
            &genesis_file,
            &dir.join("k.json"),
            "/ip4/0.0.0.0/tcp/27160",
            &[format!("/ip4/127.0.0.1/tcp/27161/p2p/{stranger}")],
            None,
            29860,
            false,
        )
        .expect_err("must refuse")
        .to_string();
        assert!(err.contains("no genesis validator's"), "the refusal must say what is wrong: {err}");
        assert!(err.contains(&peer_id_of_public(&k).unwrap().to_string()), "and list the ids that exist: {err}");
        assert!(!dir.join("node/config").exists(), "a refused init writes no home");
    }

    // -------------------------------------------- one dialer per pair ----

    /// **Over the whole set, every pair has exactly one dialer.** Sixteen
    /// validators, each listing the other fifteen; the rule applied on every
    /// side must leave each unordered pair dialed from exactly one end. The
    /// property is over PAIRS, and a probe on one node cannot see it.
    ///
    /// Mutation that bites: keep an entry whose id is lower too, or drop the
    /// comparison and keep them all. Some pair is then dialed from both ends.
    #[test]
    fn every_pair_of_validators_has_exactly_one_dialer() {
        const N: usize = 16;
        let ids: Vec<PeerId> = (0..N)
            .map(|i| peer_id_of_seed(&crate::block::dev_consensus_seed(i as u8)))
            .collect();
        let addr = |j: usize| -> Multiaddr {
            format!("/ip4/127.0.0.1/tcp/{}/p2p/{}", 26600 + j, ids[j])
                .parse()
                .expect("a multiaddr")
        };
        let mut dialers = std::collections::HashMap::new();
        for (i, own) in ids.iter().enumerate() {
            let peers: Vec<Multiaddr> = (0..N).filter(|&j| j != i).map(addr).collect();
            let (dials, dialed_by) = split_by_dialer(own, &peers).expect("every entry carries an id");
            assert_eq!(dials.len() + dialed_by.len(), N - 1, "nothing is lost or invented");
            for d in dials {
                let j = (0..N).find(|&j| addr(j) == d).expect("a configured peer");
                *dialers.entry((i.min(j), i.max(j))).or_insert(0) += 1;
            }
        }
        for i in 0..N {
            for j in i + 1..N {
                assert_eq!(dialers.get(&(i, j)).copied().unwrap_or(0), 1, "pair ({i}, {j}) has exactly one dialer");
            }
        }
    }

    /// A peer without a `/p2p/<peer id>` cannot be placed on either side of
    /// the rule, and dialing it anyway is the two-connection case again.
    #[test]
    fn a_peer_without_a_peer_id_is_refused() {
        let local = peer_id_of_seed(&crate::block::dev_consensus_seed(0));
        let peers: Vec<Multiaddr> = vec!["/ip4/127.0.0.1/tcp/1".parse().unwrap()];
        let err = split_by_dialer(&local, &peers).expect_err("must refuse").to_string();
        assert!(err.contains("/p2p/"), "the refusal must say what is missing: {err}");
    }

    /// An entry naming this node is a config copied from another operator;
    /// upstream would skip the self-dial silently.
    #[test]
    fn a_peer_naming_this_node_is_refused() {
        let local = peer_id_of_seed(&crate::block::dev_consensus_seed(0));
        let peers: Vec<Multiaddr> = vec![format!("/ip4/127.0.0.1/tcp/1/p2p/{local}").parse().unwrap()];
        let err = split_by_dialer(&local, &peers).expect_err("must refuse").to_string();
        assert!(err.contains("own peer id"), "{err}");
    }

    /// The id `keygen` prints from the seed and the id `init` derives from
    /// the genesis's public key are one id, or an operator's entry never
    /// matches the dial.
    #[test]
    fn a_seed_and_its_public_key_derive_the_same_peer_id() {
        let seed = crate::block::dev_consensus_seed(7);
        assert_eq!(peer_id_of_seed(&seed), peer_id_of_public(&crate::block::pubkey_of(&seed)).unwrap());
    }

    /// **A testnet home dials by the rule a ceremony home does.** `make_config`
    /// names every peer by id, the file lists all of them, and loading node
    /// `i` keeps exactly the peers whose id is above its own.
    #[test]
    fn a_testnet_home_lists_every_peer_and_dials_the_ones_above_it() {
        let dir = tempdir();
        write_testnet(&dir, 4).expect("a testnet");
        let id = |j: usize| peer_id_of_seed(&crate::block::dev_consensus_seed(j as u8));
        let base = consensus_base_port();
        for i in 0..4usize {
            let raw = fs::read_to_string(dir.join(format!("{i}/config/config.toml"))).unwrap();
            for j in (0..4).filter(|&j| j != i) {
                assert!(raw.contains(&id(j).to_string()), "node {i} lists peer {j} whichever side dials: {raw}");
            }
            let config = EdetApp::at(dir.join(i.to_string()), None, None).load_config().expect("loads");
            let want: Vec<String> = (0..4)
                .filter(|&j| j != i && id(j) > id(i))
                .map(|j| format!("/ip4/127.0.0.1/tcp/{}/p2p/{}", base + j, id(j)))
                .collect();
            let got: Vec<String> = config.consensus.p2p.persistent_peers.iter().map(|p| p.to_string()).collect();
            assert_eq!(got, want, "node {i} dials exactly the peers above it");
        }
    }

    // ----------------------------------------------------- peer names ----

    /// A config carrying one persistent peer, so a probe can say what the
    /// transport would be handed.
    fn config_with_peer(peer: &str) -> EdetConfig {
        let mut c = make_config(0, 2);
        c.consensus.p2p.persistent_peers = vec![peer.parse().expect("a multiaddr")];
        c
    }

    /// **The transport never dials a name.** This is the property `just
    /// audit` carries RUSTSEC-2026-0119 on, and it is a fact about the code
    /// rather than about the configurations this repo happens to write.
    ///
    /// Mutation that bites: have `resolve_peer_names` return `Ok(())` before
    /// it rewrites anything. The peer reads back as `/dns4/localhost/...` and
    /// the name reaches libp2p.
    #[test]
    fn a_dns_peer_is_resolved_at_boot_and_never_dialed_by_name() {
        let mut c = config_with_peer("/dns4/localhost/tcp/1");
        resolve_peer_names(&mut c).expect("localhost resolves on any machine that has a loopback");
        let peer = c.consensus.p2p.persistent_peers[0].to_string();
        assert_eq!(peer, "/ip4/127.0.0.1/tcp/1", "the name must be gone by the time the transport sees it");
    }

    /// The trailing `/p2p/<peer id>` is what tells the transport WHO it
    /// expects to find at the address, so a rewrite that dropped it would
    /// turn an authenticated dial into an anonymous one.
    #[test]
    fn resolving_a_peer_keeps_every_other_component() {
        let id = "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN";
        let mut c = config_with_peer(&format!("/dns4/localhost/tcp/26600/p2p/{id}"));
        resolve_peer_names(&mut c).expect("resolves");
        assert_eq!(c.consensus.p2p.persistent_peers[0].to_string(), format!("/ip4/127.0.0.1/tcp/26600/p2p/{id}"));
    }

    /// A `/dnsaddr/` resolves through a TXT lookup to FURTHER multiaddrs, so
    /// there is no `(host, port)` to hand the system resolver and nothing to
    /// rewrite. Refused rather than passed through, because passing it
    /// through is exactly the case where libp2p resolves it itself.
    #[test]
    fn a_dnsaddr_peer_is_refused() {
        let mut c = config_with_peer("/dnsaddr/bootstrap.example/tcp/1");
        let err = resolve_peer_names(&mut c).expect_err("must refuse").to_string();
        assert!(err.contains("dnsaddr"), "the refusal must name what it refused: {err}");
    }

    /// Discovery learns addresses from peers at run time, and a discovered
    /// address may be a name — so no boot-time rewrite can reach it.
    #[test]
    fn discovery_cannot_be_enabled() {
        let mut c = make_config(0, 2);
        c.consensus.p2p.discovery.enabled = true;
        let err = resolve_peer_names(&mut c).expect_err("must refuse").to_string();
        assert!(err.contains("discovery"), "the refusal must name discovery: {err}");
    }

    /// A name that does not resolve is a node that cannot reach its peer, and
    /// saying so at boot is better than a silent half-connected mesh. `.invalid`
    /// is reserved by RFC 2606 and resolves nowhere by construction.
    #[test]
    fn an_unresolvable_name_refuses_to_boot() {
        let mut c = config_with_peer("/dns4/no-such-validator.invalid/tcp/26600");
        let err = resolve_peer_names(&mut c).expect_err("must refuse").to_string();
        assert!(err.contains("no-such-validator.invalid"), "the refusal must name the host: {err}");
    }

    /// A listen address is an interface to bind rather than an endpoint to
    /// reach, and a name is never one.
    #[test]
    fn a_dns_listen_address_is_refused() {
        let mut c = make_config(0, 2);
        c.consensus.p2p.listen_addr = "/dns4/localhost/tcp/26600".parse().expect("a multiaddr");
        let err = resolve_peer_names(&mut c).expect_err("must refuse").to_string();
        assert!(err.contains("listen address"), "the refusal must say which address: {err}");
    }

    /// A peer that already names a literal is left exactly as written: this
    /// function rewrites names, and every testnet home in the tree is
    /// `/ip4/`.
    #[test]
    fn a_literal_peer_passes_through_untouched() {
        let before = make_config(0, 3);
        let mut after = before.clone();
        resolve_peer_names(&mut after).expect("no name to resolve");
        assert_eq!(before, after);
    }

    /// The state-root salt is what keeps an inclusion proof from disclosing
    /// the records next to the one being proved
    /// (the paper's §Implementation). The dev value is
    /// published, so a real chain inheriting it would publish that property
    /// away — silently, since everything would still agree and every proof
    /// would still verify. It has to fail at boot instead.
    #[test]
    fn a_real_chain_may_not_boot_without_its_own_root_salt() {
        let err = genesis_state(&genesis("edet-pilot", None)).expect_err("must refuse");
        let msg = err.to_string();
        assert!(msg.contains("root_salt"), "the error must name the missing field: {msg}");
        assert!(msg.contains("genesis init"), "and say where one comes from: {msg}");
    }

    #[test]
    fn a_real_chain_carrying_its_own_salt_boots_with_it() {
        let salt = [7u8; 32];
        let state = genesis_state(&genesis("edet-pilot", Some(salt))).expect("boots");
        assert_eq!(state.root_salt, salt);
    }

    /// A dev chain falls back: its privacy is already nil (`block::dev_seed`
    /// is published), and reproducible roots across runs are worth more there
    /// than a secret that would not be one.
    #[test]
    fn a_dev_chain_falls_back_to_the_published_salt() {
        let state = genesis_state(&genesis("edet-dev", None)).expect("boots");
        assert_eq!(state.root_salt, DEV_ROOT_SALT);
    }

    /// The presence-vs-value half. The guard above only ever saw the
    /// `None` case, so the cheapest way past it — copy a dev genesis, change
    /// the chain id, keep the salt — booted a real chain on the published
    /// value with everything still agreeing. The field being SET is not the
    /// property; the value not being the published one is.
    #[test]
    fn a_real_chain_may_not_boot_on_the_published_dev_salt() {
        let err = genesis_state(&genesis("edet-pilot", Some(DEV_ROOT_SALT))).expect_err("must refuse");
        let msg = err.to_string();
        assert!(msg.contains("dev root_salt"), "the error must name what is wrong with the value: {msg}");
        assert!(msg.contains("genesis init"), "and say where a real one comes from: {msg}");
    }

    /// An all-zero salt is an unset field wearing the shape of a set one —
    /// the natural result of hand-writing a genesis file or of a serializer
    /// default. Refused on every chain, dev included: nothing legitimately
    /// produces it.
    #[test]
    fn an_all_zero_salt_is_refused_as_unset() {
        for chain in ["edet-pilot", "edet-dev"] {
            let err = genesis_state(&genesis(chain, Some([0u8; 32]))).expect_err("must refuse");
            assert!(err.to_string().contains("all-zero"), "chain {chain}: {err}");
        }
    }

    /// The prefix half. `starts_with("edet-dev")` is true of ordinary
    /// real chain ids — a community named "devon", a "development fund" —
    /// and treating them as dev chains disables BOTH the salt requirement
    /// here and the published-dev-key tripwire in `EdetApp::start`. The test
    /// is exact equality, so these are real chains and must supply their own
    /// salt.
    #[test]
    fn a_chain_id_that_merely_begins_with_the_dev_prefix_is_not_a_dev_chain() {
        for chain in ["edet-devon-mutual-credit", "edet-development-fund", "edet-dev-2", "edet-dev "] {
            assert!(!is_dev_chain(chain), "{chain} must not be treated as the harness dev chain");
            let err = genesis_state(&genesis(chain, None)).expect_err("must refuse a real chain with no salt");
            assert!(err.to_string().contains("root_salt"), "chain {chain}: {err}");
        }
        assert!(is_dev_chain(crate::block::DEV_CHAIN_ID), "the one harness chain id must still be exempt");
    }

    /// The second protection the prefix match silently disabled: a chain
    /// merely NAMED like a dev chain kept its published-entropy validator
    /// keys, for the life of the chain, with no boot-time complaint. The
    /// tripwire fires on exact chain-id equality, so only the harness chain
    /// is exempt.
    #[test]
    fn published_dev_keys_are_refused_on_every_chain_but_the_harness_one() {
        let dev_keyed = |chain: &str| EdetGenesis {
            underwriters: Vec::new(),
            validators: vec![EdetGenesisEntry {
                address: EdetAddress(0),
                public_key: crate::block::pubkey_of(&crate::block::dev_seed(0)),
                consensus_key: Some(crate::block::pubkey_of(&crate::block::dev_consensus_seed(0))),
                voting_power: 1,
            }],
            chain_id: chain.to_string(),
            root_salt: Some([9u8; 32]),
            min_validators: None,
            seal_amounts: None,
        };

        refuse_published_dev_keys(&dev_keyed("edet-dev")).expect("the harness chain legitimately uses dev keys");
        for chain in ["edet-devon-mutual-credit", "edet-development-fund", "edet-pilot"] {
            let err = refuse_published_dev_keys(&dev_keyed(chain)).expect_err("must refuse");
            assert!(err.to_string().contains("dev key"), "chain {chain}: {err}");
        }
    }

    /// The salt has to actually reach the root, or none of the above buys
    /// anything: two chains identical but for their salt must not share one.
    #[test]
    fn the_genesis_salt_reaches_the_state_root() {
        let a = genesis_state(&genesis("edet-pilot", Some([1u8; 32]))).expect("boots");
        let b = genesis_state(&genesis("edet-pilot", Some([2u8; 32]))).expect("boots");
        assert_ne!(
            crate::block::state_hash(&a).expect("root"),
            crate::block::state_hash(&b).expect("root"),
            "the genesis salt must be reflected in the state root"
        );
    }

    // ------------------------------------------------ two keys, not one --

    /// **A validator signs with a key that is not its member key**, and the
    /// genesis file is where that separation is made or lost.
    ///
    /// A genesis entry carrying ONE key would seat it as the member's own
    /// (`add_underwriter(vec![entry.public_key], supply)`) and hand it to
    /// `EdetValidatorSet::build` to verify certificates — making the
    /// unencrypted seed in `priv_validator_key.json`, on an internet-facing
    /// host, the economic identity that signs `Accept`, `Settle` and
    /// `DeclareSupply`, and making a restore of that founder's recovery phrase
    /// into the wallet a hot consensus key on a handset.
    #[test]
    fn a_genesis_entry_may_not_use_one_key_for_both_roles() {
        let mut g = genesis("edet-pilot", Some([7u8; 32]));
        g.validators[1].consensus_key = Some(g.validators[1].public_key);
        let err = genesis_state(&g).expect_err("must refuse");
        assert!(err.to_string().contains("both its member identity and its validator"), "{err}");
    }

    /// Power with no consensus key is a validator the ledger cannot name a
    /// signing key for — which `EdetValidatorSet::build` refuses to construct
    /// a set from, so it must never be written into one.
    #[test]
    fn a_genesis_validator_needs_a_consensus_key() {
        let mut g = genesis("edet-pilot", Some([7u8; 32]));
        g.validators[2].consensus_key = None;
        let err = genesis_state(&g).expect_err("must refuse");
        assert!(err.to_string().contains("no consensus_key"), "{err}");

        // Power zero is the legitimate case: a founder who underwrites and
        // runs nothing.
        g.validators[2].voting_power = 0;
        let state = genesis_state(&g).expect("a founder with no validator is fine");
        assert!(!state.validators.contains_key(&2));
        assert_eq!(state.members[&2].consensus_key, None);
    }

    /// The dev-key tripwire reads BOTH keys. It read one while there was one,
    /// and a published consensus key secures a real chain exactly as little as
    /// a published member key does.
    #[test]
    fn the_dev_key_tripwire_covers_the_consensus_key_too() {
        let mut g = genesis("edet-pilot", Some([7u8; 32]));
        g.validators[0].consensus_key = Some(crate::block::pubkey_of(&crate::block::dev_consensus_seed(3)));
        let err = refuse_published_dev_keys(&g).expect_err("must refuse");
        assert!(err.to_string().contains("dev key"), "{err}");
    }

    /// **A consensus key any other account on the host can read is refused at
    /// boot**, and edet writes it 0600 in the first place.
    ///
    /// Refused rather than silently repaired: a key that has been readable is a
    /// key that may already have been read, and `chmod`-ing it here would hide
    /// that from the operator who has to decide whether to rotate.
    #[cfg(unix)]
    #[test]
    fn a_world_readable_consensus_key_refuses_to_boot() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("edet-keymode-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("tmp dir");
        let path = dir.join("priv_validator_key.json");
        write_private_key(&path, [3u8; 32]).expect("write");
        assert_eq!(
            fs::metadata(&path).expect("stat").permissions().mode() & 0o777,
            0o600,
            "edet must write the consensus key owner-only"
        );
        refuse_permissive_key_file(&path).expect("0600 boots");

        for mode in [0o644, 0o640, 0o604, 0o666] {
            fs::set_permissions(&path, PermissionsExt::from_mode(mode)).expect("chmod");
            let err = refuse_permissive_key_file(&path).expect_err("must refuse mode {mode:o}");
            assert!(err.to_string().contains("readable by other accounts"), "{err}");
        }
        fs::remove_dir_all(&dir).ok();
    }

    // ---------------------------------------------------- the founding roll --

    /// **A ceremony seats the whole roll at once, so the ceiling is a question
    /// about the SUM.** Every transition that names an amount refuses one the
    /// boundary cannot express, and each of those was written for a payload
    /// two parties signed. Genesis has no such payload: three founders each
    /// well inside the ceiling declare past it between them, and the roll is
    /// what governance weight, the amendment rate and every capacity in the
    /// community are a fraction of.
    #[test]
    fn a_founding_roll_past_the_ceiling_is_refused() {
        let ceiling = State::from_minor(edet_kernel::constants::MAX_AMOUNT_MINOR);
        check_seed_total([ceiling]).expect("the ceiling itself is a legal roll");
        check_seed_total([ceiling / 3.0, ceiling / 3.0, ceiling / 3.0]).expect("and so is a roll that shares it");

        let err = check_seed_total([ceiling / 2.0, ceiling / 2.0, ceiling / 2.0]).expect_err("must refuse");
        assert!(err.contains("between them"), "the message must name the total: {err}");

        let mut g = genesis("edet-pilot", Some([7u8; 32]));
        for uw in &mut g.underwriters {
            uw.supply = ceiling / 2.0;
        }
        let err = genesis_state(&g).expect_err("and a boot on such a file must refuse too");
        assert!(err.to_string().contains("between them"), "{err}");
    }

    /// The per-supply half, which the sum cannot make: `to_minor` saturates,
    /// so one founder naming `1e300` would otherwise be counted as `u64::MAX`
    /// and reported as an oversized ROLL rather than as the mis-typed figure
    /// it is.
    #[test]
    fn a_founding_supply_the_boundary_cannot_name_is_refused_by_itself() {
        let mut g = genesis("edet-pilot", Some([7u8; 32]));
        g.underwriters[0].supply = 1e300;
        let err = genesis_state(&g).expect_err("must refuse");
        let msg = err.to_string();
        assert!(msg.contains("declares a supply of"), "the message must name the underwriter: {msg}");
        assert!(msg.contains("at most"), "and what the ledger can name: {msg}");
    }
}
