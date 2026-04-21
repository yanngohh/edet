import { beforeEach, describe, expect, it, vi } from 'vitest';

import {
    clearDeviceSecrets,
    ensureDeviceKey,
    hasWrappedBackupKey,
    loadBackupKey,
    readDeviceKey,
    wrapAndStoreBackupKey,
} from '../deviceKey';
import { randomBytes } from '@noble/hashes/utils';

// In-memory localforage stub shared across tests. We isolate it per test by
// wiping the Map before each assertion block; the scheduler test suite uses
// a similar pattern.
const backingStore = new Map<string, unknown>();
vi.mock('localforage', () => {
    return {
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
    };
});

// Stub tauriBridge so `ensureDeviceKey` always falls through to the localforage
// path; production will gate on a real OS keychain once Tauri's plugin lands.
vi.mock('../tauriBridge', () => ({
    tauriBridge: {
        isTauri: () => false,
        runtime: () => 'browser',
    },
}));

describe('deviceKey', () => {
    beforeEach(() => {
        backingStore.clear();
    });

    it('ensureDeviceKey creates and persists a 32-byte key', async () => {
        const k = await ensureDeviceKey();
        expect(k).toHaveLength(32);
        // Second call returns the same bytes — it is idempotent, not
        // re-generating and silently invalidating existing wrapped state.
        const k2 = await ensureDeviceKey();
        expect(k2).toEqual(k);
    });

    it('readDeviceKey returns null when none exists', async () => {
        expect(await readDeviceKey()).toBeNull();
    });

    it('wrap + load round-trip returns the original backup key', async () => {
        const backupKey = randomBytes(32);
        await wrapAndStoreBackupKey(backupKey);
        expect(await hasWrappedBackupKey()).toBe(true);
        const loaded = await loadBackupKey();
        expect(loaded).not.toBeNull();
        expect(loaded).toEqual(backupKey);
    });

    it('loadBackupKey returns null when no wrap exists', async () => {
        expect(await loadBackupKey()).toBeNull();
    });

    it('clearDeviceSecrets erases both values', async () => {
        await wrapAndStoreBackupKey(randomBytes(32));
        expect(await hasWrappedBackupKey()).toBe(true);
        await clearDeviceSecrets();
        expect(await readDeviceKey()).toBeNull();
        expect(await hasWrappedBackupKey()).toBe(false);
    });

    it('rejects backup keys of the wrong length', async () => {
        await expect(wrapAndStoreBackupKey(new Uint8Array(31))).rejects.toThrow(/32 bytes/);
    });
});
