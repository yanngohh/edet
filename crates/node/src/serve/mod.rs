//! The node's client surface: an HTTP API over the replica the consensus
//! engine drives, plus the shared `Node`/`NodeCore` handle the Tauri app
//! reads in-process. Enabled by the `serve` feature, which the `malachite`
//! feature implies — the engine builds its client API out of this module.
//!
//! It contains no consensus. A dev-only networked consensus for
//! the local multi-node harness lived here, and `just dev`, the desktop
//! client and most of this crate's tests all ran on it rather than on the
//! engine we deploy. Two rules that held only on that driver reached a
//! release before it was removed.

mod auth;
mod core;
mod driver;
mod http;
pub mod pending;
mod ratelimit;
mod replay;
mod session;
#[cfg(test)]
mod tests;
pub mod views;

pub use auth::{Viewer, ViewerAuthError, ViewerParty};
pub use core::NodeCore;
pub use driver::{pending_decline, pending_sign, submit, Node};
pub use http::{client_router, Read};
pub use pending::{PendingDeclineReq, PendingSignReq};

/// Configuration for one node's client surface.
#[derive(Clone, Debug)]
pub struct Config {
    /// This node's slot in the validator set (its `EdetAddress`), for display
    /// and for skipping its own entry in `peers`.
    pub index: usize,
    /// Validator count, for display (`/network`).
    pub n: usize,
    pub listen_port: u16,
    /// Peer base URLs (one per validator; this node's own slot is ignored).
    /// TRANSACTION-gossip targets, not consensus peers — see `driver`.
    pub peers: Vec<String>,
    /// Accept transactions without checking their Ed25519 signatures.
    /// Harness escape hatch for shell-scripted smoke tests only — every
    /// real path (UI, tests) signs and leaves this off.
    pub allow_unsigned: bool,
    /// Directory for the durable WAL + snapshots. `None` keeps the
    /// replica in-memory only — a restart then loses the whole ledger, which
    /// is why every real launcher (the `serve` binary, the Tauri app) sets
    /// this; only ad-hoc/test configs leave it unset.
    pub data_dir: Option<String>,
    /// Write a state snapshot every this many committed blocks (0 = never;
    /// the WAL alone still replays deterministically, just from further
    /// back). Ignored when `data_dir` is `None`.
    pub snapshot_interval: u64,
    /// **How far back the WAL is kept**, in blocks — and therefore how long
    /// this validator may be down and still rejoin from a peer rather than
    /// from an operator carrying a snapshot. `crate::replica::
    /// DEFAULT_PRUNE_MARGIN_BLOCKS` is one day at the pace an empty block is
    /// held to; `--prune-margin-blocks` moves it, and it is floored at two
    /// snapshot intervals whatever it says.
    pub prune_margin_blocks: u64,
    /// Bind the HTTP listener on every interface (`0.0.0.0`) instead of
    /// loopback-only. Default `false`: the gossip surface (`/p2p/*`)
    /// carries no per-message signature of its own — see `cluster_token` —
    /// so the safe default keeps it reachable only from this device's own
    /// processes (its UI/Tauri client, and, on a same-host cluster, its
    /// sibling node processes). Set only for a genuine multi-machine
    /// deployment, behind a firewall or with `cluster_token` set; the CLI
    /// prints a warning when it's on.
    pub bind_all: bool,
    /// Extra localhost UI dev-server ports (beyond the hardcoded default,
    /// 5173) to allow over CORS. The multi-instance `just dev N` harness
    /// runs one Vite dev server per node on `5173+i`, each a distinct
    /// origin that must reach only its own node's HTTP API.
    pub cors_ports: Vec<u16>,
    /// Shared secret every `/p2p/*` request must carry, as the
    /// `x-edet-cluster-token` header, when set. `None` (the default)
    /// falls back to gating `/p2p/*` by source address (loopback or a
    /// configured peer host) instead — acceptable only because the default
    /// bind is loopback-only; a `bind_all` deployment should set a token.
    ///
    /// A perimeter for the gossip envelope, never message authentication:
    /// what travels here is transactions and co-signatures, each verified on
    /// its own signature at the ingress regardless of how it arrived.
    /// Byzantine-authenticated CONSENSUS messages are Malachite's, on its own
    /// mesh, and never cross this surface.
    pub cluster_token: Option<String>,
    /// Trust `X-Forwarded-For` for the per-IP read budget, because this node
    /// sits behind a reverse proxy that sets it.
    ///
    /// **Default `false`, and the default is the safe one.** A header any
    /// client can set is a fresh rate-limit bucket per request when it is
    /// trusted and nothing rewrites it — which turns the read budget off. The
    /// cost of leaving it off behind a proxy is the mirror image and much
    /// smaller: every reader shares the proxy's one bucket, so the community
    /// throttles itself rather than the node being defenceless.
    ///
    /// **This is an operator's assertion about their own deployment**, and it
    /// is only sound when the proxy OVERWRITES the header rather than
    /// appending to it, and nothing but the proxy can reach the node's port.
    /// Both are properties of the deployment that no code here can check,
    /// which is why it is a flag and not a heuristic.
    ///
    /// Transport security is the same kind of statement. The client API is
    /// plain HTTP: bearer tokens and viewer signatures travel in clear. A
    /// viewer signature read off the wire is at least already SPENT — each
    /// verified one is admitted once inside its window (`replay.rs`) — but a
    /// bearer token is replayable for its whole TTL and nothing here hides
    /// what a read returned. A deployment reachable from anywhere but this
    /// host must terminate TLS in front of the node — see `README.md`, which
    /// carries a reference configuration.
    pub trust_forwarded_for: bool,
}

/// Seed a dev genesis: `members` founding economic members, all Active.
/// Each founder's on-ledger key derives from a real BIP39 recovery phrase
/// (published dev entropy — see `block::dev_phrase`): a founder is entered
/// through the ordinary restore-from-phrase flow, and signatures verify
/// like anyone else's.
pub fn dev_genesis(members: u8) -> edet_state::State {
    let mut st = edet_state::State::default();
    for i in 0..members {
        let key = crate::block::pubkey_of(&crate::block::dev_seed(i));
        // Every dev founder is also a founding underwriter. Without a supply
        // the chain is inert by arithmetic — no source arc means no capacity,
        // nothing conferrable, no stake ever written — so a dev ledger with no
        // underwriter would make every test measure the same zero, and pass.
        let Ok(id) = st.add_underwriter(vec![key], DEV_SUPPLY) else { continue };
        // Seed every founder as a genesis validator too, or the ledger's
        // validator set would be empty while blocks commit — governance's
        // validator bookkeeping (Tx::ValidatorPower, Suspend/Unsuspend) would
        // have nothing real to act on, and the "validators are active members"
        // invariant (state::invariants::audit) would hold only vacuously.
        //
        // It is also the trap this branch paid for twice: EVERY founder is a
        // validator here, so a harness that acts as a founder cannot tell a
        // validator-privileged rule from an ordinary one. A test of such a
        // rule must admit a real member and assert `is_validator == false`
        // first — see `just e2e`.
        let _ = st.set_consensus_key(id, crate::block::pubkey_of(&crate::block::dev_consensus_seed(id as u8)));
        let _ = st.set_genesis_validator(id, 1);
    }
    st
}

/// The supply each dev founder declares. A round number well above
/// `v_base`, so dev traffic exercises the INSURED path rather than falling to
/// the uninsured tier and quietly testing nothing.
pub const DEV_SUPPLY: f64 = 25_000.0;

/// Build a node without starting anything — for an embedder (the Tauri
/// backend) that wants to hold the `Arc<Node>` and read it in-process. Opens
/// the durable replica when `cfg.data_dir` is set; fails only if that
/// store exists and is unreadable (a fresh directory is fine).
pub fn build(cfg: Config, genesis: edet_state::State) -> Result<std::sync::Arc<Node>, crate::replica::ReplicaError> {
    Node::open(cfg, genesis)
}

/// Bind host for the listener: loopback unless the config opts into every
/// interface (see `Config::bind_all`).
fn bind_host(cfg: &Config) -> &'static str {
    if cfg.bind_all {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    }
}

/// Start the client HTTP API for a node whose blocks the embedded Malachite
/// engine (`engine_node::EdetApp`) decides — the engine drives this very
/// `Arc<Node>`. Binds `http::client_router` and starts nothing else: the only
/// writer of blocks into this core is the engine's `Decided` handler
/// (`NodeCore::commit_decided`).
///
/// This is what lets a browser UI (`just dev`) talk to a real BFT node over
/// the ordinary HTTP API — reads, `/tx` submit, the pending pool, sessions.
/// Returns once bound; serving continues on a spawned task.
pub async fn spawn_client(node: std::sync::Arc<Node>) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind((bind_host(&node.cfg), node.cfg.listen_port)).await?;
    spawn_client_on(node, listener)
}

/// `spawn_client` on a listener the caller already bound — a test binds port
/// 0 and reads the port the OS chose, so two tests never contend for one and
/// a server leaked by an earlier run cannot fail one for a reason that has
/// nothing to do with the code.
pub fn spawn_client_on(node: std::sync::Arc<Node>, listener: tokio::net::TcpListener) -> std::io::Result<()> {
    let app = client_router(node);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
    });
    Ok(())
}
