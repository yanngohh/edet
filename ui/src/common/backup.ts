/**
 * edet identity backup bundle — format v1.
 *
 * A bundle is a single `.edet-backup` file that contains a msgpack-encoded
 * snapshot of the user's source chain, encrypted with XChaCha20-Poly1305 using
 * a key derived from the user's 12-word mnemonic.
 *
 * Layout
 * ------
 *
 * Outer envelope (plaintext, msgpack-encoded):
 *   {
 *     magic: "EDET1",
 *     schema_version: 1,
 *     mnemonic_fp: [u8; 4],
 *     nonce: [u8; 24],        // XChaCha20 24-byte random nonce
 *     ciphertext: Uint8Array, // AEAD ciphertext of the payload below
 *   }
 *
 * Decrypted payload (msgpack-encoded):
 *   {
 *     schema_version: 1,
 *     created_at_us: i64,          // microseconds since epoch
 *     app_id: "edet",
 *     dna_hash: Uint8Array,        // 39 bytes
 *     agent_pub_key: Uint8Array,   // 39 bytes
 *     source_chain: unknown[],     // Holochain Record list from DumpFullState
 *   }
 *
 * The outer envelope is unencrypted so that the restore UI can sanity-check
 * magic + schema version + fingerprint before attempting a full AEAD decrypt.
 */

import { encode, decode } from '@msgpack/msgpack';
import { xchacha20poly1305 } from '@noble/ciphers/chacha';
import { randomBytes } from '@noble/hashes/utils';

import { fingerprintMatches, MNEMONIC_FINGERPRINT_LEN } from './mnemonic';

/** Magic prefix distinguishing edet backups from other `.edet-backup`-extension files. */
export const BACKUP_MAGIC = 'EDET1';
/** Current envelope + payload schema version. Bumped on any incompatible format change. */
export const BACKUP_SCHEMA_VERSION = 1;
/** Additional authenticated data bound into the AEAD. Ties the ciphertext to this format version. */
const BACKUP_AAD = new TextEncoder().encode('edet-backup-v1');
/** XChaCha20-Poly1305 nonce length. */
const NONCE_LEN = 24;

export interface BackupEnvelope {
    magic: string;
    schema_version: number;
    mnemonic_fp: Uint8Array;
    nonce: Uint8Array;
    ciphertext: Uint8Array;
}

export interface BackupPayload {
    schema_version: number;
    created_at_us: number;
    app_id: string;
    dna_hash: Uint8Array;
    agent_pub_key: Uint8Array;
    source_chain: unknown[];
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

export class MnemonicMismatchError extends Error {
    constructor() {
        super("Recovery phrase doesn't match this backup file");
        this.name = 'MnemonicMismatchError';
    }
}

/** Build + encrypt a backup from its plaintext payload fields. */
export function encodeBackup(
    payload: BackupPayload,
    backupKey: Uint8Array,
    mnemonicFingerprint: Uint8Array,
): Uint8Array {
    if (backupKey.length !== 32) {
        throw new BackupFormatError(`backupKey must be 32 bytes, got ${backupKey.length}`);
    }
    if (mnemonicFingerprint.length !== MNEMONIC_FINGERPRINT_LEN) {
        throw new BackupFormatError(
            `mnemonic fingerprint must be ${MNEMONIC_FINGERPRINT_LEN} bytes`,
        );
    }

    const plaintext = encode(payload);
    const nonce = randomBytes(NONCE_LEN);
    const cipher = xchacha20poly1305(backupKey, nonce, BACKUP_AAD);
    const ciphertext = cipher.encrypt(plaintext);

    const envelope: BackupEnvelope = {
        magic: BACKUP_MAGIC,
        schema_version: BACKUP_SCHEMA_VERSION,
        mnemonic_fp: mnemonicFingerprint,
        nonce,
        ciphertext,
    };
    return encode(envelope);
}

/** Parse the outer envelope of a backup file. Throws `BackupFormatError` on malformed data. */
export function parseEnvelope(bytes: Uint8Array): BackupEnvelope {
    let raw: unknown;
    try {
        raw = decode(bytes);
    } catch (e) {
        throw new BackupFormatError(`Envelope is not valid msgpack: ${(e as Error).message}`);
    }
    if (!raw || typeof raw !== 'object') {
        throw new BackupFormatError('Envelope is not an object');
    }
    const env = raw as Partial<BackupEnvelope>;
    if (env.magic !== BACKUP_MAGIC) {
        throw new BackupFormatError(`Unknown magic: ${String(env.magic)}`);
    }
    if (env.schema_version !== BACKUP_SCHEMA_VERSION) {
        throw new BackupFormatError(
            `Unsupported schema version ${env.schema_version} (expected ${BACKUP_SCHEMA_VERSION})`,
        );
    }
    if (!(env.mnemonic_fp instanceof Uint8Array) || env.mnemonic_fp.length !== MNEMONIC_FINGERPRINT_LEN) {
        throw new BackupFormatError('Missing or malformed mnemonic fingerprint');
    }
    if (!(env.nonce instanceof Uint8Array) || env.nonce.length !== NONCE_LEN) {
        throw new BackupFormatError('Missing or malformed nonce');
    }
    if (!(env.ciphertext instanceof Uint8Array)) {
        throw new BackupFormatError('Missing or malformed ciphertext');
    }
    return env as BackupEnvelope;
}

/**
 * Decode + decrypt a backup. Performs the fingerprint check against the
 * supplied mnemonic fingerprint first and throws `MnemonicMismatchError` on
 * mismatch before attempting the AEAD decrypt — this lets the UI present a
 * useful error message without the delay of a full decrypt attempt.
 */
export function decodeBackup(
    bytes: Uint8Array,
    backupKey: Uint8Array,
    mnemonicFingerprint: Uint8Array,
): BackupPayload {
    const env = parseEnvelope(bytes);
    if (!fingerprintMatches(env.mnemonic_fp, mnemonicFingerprint)) {
        throw new MnemonicMismatchError();
    }
    const cipher = xchacha20poly1305(backupKey, env.nonce, BACKUP_AAD);
    let plaintext: Uint8Array;
    try {
        plaintext = cipher.decrypt(env.ciphertext);
    } catch (e) {
        throw new BackupDecryptError(
            `AEAD decrypt failed — file may be corrupted or tampered with (${(e as Error).message})`,
        );
    }

    let payload: unknown;
    try {
        payload = decode(plaintext);
    } catch (e) {
        throw new BackupFormatError(`Payload is not valid msgpack: ${(e as Error).message}`);
    }
    if (!payload || typeof payload !== 'object') {
        throw new BackupFormatError('Payload is not an object');
    }
    const p = payload as Partial<BackupPayload>;
    if (p.schema_version !== BACKUP_SCHEMA_VERSION) {
        throw new BackupFormatError(
            `Unsupported payload schema ${p.schema_version} (expected ${BACKUP_SCHEMA_VERSION})`,
        );
    }
    if (p.app_id !== 'edet') {
        throw new BackupFormatError(`app_id mismatch: ${String(p.app_id)}`);
    }
    if (!(p.dna_hash instanceof Uint8Array)) {
        throw new BackupFormatError('Missing dna_hash');
    }
    if (!(p.agent_pub_key instanceof Uint8Array)) {
        throw new BackupFormatError('Missing agent_pub_key');
    }
    if (typeof p.created_at_us !== 'number') {
        throw new BackupFormatError('Missing created_at_us');
    }
    if (!Array.isArray(p.source_chain)) {
        throw new BackupFormatError('source_chain is not an array');
    }
    return p as BackupPayload;
}

/** Generate a filename like `edet-backup-2026-04-28.edet-backup`. */
export function suggestedBackupFileName(when: Date = new Date()): string {
    const y = when.getUTCFullYear();
    const m = String(when.getUTCMonth() + 1).padStart(2, '0');
    const d = String(when.getUTCDate()).padStart(2, '0');
    return `edet-backup-${y}-${m}-${d}.edet-backup`;
}
