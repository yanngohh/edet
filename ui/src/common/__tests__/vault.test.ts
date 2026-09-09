import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import localforage from 'localforage';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
    invoke: (...args: unknown[]) => invokeMock(...args),
}));

import {
    clearVaultForTests,
    custodyDowngrade,
    custodyUnreadable,
    deviceKeyBackend,
    ensureDeviceKey,
    loadVault,
    lock,
    lockState,
    persistVault,
    setPassphrase,
    unlock,
} from '../vault';
import { bytesToHex } from '../../lib/crypto';

// In jsdom localforage falls back to its localStorage driver, so the
// "IndexedDB" stores are inspectable through window.localStorage — which
// lets these tests assert that what hits disk is ciphertext.

const RING = { 3: Array(32).fill(7), 9: Array(32).fill(200) };

// Mirrors of vault.ts's own localforage instances (same name/storeName),
// used only to seed a known pre-migration device key directly — vault.ts
// does not export `secretStore`.
const secretMirror = localforage.createInstance({ name: 'edet', storeName: 'secret', description: 'test mirror' });

function currentBackend(): string | null {
    let value: string | null = null;
    deviceKeyBackend.subscribe((v) => (value = v))();
    return value;
}

function currentLock(): string {
    let value = '';
    lockState.subscribe((v) => (value = v))();
    return value;
}

function currentUnreadable(): boolean {
    let value = false;
    custodyUnreadable.subscribe((v) => (value = v))();
    return value;
}

function currentDowngrade(): string | null {
    let value: string | null = null;
    custodyDowngrade.subscribe((v) => (value = v))();
    return value;
}

function setTauriAndroid() {
    (window as any).__TAURI_INTERNALS__ = {};
    Object.defineProperty(window.navigator, 'userAgent', {
        value: 'Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/116.0 Mobile Safari/537.36; wv',
        configurable: true,
    });
}

function clearTauriFlags() {
    delete (window as any).__TAURI_INTERNALS__;
    delete (window as any).__TAURI__;
}

function allStoredValues(): string {
    let out = '';
    for (let i = 0; i < window.localStorage.length; i++) {
        const k = window.localStorage.key(i)!;
        out += k + '=' + (window.localStorage.getItem(k) ?? '') + '\n';
    }
    return out;
}

beforeEach(async () => {
    window.localStorage.clear();
    await clearVaultForTests();
    invokeMock.mockReset();
});

afterEach(() => {
    clearTauriFlags();
});

describe('identity vault', () => {
    it('round-trips the seed ring through encryption', async () => {
        await persistVault(RING);
        const back = await loadVault();
        expect(back).toEqual(RING);
    });

    it('persists only ciphertext — no plaintext seed runs on disk', async () => {
        await persistVault(RING);
        const disk = allStoredValues();
        // A 32-byte constant seed serializes as a long "7,7,7,…" run; it must
        // not appear outside the AEAD.
        expect(disk).not.toContain(JSON.stringify(RING[3]));
        expect(disk).not.toContain('200,200,200');
        // The device key store and vault store are distinct entries.
        expect(disk).toContain('edet.deviceKey');
        expect(disk).toContain('edet.vault');
    });

    it('migrates a legacy plaintext ring and deletes it', async () => {
        window.localStorage.setItem('edet-seeds', JSON.stringify(RING));
        const ring = await loadVault();
        expect(ring).toEqual(RING);
        expect(window.localStorage.getItem('edet-seeds')).toBeNull();
        // And the migrated ring is now retrievable from the vault alone.
        const again = await loadVault();
        expect(again).toEqual(RING);
    });

    it('starts empty on a fresh device', async () => {
        expect(await loadVault()).toEqual({});
    });
});

// Android Keystore custody — see the paper
// Part 1, "Acceptance criteria". #1, #5, #6, #7 need a real device/emulator
// (Keystore hardware, an app restart, an APK build) and are not exercised
// here; #2, #3, #4 are the parts of the migration logic that live entirely
// in vault.ts and are covered below.
describe('Android Keystore custody', () => {
    it('acceptance #2: migrates an existing IndexedDB device key into the Keystore losslessly', async () => {
        // Seed a pre-upgrade install: a known (not random) device key already
        // in IndexedDB, and a vault already sealed under it — exactly the
        // state a real Android user upgrading into this plugin would have.
        const knownKey = new Uint8Array(32).fill(42);
        await secretMirror.setItem('edet.deviceKey', knownKey);
        await persistVault(RING); // ensureDeviceKey() picks up knownKey (no Tauri flags yet -> 'local' backend)
        expect(currentBackend()).toBe('local');

        // Now the app upgrades: Keystore plugin is present but has no key yet.
        setTauriAndroid();
        invokeMock.mockImplementation(async (cmd: string, args?: { value?: string }) => {
            if (cmd === 'keystore_get_device_key') return null;
            if (cmd === 'keystore_set_device_key') {
                expect(args?.value).toBe(bytesToHex(knownKey)); // wraps the EXISTING key, not a fresh one
                return undefined;
            }
            throw new Error(`unexpected invoke: ${cmd}`);
        });

        const ring = await loadVault();
        expect(ring).toEqual(RING); // same ring, decrypted with the same migrated key
        expect(currentBackend()).toBe('keystore');
        expect(invokeMock).toHaveBeenCalledWith('keystore_get_device_key');
        expect(invokeMock).toHaveBeenCalledWith('keystore_set_device_key', { value: bytesToHex(knownKey) });

        // Lossless migration deletes the IndexedDB copy only after the
        // Keystore set succeeded.
        expect(await secretMirror.getItem('edet.deviceKey')).toBeNull();

        // And the vault keeps opening the same way on a second load, reading
        // the now-migrated key back from the (mocked) Keystore.
        invokeMock.mockImplementation(async (cmd: string) => {
            if (cmd === 'keystore_get_device_key') return bytesToHex(knownKey);
            throw new Error(`unexpected invoke: ${cmd}`);
        });
        expect(await loadVault()).toEqual(RING);
    });

    it('acceptance #3: falls back to the local backend if the plugin throws, without surfacing an error', async () => {
        setTauriAndroid();
        invokeMock.mockImplementation(async () => {
            throw new Error('Android Keystore is only available on Android');
        });

        await expect(loadVault()).resolves.toEqual({});
        expect(currentBackend()).toBe('local');

        // The app stays usable: a fresh key was generated and the vault can
        // still be written and read back through the fallback path.
        await persistVault(RING);
        expect(await loadVault()).toEqual(RING);
        expect(currentBackend()).toBe('local');
    });

    it('falls back to the local backend when the custody call never answers at all', async () => {
        // The failure this pins is NOT a rejection — acceptance #3 above
        // already covers that, and a `catch` handles it. This is a call that
        // never settles, which a `catch` cannot see. It is what the Android
        // client actually did: the first `invoke` of the boot went unanswered
        // for the life of the page (its resolve/reject pair still sitting in
        // `window.__TAURI_INTERNALS__.callbacks` after 594 s on a handset),
        // so `loadVault` never returned and App.svelte's `booted` never became
        // true — the spinner ran forever while the node committed blocks fine.
        //
        // Delete the `withTimeout` wrapper in vault.ts and this test hangs
        // until vitest kills it, which is precisely the bug.
        vi.useFakeTimers();
        try {
            setTauriAndroid();
            invokeMock.mockImplementation(() => new Promise(() => {})); // never settles

            const opened = loadVault();
            await vi.advanceTimersByTimeAsync(6_000);
            await expect(opened).resolves.toEqual({});
            expect(currentBackend()).toBe('local');
        } finally {
            vi.useRealTimers();
        }
    });

    it('acceptance #4: never accepts or forwards a malformed (non-64-hex) value from/to the Keystore', async () => {
        setTauriAndroid();
        // A corrupted/short value coming back from `keystore_get_device_key`
        // must not be handed to the caller as-is (mirrors the length guard
        // the desktop `keychain_get_device_key` path already relies on) —
        // and it is a key this device cannot open, so it routes to the
        // recovery phrase (`custodyUnreadable`) rather than being read as
        // "no key" and silently replaced.
        invokeMock.mockImplementation(async (cmd: string, args?: { value?: string }) => {
            if (cmd === 'keystore_get_device_key') return 'ab'.repeat(16); // valid hex, wrong length (16 bytes)
            if (cmd === 'keystore_set_device_key') {
                // Whatever this path falls back to generating/wrapping must
                // itself be well-formed 64-hex — the same shape
                // `keystore_set_device_key`'s own Rust-side validation
                // (`src-tauri/src/lib.rs`, 64-hex-char check) requires.
                expect(args?.value).toMatch(/^[0-9a-f]{64}$/);
                return undefined;
            }
            throw new Error(`unexpected invoke: ${cmd}`);
        });

        const key = await ensureDeviceKey();
        expect(key.length).toBe(32);
        expect(currentBackend()).toBe('keystore');
        expect(currentUnreadable()).toBe(true);
        expect(invokeMock).toHaveBeenCalledWith('keystore_set_device_key', expect.objectContaining({ value: expect.any(String) }));
    });
});

// -------------------------------------------------- the passphrase gate --

/**
 * **A passphrase is the second factor beside the device.**
 *
 * The device key alone is available to whoever holds the phone unlocked — that
 * is what an OS keychain gives a running app. So a member may add a passphrase,
 * and then the seeds cannot be read by anybody, this app included, until it is
 * entered.
 */
describe('the vault passphrase', () => {
    it('seals the vault so it will not open without it', async () => {
        await persistVault(RING);
        await setPassphrase(RING, 'correct horse');
        expect(currentLock()).toBe('unlocked');

        // A new session: the derived key is in memory only, so it is gone.
        lock();
        expect(currentLock()).toBe('locked');
        await expect(loadVault()).rejects.toThrow('vault-locked');

        expect(await unlock('wrong horse')).toBe(false);
        await expect(loadVault()).rejects.toThrow('vault-locked');

        expect(await unlock('correct horse')).toBe(true);
        expect(await loadVault()).toEqual(RING);
    });

    /** Removing it puts the vault back where it was, still readable. */
    it('can be removed, and the seeds survive it', async () => {
        await persistVault(RING);
        await setPassphrase(RING, 'a passphrase');
        await setPassphrase(RING, null);
        expect(currentLock()).toBe('none');
        lock();
        expect(await loadVault()).toEqual(RING);
    });

    /**
     * It is a SECOND factor, never a replacement: the sealed vault is bound to
     * the device key as well, so a passphrase alone opens nothing.
     */
    it('does not replace the device key', async () => {
        await persistVault(RING);
        await setPassphrase(RING, 'a passphrase');
        const sealed = window.localStorage.getItem('edet/vault/edet.vault');
        expect(sealed).toBeTruthy();

        // A different device, same passphrase: the device key is regenerated,
        // so the ciphertext does not open.
        await secretMirror.removeItem('edet.deviceKey');
        expect(await unlock('a passphrase')).toBe(true);
        await expect(loadVault()).rejects.toThrow('vault-unreadable');
    });
});

/**
 * **A custody downgrade the member has not been told about is the one this app
 * must not sign through.** The fallback to browser storage is right — refusing
 * to start would strand a member from their own identity — and it leaves the
 * device key beside the ciphertext, so it must be visible rather than quiet.
 */
describe('a custody downgrade', () => {
    it('is reported when the OS key store cannot be reached', async () => {
        setTauriAndroid();
        invokeMock.mockRejectedValue(new Error('Android Keystore is only available on Android'));
        await ensureDeviceKey();
        expect(currentBackend()).toBe('local');
        expect(currentDowngrade()).toBe('keystore');
    });

    it('is absent on a healthy device', async () => {
        await ensureDeviceKey();
        expect(currentDowngrade()).toBeNull();
    });

    /**
     * **A device that holds a key it cannot open is NOT a downgrade**, and
     * reading it as one is the defect this branch exists to close.
     *
     * It means the phone was restored or reset: the wrapped device key came
     * across, the Keystore key that wraps it did not, and no backend on this
     * device can open the sealed seeds. Falling back to browser storage would
     * tell the member their key store is broken — a fault that is not there —
     * while the answer they need is their recovery phrase.
     *
     * Custody stays with the OS, a fresh key is written over the unreadable
     * blob (the old one is unrecoverable by construction, and every seal runs
     * through here, so keeping it would make the restore path unreachable),
     * and `custodyUnreadable` is what routes the member.
     */
    it('is not what an unopenable blob reports — that is the restore path', async () => {
        setTauriAndroid();
        invokeMock.mockImplementation(async (cmd: string) => {
            if (cmd === 'keystore_get_device_key') {
                throw new Error('unwrap-failed: the wrapped device key could not be opened');
            }
            if (cmd === 'keystore_set_device_key') return undefined;
            throw new Error(`unexpected invoke: ${cmd}`);
        });

        const key = await ensureDeviceKey();
        expect(key.length).toBe(32);
        expect(currentUnreadable()).toBe(true);
        expect(currentDowngrade()).toBeNull();
        expect(currentBackend()).toBe('keystore');
        expect(invokeMock).toHaveBeenCalledWith(
            'keystore_set_device_key',
            expect.objectContaining({ value: expect.stringMatching(/^[0-9a-f]{64}$/) }),
        );
    });

    /** And if the fresh key cannot be written either, THAT is a downgrade —
     *  the member still needs a working vault to restore into. */
    it('is what an unopenable blob reports when a fresh key cannot be written', async () => {
        setTauriAndroid();
        invokeMock.mockRejectedValue(new Error('unwrap-failed: nothing here opens'));
        await ensureDeviceKey();
        expect(currentUnreadable()).toBe(true);
        expect(currentDowngrade()).toBe('keystore');
        expect(currentBackend()).toBe('local');
    });
});
