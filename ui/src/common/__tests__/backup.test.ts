import { describe, expect, it } from 'vitest';

import {
    BACKUP_MAGIC,
    BACKUP_SCHEMA_VERSION,
    BackupDecryptError,
    BackupFormatError,
    MnemonicMismatchError,
    decodeBackup,
    encodeBackup,
    parseEnvelope,
    suggestedBackupFileName,
} from '../backup';
import type { BackupPayload } from '../backup';
import { deriveKeys, generateNewMnemonic, mnemonicFingerprint } from '../mnemonic';

function samplePayload(): BackupPayload {
    return {
        schema_version: BACKUP_SCHEMA_VERSION,
        created_at_us: 1_700_000_000_000_000,
        app_id: 'edet',
        dna_hash: new Uint8Array([0x84, 0x24, ...new Array(37).fill(0)]),
        agent_pub_key: new Uint8Array([0x84, 0x20, 0x24, ...new Array(36).fill(0)]),
        source_chain: [{ placeholder: true, n: 42 }],
    };
}

describe('backup', () => {
    it('round-trips an encrypted bundle with the right mnemonic', async () => {
        const mnemonic = generateNewMnemonic();
        const { backupKey, fingerprint } = await deriveKeys(mnemonic);
        const payload = samplePayload();

        const bytes = encodeBackup(payload, backupKey, fingerprint);
        const decoded = decodeBackup(bytes, backupKey, fingerprint);

        expect(decoded.schema_version).toBe(BACKUP_SCHEMA_VERSION);
        expect(decoded.app_id).toBe('edet');
        expect(decoded.created_at_us).toBe(payload.created_at_us);
        expect(Array.from(decoded.dna_hash)).toEqual(Array.from(payload.dna_hash));
        expect(Array.from(decoded.agent_pub_key)).toEqual(Array.from(payload.agent_pub_key));
        expect(decoded.source_chain).toEqual(payload.source_chain);
    });

    it('envelope carries the correct magic + version + fingerprint', async () => {
        const mnemonic = generateNewMnemonic();
        const { backupKey, fingerprint } = await deriveKeys(mnemonic);
        const bytes = encodeBackup(samplePayload(), backupKey, fingerprint);
        const env = parseEnvelope(bytes);
        expect(env.magic).toBe(BACKUP_MAGIC);
        expect(env.schema_version).toBe(BACKUP_SCHEMA_VERSION);
        expect(Array.from(env.mnemonic_fp)).toEqual(Array.from(fingerprint));
        expect(env.nonce).toHaveLength(24);
        expect(env.ciphertext.length).toBeGreaterThan(0);
    });

    it('rejects a wrong mnemonic before attempting to decrypt', async () => {
        const correct = generateNewMnemonic();
        let wrong = generateNewMnemonic();
        while (wrong === correct) wrong = generateNewMnemonic();
        const { backupKey, fingerprint } = await deriveKeys(correct);
        const bytes = encodeBackup(samplePayload(), backupKey, fingerprint);
        const wrongKeys = await deriveKeys(wrong);
        expect(() =>
            decodeBackup(bytes, wrongKeys.backupKey, wrongKeys.fingerprint),
        ).toThrow(MnemonicMismatchError);
    });

    it('rejects a tampered ciphertext even with a matching fingerprint', async () => {
        const mnemonic = generateNewMnemonic();
        const { backupKey, fingerprint } = await deriveKeys(mnemonic);
        const bytes = encodeBackup(samplePayload(), backupKey, fingerprint);
        // Flip the last byte of the whole envelope; this lands inside the
        // ciphertext region when msgpack encodes arrays sequentially.
        bytes[bytes.length - 1] ^= 0xff;
        expect(() => decodeBackup(bytes, backupKey, fingerprint)).toThrow(BackupDecryptError);
    });

    it('rejects malformed magic', () => {
        // Hand-craft an envelope with the wrong magic string.
        const evil = new Uint8Array([
            0x85, // map(5)
            0xa5, 0x6d, 0x61, 0x67, 0x69, 0x63, // "magic"
            0xa5, 0x57, 0x52, 0x4f, 0x4e, 0x47, // "WRONG"
            0xae, 0x73, 0x63, 0x68, 0x65, 0x6d, 0x61, 0x5f, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e,
            0x01,
            0xab, 0x6d, 0x6e, 0x65, 0x6d, 0x6f, 0x6e, 0x69, 0x63, 0x5f, 0x66, 0x70,
            0xc4, 0x04, 0x00, 0x00, 0x00, 0x00,
            0xa5, 0x6e, 0x6f, 0x6e, 0x63, 0x65,
            0xc4, 0x18, ...new Array(24).fill(0),
            0xaa, 0x63, 0x69, 0x70, 0x68, 0x65, 0x72, 0x74, 0x65, 0x78, 0x74,
            0xc4, 0x00,
        ]);
        expect(() => parseEnvelope(evil)).toThrow(BackupFormatError);
    });

    it('rejects junk bytes', () => {
        expect(() => parseEnvelope(new Uint8Array([0x00, 0x01, 0x02]))).toThrow(BackupFormatError);
    });

    it('suggestedBackupFileName uses ISO date shape', () => {
        const name = suggestedBackupFileName(new Date(Date.UTC(2026, 3, 28)));
        expect(name).toBe('edet-backup-2026-04-28.edet-backup');
    });
});
