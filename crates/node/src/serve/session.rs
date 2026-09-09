//! In-memory bearer session tokens (§Standing of
//! the paper's §Implementation): amortizes a per-read
//! signature into one signed request at mint time (`POST /session`),
//! trading a signature-per-read for cheap bearer-token GETs over a short
//! TTL — the read-side analogue of the seed vault's unlock.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use edet_state::types::{Key, MemberId, MemberStatus};

/// TTL for a minted session token: minutes, not hours (§Standing) — this is a hot
/// wallet-adjacent UI session, not a long-lived web login.
pub const SESSION_TTL_SECS: u64 = 15 * 60;

/// Real wall-clock seconds, independent of the ledger's own epoch clock:
/// session TTLs are a device-session concept, not an economic one, so they
/// run on actual time and are never denominated in epochs.
pub(crate) fn now_unix_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// One minted bearer token: which member it authenticates, and the exact
/// key (of that member's keyset at mint time) whose signature minted it —
/// bound to the *key*, not just the member id, so a rotated-out device's
/// token stops working the moment its key leaves `member.keys`, rather than
/// keeping read access after a rotation is supposed to have cut it off.
#[derive(Clone, Debug)]
struct SessionEntry {
    member_id: MemberId,
    key: Key,
    expires_secs: u64,
}

/// `mint` failed to produce a token. The only cause is the OS CSPRNG call
/// itself failing (`getrandom::Error`) — e.g. a sandboxed/restricted
/// environment with no working entropy source. Deliberately opaque (no
/// wrapped error payload): the caller's only sound response is "try again or
/// fail the request", never a decision made on the specific OS error.
#[derive(Debug)]
pub struct SessionError;

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "session token generation failed (OS RNG unavailable)")
    }
}

impl std::error::Error for SessionError {}

/// The node's in-memory token store. Lives on `NodeCore`, behind the same
/// mutex as everything else the HTTP handlers touch — no separate lock, no
/// persistence (a restart simply invalidates every session, which is fine:
/// re-minting is one more signature).
///
/// Keyed by `sha256(token_bytes)`, never by the raw token: (1) `resolve`
/// then never compares secret material against anything an attacker could
/// have influenced the shape of — it's a plain `BTreeMap` lookup on a
/// value derived from, but not equal to, the credential — and (2) a memory
/// dump (core dump, swapped page, debugger) of a running node yields only
/// token hashes, not live bearer tokens an attacker could replay.
#[derive(Default)]
pub struct SessionStore {
    tokens: BTreeMap<[u8; 32], SessionEntry>,
}

impl SessionStore {
    /// Mint a fresh opaque bearer token for `member_id`/`key`, valid until
    /// `now_secs + SESSION_TTL_SECS`. Returns the token (hex) and its
    /// expiry, or `SessionError` if the OS CSPRNG draw failed — this must
    /// never fall back to a weaker source, so failure is propagated rather
    /// than papered over.
    pub fn mint(&mut self, member_id: MemberId, key: Key, now_secs: u64) -> Result<(String, u64), SessionError> {
        let raw = fresh_bytes32()?;
        let token = crate::block::hex32(&raw);
        let expires_secs = now_secs + SESSION_TTL_SECS;
        self.tokens
            .insert(crate::block::sha256(&raw), SessionEntry { member_id, key, expires_secs });
        Ok((token, expires_secs))
    }

    /// Resolve a bearer token to its member id, iff present and not expired.
    /// Hashes the presented token before looking it up — the store never
    /// holds a raw token to compare against (see the struct doc comment).
    /// Does not itself check liveness against current state (rotation/
    /// suspension) — that is `prune_invalid`'s job, run after every
    /// committed block, so a resolvable-but-stale token can only exist for
    /// at most one tick.
    pub fn resolve(&self, token: &str, now_secs: u64) -> Option<MemberId> {
        let raw = decode_hex32(token)?;
        self.tokens
            .get(&crate::block::sha256(&raw))
            .filter(|e| e.expires_secs > now_secs)
            .map(|e| e.member_id)
    }

    /// Drop every token past its TTL.
    pub fn prune_expired(&mut self, now_secs: u64) {
        self.tokens.retain(|_, e| e.expires_secs > now_secs);
    }

    /// Drop every token that is no longer valid against current state
    /// The member was suspended (default: immediate
    /// invalidation, not left to expire on TTL), no longer exists, or the
    /// specific key the token was minted under has left the member's
    /// current keyset (a completed rotation — `edet_state::apply`'s
    /// `rotate_finalize` — removes the pre-rotation key from `member.keys`,
    /// so checking key membership here covers rotation-invalidation without
    /// a separate per-tx hook into `apply`). Called after every committed
    /// block (`NodeCore::try_commit`), alongside `prune_expired`.
    pub fn prune_invalid(&mut self, state: &edet_state::State) {
        self.tokens.retain(|_, e| {
            state
                .members
                .get(&e.member_id)
                .is_some_and(|m| m.status != MemberStatus::Suspended && m.keys.contains(&e.key))
        });
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Test-only: whether the raw token bytes are themselves present as a
    /// map key, i.e. proof the store did NOT take the shortcut of keying on
    /// the raw token (see the struct doc comment for why that matters).
    #[cfg(test)]
    pub(crate) fn contains_raw_key(&self, raw: &[u8; 32]) -> bool {
        self.tokens.contains_key(raw)
    }
}

/// 32 unpredictable bytes for a token, drawn straight from the OS CSPRNG via
/// `getrandom`.
///
/// It is not four calls to `RandomState::new().build_hasher().finish()`
/// on the theory that `RandomState::new()` draws fresh OS randomness every
/// time, the same way `HashMap`'s DoS-resistant hashing does. It does not:
/// std seeds one thread-local `(k0, k1)` key pair from the OS *once* and then
/// every subsequent `RandomState::new()` on that thread does
/// `keys.set((k0.wrapping_add(1), k1))` — `k0` is a plain per-thread counter,
/// `k1` is fixed for the thread's lifetime (verified against
/// `library/std/src/hash/random.rs` in the installed toolchain). `finish()`
/// on a hasher that has been fed no data is a pure function of that key, so
/// the four "independent draws" were four SipHash-1-3 outputs at four
/// consecutive, related keys — and the next call on the same thread
/// continues the same counter, so tokens minted later are not independent of
/// tokens minted earlier either. SipHash-1-3 is a hash-table PRF (built for
/// speed and DoS-resistance against chosen-input collisions), not a CSPRNG
/// (built to resist key/output recovery); nothing about the construction
/// stops an attacker who sees a few tokens from a thread from predicting the
/// rest. This credential authenticates reads of private financial positions
/// on a live Malachite validator (`/session` is wired through
/// `client_routes()`/`client_router()` in `serve/http.rs`, which is exactly
/// what a validator node exposes — this was never a dev-only code path), so
/// it draws from the OS RNG on every call, full stop.
fn fresh_bytes32() -> Result<[u8; 32], SessionError> {
    let mut out = [0u8; 32];
    getrandom::getrandom(&mut out).map_err(|_| SessionError)?;
    Ok(out)
}

/// Decode a lowercase-hex bearer token back to 32 raw bytes, the inverse of
/// `block::hex32`. `None` on any malformed input (wrong length, non-hex
/// characters) rather than a panic — a bearer token is attacker-controlled
/// text arriving over HTTP. Kept local to this module (not added to
/// `block.rs`, which this task does not own) rather than shared.
fn decode_hex32(s: &str) -> Option<[u8; 32]> {
    let bytes = s.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in bytes.chunks(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_resolves_and_expires_on_ttl() {
        let mut store = SessionStore::default();
        let (token, expires) = store.mint(3, [7u8; 32], 1_000).expect("mint");
        assert_eq!(expires, 1_000 + SESSION_TTL_SECS);
        assert_eq!(store.resolve(&token, 1_000), Some(3));
        assert_eq!(store.resolve(&token, 1_000 + SESSION_TTL_SECS - 1), Some(3), "still within TTL");
        assert_eq!(store.resolve(&token, 1_000 + SESSION_TTL_SECS), None, "expired at the boundary");
        assert_eq!(store.resolve(&token, 1_000 + SESSION_TTL_SECS + 1), None);
    }

    #[test]
    fn unknown_token_never_resolves() {
        let store = SessionStore::default();
        assert_eq!(store.resolve("not-a-real-token", 0), None);
    }

    #[test]
    fn prune_expired_drops_only_stale_tokens() {
        let mut store = SessionStore::default();
        let (_fresh, _) = store.mint(1, [1u8; 32], 0).expect("mint");
        let (_stale, _) = store.mint(2, [2u8; 32], 0).expect("mint");
        assert_eq!(store.len(), 2);
        store.prune_expired(SESSION_TTL_SECS + 1);
        assert_eq!(store.len(), 0, "both tokens minted at t=0 are stale by t=TTL+1");

        let (still_fresh, _) = store.mint(1, [1u8; 32], 100).expect("mint");
        store.prune_expired(50);
        assert_eq!(store.len(), 1, "a token minted after the sweep time must survive");
        assert_eq!(store.resolve(&still_fresh, 100), Some(1));
    }

    /// Tokens must come from the OS CSPRNG, not a per-thread counter —
    /// 10k mints with no collision is a cheap sanity floor (a per-thread
    /// SipHash counter construction would also pass this particular check,
    /// so it does not alone prove the fix; `token_is_not_the_map_key` below
    /// checks the structural half of the fix directly).
    #[test]
    fn ten_thousand_mints_are_all_distinct() {
        let mut store = SessionStore::default();
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..10_000u64 {
            let (token, _) = store.mint(i, [0u8; 32], 0).expect("mint");
            assert!(seen.insert(token), "duplicate token minted");
        }
        assert_eq!(seen.len(), 10_000);
    }

    /// A token string that was never returned by `mint` must never resolve,
    /// even when it is well-formed hex of the right length (i.e. this is not
    /// just testing the length/format guard in `decode_hex32`).
    #[test]
    fn never_minted_well_formed_token_does_not_resolve() {
        let mut store = SessionStore::default();
        let (_minted, _) = store.mint(1, [1u8; 32], 0).expect("mint");
        let guessed = "ab".repeat(32); // 64 hex chars, well-formed, never minted
        assert_eq!(store.resolve(&guessed, 0), None);
    }

    /// Structural check: the store must key on `sha256(token)`, never on
    /// the raw token bytes, so neither a map lookup nor a memory dump ever
    /// puts the live credential next to secret material.
    #[test]
    fn raw_token_bytes_are_not_the_map_key() {
        let mut store = SessionStore::default();
        let (token, _) = store.mint(1, [1u8; 32], 0).expect("mint");
        let raw = decode_hex32(&token).expect("mint returns well-formed hex");
        assert!(!store.contains_raw_key(&raw), "the raw token bytes must not appear as a map key");
        // Sanity: the token still resolves via its hash.
        assert_eq!(store.resolve(&token, 0), Some(1));
    }

    /// Ids are assigned by the ledger, densely from zero, so `id` names the
    /// member the CALLER expects to get — asserted rather than trusted, since
    /// a fixture that silently seats somebody else would make every token
    /// assertion below vacuous.
    fn state_with_member(id: MemberId, key: Key, status: MemberStatus) -> edet_state::State {
        let mut st = edet_state::State::default();
        let real_id = st.add_underwriter(vec![key], 25_000.0).expect("add member");
        assert_eq!(real_id, id, "the fixture must seat the member the test names");
        if let Some(m) = st.members.get_mut(&real_id) {
            m.status = status;
        }
        st
    }

    #[test]
    fn prune_invalid_keeps_a_live_active_member_matching_key() {
        let mut store = SessionStore::default();
        let key0 = [10u8; 32];
        let (token, _) = store.mint(0, key0, 0).expect("mint");
        let st = state_with_member(0, key0, MemberStatus::Active);
        store.prune_invalid(&st);
        assert_eq!(store.resolve(&token, 0), Some(0), "active member with matching key keeps its token");
    }

    #[test]
    fn prune_invalid_drops_a_suspended_members_token_even_with_a_matching_key() {
        let mut store = SessionStore::default();
        let key0 = [10u8; 32];
        let (token, _) = store.mint(0, key0, 0).expect("mint");
        let st = state_with_member(0, key0, MemberStatus::Suspended);
        store.prune_invalid(&st);
        assert_eq!(store.resolve(&token, 0), None, "suspension invalidates immediately");
    }

    #[test]
    fn prune_invalid_drops_a_rotated_out_key() {
        let mut store = SessionStore::default();
        let key0 = [10u8; 32];
        let (token, _) = store.mint(0, key0, 0).expect("mint");
        let mut st = state_with_member(0, key0, MemberStatus::Active);
        if let Some(m) = st.members.get_mut(&0) {
            m.keys = vec![[99u8; 32]]; // simulate `rotate_finalize` swapping in new keys
        }
        store.prune_invalid(&st);
        assert_eq!(store.resolve(&token, 0), None, "a rotated-out key must no longer authenticate");
    }

    #[test]
    fn prune_invalid_drops_a_token_for_a_member_that_no_longer_exists() {
        let mut store = SessionStore::default();
        let (token, _) = store.mint(42, [10u8; 32], 0).expect("mint");
        store.prune_invalid(&edet_state::State::default());
        assert_eq!(store.resolve(&token, 0), None);
    }
}
