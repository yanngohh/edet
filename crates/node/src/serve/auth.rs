//! Viewer identification (the paper's §Implementation): resolves "who is asking" for a read, and the views act on the
//! answer — per-field visibility, the `whois` gate, `pending`'s
//! viewer-must-be-the-member rule (`views.rs`, §Standing's matrix). An anonymous
//! caller is answered, and answered less.
//!
//! Two credentials, one rule. A per-request signed header set names either
//! a member id (`x-edet-viewer`) or the key itself (`x-edet-viewer-key`, the
//! only form onboarding can present); a bearer session token (§Standing,
//! `session.rs`) amortizes the signature over a short TTL. Both end at the
//! same fact — this caller controls a key the ledger attributes to a member —
//! and both verify through `verify_signed_headers`'s one signed message, so
//! the replay window, the single-use rule and the path binding are shared
//! rather than reasoned about twice.
//!
//! There is no third. The client embeds no node, so no read arrives over
//! Tauri IPC and nothing carries these values as command arguments; a second
//! construction of a credential nobody presents would be a rule that drifts
//! out of step with the one that is enforced.
//!
//! Structurally parallel to `http.rs::p2p_guard`: both are perimeter checks
//! ahead of a handler, reusing the same Ed25519 verification shape
//! `block::SignedTx::verify` already established for writes.
//!
//! (A header claiming "plumbing only … no view
//! yet CONSULTS it". That stopped being true at step 4 and stayed on the file
//! for weeks — tracked as `STATUS.md` T8. A module header is the first thing
//! read and the last thing updated.)

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use edet_state::types::{Key, MemberId};

use super::driver::Node;
use super::session::now_unix_secs;

/// Replay bound for a §Standing signed read (small skew window, checked both
/// directions): reused from the same rationale `tx_digest`/`SignedTx`
/// already establish for writes — the signature binds the caller to
/// content only they control (here: the path and a timestamp), so a
/// captured request is only ever replayable within this window, and only
/// against the one path it was signed over.
const TIMESTAMP_SKEW_SECS: u64 = 60;

pub(crate) const VIEWER_HEADER: &str = "x-edet-viewer";
pub(crate) const VIEWER_SIG_HEADER: &str = "x-edet-viewer-sig";
pub(crate) const VIEWER_TS_HEADER: &str = "x-edet-viewer-ts";
/// Names the KEY the caller holds, in place of `x-edet-viewer`'s member id.
///
/// The id-addressed form cannot serve a caller that does not yet know its
/// own member id, and that is not a corner case: it is exactly the state
/// every device is in during onboarding. A restoring device has re-derived
/// its key from a recovery phrase and is asking the ledger which membership
/// holds it; a newly-created identity is polling for its own admission. Both
/// hold a key and no id, so neither can fill in `x-edet-viewer` — and
/// `/session`, the only way to get a bearer token, needs an id too. Without
/// this header those two flows have no credential they can possibly present.
pub(crate) const VIEWER_KEY_HEADER: &str = "x-edet-viewer-key";
/// The 16 random bytes that make two identical requests two different
/// credentials.
///
/// **Ed25519 is deterministic.** Without this header a wallet reading
/// `/members` twice in the same second produces the identical signature both
/// times, so the replay cache below would refuse the second — the honest one.
/// The nonce is part of the signed bytes, so it is the caller who chooses it
/// and nobody in the middle who can vary it.
pub(crate) const VIEWER_NONCE_HEADER: &str = "x-edet-viewer-nonce";

/// Every header this module reads, so `http.rs`'s CORS allowlist is derived
/// from the list rather than repeating it.
///
/// Worth the indirection: a header missing from the allowlist does not
/// degrade to an unauthenticated read, it makes the browser refuse to send
/// the request at all — the failure surfaces as an opaque transport error
/// far from its cause, which is exactly how `VIEWER_KEY_HEADER` was missed
/// on its first run.
pub(crate) const VIEWER_HEADERS: [&str; 5] =
    [VIEWER_HEADER, VIEWER_KEY_HEADER, VIEWER_SIG_HEADER, VIEWER_TS_HEADER, VIEWER_NONCE_HEADER];

/// The exact bytes a viewer signature covers:
/// `"edet-view-v2\n<chain>\n<METHOD> <path>\n<unix_secs>\n<nonce hex>"`.
/// Ed25519 signs this message directly — no separate digest step like
/// `tx_digest`'s encode-then-sha256, since Ed25519 already hashes internally.
///
/// **The domain tag and the chain id are both bindings, not decoration.** The
/// tag says what kind of thing was signed, so a viewer credential can never be
/// reinterpreted as a payload of another kind however the bytes line up. The
/// chain id says WHICH LEDGER it authenticates against: without it, a
/// credential captured in transit — these travel in clear unless an operator
/// terminates TLS in front of the node, which `Config::bind_all` documents as
/// a requirement — reads as that member on every other chain where the same
/// key is a member, for the whole 60-second window.
///
/// **The nonce is what makes the credential single-use.** Ed25519 is
/// deterministic, so two honest reads of one path in one second would
/// otherwise carry the identical signature and a signature-keyed replay cache
/// (`super::replay`) would refuse the second. `v2` rather than `v1` because
/// the bytes changed: a wallet and a node disagreeing about the message would
/// otherwise fail as a bad signature, which reads as a wrong key.
pub(crate) fn viewer_auth_message(chain_id: &str, method: &str, path: &str, ts: u64, nonce_hex: &str) -> Vec<u8> {
    format!("edet-view-v2\n{chain_id}\n{method} {path}\n{ts}\n{nonce_hex}").into_bytes()
}

/// The nonce a credential carries, as the caller wrote it.
///
/// Checked for LENGTH and hex, and otherwise opaque: what it has to be is
/// unpredictable to whoever is reading the wire, and that is the caller's own
/// interest to serve. A node that tried to police randomness would be
/// asserting something it cannot check.
const VIEWER_NONCE_HEX_LEN: usize = 32;

/// Why a claimed viewer identity didn't resolve. Every variant is a hard
/// rejection (401) at the extractor boundary — a credential that IS present
/// but doesn't check out is never silently downgraded to anonymous, per the
/// fail-closed style the module comments elsewhere in this crate keep to.
#[derive(Debug, PartialEq, Eq)]
pub enum ViewerAuthError {
    MalformedHeader,
    UnknownMember,
    BadSignature,
    StaleTimestamp,
    UnknownOrExpiredToken,
    /// A key-addressed proof whose signature VERIFIED against the key it
    /// names, but that key belongs to no member of this ledger.
    ///
    /// Kept separate from `UnknownMember` because the two mean opposite
    /// things to a caller. `UnknownMember` is a claim that failed — someone
    /// asserted they are member N and could not back it. This is a claim
    /// that succeeded and simply names nobody: the holder of a key the
    /// ledger has never admitted is, correctly, anonymous. Onboarding lives
    /// in exactly that state while it waits for a sponsor, so the read path
    /// resolves it to `None` rather than 401 (see the `Viewer` extractor);
    /// `/session` still treats it as a hard failure, because there is no
    /// member to mint a token for.
    ///
    /// Carries the key, because the caller that cares about it — the pending
    /// pool, where a newcomer co-signs the trade that will seat them — needs
    /// the name the proof DID establish. Re-deriving it meant a second
    /// construction of the signed message beside the one that is enforced,
    /// which is exactly the shape that drifts.
    UnattributedKey(Key),
    /// This exact signature has already been presented inside its window.
    ///
    /// A credential travels in clear unless an operator terminates TLS in
    /// front of the node, so whoever reads one off the wire could replay it as
    /// that member for the rest of the 60-second skew. Refusing the second
    /// presentation makes a captured credential worthless. Distinct from
    /// `BadSignature` because it is: the signature is perfectly good and it
    /// has been spent.
    Replayed,
}

impl ViewerAuthError {
    fn message(&self) -> &'static str {
        match self {
            ViewerAuthError::MalformedHeader => "malformed viewer auth header",
            ViewerAuthError::UnknownMember => "unknown member",
            ViewerAuthError::BadSignature => "bad signature",
            ViewerAuthError::StaleTimestamp => "stale or future timestamp",
            ViewerAuthError::UnknownOrExpiredToken => "unknown or expired session token",
            ViewerAuthError::UnattributedKey(_) => "no member holds this key",
            ViewerAuthError::Replayed => "replayed viewer signature",
        }
    }
}

impl IntoResponse for ViewerAuthError {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED, self.message()).into_response()
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Verify a §Standing signed-header request against `state`: returns the member id
/// and the specific key (of that member's current keyset) whose signature
/// verified, or the reason verification failed. Shared by the `Viewer`
/// extractor (for ordinary reads) and the `/session` mint handler (which
/// exchanges one such signed request for a bearer token).
///
/// Two ways to name the caller, both proving the same thing — possession of
/// a key the ledger attributes to a member — and both signing the identical
/// `viewer_auth_message`, so the replay and path-binding properties are
/// shared rather than reasoned about twice:
///
///   - `x-edet-viewer`: a member id. The signature is checked against every
///     key in that member's current keyset, so a rotation does not
///     invalidate a client that reaches for a still-listed key.
///   - `x-edet-viewer-key`: a public key. The signature is checked against
///     that key alone — it is supplied in the request, so it is verifiable
///     with no ledger lookup at all — and the ledger is then asked who holds
///     it. This is what a caller uses when it holds a key but does not know
///     its own id (see `VIEWER_KEY_HEADER`).
///
/// The key-addressed form grants nothing the id-addressed form does not:
/// both end at "this caller controls a key member M holds", which is the
/// same fact by either route, and neither can be produced without the
/// private half.
pub(crate) fn verify_signed_headers(
    state: &edet_state::State,
    seen: &mut super::replay::SeenSignatures,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    now_secs: u64,
) -> Result<(MemberId, Key), ViewerAuthError> {
    let sig_hex = headers
        .get(VIEWER_SIG_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(ViewerAuthError::MalformedHeader)?;
    let ts_str = headers
        .get(VIEWER_TS_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(ViewerAuthError::MalformedHeader)?;
    let nonce_hex = headers
        .get(VIEWER_NONCE_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(ViewerAuthError::MalformedHeader)?;
    if nonce_hex.len() != VIEWER_NONCE_HEX_LEN || hex_decode(nonce_hex).is_none() {
        return Err(ViewerAuthError::MalformedHeader);
    }
    let ts: u64 = ts_str.parse().map_err(|_| ViewerAuthError::MalformedHeader)?;
    let sig_bytes = hex_decode(sig_hex).ok_or(ViewerAuthError::MalformedHeader)?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|_| ViewerAuthError::MalformedHeader)?;

    // Checked before either branch: a stale request is stale whichever way
    // it named its caller, and doing it first keeps the window uniform.
    if now_secs.abs_diff(ts) > TIMESTAMP_SKEW_SECS {
        return Err(ViewerAuthError::StaleTimestamp);
    }
    let msg = viewer_auth_message(&state.chain_id, method, path, ts, nonce_hex);

    // Key-addressed. Takes precedence when both headers are present: it is
    // the strictly more specific claim (one named key, not "some key of
    // member N"), and letting the weaker one win would make which check ran
    // depend on header order.
    let attributed = if let Some(key_hex) = headers.get(VIEWER_KEY_HEADER).and_then(|v| v.to_str().ok()) {
        attribute_key_proof(state, key_hex, &sig, &msg)
    } else {
        // Member-id-addressed.
        let id_str = headers
            .get(VIEWER_HEADER)
            .and_then(|v| v.to_str().ok())
            .ok_or(ViewerAuthError::MalformedHeader)?;
        let member_id: MemberId = id_str.parse().map_err(|_| ViewerAuthError::MalformedHeader)?;
        let member = state.members.get(&member_id).ok_or(ViewerAuthError::UnknownMember)?;
        member
            .keys
            .iter()
            .find(|key| VerifyingKey::from_bytes(key).is_ok_and(|vk| vk.verify(&msg, &sig).is_ok()))
            .map(|key| (member_id, *key))
            .ok_or(ViewerAuthError::BadSignature)
    };

    // **Spent AFTER the signature verified, and only then.** A cache fed by
    // unverified bytes is a store anybody can fill for the cost of sending
    // them; here every entry cost a member key and a signature. The credential
    // is spent even when it names a key the ledger does not hold
    // (`UnattributedKey`), because that proof verified too and is just as
    // replayable — but never when the signature itself failed, or a bad
    // signature would burn the nonce an honest retry is about to use.
    match &attributed {
        Ok(_) | Err(ViewerAuthError::UnattributedKey(_)) => {
            let bytes: [u8; 64] = sig.to_bytes();
            if !seen.admit(bytes, ts, now_secs, TIMESTAMP_SKEW_SECS) {
                return Err(ViewerAuthError::Replayed);
            }
        }
        Err(_) => {}
    }
    attributed
}

/// The key-addressed check itself: verify `sig` over `msg` under the named
/// key, then ask the ledger who holds it.
///
/// Order matters and is not an implementation detail. The signature is
/// checked BEFORE the `key_index` lookup, so a caller who cannot sign learns
/// nothing about whether the key they named is on the ledger — the reverse
/// order would make this a free membership oracle for any key an
/// unauthenticated caller cared to try.
fn attribute_key_proof(
    state: &edet_state::State,
    key_hex: &str,
    sig: &Signature,
    msg: &[u8],
) -> Result<(MemberId, Key), ViewerAuthError> {
    let key: Key = hex_decode(key_hex)
        .and_then(|b| b.try_into().ok())
        .ok_or(ViewerAuthError::MalformedHeader)?;
    let vk = VerifyingKey::from_bytes(&key).map_err(|_| ViewerAuthError::MalformedHeader)?;
    vk.verify(msg, sig).map_err(|_| ViewerAuthError::BadSignature)?;
    state
        .key_index
        .get(&key)
        .copied()
        .map(|id| (id, key))
        .ok_or(ViewerAuthError::UnattributedKey(key))
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn has_any_viewer_header(headers: &HeaderMap) -> bool {
    VIEWER_HEADERS.iter().any(|h| headers.contains_key(*h))
}

/// `viewer: Option<u64>`, resolved before any read view is built.
/// `None` means anonymous/unauthenticated — every endpoint still ANSWERS it,
/// and answers it less (`views.rs`). Preferred order when both are present:
/// a Bearer session token (§Standing, the cheap path a polling UI actually uses),
/// falling back to a §Standing signed header set; neither present at all is
/// anonymous, not an error.
pub struct Viewer(pub Option<u64>);

#[async_trait::async_trait]
impl FromRequestParts<Arc<Node>> for Viewer {
    type Rejection = ViewerAuthError;

    async fn from_request_parts(parts: &mut Parts, node: &Arc<Node>) -> Result<Self, Self::Rejection> {
        if let Some(token) = bearer_token(&parts.headers) {
            let now = now_unix_secs();
            let core = node.lock();
            return match core.sessions.resolve(token, now) {
                Some(id) => Ok(Viewer(Some(id))),
                None => Err(ViewerAuthError::UnknownOrExpiredToken),
            };
        }
        if has_any_viewer_header(&parts.headers) {
            let now = now_unix_secs();
            let mut guard = node.lock();
            let core = &mut *guard;
            return match verify_signed_headers(
                &core.replica.state,
                &mut core.seen,
                &parts.headers,
                parts.method.as_str(),
                parts.uri.path(),
                now,
            ) {
                Ok((id, _key)) => Ok(Viewer(Some(id))),
                // Not a downgrade of a credential that failed — the
                // signature verified, it just names a key this ledger has
                // never admitted, and the honest identity for its holder is
                // nobody. 401 here would break the one flow that legitimately
                // occupies this state: a newly created identity polling for
                // its own admission, which must be able to keep asking (and
                // be told "not yet") until a sponsor acts.
                Err(ViewerAuthError::UnattributedKey(_)) => Ok(Viewer(None)),
                Err(e) => Err(e),
            };
        }
        Ok(Viewer(None))
    }
}

/// The caller as a PARTY rather than as a member: `Party::Member` when the
/// ledger attributes their key, `Party::Key` when the proof verified and names
/// a key no member holds.
///
/// `Viewer` collapses that second case to anonymous, which is right for every
/// read whose subject is a member — a key nobody holds is nobody, and a reader
/// who is nobody sees the public row. It is wrong for exactly one surface: the
/// pending pool, where a newcomer must be able to see and co-sign
/// the first trade that will seat their account. So this extractor exists
/// beside `Viewer` rather than replacing it, and it is used only there.
///
/// It grants nothing `Viewer` does not. Both end at "this caller controls this
/// key"; this one simply keeps the key instead of discarding it when the
/// ledger has no name for it, and the handler still has to decide what a
/// caller who is only a key may see.
pub struct ViewerParty(pub Option<edet_state::types::Party>);

#[async_trait::async_trait]
impl FromRequestParts<Arc<Node>> for ViewerParty {
    type Rejection = ViewerAuthError;

    async fn from_request_parts(parts: &mut Parts, node: &Arc<Node>) -> Result<Self, Self::Rejection> {
        use edet_state::types::Party;
        if let Some(token) = bearer_token(&parts.headers) {
            let now = now_unix_secs();
            let core = node.lock();
            return match core.sessions.resolve(token, now) {
                Some(id) => Ok(ViewerParty(Some(Party::Member(id)))),
                None => Err(ViewerAuthError::UnknownOrExpiredToken),
            };
        }
        if has_any_viewer_header(&parts.headers) {
            let now = now_unix_secs();
            let mut guard = node.lock();
            let core = &mut *guard;
            return match verify_signed_headers(
                &core.replica.state,
                &mut core.seen,
                &parts.headers,
                parts.method.as_str(),
                parts.uri.path(),
                now,
            ) {
                Ok((id, _key)) => Ok(ViewerParty(Some(Party::Member(id)))),
                // The one case `Viewer` throws away. The signature verified;
                // what the ledger cannot do is give the holder a name yet.
                Err(ViewerAuthError::UnattributedKey(key)) => Ok(ViewerParty(Some(Party::Key(key)))),
                Err(e) => Err(e),
            };
        }
        Ok(ViewerParty(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// A fresh nonce per credential, which is what makes two otherwise
    /// identical requests two credentials.
    fn nonce() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(1);
        format!("{:032x}", N.fetch_add(1, Ordering::Relaxed))
    }

    fn signed_headers(sk: &SigningKey, member_id: MemberId, method: &str, path: &str, ts: u64) -> HeaderMap {
        signed_headers_with(sk, member_id, method, path, ts, &nonce())
    }

    fn signed_headers_with(
        sk: &SigningKey,
        member_id: MemberId,
        method: &str,
        path: &str,
        ts: u64,
        nonce_hex: &str,
    ) -> HeaderMap {
        // Every state built below is a `State::default()`, whose chain id this is.
        let msg = viewer_auth_message(crate::block::DEV_CHAIN_ID, method, path, ts, nonce_hex);
        let sig = sk.sign(&msg);
        let mut h = HeaderMap::new();
        h.insert(VIEWER_HEADER, member_id.to_string().parse().unwrap());
        let sig_hex: String = sig.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        h.insert(VIEWER_SIG_HEADER, sig_hex.parse().unwrap());
        h.insert(VIEWER_TS_HEADER, ts.to_string().parse().unwrap());
        h.insert(VIEWER_NONCE_HEADER, nonce_hex.parse().unwrap());
        h
    }

    /// A store nothing else is sharing, so each probe measures its own claim.
    fn seen() -> super::super::replay::SeenSignatures {
        Default::default()
    }

    fn genesis_with_key(sk: &SigningKey) -> edet_state::State {
        let mut st = edet_state::State::default();
        st.add_underwriter(vec![sk.verifying_key().to_bytes()], 25_000.0)
            .expect("add member");
        st
    }

    #[test]
    fn valid_signed_request_resolves_the_right_viewer() {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let st = genesis_with_key(&sk);
        let h = signed_headers(&sk, 0, "GET", "/members", 1_000);
        let (id, key) = verify_signed_headers(&st, &mut seen(), &h, "GET", "/members", 1_000).expect("should verify");
        assert_eq!(id, 0);
        assert_eq!(key, sk.verifying_key().to_bytes());
    }

    #[test]
    fn bad_signature_is_rejected() {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let other = SigningKey::from_bytes(&[4u8; 32]);
        let st = genesis_with_key(&sk);
        // Signed by a different key than member 0 holds.
        let h = signed_headers(&other, 0, "GET", "/members", 1_000);
        let err = verify_signed_headers(&st, &mut seen(), &h, "GET", "/members", 1_000).unwrap_err();
        assert_eq!(err, ViewerAuthError::BadSignature);
    }

    #[test]
    fn unknown_member_is_rejected() {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let st = genesis_with_key(&sk);
        let h = signed_headers(&sk, 99, "GET", "/members", 1_000);
        let err = verify_signed_headers(&st, &mut seen(), &h, "GET", "/members", 1_000).unwrap_err();
        assert_eq!(err, ViewerAuthError::UnknownMember);
    }

    #[test]
    fn stale_timestamp_is_rejected() {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let st = genesis_with_key(&sk);
        let h = signed_headers(&sk, 0, "GET", "/members", 1_000);
        let err = verify_signed_headers(&st, &mut seen(), &h, "GET", "/members", 1_000 + TIMESTAMP_SKEW_SECS + 1)
            .unwrap_err();
        assert_eq!(err, ViewerAuthError::StaleTimestamp);
    }

    #[test]
    fn a_signature_over_a_different_path_does_not_verify_for_this_one() {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let st = genesis_with_key(&sk);
        let h = signed_headers(&sk, 0, "GET", "/member/3", 1_000);
        let err = verify_signed_headers(&st, &mut seen(), &h, "GET", "/members", 1_000).unwrap_err();
        assert_eq!(err, ViewerAuthError::BadSignature, "path is inside the signed payload");
    }

    #[test]
    fn missing_headers_are_malformed_not_a_panic() {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let st = genesis_with_key(&sk);
        let err = verify_signed_headers(&st, &mut seen(), &HeaderMap::new(), "GET", "/members", 1_000).unwrap_err();
        assert_eq!(err, ViewerAuthError::MalformedHeader);
    }

    // --- key-addressed proofs (the onboarding credential) -------------------

    /// Same shape as `signed_headers`, but naming the KEY instead of a
    /// member id — what a device that has not yet learned its own id sends.
    fn key_headers(sk: &SigningKey, claimed: [u8; 32], method: &str, path: &str, ts: u64) -> HeaderMap {
        // Every state built below is a `State::default()`, whose chain id this is.
        let nonce_hex = nonce();
        let msg = viewer_auth_message(crate::block::DEV_CHAIN_ID, method, path, ts, &nonce_hex);
        let sig = sk.sign(&msg);
        let mut h = HeaderMap::new();
        let hex = |b: &[u8]| -> String { b.iter().map(|x| format!("{x:02x}")).collect() };
        h.insert(VIEWER_KEY_HEADER, hex(&claimed).parse().unwrap());
        h.insert(VIEWER_SIG_HEADER, hex(&sig.to_bytes()).parse().unwrap());
        h.insert(VIEWER_TS_HEADER, ts.to_string().parse().unwrap());
        h.insert(VIEWER_NONCE_HEADER, nonce_hex.parse().unwrap());
        h
    }

    #[test]
    fn a_key_proof_resolves_the_holder_without_being_told_the_member_id() {
        // The whole point: no `x-edet-viewer` anywhere in this request, which
        // is the only state a restoring device can be in.
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let st = genesis_with_key(&sk);
        let h = key_headers(&sk, sk.verifying_key().to_bytes(), "GET", "/whois/abc", 1_000);
        let (id, key) = verify_signed_headers(&st, &mut seen(), &h, "GET", "/whois/abc", 1_000).expect("should verify");
        assert_eq!(id, 0);
        assert_eq!(key, sk.verifying_key().to_bytes());
    }

    #[test]
    fn naming_a_key_you_do_not_hold_proves_nothing() {
        // The impersonation attempt this credential has to refuse: claim the
        // victim's key, sign with your own. Without the private half there is
        // no signature that verifies under the named key.
        let victim = SigningKey::from_bytes(&[3u8; 32]);
        let attacker = SigningKey::from_bytes(&[4u8; 32]);
        let st = genesis_with_key(&victim);
        let h = key_headers(&attacker, victim.verifying_key().to_bytes(), "GET", "/whois/abc", 1_000);
        let err = verify_signed_headers(&st, &mut seen(), &h, "GET", "/whois/abc", 1_000).unwrap_err();
        assert_eq!(err, ViewerAuthError::BadSignature);
    }

    #[test]
    fn a_key_no_member_holds_is_unattributed_rather_than_a_failure() {
        // A freshly created identity waiting for a sponsor. The proof is
        // sound; there is simply nobody it names yet. The extractor turns
        // this into anonymous so the admission poll can keep running.
        let stranger = SigningKey::from_bytes(&[9u8; 32]);
        let st = genesis_with_key(&SigningKey::from_bytes(&[3u8; 32]));
        let h = key_headers(&stranger, stranger.verifying_key().to_bytes(), "GET", "/whois/abc", 1_000);
        let err = verify_signed_headers(&st, &mut seen(), &h, "GET", "/whois/abc", 1_000).unwrap_err();
        assert_eq!(err, ViewerAuthError::UnattributedKey(stranger.verifying_key().to_bytes()));
    }

    #[test]
    fn a_key_proof_is_bound_to_its_path_and_its_clock_like_the_id_form() {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let st = genesis_with_key(&sk);
        let pk = sk.verifying_key().to_bytes();

        let h = key_headers(&sk, pk, "GET", "/member/3", 1_000);
        assert_eq!(
            verify_signed_headers(&st, &mut seen(), &h, "GET", "/members", 1_000).unwrap_err(),
            ViewerAuthError::BadSignature,
            "the path is inside the signed payload here too"
        );

        let h = key_headers(&sk, pk, "GET", "/members", 1_000);
        assert_eq!(
            verify_signed_headers(&st, &mut seen(), &h, "GET", "/members", 1_000 + TIMESTAMP_SKEW_SECS + 1)
                .unwrap_err(),
            ViewerAuthError::StaleTimestamp,
            "the replay window is shared, not re-derived"
        );
    }

    #[test]
    fn a_malformed_key_header_never_falls_through_to_the_id_path() {
        // Both headers present, the key one garbage. It must fail rather
        // than quietly resolve via the member id — otherwise a caller could
        // pick which check runs by malforming the stricter one.
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let st = genesis_with_key(&sk);
        let mut h = signed_headers(&sk, 0, "GET", "/members", 1_000);
        h.insert(VIEWER_KEY_HEADER, "not-hex".parse().unwrap());
        assert_eq!(
            verify_signed_headers(&st, &mut seen(), &h, "GET", "/members", 1_000).unwrap_err(),
            ViewerAuthError::MalformedHeader
        );
    }
}
