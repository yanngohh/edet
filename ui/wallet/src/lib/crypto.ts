/**
 * Client-side identity cryptography.
 *
 * The identity keypair is Ed25519, derived from a 12-word BIP39 recovery
 * phrase: seed = first 32 bytes of the BIP39 seed. The phrase is shown once
 * at creation and NEVER stored; the derived seed lives in the device keyring
 * (see lib/actors.ts). Whoever holds the phrase can re-derive the key and
 * restore the identity on any device.
 *
 * Signatures are made over a digest THIS DEVICE computes (`lib/txdigest.ts`,
 * sha256 of the canonical encoding under the chain id); the node verifies them
 * at submit with the same curve (ed25519-dalek).
 *
 * A digest the NODE computes would make sense only if the app embedded that
 * node. It does not, and a wallet that signs what a node hands it can be handed
 * anything.
 */

import * as ed from '@noble/ed25519';
import { sha512 } from '@noble/hashes/sha2';
import { generateMnemonic, mnemonicToSeedSync, validateMnemonic } from '@scure/bip39';
import { wordlist } from '@scure/bip39/wordlists/english';

// noble-ed25519 v2 requires a sha512 provider for its sync API.
ed.etc.sha512Sync = (...m: Uint8Array[]) => sha512(ed.etc.concatBytes(...m));

export function bytesToHex(bytes: Uint8Array | number[]): string {
    return Array.from(bytes)
        .map((b) => b.toString(16).padStart(2, '0'))
        .join('');
}

export function hexToBytes(hex: string): Uint8Array {
    const clean = hex.startsWith('0x') ? hex.slice(2) : hex;
    if (clean.length % 2 !== 0 || /[^0-9a-fA-F]/.test(clean)) {
        throw new Error('invalid hex');
    }
    const out = new Uint8Array(clean.length / 2);
    for (let i = 0; i < out.length; i++) {
        out[i] = parseInt(clean.slice(2 * i, 2 * i + 2), 16);
    }
    return out;
}

/** Best-effort zeroisation of secret material. */
export function zero(bytes: Uint8Array): void {
    bytes.fill(0);
}

// ------------------------------------------------------------ mnemonics ----

/** A fresh 12-word recovery phrase (128 bits of entropy). */
export function generatePhrase(): string {
    return generateMnemonic(wordlist, 128);
}

export function isValidPhrase(phrase: string): boolean {
    return validateMnemonic(normalizePhrase(phrase), wordlist);
}

/** Collapse whitespace/case so hand-typed phrases validate predictably. */
export function normalizePhrase(phrase: string): string {
    return phrase.trim().toLowerCase().split(/\s+/).join(' ');
}

/** The Ed25519 seed for a phrase: first 32 bytes of the BIP39 seed. */
export function seedFromPhrase(phrase: string): Uint8Array {
    return mnemonicToSeedSync(normalizePhrase(phrase)).slice(0, 32);
}

// -------------------------------------------------------------- keypairs ----

/** A fresh random Ed25519 seed (browser CSPRNG). */
export function randomSeed(): Uint8Array {
    const buf = new Uint8Array(32);
    crypto.getRandomValues(buf);
    return buf;
}

/**
 * A fresh transaction nonce (C2): 16 bytes from the platform CSPRNG, never
 * `Math.random` — this is what makes an otherwise byte-identical
 * resubmission of the same `tx` a distinct signed envelope, so it MUST be
 * unpredictable, not merely different. Call this exactly ONCE per new
 * signing intent: whoever opens a transaction (direct or a pending
 * co-signature request) picks the nonce, and every subsequent co-signer
 * must reuse that exact value (read back off the pending entry, never
 * regenerated) — see `lib/submit.ts`'s doc comment on why a second signer
 * generating its own nonce silently breaks the request forever (every
 * signature ends up covering a different digest, so the threshold can
 * never be reached).
 */
export function randomNonce(): number[] {
    const buf = new Uint8Array(16);
    crypto.getRandomValues(buf);
    return Array.from(buf);
}

/** Public key (32 bytes) for a seed, as the number[] the API layer speaks. */
export function derivePublicKey(seed: Uint8Array): number[] {
    return Array.from(ed.getPublicKey(seed));
}

/** Sign a 32-byte digest; returns the 64-byte signature as number[]. */
export function signDigest(digest: Uint8Array, seed: Uint8Array): number[] {
    return Array.from(ed.sign(digest, seed));
}

/** Verify (used by tests to prove the sign path). */
export function verifyDigest(signature: number[] | Uint8Array, digest: Uint8Array, publicKey: number[] | Uint8Array): boolean {
    return ed.verify(Uint8Array.from(signature), digest, Uint8Array.from(publicKey));
}
