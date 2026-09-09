//! The async layer around the shared node core: the client ingress
//! (`submit`, the pending pool) and the gossip that carries what it accepts
//! to the other nodes' mempools. Locks are never held across an await — the
//! locked call decides what to send, and the send happens after the guard is
//! dropped.
//!
//! Consensus is NOT here. Blocks are decided by the embedded Malachite engine
//! (`engine_malachite`), which gossips consensus messages over its own libp2p
//! mesh and knows nothing about the mempool — so this `/p2p/tx` hop is what
//! lets a transaction submitted to one device be proposed by whichever node's
//! turn it is. It runs no consensus tick against the
//! same core; that is gone.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::block::SignedTx;

use super::core::NodeCore;
use super::pending::{PendingDeclineReq, PendingSignReq, SignOutcome};
use super::Config;

pub struct Node {
    pub cfg: Config,
    pub core: Mutex<NodeCore>,
    pub client: reqwest::Client,
}

impl Node {
    /// Lock the node core, tolerating a previously poisoned mutex: a
    /// panic under the lock in one handler must not brick every other
    /// caller (HTTP handler, IPC command, consensus tick) — take the
    /// guard's inner value rather than propagating the poison. The state
    /// machine's `apply` never panics, so a poisoned core is never left
    /// mid-mutation.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, NodeCore> {
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn new(cfg: Config, genesis: edet_state::State) -> Arc<Node> {
        let core = NodeCore::new(cfg.index, cfg.n, genesis);
        Arc::new(Node { core: Mutex::new(core), client: reqwest::Client::new(), cfg })
    }

    /// Build a node, opening a durable replica under `cfg.data_dir` when set
    /// instead of always starting in-memory like `new`.
    pub fn open(cfg: Config, genesis: edet_state::State) -> Result<Arc<Node>, crate::replica::ReplicaError> {
        let core = NodeCore::open(
            cfg.index,
            cfg.n,
            genesis,
            cfg.data_dir.as_deref(),
            cfg.snapshot_interval,
            cfg.prune_margin_blocks,
        )?;
        Ok(Arc::new(Node { core: Mutex::new(core), client: reqwest::Client::new(), cfg }))
    }

    /// The block timestamp a fresh proposal would carry. Public so the
    /// dry-run check endpoint (`/tx/check`) evaluates a transaction at the
    /// same clock the block including it would.
    ///
    /// Real time, floored by the last committed block's timestamp so a
    /// device with a slow or skewed clock never reads back a moment that
    /// moves backward. The economic constants are denominated in wall-clock
    /// durations (30/90 EPOCHS), not in block counts, so this must be a real
    /// clock. A second, logical source here — `(head+1) *
    /// epoch_secs` — for the deterministic dev-cluster tests; it went with
    /// the consensus that needed it.
    pub fn propose_time(&self) -> u64 {
        let last_time = self.lock().replica.last_time_secs;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(last_time);
        now.max(last_time + 1)
    }

    /// Peer base URLs excluding this node's own slot.
    fn peer_urls(&self) -> Vec<String> {
        self.cfg
            .peers
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.cfg.index)
            .map(|(_, u)| u.clone())
            .collect()
    }
}

/// Wire form of a gossip message between nodes. Transactions and pending-pool
/// signatures only: consensus messages are Malachite's, over its own mesh, and
/// never travel this way.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Wire {
    Tx { tx: SignedTx },
    PendingSign { req: PendingSignReq },
    PendingDecline { req: PendingDeclineReq },
}

impl Wire {
    fn path(&self) -> &'static str {
        match self {
            Wire::Tx { .. } => "/p2p/tx",
            Wire::PendingSign { .. } => "/p2p/pending-sign",
            Wire::PendingDecline { .. } => "/p2p/pending-decline",
        }
    }
}

/// Queue a transaction into the node and gossip it if newly accepted. Shared
/// by the HTTP handler and the Tauri IPC command. Returns whether it queued.
/// The ingress gate: every claimed signer must have actually signed the
/// transaction digest, or the submission is dropped.
pub async fn submit(node: &Arc<Node>, tx: SignedTx) -> bool {
    // `is_authenticated` is signature verification PLUS the empty-signer rule (an empty signer
    // set is only legitimate for the permissionless cranks, MarkExpired and
    // RotateFinalize) — the same single rule every consensus driver applies
    // to a peer's block via `Block::verify_txs`, so a transaction this
    // ingress would refuse cannot enter by being proposed instead.
    //
    // The `allow_unsigned` harness hatch waives the SIGNATURE half only: the empty-signer rule
    // holds even there, because a shell smoke test can name signers it cannot
    // sign for, and accountability-free transactions must not exist on any
    // path.
    // Bound the verification work BEFORE paying for any of it. `verify` runs
    // one curve operation per claimed signer, `signers` is caller-supplied,
    // and this endpoint sits ahead of the rate limiter — so an unauthenticated
    // request could buy thousands of verifications (measured: 118 ms for one
    // 1.38 MB body, answered `200 OK`). No legitimate envelope comes near this
    // many parties; the transport's body cap (`http::MAX_BODY_BYTES`) bounds
    // the same attack by size, and this bounds it in the unit that costs.
    if tx.signers.len() > super::http::MAX_SIGNERS || tx.signatures.len() > super::http::MAX_SIGNERS {
        return false;
    }
    let authenticated = if node.cfg.allow_unsigned {
        !tx.signers.is_empty() || crate::block::is_permissionless(&tx.tx)
    } else {
        let chain_id = node.lock().replica.state.chain_id.clone();
        tx.is_authenticated(&chain_id)
    };
    if !authenticated {
        return false;
    }
    let queued = node.lock().submit(tx.clone());
    if queued {
        broadcast_wire(node, Wire::Tx { tx }).await;
    }
    queued
}

/// Record a signature on a multi-party proposal. Shared by the client
/// endpoint, the Tauri IPC command, and the peer gossip handler. New
/// signatures flood to peers like transactions; reaching the threshold
/// assembles the SignedTx and pushes it through the verified ingress.
pub async fn pending_sign(node: &Arc<Node>, req: PendingSignReq) -> serde_json::Value {
    let now = node.propose_time();
    let outcome = {
        let mut guard = node.lock();
        let core = &mut *guard;
        // The signature is checked BEFORE the request may occupy a
        // rate-limiter bucket. The limiter keys on `signer`, and this endpoint
        // is otherwise unauthenticated — so keying first let anyone mint
        // 10 000 junk signer values (measured: 244 ms) and fill a table that
        // nothing pruned, locking out every co-signer the node had not already
        // seen. Establishing that a real member really signed these bytes is
        // what makes a bucket cost something to claim.
        //
        // `PendingPool::sign` re-checks by calling the same function rather
        // than trusting this one, so the door and the room enforce one rule.
        let state = &core.replica.state;
        let Some(digest) = super::pending::request_digest(state, &req) else {
            return serde_json::json!({ "ok": false, "error": "undigestable transaction" });
        };
        // A signer with no account may claim a bucket only against an entry a
        // MEMBER already opened naming that exact key — the newcomer co-signing
        // their own first trade — or on a member's INVITATION, in which case
        // the bucket it spends is that member's (`pending::invited_by`).
        // Without this half, "a real signature" would be free again: keys
        // cost nothing, so anyone could mint them and fill the limiter table,
        // which is the flood the check above exists to stop.
        let bucket = match super::pending::verified_signer(state, &req, &digest) {
            None => return serde_json::json!({ "ok": false, "error": "bad signature or unknown signer key" }),
            Some(p @ edet_state::types::Party::Key(_)) if !core.pending.awaits(&digest, &p) => {
                match super::pending::invited_by(state, &req, now) {
                    Some((_, inviter_key)) => inviter_key,
                    None => {
                        return serde_json::json!({
                            "ok": false,
                            "error": "a key with no account may open a proposal only on a member's invitation",
                        })
                    }
                }
            }
            Some(_) => req.signer,
        };
        if !core.pending_limiter.allow(&bucket) {
            return serde_json::json!({ "ok": false, "error": "rate limited" });
        }
        core.pending.sign(&core.replica.state, req.clone(), now)
    };
    match outcome {
        SignOutcome::Rejected(reason) => serde_json::json!({ "ok": false, "error": reason }),
        SignOutcome::Recorded { new } => {
            if new {
                broadcast_wire(node, Wire::PendingSign { req }).await;
            }
            serde_json::json!({ "ok": true, "completed": false })
        }
        SignOutcome::Complete(stx) => {
            let stx = *stx;
            broadcast_wire(node, Wire::PendingSign { req }).await;
            // The assembled envelope's hash, for the same reason `/tx` returns
            // one: the co-signer who completed it is the one who learns whether
            // it then committed.
            let hash = stx.hash().ok().map(|h| crate::block::hex32(&h));
            let queued = submit(node, stx).await;
            serde_json::json!({ "ok": true, "completed": true, "queued": queued, "hash": hash })
        }
    }
}

/// Decline a pending proposal (any required party or the initiator).
pub async fn pending_decline(node: &Arc<Node>, req: PendingDeclineReq) -> serde_json::Value {
    let Some(digest) = crate::block::unhex32(&req.digest) else {
        return serde_json::json!({ "ok": false, "error": "bad digest" });
    };
    let changed = {
        let mut guard = node.lock();
        let core = &mut *guard;
        core.pending.decline(&core.replica.state, digest, &req.signer, &req.signature)
    };
    if changed {
        broadcast_wire(node, Wire::PendingDecline { req }).await;
    }
    serde_json::json!({ "ok": changed })
}

async fn broadcast_wire(node: &Arc<Node>, wire: Wire) {
    let peers = node.peer_urls();
    let path = wire.path();
    let Ok(body) = serde_json::to_vec(&wire) else { return };
    for base in &peers {
        let url = format!("{base}{path}");
        let client = node.client.clone();
        let body = body.clone();
        let token = node.cfg.cluster_token.clone();
        tokio::spawn(async move {
            let _ = post_p2p(&client, &url, body, token.as_deref()).await;
        });
    }
}

/// POST a gossip message to a peer, carrying the cluster token header when
/// this node is configured with one (the peer's own `/p2p/*` guard
/// requires it, so honest peers must present it or every gossip send would
/// be rejected).
async fn post_p2p(
    client: &reqwest::Client,
    url: &str,
    body: Vec<u8>,
    token: Option<&str>,
) -> Result<reqwest::Response, reqwest::Error> {
    let mut req = client.post(url).header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header(super::http::CLUSTER_TOKEN_HEADER, t);
    }
    req.body(body).send().await
}
