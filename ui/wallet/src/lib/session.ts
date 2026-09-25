/**
 * Read-session tokens (the paper's §Implementation): one signed `POST /session`
 * request, made with the seed the
 * device already holds for the acting member, exchanged for a short-lived
 * opaque bearer token. Every subsequent read carries
 * `Authorization: Bearer <token>` (wired into lib/api.ts's `read()` via
 * `configureAuth`) instead of signing every single GET — the same
 * "one signature behind an unlock, then cheap reads" shape the encrypted
 * seed vault already uses (common/vault.ts), which is why this module is
 * described as the read-side analogue of that unlock, not a new primitive.
 *
 * Every host, one shape. A second would be needed only if the app read its own
 * embedded node over IPC, which carries no headers, so an in-app read signed
 * a `keyProof` per read instead and `mintSession` no-opped there. The app has
 * no node of its own now, so a token rides on every read.
 *
 * `keyProof` below stays, for the two reads that authenticate by KEY rather
 * than by member id — `whois` and `pendingListByKey`. Their caller has no
 * member id yet (that is the point of the read), so there is nothing for a
 * session to be a session FOR, and the proof travels in headers.
 */

import { get, writable } from 'svelte/store';

import { signDigest } from './crypto';
import { heldSeed } from './actors';

const VIEWER_HEADER = 'x-edet-viewer';
const VIEWER_SIG_HEADER = 'x-edet-viewer-sig';
const VIEWER_TS_HEADER = 'x-edet-viewer-ts';
const VIEWER_NONCE_HEADER = 'x-edet-viewer-nonce';

/** Safety margin subtracted from the node's reported expiry: prefer
 *  re-minting a little early over racing the node's own TTL check
 *  (`crates/node/src/serve/session.rs`'s TTL is minutes, this is seconds). */
const EXPIRY_SAFETY_SECS = 5;

export function nowUnixSecs(): number {
    return Math.floor(Date.now() / 1000);
}

/**
 * The chain a viewer credential authenticates against.
 *
 * Set by the poll (`lib/node.ts`), which is the one place that knows which
 * network this device is on; kept here because `node.ts` imports this module
 * and not the other way round. Empty until the first successful poll, which is
 * also when the app has nothing to authenticate to.
 */
export const viewerChain = writable<string>('');

/** The exact bytes a signed read request covers — byte-for-byte the same
 *  message `crates/node/src/serve/auth.rs::viewer_auth_message` recomputes and
 *  verifies against.
 *
 *  The domain tag and the chain id are bindings rather than decoration: the tag
 *  says what kind of thing was signed, and the chain says which ledger it
 *  authenticates against, so a credential captured in transit does not read as
 *  this member on every other chain where the same key is a member.
 *
 *  **The nonce is what makes the credential single-use.** The node admits each
 *  verified signature once inside its window, so a credential read off the wire
 *  is already spent — and Ed25519 is deterministic, so without a fresh nonce
 *  two honest reads of one path in one second would carry the identical
 *  signature and the node would refuse the second. */
export function viewerAuthMessage(method: string, path: string, ts: number, nonce: string): Uint8Array {
    return new TextEncoder().encode(`edet-view-v2\n${get(viewerChain)}\n${method} ${path}\n${ts}\n${nonce}`);
}

/** 16 bytes from the platform CSPRNG, hex — a fresh one per credential.
 *
 *  From `crypto.getRandomValues` rather than a counter or a clock: what it has
 *  to be is unpredictable to whoever is reading the wire, and a counter this
 *  device restarts is neither. */
export function viewerNonce(): string {
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    return toHex(bytes);
}

function signedHeaders(memberId: number, seed: Uint8Array, method: string, path: string, ts: number): Record<string, string> {
    const nonce = viewerNonce();
    return {
        [VIEWER_HEADER]: String(memberId),
        [VIEWER_SIG_HEADER]: toHex(signDigest(viewerAuthMessage(method, path, ts, nonce), seed)),
        [VIEWER_TS_HEADER]: String(ts),
        [VIEWER_NONCE_HEADER]: nonce,
    };
}

/** Hex, lowercase, no `0x` — the encoding both viewer headers use. */
function toHex(bytes: ArrayLike<number>): string {
    return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/**
 * A viewer credential that names the KEY this device holds instead of a
 * member id — for the two moments where no member id exists to name.
 *
 * Onboarding is the whole reason this form exists. A device restoring from a
 * recovery phrase has re-derived its key and is asking the ledger which
 * membership holds it; a freshly created identity is polling for its own
 * admission. Neither knows an id, and `mintSession` needs one, so the
 * id-addressed header and the bearer token are both out of reach — this is
 * the only credential either flow can present.
 *
 * Same signed message, same replay window, same path binding as
 * `signedHeaders`: the node verifies both through one function
 * (`crates/node/src/serve/auth.rs::verify_signed_headers`), and a proof over
 * a key no member holds resolves to anonymous rather than an error, so the
 * admission poll can keep asking until a sponsor acts.
 *
 * Returned as loose parts rather than HTTP headers so `lib/api.ts` maps them
 * onto the request, which is what keeps this module out of an import cycle
 * with it.
 */
export interface KeyProof {
    /** The public key being proven, hex. */
    key: string;
    sig: string;
    ts: number;
    /** The 16 random bytes this credential is bound to, hex. */
    nonce: string;
}

export function keyProof(
    pubkey: ArrayLike<number>,
    seed: Uint8Array,
    method: string,
    path: string,
    ts: number = nowUnixSecs(),
): KeyProof {
    const nonce = viewerNonce();
    return {
        key: toHex(pubkey),
        sig: toHex(signDigest(viewerAuthMessage(method, path, ts, nonce), seed)),
        ts,
        nonce,
    };
}

export interface Session {
    token: string;
    memberId: number;
    expiresSecs: number;
}

const sessionStore = writable<Session | null>(null);
/** Read-only view of the current session, for Settings/diagnostics UI. */
export const session = { subscribe: sessionStore.subscribe };

/** The bearer token for `memberId`, if one is minted and still safely
 *  within its TTL; null otherwise (none minted, expired, or minted for a
 *  different acting identity — a token is bound to the key that signed the
 *  mint request, so switching actors must not reuse the old one). */
export function currentToken(memberId: number | null): string | null {
    if (memberId === null) return null;
    const s = get(sessionStore);
    if (!s || s.memberId !== memberId) return null;
    if (nowUnixSecs() + EXPIRY_SAFETY_SECS >= s.expiresSecs) return null;
    return s.token;
}

/** Drop the held session (actor switch, sign-out, or a rejected token). */
export function clearSession(): void {
    sessionStore.set(null);
}

/**
 * Mint a fresh session token for `memberId`, unconditionally (no "already
 * valid?" check — callers that want that should use `ensureSession`
 * instead). Signs with the seed this device holds for `memberId`; resolves
 * to null (no network call) when that seed isn't held yet — e.g. the actor
 * was just chosen but the vault hasn't finished adopting the pending seed —
 * A non-2xx response (bad/unknown member, stale clock) also resolves to null
 * rather than throwing: an unauthenticated read is a degraded state the UI
 * already handles gracefully, not a fatal error.
 *
 * A no-op in Tauri would be right only if the app read its own node over IPC
 * and signed a key proof per read instead. Every host mints now.
 */
export async function mintSession(base: string, memberId: number): Promise<string | null> {
    const seed = heldSeed(memberId);
    if (!seed) return null;
    const path = '/session';
    const ts = nowUnixSecs();
    const headers = signedHeaders(memberId, seed, 'POST', path, ts);
    let res: Response;
    try {
        res = await fetch(`${base}${path}`, { method: 'POST', headers });
    } catch {
        return null;
    }
    if (!res.ok) return null;
    const body = (await res.json()) as { token?: string; expires_secs?: number };
    if (!body.token || typeof body.expires_secs !== 'number') return null;
    sessionStore.set({ token: body.token, memberId, expiresSecs: body.expires_secs });
    return body.token;
}

/** Mint only if there is no still-valid token for `memberId` (the common
 *  case: called at unlock and on every actor/node-URL change, cheap when a
 *  token is already live). */
export async function ensureSession(base: string, memberId: number | null): Promise<string | null> {
    if (memberId === null) return null;
    const existing = currentToken(memberId);
    if (existing) return existing;
    return mintSession(base, memberId);
}

/** Force a re-mint, discarding whatever token is held first — the reactive
 *  half of "re-mint on expiry/401": the node just told us the held token
 *  (if any) no longer works, so the client-side TTL check in
 *  `ensureSession` must not short-circuit back to it. */
export async function remintSession(base: string, memberId: number | null): Promise<string | null> {
    clearSession();
    return ensureSession(base, memberId);
}
