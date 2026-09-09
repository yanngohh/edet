//! The HTTP API a node serves to clients: read/query/submit endpoints (the
//! same surface the Tauri app reaches in-process via `views`), plus the
//! `/p2p/*` endpoints that carry transactions and pending-pool signatures
//! between nodes.
//!
//! There is no propose/vote surface here. Consensus is the embedded Malachite
//! engine's, over its own authenticated libp2p mesh; this crate does not also
//! serve a dev consensus's `/p2p/propose` and `/p2p/vote` over plain HTTP,
//! guarded by nothing but a shared token, and that is gone.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

use super::auth::verify_signed_headers;
use super::driver::{Node, Wire};
use super::pending::{PendingDeclineReq, PendingSignReq};
use super::session::now_unix_secs;
use super::{views, Config, Viewer, ViewerParty};

/// Header carrying the shared cluster secret (see `Config::cluster_token`).
/// `pub(crate)` so the outbound gossip in `driver.rs` sets the same header it
/// is checked against here.
pub(crate) const CLUSTER_TOKEN_HEADER: &str = "x-edet-cluster-token";

/// The API a node serves: reads, submit, the pending pool and sessions,
/// plus the tx/pending gossip ingress (`/p2p/tx`, `/p2p/pending-*`) that
/// carries a submission to every validator's mempool — Malachite gossips
/// consensus messages, not transactions, so without this hop a transaction
/// could only ever be proposed by the one node it was submitted to.
///
/// The gossip routes get their own perimeter guard (`p2p_guard`), distinct
/// from the CORS-only client surface, because they carry no per-message
/// signature of their own; the transactions inside them are verified at the
/// ingress like any other (`driver::submit`).
pub fn client_router(node: Arc<Node>) -> Router {
    let gossip = Router::new()
        .route("/p2p/tx", post(p2p_tx))
        .route("/p2p/pending-sign", post(p2p_pending))
        .route("/p2p/pending-decline", post(p2p_pending))
        .layer(middleware::from_fn_with_state(node.clone(), p2p_guard));

    client_routes()
        .merge(gossip)
        .layer(middleware::from_fn_with_state(node.clone(), read_rate_limit))
        // Outside the rate limiter, because a halted node owes the same
        // answer to every caller however often they ask, and inside CORS,
        // because a browser must be able to READ that answer.
        .layer(middleware::from_fn_with_state(node.clone(), halt_guard))
        .layer(cors_layer(&node.cfg))
        // Outermost, so an oversized body is refused before any handler,
        // extractor or signature verification runs.
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(node)
}

/// A read a client makes, named independently of how it travels.
///
/// This exists because a viewer credential is signed over the request's PATH
/// (`auth::viewer_auth_message`), and the embedded client reaches these same
/// reads over Tauri IPC, where there is no request and therefore no path to
/// sign. Rather than invent a second naming scheme for the same reads — two
/// schemes meaning one credential could not be verified by one function, and
/// the two transports would drift — an IPC caller signs over the path its
/// read has HERE. `Read::path` is that one answer, used by both sides.
///
/// A credential is consequently interchangeable across the two transports,
/// which is the point rather than an oversight: it proves possession of a
/// key the ledger attributes to a member, and that fact does not change with
/// how the request arrived. Every other property of the credential — the
/// 60-second replay window, the binding to one path — is inherited unchanged.
pub enum Read {
    Network,
    Members,
    Member(u64),
    Contracts,
    Params,
    Proposals,
    Pending(u64),
    /// The same queue addressed by KEY, for a device whose first trade has not
    /// been assembled yet and which therefore has no member id . The
    /// hex key here IS the caller's own — it is the only name they have — and
    /// the credential must prove possession of it.
    PendingByKey(String),
    /// The needle being resolved (key or address, hex) — NOT the caller's own
    /// key, which travels in the credential itself.
    Whois(String),
    TxOutcome(String),
    /// The dry-run endpoint. A read in the sense that matters here: it
    /// discloses a transaction outcome, and that outcome is gated on the
    /// caller being a party (`views::check_tx`).
    TxCheck,
    /// P-4: a record's inclusion proof. The most disclosing read there is —
    /// the leaf is the whole record, verbatim — so it carries the same
    /// `full_access` rule the record's own view does.
    ProofMember(u64),
    ProofContract(u64),
}

impl Read {
    /// The concrete path this read is served at, and thus the path its viewer
    /// credential must be signed over. Pinned to the routes below by
    /// `serve::tests::every_read_path_names_a_route_this_node_serves`.
    pub fn path(&self) -> String {
        match self {
            Read::Network => "/network".to_string(),
            Read::Members => "/members".to_string(),
            Read::Member(id) => format!("/member/{id}"),
            Read::Contracts => "/contracts".to_string(),
            Read::Params => "/params".to_string(),
            Read::Proposals => "/proposals".to_string(),
            Read::Pending(member) => format!("/pending/{member}"),
            Read::PendingByKey(key) => format!("/pending/key/{key}"),
            Read::Whois(needle) => format!("/whois/{needle}"),
            Read::TxOutcome(hash) => format!("/tx/outcome/{hash}"),
            Read::TxCheck => "/tx/check".to_string(),
            Read::ProofMember(id) => format!("/proof/member/{id}"),
            Read::ProofContract(id) => format!("/proof/contract/{id}"),
        }
    }
}

/// The read/submit/pending/session routes.
/// Hard cap on a client request body.
///
/// `/tx` runs one Ed25519 verification per CLAIMED signer before any rate
/// limit, and `signers` is caller-supplied with no length bound of its own —
/// one 1.38 MB request bought 118 ms of verification on a live validator and
/// returned `200 OK`. A legitimate envelope is a transaction plus a handful of
/// 32-byte keys and 64-byte signatures; 64 KiB is orders of magnitude above
/// any real one and orders below what makes the work worth buying.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// Cap on how many signatures one envelope may claim, checked before any of
/// them is verified. The body limit already bounds the work; this bounds it
/// again in the unit that actually costs — a curve operation each — and gives
/// a named refusal instead of a truncated parse.
pub const MAX_SIGNERS: usize = 32;

fn client_routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/head", get(head))
        .route("/network", get(network))
        .route("/members", get(members))
        .route("/member/:id", get(member))
        .route("/contracts", get(contracts))
        .route("/params", get(params))
        .route("/proposals", get(proposals))
        .route("/whois/:key", get(whois))
        .route("/session", post(session))
        .route("/tx", post(submit_tx))
        .route("/tx/check", post(check_tx))
        .route("/tx/digest", post(tx_digest))
        .route("/tx/outcome/:hash", get(tx_outcome))
        .route("/proof/member/:id", get(proof_member))
        .route("/proof/contract/:id", get(proof_contract))
        .route("/pending/:member", get(pending_list))
        .route("/pending/key/:key", get(pending_by_key))
        .route("/pending/sign", post(pending_sign))
        .route("/pending/decline", post(pending_decline))
}

/// A tight CORS allowlist instead of `CorsLayer::permissive()`.
///
/// **The desktop and mobile app is a browser here too.** It embeds no
/// node and read it over IPC, so this layer only had to serve dev UIs; the
/// app runs no node now and every read it makes is a cross-origin fetch from
/// the WebView's own origin. Which origin that is depends on the platform and
/// on `useHttpsScheme`: wry serves the app from `tauri://localhost` on Linux
/// and macOS, and from `http://tauri.localhost` on Android with the scheme
/// left at its default (`https://tauri.localhost` when it is turned on).
/// Read off a handset rather than assumed — `location.href` there is
/// `http://tauri.localhost/`.
///
/// An origin missing here does not degrade to an unauthenticated read: the
/// browser refuses to send the request at all, so the app shows an
/// unreachable node against one that is answering perfectly well.
///
/// `Config::cors_ports` is the documented opt-in for the multi-instance
/// browser dev cluster (`just dev N`, one Vite server per node on `5173+i`).
fn cors_layer(cfg: &Config) -> CorsLayer {
    let mut origins: Vec<HeaderValue> = vec![
        HeaderValue::from_static("http://localhost:5173"),
        HeaderValue::from_static("http://127.0.0.1:5173"),
        // The Tauri webview's origins — all three, see above.
        HeaderValue::from_static("tauri://localhost"),
        HeaderValue::from_static("http://tauri.localhost"),
        HeaderValue::from_static("https://tauri.localhost"),
    ];
    for port in &cfg.cors_ports {
        if let Ok(v) = HeaderValue::from_str(&format!("http://localhost:{port}")) {
            origins.push(v);
        }
        if let Ok(v) = HeaderValue::from_str(&format!("http://127.0.0.1:{port}")) {
            origins.push(v);
        }
    }
    // Authenticated-reads §Standing signed-header scheme: a browser UI signing its
    // own reads needs every viewer header allowed through CORS preflight
    // like any other custom header. Taken from `auth::VIEWER_HEADERS` rather
    // than re-listed, because a header this layer forgets does not degrade
    // to an unauthenticated read — the browser refuses to send the request
    // at all, and the failure surfaces as an opaque transport error far from
    // its cause.
    let mut allowed = vec![
        axum::http::header::CONTENT_TYPE,
        axum::http::header::AUTHORIZATION,
        CLUSTER_TOKEN_HEADER.parse().unwrap(),
    ];
    allowed.extend(super::auth::VIEWER_HEADERS.iter().map(|h| h.parse().unwrap()));
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(allowed)
}

/// The host part of a `peers` entry (`"http://host:port"` -> `"host"`), for
/// matching a gossip request's source address against configured peers.
fn peer_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://"))?;
    let host_port = rest.split('/').next().unwrap_or(rest);
    Some(host_port.rsplit_once(':').map(|(h, _)| h).unwrap_or(host_port))
}

/// Compare two byte strings without leaking where they first differ.
///
/// The cluster token is a perimeter rather than authentication — every message
/// inside it is verified independently — and network timing is noisy, so this
/// is defence in depth rather than a closed hole. It is also two lines.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // The length difference is not secret (it is a configured token, not a
    // guess-one-byte-at-a-time secret), but folding it in costs nothing.
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

/// Guard every `/p2p/*` request. These carry transactions and
/// pending-pool signatures between nodes, with no per-message signature of
/// their own, so this is the perimeter for the ENVELOPE; its contents are
/// authenticated separately and unconditionally at the ingress
/// (`driver::submit` verifies every transaction signature, `pending::sign`
/// every co-signature), which is what keeps a breach of this perimeter from
/// being able to book anything.
///
/// If `cluster_token` is configured, it is required (the header must match
/// exactly); otherwise the request's source must be loopback or a configured
/// peer host, which is a thin claim on a node bound to every interface.
///
/// **A missing token is a warning rather than a refusal, and the reason is
/// what this guard is worth rather than laxity.** These three routes call the
/// same three `driver::*` functions the client routes call — `/p2p/tx` and
/// `/tx` are one `submit`, `/p2p/pending-sign` and `/pending/sign` are one
/// `pending_sign` — and the client routes carry no token at all. So an
/// untokened gossip surface admits exactly what the surface beside it already
/// admits, under the same signature check, the same empty-signer rule and the
/// same per-signer bucket (`the_peer_ingress_admits_exactly_what_the_client_ingress_admits`).
/// Refusing to start without a token would close a door standing open next to
/// it, and would take with it the deployment where the perimeter really is the
/// network segment. What a token buys is a perimeter, not a capability.
async fn p2p_guard(
    State(node): State<Arc<Node>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    if let Some(token) = &node.cfg.cluster_token {
        let presented = req.headers().get(CLUSTER_TOKEN_HEADER).and_then(|v| v.to_str().ok());
        return if presented.is_some_and(|p| constant_time_eq(p.as_bytes(), token.as_bytes())) {
            next.run(req).await
        } else {
            (StatusCode::FORBIDDEN, "p2p: bad or missing cluster token").into_response()
        };
    }
    let ip = remote.ip();
    let source_ok = ip.is_loopback() || node.cfg.peers.iter().any(|p| peer_host(p) == Some(ip.to_string().as_str()));
    if source_ok {
        next.run(req).await
    } else {
        (StatusCode::FORBIDDEN, "p2p: unrecognized source").into_response()
    }
}

/// Is `path` on the rate-limited read surface? Excludes `/health`
/// (liveness probes must never be throttled), `/p2p/*` (its own guard and
/// semantics — see `p2p_guard`), and the endpoints that genuinely do carry
/// their own per-signer bucket: `/tx` (`NodeCore::submit`'s `tx_limiter`) and
/// `/pending/sign` + `/pending/decline` (`pending_limiter`). Layering a
/// second, differently-keyed limiter over those would make the existing flood
/// tests' assumptions (a fixed number of requests, a definite rejection
/// point) depend on which bucket trips first, for no safety gain the
/// per-signer bucket doesn't already give.
///
/// `/session` is NOT on that list and IS rate-limited here, contrary to what
/// an earlier version of this comment claimed. There is no per-signer bucket
/// for it — grep `RateLimiter::allow`: the only three call sites are
/// `tx_limiter`, `pending_limiter` and this one — and it is the wrong
/// endpoint to leave open. It runs a full Ed25519 verification on
/// attacker-supplied headers before it can reject them, and on success it
/// inserts into an in-memory token store, so an unthrottled caller gets both
/// a CPU amplifier and unbounded memory growth from a single held key. The
/// per-IP bucket is the right shape for it precisely because the caller is
/// not yet authenticated when the work happens.
///
/// Matched by exact string, not prefix, so `/tx` doesn't accidentally
/// swallow `/tx/check`, `/tx/digest` or `/tx/outcome/:hash` — those three
/// ARE covered, since none of them has a bucket of its own today.
/// Every client path is metered per source address, the write paths included.
/// `/tx` and the pending-pool endpoints have their own per-SIGNER buckets
/// behind them, but those are keyed on a verified signer, so the signature
/// verification itself — up to `MAX_SIGNERS` curve operations per request —
/// ran before any limit at all. The per-IP bucket ahead of it bounds that work
/// in the unit that costs (`read_cost`).
fn is_rate_limited_path(path: &str) -> bool {
    if path.starts_with("/p2p/") {
        return false;
    }
    path != "/health"
}

/// What one read of `path` may cost this node, in read-budget tokens.
///
/// **A budget by request count is a budget on the cheapest request.**
/// `/members` answers a max-flow per member it serves — bounded per read by
/// `core::MAX_COLD_CAPACITY_PER_READ`, and each of those is milliseconds under
/// the node lock at community scale, against microseconds for `/params`. One
/// price for both either lets the expensive read run at the cheap one's rate or
/// refuses ordinary polling.
///
/// The worst case rather than the measured cost, so the price is a property of
/// the endpoint that a caller can predict and a reader can check: a listing
/// served entirely from the capacity cache is charged the same as one that
/// filled it, which over-charges by exactly the amount the cache saved.
///
/// **A PAGE is a read**, at the same price, and that is deliberate. The
/// cold-capacity budget is per read, so a walk of N pages does N reads' worth
/// of work and pays for N reads. Pricing by rows served instead would make
/// `?limit=1` the cheapest way to drive the whole listing.
///
/// **A write is priced by the verification it forces.** `/tx` and the
/// pending-pool endpoints verify one signature per claimed signer before their
/// own per-signer bucket can key on anything, and stood outside this limiter
/// altogether — up to `MAX_SIGNERS` curve operations per unauthenticated
/// request, at no cost to the sender. The body is not parsed at this layer;
/// its size bounds the signers it can carry (a key is 32 bytes and a signature
/// 64), so the charge is one token for the request and one more per eight
/// signers the body could hold.
fn read_cost(path: &str, body_len: Option<usize>) -> f64 {
    match path {
        "/members" => super::core::MAX_COLD_CAPACITY_PER_READ as f64,
        "/tx" | "/pending/sign" | "/pending/decline" => 1.0 + body_len.unwrap_or(0) as f64 / (96.0 * 8.0),
        _ => 1.0,
    }
}

/// Per-source-IP token-bucket guard over the read surface (see
/// `is_rate_limited_path`), so an anonymous caller cannot drive unbounded
/// full-state serialization (`/members`, `/contracts`) or dry-run/encoding
/// work (`/tx/check`, `/tx/digest`) in a tight loop. Applied as an outer
/// layer over the merged router (client routes + `/p2p/*`) rather than
/// per-route, matching `p2p_guard`'s shape — the path check inside is what
/// keeps `/health` and `/p2p/*` untouched, since a middleware `Router::layer`
/// call has no per-route granularity of its own.
/// Whose budget this read spends.
///
/// The socket's peer address, unless the operator has asserted that a reverse
/// proxy in front of this node sets `X-Forwarded-For`
/// (`Config::trust_forwarded_for`). **Trusting that header by default would
/// turn the read budget off**: any client can set it, so every request would
/// arrive with a fresh bucket. Not trusting it behind a proxy has the mirror
/// cost and a much smaller one — every reader shares the proxy's bucket, so
/// the community throttles itself rather than the node being defenceless.
///
/// The FIRST entry, which is the original client where the proxy appends and
/// the client's claim where it does not. That is exactly why this is an
/// operator's assertion: it is only sound when the proxy overwrites the header
/// and nothing else can reach this port, and neither is checkable here.
fn reader_ip(node: &Arc<Node>, req: &axum::extract::Request, remote: IpAddr) -> IpAddr {
    if !node.cfg.trust_forwarded_for {
        return remote;
    }
    req.headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(remote)
}

/// **A halted node answers nothing but its halt.**
///
/// A block applies in place, so a block the audit refused leaves the ledger
/// it refused in memory; the engine's answer is to bring the process down,
/// and that takes a moment during which this surface is still bound. Serving
/// a state the invariants say cannot exist — even for a moment, even
/// read-only — is the one thing a fail-stop exists to prevent, so every
/// route answers 503 with the height it stopped at and why.
///
/// 503 rather than 500: the node is not broken, it has stopped on purpose,
/// and a client's right answer is to read another node
/// (`views::head`'s cross-check).
async fn halt_guard(State(node): State<Arc<Node>>, req: axum::extract::Request, next: Next) -> Response {
    let halt = node.lock().replica.halted().cloned();
    match halt {
        Some(h) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "this node stopped: a committed block left the ledger in a state the invariants say cannot exist",
                "halted_at_height": h.height,
                "reason": h.reason,
            })),
        )
            .into_response(),
        None => next.run(req).await,
    }
}

async fn read_rate_limit(
    State(node): State<Arc<Node>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    if !is_rate_limited_path(req.uri().path()) {
        return next.run(req).await;
    }
    let source = reader_ip(&node, &req, remote.ip());
    let body_len = req
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    let allowed = node.lock().allow_read_cost(source, read_cost(req.uri().path(), body_len));
    if allowed {
        next.run(req).await
    } else {
        (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response()
    }
}

async fn head(State(node): State<Arc<Node>>, Viewer(viewer): Viewer) -> Json<Value> {
    Json(views::head(&node.lock(), viewer))
}

async fn network(State(node): State<Arc<Node>>, Viewer(viewer): Viewer) -> Json<Value> {
    Json(views::network(&node.lock(), &node.cfg, viewer))
}

/// **Every page is one read at the endpoint's price**, and the cold-capacity
/// budget is per read — so a walk of N pages costs N reads and N budgets,
/// which is the point. Pricing a page cheaper than a whole listing would make
/// `?limit=1` the cheapest way to drive the same total work.
/// Three phases, and the lock is held for the first and the third only. The
/// rows and the cached capacities come out under the lock with a copy of the
/// cut's inputs; up to `MAX_COLD_CAPACITY_PER_READ` max-flows then run on that
/// copy on this handler's thread; and the answers are banked for the next read
/// if a block has not moved the inputs meanwhile. A commit never waits behind a
/// capacity query.
async fn members(
    State(node): State<Arc<Node>>,
    Viewer(viewer): Viewer,
    Query(page): Query<views::Page>,
) -> Json<Value> {
    let read = views::members_read(&node.lock(), viewer, page);
    let computed: Vec<(u64, f64)> = match &read.snapshot {
        Some(snap) => read
            .pending
            .iter()
            .take(super::core::MAX_COLD_CAPACITY_PER_READ)
            .map(|p| (p.id, snap.capacity_of(p.id)))
            .collect(),
        None => Vec::new(),
    };
    if let Some(snap) = &read.snapshot {
        node.lock().fill_capacity_cache(snap, &computed);
    }
    Json(views::members_finish(read, &computed))
}

async fn member(State(node): State<Arc<Node>>, Path(id): Path<u64>, Viewer(viewer): Viewer) -> Json<Value> {
    Json(views::member(&node.lock(), id, viewer))
}

async fn contracts(
    State(node): State<Arc<Node>>,
    Viewer(viewer): Viewer,
    Query(page): Query<views::Page>,
) -> Json<Value> {
    Json(views::contracts(&node.lock(), viewer, page))
}

async fn params(State(node): State<Arc<Node>>, Viewer(viewer): Viewer) -> Json<Value> {
    Json(views::params(&node.lock(), viewer))
}

async fn proposals(State(node): State<Arc<Node>>, Viewer(viewer): Viewer) -> Json<Value> {
    Json(views::proposals(&node.lock(), viewer))
}

/// Dry-run a transaction against current state; returns the bond quote to
/// anyone, and `{ok}` or the rejection code to a party (see `views::check_tx`
/// for why the outcome is the sensitive half).
///
/// The extractor must come BEFORE the `Json` body: axum runs extractors in
/// declaration order and `Json` consumes the request, so a body extractor
/// ahead of a header one fails to compile rather than silently skipping it.
async fn check_tx(
    State(node): State<Arc<Node>>,
    Viewer(viewer): Viewer,
    Json(tx): Json<crate::block::SignedTx>,
) -> Json<Value> {
    let now = node.propose_time();
    Json(views::check_tx(&node.lock(), &tx, now, viewer))
}

/// What this node knows became of a submitted transaction: committed
/// and applied, committed and rejected (with the ET code), still pending,
/// or unknown.
async fn tx_outcome(State(node): State<Arc<Node>>, Path(hash): Path<String>, Viewer(viewer): Viewer) -> Json<Value> {
    Json(views::tx_outcome(&node.lock(), &hash, viewer))
}

/// The digest a client must sign for this transaction envelope: the
/// client supplies its own `nonce`/`not_after_epoch`, and the response is
/// bound to THIS node's `chain_id`.
async fn tx_digest(State(node): State<Arc<Node>>, Json(req): Json<views::TxDigestReq>) -> Json<Value> {
    let chain_id = node.lock().replica.state.chain_id.clone();
    Json(views::tx_digest(&chain_id, &req))
}

/// Which member holds this public key (hex)?
async fn whois(State(node): State<Arc<Node>>, Path(key): Path<String>, Viewer(viewer): Viewer) -> Json<Value> {
    Json(views::whois(&node.lock(), &key, viewer))
}

/// P-4: a record's inclusion proof against this node's current state root,
/// with the root and height it was computed against. See `views::proof_member`
/// for why this is gated as tightly as the record's own view.
async fn proof_member(State(node): State<Arc<Node>>, Path(id): Path<u64>, Viewer(viewer): Viewer) -> Json<Value> {
    Json(views::proof_member(&node.lock(), id, viewer))
}

async fn proof_contract(State(node): State<Arc<Node>>, Path(id): Path<u64>, Viewer(viewer): Viewer) -> Json<Value> {
    Json(views::proof_contract(&node.lock(), id, viewer))
}

/// Multi-party proposals involving this member.
async fn pending_list(State(node): State<Arc<Node>>, Path(member): Path<u64>, Viewer(viewer): Viewer) -> Json<Value> {
    Json(views::pending(&node.lock(), member, viewer))
}

/// The same, for a caller the ledger cannot name yet: a device holding a key
/// whose first trade is still waiting for its counterparty's signature.
///
/// `ViewerParty` rather than `Viewer` is the whole difference — the latter
/// resolves an unattributed key to anonymous, which is right everywhere else
/// and would make this endpoint unreachable by the only callers it is for.
async fn pending_by_key(
    State(node): State<Arc<Node>>,
    Path(key): Path<String>,
    ViewerParty(viewer): ViewerParty,
) -> Json<Value> {
    let Some(bytes) = crate::block::unhex32(&key) else {
        return Json(serde_json::json!({ "error": "malformed key" }));
    };
    Json(views::pending_by_key(&node.lock(), bytes, viewer))
}

/// §Standing: exchange one §Standing-style signed request for a short-lived opaque
/// bearer token. The signed payload covers this exact request (`POST` +
/// `/session` + the timestamp), so this is not itself a `Viewer` extraction
/// (which also accepts a Bearer token — circular for the endpoint that
/// mints one) but a direct call into the same verification `auth.rs`
/// exposes to it.
async fn session(State(node): State<Arc<Node>>, method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let now = now_unix_secs();
    let mut guard = node.lock();
    let core = &mut *guard;
    match verify_signed_headers(&core.replica.state, &mut core.seen, &headers, method.as_str(), uri.path(), now) {
        Ok((member_id, key)) => match core.sessions.mint(member_id, key, now) {
            Ok((token, expires_secs)) => {
                Json(json!({ "token": token, "member_id": member_id, "expires_secs": expires_secs })).into_response()
            }
            // `mint` only fails if the OS CSPRNG draw itself fails (`session.rs`)
            // — not a caller error, so 503 rather than 4xx.
            Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "session: token generation unavailable").into_response(),
        },
        Err(e) => e.into_response(),
    }
}

/// Sign (or open) a multi-party proposal.
async fn pending_sign(State(node): State<Arc<Node>>, Json(req): Json<PendingSignReq>) -> Json<Value> {
    Json(super::driver::pending_sign(&node, req).await)
}

/// Decline a multi-party proposal.
async fn pending_decline(State(node): State<Arc<Node>>, Json(req): Json<PendingDeclineReq>) -> Json<Value> {
    Json(super::driver::pending_decline(&node, req).await)
}

/// A client submits a transaction (JSON `SignedTx`). Queue + gossip.
async fn submit_tx(State(node): State<Arc<Node>>, Json(tx): Json<crate::block::SignedTx>) -> Json<Value> {
    // The hash the outcome is keyed on (`/tx/outcome/:hash`), handed back so
    // the wallet can learn that a queued transaction was refused at commit —
    // without it a member saw "sent" and nothing else, since the client cannot
    // reproduce the consensus encoding of the signed envelope itself.
    let hash = tx.hash().ok().map(|h| crate::block::hex32(&h));
    let queued = super::driver::submit(&node, tx).await;
    Json(json!({ "queued": queued, "hash": hash }))
}

// --- peer endpoints --------------------------------------------------------

async fn p2p_tx(State(node): State<Arc<Node>>, Json(w): Json<Wire>) -> Json<Value> {
    if let Wire::Tx { tx } = w {
        super::driver::submit(&node, tx).await;
    }
    Json(json!({ "ok": true }))
}

/// Peer gossip of pending-pool signatures/declines: same handlers as the
/// client endpoints (idempotent; only new information re-floods).
async fn p2p_pending(State(node): State<Arc<Node>>, Json(w): Json<Wire>) -> Json<Value> {
    match w {
        Wire::PendingSign { req } => {
            super::driver::pending_sign(&node, req).await;
        }
        Wire::PendingDecline { req } => {
            super::driver::pending_decline(&node, req).await;
        }
        _ => {}
    }
    Json(json!({ "ok": true }))
}

#[cfg(test)]
mod limiter_tests {
    use super::*;

    /// Every client path but `/health` is metered per source, and a write is
    /// priced by the verification work its body can carry: one token for the
    /// request and one more per eight signers' worth of bytes.
    #[test]
    fn every_client_path_but_health_is_metered_and_a_write_is_priced_by_its_signers() {
        assert!(!is_rate_limited_path("/health"));
        assert!(!is_rate_limited_path("/p2p/tx"), "gossip is guarded by the cluster token or the peer list");
        for path in ["/tx", "/pending/sign", "/pending/decline", "/members", "/head", "/tx/check"] {
            assert!(is_rate_limited_path(path), "{path} is metered");
        }
        assert_eq!(read_cost("/members", None), super::super::core::MAX_COLD_CAPACITY_PER_READ as f64);
        assert_eq!(read_cost("/tx", None), 1.0);
        assert_eq!(read_cost("/tx", Some(MAX_SIGNERS * 96)), 5.0, "thirty-two signers is five tokens");
        assert_eq!(read_cost("/pending/sign", Some(768)), 2.0);
        assert_eq!(read_cost("/head", Some(100_000)), 1.0, "a read's body is not its price");
    }
}
