/**
 * The identity vault: seed material encrypted at rest.
 *
 * The seed ring (member id → Ed25519 seed) is the only secret the app
 * persists. It is stored as an XChaCha20-Poly1305 ciphertext in IndexedDB,
 * encrypted with a random 32-byte device key kept in a *separate* IndexedDB
 * store — the same wrapping layer the backup key uses.
 *
 * Where the device key lives (threat model, stated honestly)
 * -----------------------------------------------------------
 *   - Tauri app: in the OS keychain (macOS Keychain / Windows Credential
 *     Manager / freedesktop Secret Service) via the `keychain_*` IPC
 *     commands. The ciphertext in IndexedDB is then genuine at-rest
 *     protection: browser-storage dumps alone cannot open the vault.
 *   - Android Tauri: in the Android Keystore, via the `keystore_*` IPC
 *     commands (the `edet-keystore` mobile plugin wraps a random 32-byte
 *     device key with a non-exportable Keystore-resident AES key; see
 *     the paper's §Implementation). Falls back to the
 *     IndexedDB store below on any plugin error (old API level, corrupted
 *     Keystore) so the app stays usable.
 *   - Browser dev mode, or keychain/keystore unavailable: in a separate
 *     IndexedDB store. That wrapping is NOT at-rest encryption against full
 *     device access — the key sits beside the data. On Android the WebView
 *     storage is at least app-sandboxed by the OS (other apps cannot read
 *     it). `deviceKeyBackend` reports which mode is live.
 *
 * A pre-keychain/pre-keystore IndexedDB device key is migrated into the OS
 * backend on first launch so existing vaults stay decryptable — the
 * *existing* key is wrapped, never regenerated, and the IndexedDB copy is
 * deleted only once the OS backend confirms the key is stored.
 *
 * The recovery phrase is NEVER stored, in the vault or anywhere else.
 *
 * The passphrase
 * --------------
 * A member may set one. When they have, the vault key is the device key mixed
 * with a key derived from the passphrase, so the seeds cannot be read — by
 * anybody, including this app — until it is entered. That is what makes an
 * unlocked device a smaller loss than an unlocked vault, and it is the only
 * thing standing between somebody holding the device and somebody signing with
 * it: the device key alone is available to whoever has the phone unlocked.
 *
 * The passphrase is never stored. What is stored is its scrypt SALT and a
 * verifier — a hash of the derived key — so a wrong entry can be told apart
 * from a corrupt vault. `scrypt` at N=2^15 costs about a tenth of a second per
 * attempt on a handset, which is what makes a short passphrase worth having at
 * all.
 *
 * **Forgetting it is not recoverable, and must not be.** A vault that could be
 * opened without it would not be protecting anything. The recovery phrase is
 * the way back, which is why it is shown once at creation and why the backup
 * export exists.
 */

import localforage from 'localforage';
import { invoke } from '@tauri-apps/api/core';
import { xchacha20poly1305 } from '@noble/ciphers/chacha';
import { scrypt } from '@noble/hashes/scrypt';
import { sha256 } from '@noble/hashes/sha256';
import { randomBytes } from '@noble/hashes/utils';
import { writable, type Readable } from 'svelte/store';

import { lsGet, lsRemove } from './safeStorage';
import { isTauri } from '../lib/api';
import { bytesToHex, hexToBytes } from '../lib/crypto';

const DEVICE_KEY_KEY = 'edet.deviceKey';
const VAULT_KEY = 'edet.vault';
const META_KEY = 'edet.vaultMeta';
const LEGACY_SEEDS_KEY = 'edet-seeds';

const NONCE_LEN = 24;
const VAULT_AAD = new TextEncoder().encode('edet-vault-v1');

// Distinct store names so clearing the vault does not drop the device key
// (and vice versa) — the `secret` store split.
const secretStore = localforage.createInstance({
    name: 'edet',
    storeName: 'secret',
    description: 'edet device-local key material',
});
const vaultStore = localforage.createInstance({
    name: 'edet',
    storeName: 'vault',
    description: 'edet encrypted identity vault',
});

/** Seed ring shape as persisted: member id (or the reserved 'pending'
 *  slot for a not-yet-admitted identity) → 32 bytes. */
export type SeedRing = Record<string, number[]>;

export interface VaultMeta {
    updated_at_ms: number;
    identities: number;
}

const metaStore = writable<VaultMeta | null>(null);
/** Last vault write (time + identity count), for the Settings status line. */
export const vaultMeta: Readable<VaultMeta | null> = { subscribe: metaStore.subscribe };

/** Which custody backend holds the device key (null until first resolved). */
export const deviceKeyBackend = writable<'keychain' | 'keystore' | 'local' | null>(null);

/**
 * **A custody downgrade the member has not been told about is the one this
 * app must not sign through.**
 *
 * `ensureDeviceKey` falls back to browser storage on any keychain or keystore
 * error — an old API level, a corrupt Keystore, a Secret Service that is not
 * running — because refusing to start would strand a member from their own
 * identity. What it must not do is fall back QUIETLY: on that path the device
 * key sits beside the ciphertext, so the vault is no longer protected against
 * anybody who can read the app's storage, and a member who believes their
 * seeds are in the OS keychain is acting on a promise the app is not keeping.
 *
 * So the fallback sets this, and the app blocks signing until the member has
 * seen it and said so. `null` means nothing to acknowledge.
 */
export const custodyDowngrade = writable<'keychain' | 'keystore' | null>(null);

/** Whether the member has seen the downgrade above and accepted it. */
export const custodyDowngradeAcknowledged = writable<boolean>(false);

/**
 * **This device holds a key it cannot open**, which is a different fact from
 * every other custody failure and needs a different answer.
 *
 * It means the phone was restored from a backup or reset: the wrapped device
 * key came across, the Keystore key that wraps it did not, and nothing on this
 * device can ever open the vault beside it. Reading that as a downgrade would
 * send the member to browser storage and tell them their key store is broken —
 * looking for a fault that is not there, while their seeds sit unreachable.
 *
 * The answer is the recovery phrase, which is the answer this state was
 * designed around. After a successful restore the vault writes a FRESH device
 * key over the unreadable blob, and custody is re-established rather than
 * downgraded.
 */
export const custodyUnreadable = writable<boolean>(false);

/** The passphrase state of this device's vault. */
export type LockState =
    /** No passphrase set: the vault opens with the device key alone. */
    | 'none'
    /** A passphrase is set and has not been entered this session. */
    | 'locked'
    /** Entered, and the derived key is held in memory for this session only. */
    | 'unlocked';

export const lockState = writable<LockState>('none');

/** scrypt parameters. `N` is what a handset can afford per attempt. */
const SCRYPT = { N: 1 << 15, r: 8, p: 1, dkLen: 32 } as const;

/** What is stored ABOUT a passphrase — never the passphrase. */
interface LockRecord {
    v: 1;
    salt: number[];
    /** `sha256(derived key)`, so a wrong entry is distinguishable from a
     *  corrupt vault. It reveals nothing the derivation does not already
     *  cost an attacker `SCRYPT` per guess to test. */
    verifier: number[];
}

const LOCK_KEY = 'edet.vault.lock';

/** The passphrase-derived half of the vault key, for this session only. */
let unlocked: Uint8Array | null = null;

function deriveLockKey(passphrase: string, salt: Uint8Array): Uint8Array {
    return scrypt(new TextEncoder().encode(passphrase.normalize('NFKC')), salt, SCRYPT);
}

/** Is a passphrase set on this device? */
export async function isLocked(): Promise<boolean> {
    return (await secretStore.getItem<LockRecord>(LOCK_KEY)) !== null;
}

/**
 * Enter the passphrase. `true` when it opens the vault; `false` leaves the
 * vault shut and nothing is written, so a wrong entry costs an attacker one
 * scrypt and tells them nothing else.
 */
export async function unlock(passphrase: string): Promise<boolean> {
    const record = await secretStore.getItem<LockRecord>(LOCK_KEY);
    if (!record) {
        lockState.set('none');
        return true;
    }
    const derived = deriveLockKey(passphrase, Uint8Array.from(record.salt));
    if (bytesToHex(sha256(derived)) !== bytesToHex(Uint8Array.from(record.verifier))) {
        return false;
    }
    unlocked = derived;
    lockState.set('unlocked');
    return true;
}

/** Forget the passphrase-derived key, so the next read needs it again. */
export function lock(): void {
    unlocked = null;
    lockState.set('locked');
}

/**
 * Set, change or remove the passphrase.
 *
 * The vault is re-sealed under the new key in the same call, because the two
 * are one act: a lock record written without re-sealing would lock the member
 * out of seeds the old key still encrypts.
 */
export async function setPassphrase(ring: SeedRing, passphrase: string | null): Promise<void> {
    if (passphrase === null || passphrase === '') {
        await secretStore.removeItem(LOCK_KEY);
        unlocked = null;
        lockState.set('none');
    } else {
        const salt = randomBytes(16);
        const derived = deriveLockKey(passphrase, salt);
        const record: LockRecord = { v: 1, salt: Array.from(salt), verifier: Array.from(sha256(derived)) };
        await secretStore.setItem(LOCK_KEY, record);
        unlocked = derived;
        lockState.set('unlocked');
    }
    await persistVault(ring);
}

/**
 * The key the vault is sealed with: the device key, mixed with the
 * passphrase-derived key when one is set.
 *
 * Mixed by hashing both rather than by using the passphrase key alone, so a
 * passphrase never replaces the OS-held secret — an attacker needs the device
 * AND the passphrase, which is the whole point of having two.
 */
async function vaultKey(): Promise<Uint8Array> {
    const device = await ensureDeviceKey();
    const record = await secretStore.getItem<LockRecord>(LOCK_KEY);
    if (!record) {
        lockState.set('none');
        return device;
    }
    if (!unlocked) {
        lockState.set('locked');
        throw new Error('vault-locked');
    }
    const mixed = new Uint8Array(device.length + unlocked.length);
    mixed.set(device);
    mixed.set(unlocked, device.length);
    return sha256(mixed);
}

async function localDeviceKey(): Promise<Uint8Array | null> {
    const existing = await secretStore.getItem<ArrayBuffer | Uint8Array>(DEVICE_KEY_KEY);
    if (!existing) return null;
    return existing instanceof Uint8Array ? existing : new Uint8Array(existing);
}

/**
 * True only for a Tauri build actually running on Android (desktop and iOS
 * Tauri builds keep using the `keychain_*` commands). Read by
 * `lib/background.ts` too, for the one control that has no meaning off
 * Android — the deep link into battery-optimisation settings.
 * No `@tauri-apps/plugin-os`
 * dependency exists in this project yet, so this reads `navigator.userAgent`
 * the same way any WebView-hosted app detects its host OS — a build-time
 * flag was the spec's other suggested option
 * (the paper's §Implementation, "vault.ts changes
 * needed"), but that would need wiring through the Vite config for no
 * behavioural difference, so the runtime check is the smaller change.
 */
export function isTauriAndroid(): boolean {
    return isTauri() && typeof navigator !== 'undefined' && /android/i.test(navigator.userAgent ?? '');
}

/**
 * How long a platform-custody IPC call may take before the vault stops
 * waiting and falls back to browser storage.
 *
 * **A hang is not an error, and the `catch` below only ever caught errors.**
 * This is the whole boot of the app hanging on one unanswered call: the
 * Android client shipped a window, while the engine started, in which the
 * first `invoke` was never answered — its resolve/reject pair sat in
 * `window.__TAURI_INTERNALS__.callbacks` for the life of the page. The vault
 * never opened, so `booted` in App.svelte never became true and the client
 * showed its spinner forever, on a node that was committing blocks normally.
 * That window is closed on the Rust side now (`src-tauri/src/lib.rs`'s
 * `Ledger`); this bound is what stops any future one from being unbounded,
 * because the fallback documented above is only real if it can be REACHED.
 *
 * Generous on purpose: the same call, measured on a handset once the
 * backend was up, answers in about 150 ms. Anything near this bound is a
 * wedge, not a slow device.
 */
const CUSTODY_TIMEOUT_MS = 5_000;

/**
 * Reject if `p` has not settled within `ms`, naming what stopped answering.
 *
 * Exported because every IPC call in the client wants it, not only custody's:
 * a hang is not an error and no `catch` can see one, so a fallback is only
 * real if it can be reached (`lib/background.ts` is the other caller).
 */
export function withTimeout<T>(p: Promise<T>, ms: number, what: string): Promise<T> {
    return new Promise<T>((resolve, reject) => {
        const timer = setTimeout(
            () => reject(new Error(`${what} did not answer within ${ms} ms`)),
            ms,
        );
        p.then(
            (v) => { clearTimeout(timer); resolve(v); },
            (e) => { clearTimeout(timer); reject(e); },
        );
    });
}

/**
 * Return the persisted device key, creating one if none exists.
 *
 * Tauri desktop/iOS: OS keychain first. Tauri Android: the hardware-backed
 * Keystore instead (same call shape, different IPC command names — see the
 * doc comment at the top of this file). Either way, any pre-existing
 * IndexedDB key is migrated into the OS backend — wrapping the *existing*
 * key, never regenerating it, so already-sealed vaults stay decryptable —
 * and the IndexedDB copy is deleted only after the OS backend confirms the
 * key is stored. Any OS-backend failure (no Secret Service on a headless
 * Linux, locked keychain, unavailable/corrupted Keystore) falls back to the
 * IndexedDB path so the app stays usable; the active backend is reported
 * through `deviceKeyBackend`.
 *
 * "Any failure" includes a call that never comes back, which is why the two
 * IPC calls below are bounded — see `CUSTODY_TIMEOUT_MS`.
 */
export async function ensureDeviceKey(): Promise<Uint8Array> {
    if (isTauri()) {
        const android = isTauriAndroid();
        const getCmd = android ? 'keystore_get_device_key' : 'keychain_get_device_key';
        const setCmd = android ? 'keystore_set_device_key' : 'keychain_set_device_key';
        const backend: 'keychain' | 'keystore' = android ? 'keystore' : 'keychain';
        try {
            const stored = await withTimeout(invoke<string | null>(getCmd), CUSTODY_TIMEOUT_MS, getCmd);
            if (stored) {
                const key = hexToBytes(stored);
                if (key.length === 32) {
                    deviceKeyBackend.set(backend);
                    return key;
                }
                // A key that opened to the wrong shape is a key this device
                // cannot use, which is the `unwrap-failed:` case below and
                // not "no key": read as absent, it would be overwritten by a
                // fresh one and the recovery phrase never shown. The Android
                // plugin says so itself; the desktop keyring has no wrapping
                // layer to, so it is said here.
                throw new Error(`unwrap-failed: the stored device key is ${key.length} bytes, not 32`);
            }
            const legacy = await localDeviceKey();
            const key = legacy ?? randomBytes(32);
            await withTimeout(invoke(setCmd, { value: bytesToHex(key) }), CUSTODY_TIMEOUT_MS, setCmd);
            if (legacy) await secretStore.removeItem(DEVICE_KEY_KEY);
            deviceKeyBackend.set(backend);
            return key;
        } catch (e) {
            // **A blob this device cannot open is not a downgrade.** The
            // Android plugin prefixes exactly that case `unwrap-failed:`
            // (`DeviceKeyStore.UnwrapFailed`): the wrapped key survived a
            // restore or a reset and the Keystore key that wraps it did not,
            // so there is nothing to fall back TO — the sealed seeds are
            // unreadable on this device whatever backend it uses next. The way
            // back is the recovery phrase.
            if (String((e as Error)?.message ?? e).includes('unwrap-failed:')) {
                console.warn('[vault] this device holds a key it cannot open — restore from the recovery phrase', e);
                custodyUnreadable.set(true);
                // A fresh key, written over the blob, and custody stays with
                // the OS. The old blob is unrecoverable by construction — the
                // key that wrapped it is gone — so keeping it would only make
                // the restore path unreachable, since every seal and every
                // open runs through here. What the member has lost is the
                // sealed ring, and the recovery phrase is what replaces it.
                const fresh = randomBytes(32);
                try {
                    await withTimeout(invoke(setCmd, { value: bytesToHex(fresh) }), CUSTODY_TIMEOUT_MS, setCmd);
                    deviceKeyBackend.set(backend);
                    return fresh;
                } catch (write) {
                    console.warn(`[vault] and could not write a fresh one; falling back to browser storage`, write);
                    custodyDowngrade.set(backend);
                }
            }
            console.warn(`[vault] OS ${backend} unavailable; falling back to browser storage`, e);
            // Loud, not silent: on this path the device key sits beside the
            // ciphertext, so the vault is no longer protected against anybody
            // who can read the app's storage. Signing is blocked until the
            // member has seen this and said so.
            custodyDowngrade.set(backend);
        }
    }
    const existing = await localDeviceKey();
    if (existing) {
        deviceKeyBackend.set('local');
        return existing;
    }
    const key = randomBytes(32);
    await secretStore.setItem(DEVICE_KEY_KEY, key);
    deviceKeyBackend.set('local');
    return key;
}

interface StoredVault {
    v: 1;
    nonce: number[];
    ciphertext: number[];
}

function seal(ring: SeedRing, key: Uint8Array): StoredVault {
    const plaintext = new TextEncoder().encode(JSON.stringify(ring));
    const nonce = randomBytes(NONCE_LEN);
    const ciphertext = xchacha20poly1305(key, nonce, VAULT_AAD).encrypt(plaintext);
    return { v: 1, nonce: Array.from(nonce), ciphertext: Array.from(ciphertext) };
}

function open(stored: StoredVault, key: Uint8Array): SeedRing {
    const cipher = xchacha20poly1305(key, Uint8Array.from(stored.nonce), VAULT_AAD);
    const plaintext = cipher.decrypt(Uint8Array.from(stored.ciphertext));
    return JSON.parse(new TextDecoder().decode(plaintext));
}

/**
 * Load the seed ring, migrating any legacy plaintext localStorage ring into
 * the vault (and deleting the plaintext copy) on first run.
 */
export async function loadVault(): Promise<SeedRing> {
    const key = await vaultKey();

    let ring: SeedRing = {};
    const stored = await vaultStore.getItem<StoredVault>(VAULT_KEY);
    if (stored) {
        try {
            ring = open(stored, key);
        } catch (e) {
            // Wrong device key or corrupt ciphertext: surface loudly rather
            // than silently starting empty and overwriting the vault later.
            console.error('[vault] failed to open the identity vault', e);
            throw new Error('vault-unreadable');
        }
    }

    // Legacy migration: plaintext seeds from the pre-vault build.
    const legacy = lsGet(LEGACY_SEEDS_KEY);
    if (legacy) {
        try {
            const parsed: SeedRing = JSON.parse(legacy);
            ring = { ...parsed, ...ring };
            await persistVault(ring);
        } catch {
            // Unparseable legacy blob: drop it below either way.
        }
        lsRemove(LEGACY_SEEDS_KEY);
    }

    const meta = await vaultStore.getItem<VaultMeta>(META_KEY);
    metaStore.set(meta ?? (stored || legacy ? { updated_at_ms: 0, identities: Object.keys(ring).length } : null));
    return ring;
}

/** Encrypt and persist the seed ring; updates the vault metadata. */
export async function persistVault(ring: SeedRing): Promise<void> {
    const key = await vaultKey();
    await vaultStore.setItem(VAULT_KEY, seal(ring, key));
    const meta: VaultMeta = { updated_at_ms: Date.now(), identities: Object.keys(ring).length };
    await vaultStore.setItem(META_KEY, meta);
    metaStore.set(meta);
}

/** Test hook: wipe both stores. */
export async function clearVaultForTests(): Promise<void> {
    await secretStore.clear();
    await vaultStore.clear();
    metaStore.set(null);
    unlocked = null;
    lockState.set('none');
    custodyDowngrade.set(null);
    custodyDowngradeAcknowledged.set(false);
    custodyUnreadable.set(false);
}
