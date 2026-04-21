/**
 * Device-local key management for backup re-encryption after onboarding.
 *
 * After the user completes onboarding their mnemonic is no longer in memory
 * — we deliberately zero it. But the backup scheduler needs to keep
 * producing up-to-date encrypted bundles without interrupting the user to
 * ask for their phrase again. This module provides that "device-local"
 * wrapping layer:
 *
 *   1. At onboarding completion we generate a 32-byte random `deviceKey`
 *      using the platform CSPRNG and persist it in the most secure local
 *      store available (Tauri OS keychain on desktop, IndexedDB in dev).
 *   2. We AEAD-encrypt (XChaCha20-Poly1305) the mnemonic-derived `backupKey`
 *      with `deviceKey` and persist the ciphertext ("wrappedBackupKey")
 *      alongside the backup bundle.
 *   3. The scheduler calls `loadBackupKey()`, which reads both values and
 *      returns the unwrapped `backupKey` only in ephemeral memory.
 *
 * Threat model
 * ------------
 *
 * An attacker with offline access to the user's backup file alone cannot
 * derive `backupKey` — they still need either the mnemonic (to re-derive
 * it) or the `deviceKey` from this device.
 *
 * An attacker with full device access has the conductor's plaintext source
 * chain already (it's stored in the conductor's SQLite database), so the
 * device-wrapping does not pretend to raise that bar. The wrapping's sole
 * purpose is UX: letting the scheduler work without prompting for the
 * mnemonic, while keeping the "lose device and backup file" recovery path
 * intact.
 */

import localforage from 'localforage';
import { xchacha20poly1305 } from '@noble/ciphers/chacha';
import { randomBytes } from '@noble/hashes/utils';

import { tauriBridge } from './tauriBridge';

const DEVICE_KEY_STORE = 'edet.deviceKey';
const WRAPPED_BACKUP_KEY_STORE = 'edet.wrappedBackupKey';

const DEVICE_KEY_LEN = 32;
const NONCE_LEN = 24;
const KEY_WRAP_AAD = new TextEncoder().encode('edet-device-wrap-v1');

// Shared localforage instance for device-local secret material. Kept in a
// distinct store name from the backup bundle so clearing the bundle
// (e.g. during "forget identity" flows) does not drop the device key.
const secretStore = localforage.createInstance({
    name: 'edet',
    storeName: 'secret',
    description: 'edet device-local key material',
});

/** A wrapped backup key as stored on disk: nonce || ciphertext. */
export interface WrappedBackupKey {
    nonce: Uint8Array;
    ciphertext: Uint8Array;
}

function hasKeychain(): boolean {
    // Hook for Tauri OS-keychain integration. The Tauri shell can expose a
    // `keychain:put` / `keychain:get` command pair; when it does, we route
    // the device key through it instead of IndexedDB. For now we rely on
    // localforage on both paths — see PLAN.md §10 follow-ups.
    return false && tauriBridge.isTauri();
}

/** Return the persisted device key, creating one if none exists. */
export async function ensureDeviceKey(): Promise<Uint8Array> {
    if (hasKeychain()) {
        // TODO(keychain): wire Tauri keychain plugin.
        throw new Error('keychain backend not implemented');
    }
    const existing = await secretStore.getItem<Uint8Array | ArrayBuffer>(DEVICE_KEY_STORE);
    if (existing) {
        const bytes = existing instanceof Uint8Array ? existing : new Uint8Array(existing);
        if (bytes.length === DEVICE_KEY_LEN) return bytes;
        // Corrupt or wrong-length value; start fresh.
    }
    const fresh = randomBytes(DEVICE_KEY_LEN);
    await secretStore.setItem(DEVICE_KEY_STORE, fresh);
    return fresh;
}

/** Read the persisted device key without creating one. Returns `null` if none. */
export async function readDeviceKey(): Promise<Uint8Array | null> {
    const existing = await secretStore.getItem<Uint8Array | ArrayBuffer>(DEVICE_KEY_STORE);
    if (!existing) return null;
    const bytes = existing instanceof Uint8Array ? existing : new Uint8Array(existing);
    return bytes.length === DEVICE_KEY_LEN ? bytes : null;
}

/** Forget the device key AND the wrapped backup key. Intended only for
 *  "reset identity" flows — this invalidates the scheduler's ability to
 *  produce new backups until onboarding runs again. */
export async function clearDeviceSecrets(): Promise<void> {
    await secretStore.removeItem(DEVICE_KEY_STORE);
    await secretStore.removeItem(WRAPPED_BACKUP_KEY_STORE);
}

/** Wrap a 32-byte backup key with the device key and persist it. */
export async function wrapAndStoreBackupKey(backupKey: Uint8Array): Promise<void> {
    if (backupKey.length !== 32) {
        throw new Error(`wrapAndStoreBackupKey: backupKey must be 32 bytes, got ${backupKey.length}`);
    }
    const deviceKey = await ensureDeviceKey();
    const nonce = randomBytes(NONCE_LEN);
    const cipher = xchacha20poly1305(deviceKey, nonce, KEY_WRAP_AAD);
    const ciphertext = cipher.encrypt(backupKey);
    const wrapped: WrappedBackupKey = { nonce, ciphertext };
    await secretStore.setItem(WRAPPED_BACKUP_KEY_STORE, wrapped);
}

/**
 * Unwrap and return the backup key. The returned buffer is a fresh
 * Uint8Array; callers SHOULD zero it immediately after use.
 *
 * Returns `null` if no wrapped key has been stored yet (i.e. onboarding
 * hasn't completed on this device).
 */
export async function loadBackupKey(): Promise<Uint8Array | null> {
    const deviceKey = await readDeviceKey();
    if (!deviceKey) return null;
    const wrapped = await secretStore.getItem<WrappedBackupKey>(WRAPPED_BACKUP_KEY_STORE);
    if (!wrapped) return null;
    if (!(wrapped.nonce instanceof Uint8Array) || wrapped.nonce.length !== NONCE_LEN) return null;
    if (!(wrapped.ciphertext instanceof Uint8Array)) return null;

    const cipher = xchacha20poly1305(deviceKey, wrapped.nonce, KEY_WRAP_AAD);
    try {
        return cipher.decrypt(wrapped.ciphertext);
    } catch (e) {
        console.warn('[deviceKey] wrapped backup key failed AEAD — clearing', e);
        await secretStore.removeItem(WRAPPED_BACKUP_KEY_STORE);
        return null;
    }
}

/** Return `true` iff a wrapped backup key is persisted. */
export async function hasWrappedBackupKey(): Promise<boolean> {
    const wrapped = await secretStore.getItem<WrappedBackupKey>(WRAPPED_BACKUP_KEY_STORE);
    return Boolean(wrapped?.nonce && wrapped?.ciphertext);
}
