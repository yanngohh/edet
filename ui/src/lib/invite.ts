/**
 * The admission QR: how a newcomer's public key travels to their sponsor
 * without anyone retyping 64 hex characters.
 *
 * The onboarding share screen renders this payload as a QR the sponsor
 * scans from the admit page; the copyable text next to it stays the BARE
 * key, because "invite link" reads backwards — it is the JOINER who sends
 * it — and a URI pasted into a chat explains nothing. The QR payload is a
 * custom-scheme URI rather than the bare key so it can carry the chain id
 * alongside: an admission signs the newcomer onto ONE ledger, and the
 * sponsor's app should refuse a code minted on a different community's
 * ledger rather than admit a key whose holder expects to be somewhere else.
 *
 * Deliberately NOT registered as an OS deep link: nothing here needs the
 * URI to open the app — both ends of the exchange are already inside it —
 * and scheme registration would be attack surface (any page could poke
 * admission UI open) for a convenience nobody exercises.
 */

/** `edet://join?c=<chain id>&k=<64-hex public key>` */
export function buildAdmitQrPayload(chainId: string, keyHex: string): string {
    return `edet://join?c=${encodeURIComponent(chainId)}&k=${keyHex.toLowerCase()}`;
}

export interface AdmitInput {
    keyHex: string;
    /** Chain id the link was minted for; null when a bare key was given. */
    chainId: string | null;
}

/**
 * Parse whatever the sponsor pasted or scanned into the admit page: the QR
 * payload or a bare 64-hex public key. Null when it is neither — the
 * caller owns the error copy.
 */
export function parseAdmitInput(raw: string): AdmitInput | null {
    const trimmed = raw.trim();
    const asKey = trimmed.toLowerCase();
    if (/^[0-9a-f]{64}$/.test(asKey)) return { keyHex: asKey, chainId: null };
    // URL's parser mangles opaque custom schemes inconsistently across
    // engines, so the link is taken apart by hand: scheme + path, then
    // ordinary query parsing.
    const m = trimmed.match(/^edet:\/\/join\?(.*)$/i);
    if (!m) return null;
    const params = new URLSearchParams(m[1]);
    const key = (params.get('k') ?? '').toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(key)) return null;
    const chain = params.get('c');
    return { keyHex: key, chainId: chain === null || chain === '' ? null : chain };
}

// ---------------------------------------------- the seller's invitation ----

import { derivePublicKey, hexToBytes, randomNonce, signDigest } from './crypto';
import type { InviteWire } from './api';

/**
 * A member's signed invitation to be bought from, minted beside the "pay
 * me" QR (`MyWallet`) and carried back by a buyer who has no account yet.
 *
 * A key cannot be charged for a pending-pool entry, so an entry it opens is
 * charged to the member it names — and only with that member's signature
 * over this, or anyone who knew a member id could keep their inbox full of
 * junk from fresh keys. The node bounds what one buys: it expires, what it
 * opens spends the inviter's own rate bucket, and an inviter holds a few
 * such entries at once (`serve/pending.rs::Invite`).
 */
export interface Invite {
    keyHex: string;
    notAfterSecs: number;
    nonceHex: string;
    signatureHex: string;
}

/**
 * The bytes an invitation signs — mirrored byte-for-byte by
 * `serve/pending.rs::invite_message`, and pinned on both sides against one
 * fixed vector.
 */
export function inviteMessage(chainId: string, keyHex: string, notAfterSecs: number, nonceHex: string): Uint8Array {
    return new TextEncoder().encode(`edet-invite-v1\n${chainId}\n${keyHex.toLowerCase()}\n${notAfterSecs}\n${nonceHex.toLowerCase()}`);
}

/** Sign a fresh invitation with this device's key, void after `notAfterSecs`. */
export function mintInvite(seed: Uint8Array, chainId: string, notAfterSecs: number): Invite {
    const keyHex = hex(derivePublicKey(seed));
    const nonceHex = hex(randomNonce());
    return {
        keyHex,
        notAfterSecs,
        nonceHex,
        signatureHex: hex(signDigest(inviteMessage(chainId, keyHex, notAfterSecs, nonceHex), seed)),
    };
}

/** The invitation as `/pending/sign` carries it. */
export function inviteWire(inv: Invite): InviteWire {
    return {
        key: Array.from(hexToBytes(inv.keyHex)),
        not_after_secs: inv.notAfterSecs,
        nonce: Array.from(hexToBytes(inv.nonceHex)),
        signature: Array.from(hexToBytes(inv.signatureHex)),
    };
}

/**
 * The "pay me" QR: `edet://pay?c=<chain>&a=<address>&k=<key>&e=<expiry>&n=<nonce>&s=<sig>`.
 *
 * Without an invitation (the vault is locked, or no chain is declared) it is
 * the bare address, which every scanner already reads: a buyer with an
 * account never needs the invitation, and one without falls back to a code
 * shown by hand.
 */
export function buildPayQrPayload(chainId: string, address: string, invite: Invite | null): string {
    if (!invite) return address;
    const q = new URLSearchParams();
    q.set('c', chainId);
    q.set('a', address);
    q.set('k', invite.keyHex);
    q.set('e', String(invite.notAfterSecs));
    q.set('n', invite.nonceHex);
    q.set('s', invite.signatureHex);
    return `edet://pay?${q.toString()}`;
}

export interface PayInput {
    address: string;
    chainId: string | null;
    invite: Invite | null;
}

/**
 * A scanned or pasted "pay me" payload: the address it names and the
 * invitation it carries, or `null` when it is not one. A bare address is
 * not a pay payload; `classifyAddressInput` reads those.
 */
export function parsePayInput(raw: string): PayInput | null {
    const m = raw.trim().match(/^edet:\/\/pay\?(.*)$/i);
    if (!m) return null;
    const q = new URLSearchParams(m[1]);
    const address = (q.get('a') ?? '').toLowerCase();
    if (!/^0x[0-9a-f]{40}$/.test(address)) return null;
    const chainId = q.get('c');
    const k = (q.get('k') ?? '').toLowerCase();
    const e = q.get('e') ?? '';
    const n = (q.get('n') ?? '').toLowerCase();
    const s = (q.get('s') ?? '').toLowerCase();
    const whole = /^[0-9a-f]{64}$/.test(k) && /^\d{1,12}$/.test(e) && /^[0-9a-f]{32}$/.test(n) && /^[0-9a-f]{128}$/.test(s);
    return {
        address,
        chainId: chainId === null || chainId === '' ? null : chainId,
        invite: whole ? { keyHex: k, notAfterSecs: Number(e), nonceHex: n, signatureHex: s } : null,
    };
}

function hex(bytes: ArrayLike<number>): string {
    return Array.from(bytes)
        .map((b) => b.toString(16).padStart(2, '0'))
        .join('');
}
