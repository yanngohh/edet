/**
 * Mnemonic + key derivation for edet identity recovery.
 *
 * The user is shown a single 12-word BIP-39 mnemonic at first run. From that
 * mnemonic we deterministically derive two 32-byte keys:
 *
 *   agent_seed     = HKDF(seed, info: "edet agent v1")
 *   backup_enc_key = HKDF(seed, info: "edet backup v1")
 *
 * The mnemonic itself is never persisted. The agent seed is fed to
 * lair-keystore by the Tauri shell to regenerate the same AgentPubKey on a
 * new device; the backup encryption key is used to XChaCha20-Poly1305-encrypt
 * the identity backup bundle. Neither key material is included in the bundle
 * envelope — if the user loses the mnemonic, the bundle is permanently opaque.
 *
 * A 4-byte SHA-256 fingerprint of the mnemonic is kept in the plaintext
 * envelope so the restore UI can reject a wrong phrase before attempting a
 * full AEAD decrypt.
 */

import {
    generateMnemonic,
    validateMnemonic,
    mnemonicToSeed,
} from '@scure/bip39';
import { wordlist as english } from '@scure/bip39/wordlists/english';
import { hkdf } from '@noble/hashes/hkdf';
import { sha256 } from '@noble/hashes/sha256';
import { randomBytes } from '@noble/hashes/utils';

/** Length in bytes of every derived key (agent seed, backup encryption key). */
export const DERIVED_KEY_LEN = 32;

/** Length in bytes of the mnemonic fingerprint retained in the backup envelope. */
export const MNEMONIC_FINGERPRINT_LEN = 4;

/** HKDF `info` for the Holochain agent ed25519 seed. */
const AGENT_SEED_INFO = new TextEncoder().encode('edet agent v1');
/** HKDF `info` for the symmetric key that encrypts the backup bundle. */
const BACKUP_KEY_INFO = new TextEncoder().encode('edet backup v1');

/** The fixed BIP-39 wordlist we ship (English). Kept explicit so restore flows
 *  don't silently shift if the default changes upstream. */
export const WORDLIST: readonly string[] = english;

/** A BIP-39 mnemonic presented as a normal, lowercased, single-spaced string. */
export type Mnemonic = string;

export interface DerivedKeys {
    /** ed25519 seed to feed lair-keystore so the AgentPubKey is deterministic. */
    agentSeed: Uint8Array;
    /** 32-byte XChaCha20-Poly1305 key used to encrypt the backup bundle. */
    backupKey: Uint8Array;
    /** 4-byte SHA-256 prefix of the (normalised) mnemonic, for quick mismatch detection. */
    fingerprint: Uint8Array;
}

/** Normalise a mnemonic: NFKD + lowercase + collapse whitespace. Matches BIP-39. */
export function normaliseMnemonic(phrase: string): Mnemonic {
    return phrase.normalize('NFKD').trim().toLowerCase().split(/\s+/).join(' ');
}

/** Generate a fresh 12-word mnemonic (128 bits of entropy) using the CSPRNG. */
export function generateNewMnemonic(): Mnemonic {
    // 128 bits → 12 words. Using the package's generator ensures strict
    // wordlist adherence and checksum correctness.
    return generateMnemonic(WORDLIST as string[], 128);
}

/** Return `true` iff `phrase` is a syntactically valid BIP-39 mnemonic. */
export function isValidMnemonic(phrase: string): boolean {
    try {
        return validateMnemonic(normaliseMnemonic(phrase), WORDLIST as string[]);
    } catch {
        return false;
    }
}

/**
 * Derive the agent seed, backup encryption key, and fingerprint from a
 * mnemonic. The mnemonic is normalised before hashing.
 *
 * @throws if the mnemonic is not valid BIP-39.
 */
export async function deriveKeys(phrase: Mnemonic): Promise<DerivedKeys> {
    const normal = normaliseMnemonic(phrase);
    if (!validateMnemonic(normal, WORDLIST as string[])) {
        throw new Error('Invalid BIP-39 mnemonic');
    }

    // BIP-39 seed: PBKDF2(mnemonic, "mnemonic"+passphrase, 2048, SHA-512) -> 64 B
    // We always use an empty passphrase; adding one is undocumented user risk.
    const seed = await mnemonicToSeed(normal, '');

    // HKDF-SHA256 with distinct `info` tags keeps the two keys cryptographically
    // independent even though they share input material.
    const agentSeed = hkdf(sha256, seed, undefined, AGENT_SEED_INFO, DERIVED_KEY_LEN);
    const backupKey = hkdf(sha256, seed, undefined, BACKUP_KEY_INFO, DERIVED_KEY_LEN);

    const fingerprint = sha256(new TextEncoder().encode(normal)).slice(
        0,
        MNEMONIC_FINGERPRINT_LEN,
    );

    return { agentSeed, backupKey, fingerprint };
}

/**
 * Compute only the fingerprint of a mnemonic (cheaper than full derivation).
 * Used by the restore UI to reject a wrong phrase before attempting to
 * decrypt the full backup payload.
 */
export function mnemonicFingerprint(phrase: Mnemonic): Uint8Array {
    const normal = normaliseMnemonic(phrase);
    return sha256(new TextEncoder().encode(normal)).slice(0, MNEMONIC_FINGERPRINT_LEN);
}

/** Constant-time equality for two fingerprints. */
export function fingerprintMatches(a: Uint8Array, b: Uint8Array): boolean {
    if (a.length !== b.length) return false;
    let diff = 0;
    for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
    return diff === 0;
}

/**
 * Best-effort zero of a Uint8Array. JavaScript cannot guarantee the underlying
 * bytes are not copied by the GC, but zeroing the view reduces the window in
 * which leaked references hold plaintext keys. Callers should still try to
 * release references immediately after use.
 */
export function zero(bytes: Uint8Array): void {
    bytes.fill(0);
}

/** Produce `n` random indices in [0, upper), without replacement, using the CSPRNG.
 *  Used by the mnemonic confirmation screen to pick which words to re-check. */
export function pickDistinctIndices(n: number, upper: number): number[] {
    if (n > upper) throw new Error('pickDistinctIndices: n > upper');
    const picked = new Set<number>();
    const buf = randomBytes(Math.max(n * 2, 16));
    let i = 0;
    while (picked.size < n) {
        if (i + 1 >= buf.length) {
            const more = randomBytes(16);
            for (let j = 0; j < more.length; j++) buf[(i + j) % buf.length] = more[j];
            i = 0;
        }
        // Two bytes → 0..65535; modulo bias at upper≤32 is negligible.
        const pick = ((buf[i] << 8) | buf[i + 1]) % upper;
        picked.add(pick);
        i += 2;
    }
    return [...picked].sort((a, b) => a - b);
}
