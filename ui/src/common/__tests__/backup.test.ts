import { describe, expect, it } from 'vitest';

import {
    BACKUP_MAGIC,
    BackupDecryptError,
    BackupFormatError,
    decodeBackup,
    encodeBackup,
    parseEnvelope,
    suggestedBackupFileName,
    type BackupPayload,
} from '../backup';

// Light KDF for tests — production uses DEFAULT_KDF (N=2^15).
const KDF = { N: 2 ** 8, r: 8, p: 1 };

const payload: BackupPayload = {
    schema_version: 2,
    seeds: { 5: Array(32).fill(42), 7: Array(32).fill(9) },
    nicknames: { 5: 'Ada' },
    actor: 5,
};

describe('backup envelope', () => {
    it('round-trips through encrypt/decrypt', () => {
        const text = encodeBackup(payload, 'correct horse battery', KDF);
        const back = decodeBackup(text, 'correct horse battery');
        expect(back).toEqual(payload);
    });

    it('rejects a wrong passphrase via AEAD, not silently', () => {
        const text = encodeBackup(payload, 'right-pass', KDF);
        expect(() => decodeBackup(text, 'wrong-pass')).toThrow(BackupDecryptError);
    });

    it('never contains seed material in the plaintext envelope', () => {
        const text = encodeBackup(payload, 'right-pass', KDF);
        // The seeds as JSON would appear as long runs of "42," — assert the
        // serialized plaintext payload is not embedded anywhere.
        expect(text).not.toContain(JSON.stringify(payload.seeds));
        expect(text).not.toContain('"Ada"');
        const env = parseEnvelope(text);
        expect(env.magic).toBe(BACKUP_MAGIC);
        expect(env.kdf.algo).toBe('scrypt');
    });

    it('rejects garbage, foreign files, and memory-bomb KDF params', () => {
        expect(() => parseEnvelope('not json')).toThrow(BackupFormatError);
        expect(() => parseEnvelope(JSON.stringify({ magic: 'OTHER' }))).toThrow(BackupFormatError);
        const text = encodeBackup(payload, 'p'.repeat(8), KDF);
        const env = JSON.parse(text);
        env.kdf.N = 2 ** 30;
        expect(() => parseEnvelope(JSON.stringify(env))).toThrow(BackupFormatError);
    });

    it('refuses an empty passphrase', () => {
        expect(() => encodeBackup(payload, '', KDF)).toThrow(BackupFormatError);
    });

    it('suggests a dated .edet filename', () => {
        expect(suggestedBackupFileName(new Date('2026-07-24T12:00:00Z'))).toBe('edet-backup-2026-07-24.edet');
    });
});
