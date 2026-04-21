/**
 * Local persistence for the current identity backup bundle.
 *
 * Two backends share this interface:
 *
 *  - In the browser / hc-spin dev flow we use localforage (IndexedDB) under
 *    the key `edet.backup.current`.
 *  - In the Tauri shell the bridge redirects reads and writes to
 *    `$APPDATA/edet/backup.edet-backup` on disk so the bundle lives alongside
 *    the conductor data and survives the wiping of site storage.
 *
 * Consumers talk to this module regardless of which backend is active. The
 * backend is chosen at import time based on `tauriBridge.isTauri()`.
 */

import localforage from 'localforage';

import { tauriBridge } from './tauriBridge';

const STORE_KEY = 'edet.backup.current';
const METADATA_KEY = 'edet.backup.metadata';

export interface BackupMetadata {
    /** When this bundle was written, in wall-clock milliseconds. */
    createdAt: number;
    /** Base64 agent pub key recorded at write time; lets the UI detect a
     *  mismatch if the conductor later reports a different identity. */
    agentPubKeyB64: string;
    /** Size of the ciphertext blob, for progress + debugging. */
    sizeBytes: number;
}

// `localforage` transparently falls back to WebSQL or localStorage on browsers
// without IndexedDB. We keep the store name explicit so multiple installs on
// the same origin don't collide.
const store = localforage.createInstance({
    name: 'edet',
    storeName: 'backup',
    description: 'edet encrypted identity backup bundle',
});

function encodeBase64(bytes: Uint8Array): string {
    let binary = '';
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
    // eslint-disable-next-line no-restricted-globals
    return typeof btoa === 'function' ? btoa(binary) : Buffer.from(bytes).toString('base64');
}

/** Read the most recent backup ciphertext, or `null` if none has been saved. */
export async function readCurrentBackup(): Promise<Uint8Array | null> {
    if (tauriBridge.isTauri()) {
        return await tauriBridge.getCurrentBackup();
    }
    const raw = (await store.getItem<Uint8Array | ArrayBuffer>(STORE_KEY)) ?? null;
    if (!raw) return null;
    return raw instanceof Uint8Array ? raw : new Uint8Array(raw);
}

/** Write the given ciphertext as the current backup, replacing any previous one. */
export async function writeCurrentBackup(
    bytes: Uint8Array,
    agentPubKey: Uint8Array,
): Promise<BackupMetadata> {
    const meta: BackupMetadata = {
        createdAt: Date.now(),
        agentPubKeyB64: encodeBase64(agentPubKey),
        sizeBytes: bytes.length,
    };
    if (tauriBridge.isTauri()) {
        await tauriBridge.saveCurrentBackup(bytes);
    } else {
        await store.setItem(STORE_KEY, bytes);
    }
    await store.setItem(METADATA_KEY, meta);
    return meta;
}

/** Read the metadata for the most recent backup, if any. */
export async function readBackupMetadata(): Promise<BackupMetadata | null> {
    return (await store.getItem<BackupMetadata>(METADATA_KEY)) ?? null;
}

/** Return `true` iff a current backup exists in local storage. */
export async function hasCurrentBackup(): Promise<boolean> {
    const bytes = await readCurrentBackup();
    return bytes !== null && bytes.length > 0;
}

/** Forget the current backup and metadata. Intended for "reset identity" flows only. */
export async function clearBackup(): Promise<void> {
    if (tauriBridge.isTauri()) {
        await tauriBridge.saveCurrentBackup(new Uint8Array(0));
    } else {
        await store.removeItem(STORE_KEY);
    }
    await store.removeItem(METADATA_KEY);
}

/** Export the current bundle to a user-chosen location (Tauri) or download (web). */
export async function exportBackupCopy(suggestedName: string): Promise<boolean> {
    const bytes = await readCurrentBackup();
    if (!bytes) return false;
    if (tauriBridge.isTauri()) {
        const path = await tauriBridge.exportBackupFile(bytes, suggestedName);
        return path !== null;
    }
    // Browser fallback: trigger a <a download="..."> click. The `BlobPart`
    // cast silences a newer-TypeScript-lib `Uint8Array<ArrayBufferLike>`
    // vs `BlobPart` mismatch that does not affect runtime behaviour.
    const blob = new Blob([bytes as BlobPart], { type: 'application/octet-stream' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = suggestedName;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
    return true;
}

/**
 * Import a backup file chosen by the user. Returns the ciphertext bytes, or
 * `null` if the user cancelled the native dialog or browser picker.
 */
export async function importBackupCopy(): Promise<Uint8Array | null> {
    if (tauriBridge.isTauri()) {
        return await tauriBridge.importBackupFile();
    }
    // Browser fallback: open a one-off <input type="file"> and wait for a selection.
    return new Promise<Uint8Array | null>((resolve) => {
        const input = document.createElement('input');
        input.type = 'file';
        input.accept = '.edet-backup,application/octet-stream';
        input.onchange = async () => {
            const file = input.files?.[0];
            if (!file) {
                resolve(null);
                return;
            }
            const buf = await file.arrayBuffer();
            resolve(new Uint8Array(buf));
        };
        input.oncancel = () => resolve(null);
        input.click();
    });
}
