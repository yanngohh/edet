/**
 * edet identity backup — format v2 (the successor of the v1
 * `.edet-backup` bundle).
 *
 * A backup is a single JSON envelope containing an XChaCha20-Poly1305
 * ciphertext of the device's identity material, keyed by a passphrase
 * through scrypt. There is no source chain to carry any more — ledger state
 * lives on the community's nodes — so the payload is exactly what cannot be
 * recomputed: the seed ring (identities held on this device), local
 * nicknames, and the acting identity.
 *
 * Envelope (plaintext JSON, sanity-checkable before any decrypt attempt):
 *   {
 *     magic: "EDET-BACKUP", schema_version: 2, created_at_ms,
 *     kdf: { algo: "scrypt", N, r, p, salt: base64 },
 *     cipher: "xchacha20poly1305", nonce: base64, ciphertext: base64
 *   }
 *
 * Decrypted payload (JSON):
 *   { schema_version: 2, seeds: {id: number[32]}, nicknames: {id: string},
 *     actor: number | null }
 *
 * A wrong passphrase fails AEAD authentication (BackupDecryptError) — no
 * separate fingerprint is needed. The recovery phrase itself is never part
 * of a backup; a phrase-derived identity is restorable from the phrase
 * alone, and the backup exists for the seeds that have no phrase (members
 * admitted or rotated from this device) plus local labels.
 */

import { xchacha20poly1305 } from '@noble/ciphers/chacha';
import { scrypt } from '@noble/hashes/scrypt';
import { randomBytes } from '@noble/hashes/utils';

export const BACKUP_MAGIC = 'EDET-BACKUP';
export const BACKUP_SCHEMA_VERSION = 2;
const BACKUP_AAD = new TextEncoder().encode('edet-backup-v2');
const NONCE_LEN = 24;
const SALT_LEN = 16;

/** Interactive-login-grade scrypt cost (~100 ms–1 s, 32 MiB). Tests pass
 *  lighter params explicitly. */
export const DEFAULT_KDF = { N: 2 ** 15, r: 8, p: 1 } as const;

export interface KdfParams {
    N: number;
    r: number;
    p: number;
}

export interface BackupPayload {
    schema_version: number;
    seeds: Record<number, number[]>;
    nicknames: Record<number, string>;
    actor: number | null;
}

export interface BackupEnvelope {
    magic: string;
    schema_version: number;
    created_at_ms: number;
    kdf: KdfParams & { algo: string; salt: string };
    cipher: string;
    nonce: string;
    ciphertext: string;
}

export class BackupFormatError extends Error {
    constructor(message: string) {
        super(message);
        this.name = 'BackupFormatError';
    }
}

export class BackupDecryptError extends Error {
    constructor(message: string) {
        super(message);
        this.name = 'BackupDecryptError';
    }
}

function toBase64(bytes: Uint8Array): string {
    let bin = '';
    for (const b of bytes) bin += String.fromCharCode(b);
    return btoa(bin);
}

function fromBase64(s: string): Uint8Array {
    const bin = atob(s);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
}

function deriveKey(passphrase: string, salt: Uint8Array, kdf: KdfParams): Uint8Array {
    return scrypt(new TextEncoder().encode(passphrase.normalize('NFKD')), salt, { ...kdf, dkLen: 32 });
}

/** Build + encrypt a backup; returns the envelope as pretty JSON text. */
export function encodeBackup(payload: BackupPayload, passphrase: string, kdf: KdfParams = DEFAULT_KDF): string {
    if (!passphrase) throw new BackupFormatError('empty passphrase');
    const salt = randomBytes(SALT_LEN);
    const nonce = randomBytes(NONCE_LEN);
    const key = deriveKey(passphrase, salt, kdf);
    const plaintext = new TextEncoder().encode(JSON.stringify(payload));
    const ciphertext = xchacha20poly1305(key, nonce, BACKUP_AAD).encrypt(plaintext);
    key.fill(0);
    const envelope: BackupEnvelope = {
        magic: BACKUP_MAGIC,
        schema_version: BACKUP_SCHEMA_VERSION,
        created_at_ms: Date.now(),
        kdf: { algo: 'scrypt', ...kdf, salt: toBase64(salt) },
        cipher: 'xchacha20poly1305',
        nonce: toBase64(nonce),
        ciphertext: toBase64(ciphertext),
    };
    return JSON.stringify(envelope, null, 2);
}

/** Parse and sanity-check the plaintext envelope (no key material needed). */
export function parseEnvelope(text: string): BackupEnvelope {
    let raw: any;
    try {
        raw = JSON.parse(text);
    } catch {
        throw new BackupFormatError('not a JSON backup file');
    }
    if (raw?.magic !== BACKUP_MAGIC) throw new BackupFormatError('not an edet backup');
    if (raw.schema_version !== BACKUP_SCHEMA_VERSION) {
        throw new BackupFormatError(`unsupported backup version ${raw.schema_version}`);
    }
    if (raw.kdf?.algo !== 'scrypt' || raw.cipher !== 'xchacha20poly1305') {
        throw new BackupFormatError('unsupported backup algorithms');
    }
    // Refuse absurd KDF parameters (memory-bomb envelopes).
    if (!(raw.kdf.N >= 2 && raw.kdf.N <= 2 ** 22) || !(raw.kdf.r >= 1 && raw.kdf.r <= 32) || !(raw.kdf.p >= 1 && raw.kdf.p <= 4)) {
        throw new BackupFormatError('unreasonable KDF parameters');
    }
    return raw as BackupEnvelope;
}

/** Decrypt a backup file with the passphrase. */
export function decodeBackup(text: string, passphrase: string): BackupPayload {
    const env = parseEnvelope(text);
    const key = deriveKey(passphrase, fromBase64(env.kdf.salt), env.kdf);
    let plaintext: Uint8Array;
    try {
        plaintext = xchacha20poly1305(key, fromBase64(env.nonce), BACKUP_AAD).decrypt(fromBase64(env.ciphertext));
    } catch {
        throw new BackupDecryptError('wrong passphrase or corrupted backup');
    } finally {
        key.fill(0);
    }
    const payload = JSON.parse(new TextDecoder().decode(plaintext));
    if (payload?.schema_version !== BACKUP_SCHEMA_VERSION || typeof payload.seeds !== 'object') {
        throw new BackupFormatError('malformed backup payload');
    }
    return payload as BackupPayload;
}

/** Suggested file name for an exported backup. */
export function suggestedBackupFileName(now = new Date()): string {
    const d = now.toISOString().slice(0, 10);
    return `edet-backup-${d}.edet`;
}
