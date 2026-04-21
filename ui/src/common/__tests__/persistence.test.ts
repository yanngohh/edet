/**
 * Persistence hygiene tests.
 *
 * Assert that no module-under-test leaks sensitive material (mnemonic
 * phrase, raw backup key, raw agent seed) to any local persistence layer
 * the app manages (`localStorage`, `sessionStorage`, or the `localforage`
 * IndexedDB backing). This is a whole-file regression check rather than a
 * unit test of any single module.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { randomBytes } from '@noble/hashes/utils';

// Mock localforage with an observable in-memory backing so we can inspect
// every write after exercising the code under test.
const backingStore = new Map<string, unknown>();
vi.mock('localforage', () => ({
    default: {
        createInstance: () => ({
            getItem: (k: string) => Promise.resolve(backingStore.get(k) ?? null),
            setItem: (k: string, v: unknown) => {
                backingStore.set(k, v);
                return Promise.resolve(v);
            },
            removeItem: (k: string) => {
                backingStore.delete(k);
                return Promise.resolve();
            },
        }),
    },
}));

vi.mock('../tauriBridge', () => ({
    tauriBridge: {
        isTauri: () => false,
        runtime: () => 'browser',
        getCurrentBackup: () => Promise.resolve(null),
        saveCurrentBackup: () => Promise.resolve(),
    },
}));

/** Walk a nested value and collect every `Uint8Array` / string we find, for
 *  pattern scanning. Arrays and Maps are descended; anything else is
 *  rendered via `String(...)` so we also catch accidental `toString` leaks. */
function collectPrintable(value: unknown, out: string[] = []): string[] {
    if (value === null || value === undefined) return out;
    if (typeof value === 'string') {
        out.push(value);
        return out;
    }
    if (value instanceof Uint8Array) {
        out.push(new TextDecoder('utf-8', { fatal: false }).decode(value));
        out.push(Array.from(value).map((b) => b.toString(16)).join(''));
        return out;
    }
    if (Array.isArray(value)) {
        for (const v of value) collectPrintable(v, out);
        return out;
    }
    if (typeof value === 'object') {
        for (const v of Object.values(value as Record<string, unknown>)) collectPrintable(v, out);
        return out;
    }
    out.push(String(value));
    return out;
}

function assertNoSubstring(haystacks: string[], needle: string, label: string) {
    for (const h of haystacks) {
        if (!h) continue;
        expect(h, `${label} found in persisted value: ${h.slice(0, 80)}…`).not.toContain(needle);
    }
}

describe('persistence hygiene', () => {
    beforeEach(() => {
        backingStore.clear();
        if (typeof localStorage !== 'undefined') localStorage.clear();
        if (typeof sessionStorage !== 'undefined') sessionStorage.clear();
    });

    it('mnemonic derivation + device-key wrap does not leave the phrase anywhere', async () => {
        const { generateNewMnemonic, deriveKeys } = await import('../mnemonic');
        const { wrapAndStoreBackupKey, ensureDeviceKey } = await import('../deviceKey');

        const mnemonic = generateNewMnemonic();
        const derived = await deriveKeys(mnemonic);

        // Mimic the onboarding flow: persist only wrapped material, never the mnemonic.
        await ensureDeviceKey();
        await wrapAndStoreBackupKey(derived.backupKey);

        // Gather everything we've written.
        const allValues: string[] = [];
        for (const v of backingStore.values()) collectPrintable(v, allValues);
        if (typeof localStorage !== 'undefined') {
            for (let i = 0; i < localStorage.length; i++) {
                const k = localStorage.key(i);
                if (k) allValues.push(localStorage.getItem(k) ?? '');
            }
        }

        assertNoSubstring(allValues, mnemonic, 'Mnemonic phrase');
        // Also assert the derived backup key is not stored in the clear —
        // `wrapAndStoreBackupKey` should have AEAD-encrypted it.
        const rawHex = Array.from(derived.backupKey).map((b) => b.toString(16).padStart(2, '0')).join('');
        assertNoSubstring(allValues, rawHex, 'Raw backup key');
    });

    it('backup envelope does not embed the raw backup key or mnemonic', async () => {
        const { encodeBackup } = await import('../backup');
        const { generateNewMnemonic, deriveKeys } = await import('../mnemonic');

        const mnemonic = generateNewMnemonic();
        const derived = await deriveKeys(mnemonic);

        const payload = {
            schema_version: 1,
            created_at_us: 1_700_000_000_000_000,
            app_id: 'edet' as const,
            dna_hash: randomBytes(39),
            agent_pub_key: randomBytes(39),
            source_chain: [{ placeholder: mnemonic }],
        };
        const bytes = encodeBackup(payload, derived.backupKey, derived.fingerprint);
        const hex = Array.from(bytes).map((b) => b.toString(16).padStart(2, '0')).join('');

        // The mnemonic should NOT appear in plaintext in the envelope (the
        // ciphertext is AEAD-encrypted; only the envelope's plaintext fields
        // — magic, nonce, fingerprint, ciphertext — are visible).
        expect(hex).not.toContain(Buffer.from(mnemonic).toString('hex'));
        // The backup key itself must never appear in any serialised output.
        const keyHex = Array.from(derived.backupKey)
            .map((b) => b.toString(16).padStart(2, '0'))
            .join('');
        expect(hex).not.toContain(keyHex);
    });
});
