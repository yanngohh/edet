/**
 * Typed contract between the Svelte UI and the Tauri shell.
 *
 * In production the UI runs inside our Tauri v2 application and talks to the
 * embedded conductor + lair via the commands declared in `src-tauri/src/
 * commands.rs`. During development against `hc-spin` (no Tauri), the same
 * interface is satisfied by a mock implementation so onboarding, tour, and
 * backup encode / decode flows can be exercised end-to-end in a browser
 * without wiring up a real conductor key-import path.
 *
 * Keeping both implementations behind a single `tauriBridge` export means the
 * rest of the UI never has to know which mode it is running in.
 */

import type { AgentPubKey, CellId, Record as HolochainRecord } from '@holochain/client';

declare global {
    // eslint-disable-next-line no-var
    var __TAURI_INTERNALS__: { invoke?: (cmd: string, payload?: unknown) => Promise<unknown> } | undefined;
    // Legacy pre-v2 reference; still exposed under the hood in some tests.
    // eslint-disable-next-line no-var
    var __TAURI__: unknown | undefined;
}

/** Parallel Rust enum `commands::StartupState`. */
export type StartupState =
    | { kind: 'fresh' }
    | { kind: 'installed'; app_id: string }
    | { kind: 'error'; message: string };

export interface TauriBridge {
    /** `true` iff the running renderer is inside the Tauri shell. */
    isTauri(): boolean;

    /** Best-effort human-readable runtime label for logging. */
    runtime(): 'tauri' | 'browser';

    /**
     * Ask the shell whether the conductor already has an installed app, or
     * whether this is a fresh device that needs onboarding / restore.
     */
    startupState(): Promise<StartupState>;

    /**
     * Return the app-websocket port and auth token for the installed edet app.
     * Call this after `startupState()` returns `installed`, or after
     * `installAppWithAgentKey()` completes, to obtain the credentials needed
     * to open an `AppWebsocket` manually.
     *
     * Throws in the browser/mock environment — callers should use plain
     * `AppWebsocket.connect()` (no args) on the hc-spin path.
     */
    getAppWebsocketAuth(): Promise<{ port: number; token: number[] }>;

    /**
     * Dump the full source chain for the given cell. Used when producing a
     * backup bundle.
     */
    dumpSourceChain(cellId: CellId): Promise<HolochainRecord[]>;

    /**
     * Graft a previously-dumped source chain onto a freshly-installed cell.
     * Must be called before any new action is authored under the new agent.
     */
    graftSourceChain(cellId: CellId, records: HolochainRecord[]): Promise<void>;

    /**
     * Seed lair-keystore with a 32-byte ed25519 seed. Returns the matching
     * AgentPubKey. The seed itself is zeroed by the bridge after use; the
     * caller should not retain it any longer than necessary.
     */
    importLairSeed(seed: Uint8Array): Promise<AgentPubKey>;

    /**
     * Install the bundled `edet.happ` under the given agent pubkey. Used both
     * for fresh installs and for recovery. The app is enabled on success.
     */
    installAppWithAgentKey(agentPubKey: AgentPubKey): Promise<void>;

    /**
     * Ask the shell to present a native "save file" dialog for a backup
     * bundle. Returns the absolute path the user picked, or `null` if they
     * cancelled.
     */
    exportBackupFile(bytes: Uint8Array, suggestedName: string): Promise<string | null>;

    /**
     * Ask the shell to present a native "open file" dialog for a backup
     * bundle. Returns the raw bytes, or `null` if the user cancelled.
     */
    importBackupFile(): Promise<Uint8Array | null>;

    /**
     * Read the most recent backup bundle from the shell-managed location.
     * Returns `null` if no backup has ever been saved.
     */
    getCurrentBackup(): Promise<Uint8Array | null>;

    /**
     * Atomically overwrite the shell-managed current backup. Passing an empty
     * array clears the file.
     */
    saveCurrentBackup(bytes: Uint8Array): Promise<void>;

    /**
     * Start a native QR scan. The command opens the camera and begins
     * streaming JPEG frames as `qr://frame` events (payload: number[]).
     * When a QR code is decoded a `qr://result` event fires with the text.
     * If an error occurs a `qr://error` event fires with the message.
     * Call `stopQrScan()` to close the camera early.
     */
    startQrScan(): Promise<void>;

    /** Stop an in-progress QR scan and close the camera. */
    stopQrScan(): Promise<void>;
}

function invoke<T>(cmd: string, payload?: Record<string, unknown>): Promise<T> {
    const internals = globalThis.__TAURI_INTERNALS__;
    if (!internals?.invoke) {
        throw new Error(`tauriBridge: not running inside Tauri (invoke "${cmd}" unavailable)`);
    }
    return internals.invoke(cmd, payload) as Promise<T>;
}

function hasTauriGlobals(): boolean {
    return (
        typeof globalThis !== 'undefined' &&
        (Boolean(globalThis.__TAURI_INTERNALS__?.invoke) || Boolean(globalThis.__TAURI__))
    );
}

/** Concrete Tauri implementation. Each method is a thin wrapper around `invoke`. */
const tauriImpl: TauriBridge = {
    isTauri: () => true,
    runtime: () => 'tauri',

    async startupState() {
        return invoke<StartupState>('startup_state');
    },

    async getAppWebsocketAuth() {
        return invoke<{ port: number; token: number[] }>('get_app_websocket_auth');
    },

    async dumpSourceChain(cellId) {
        return invoke<HolochainRecord[]>('dump_source_chain', { cellId });
    },

    async graftSourceChain(cellId, records) {
        await invoke<void>('graft_source_chain', { cellId, records });
    },

    async importLairSeed(seed) {
        if (seed.length !== 32) {
            throw new Error(`importLairSeed: seed must be 32 bytes, got ${seed.length}`);
        }
        return invoke<AgentPubKey>('import_lair_seed', { seed: Array.from(seed) });
    },

    async installAppWithAgentKey(agentPubKey) {
        await invoke<void>('install_app_with_agent_key', { agentPubKey });
    },

    async exportBackupFile(bytes, suggestedName) {
        return invoke<string | null>('export_backup_file', {
            bytes: Array.from(bytes),
            suggestedName,
        });
    },

    async importBackupFile() {
        const arr = await invoke<number[] | null>('import_backup_file');
        return arr ? Uint8Array.from(arr) : null;
    },

    async getCurrentBackup() {
        const arr = await invoke<number[] | null>('get_current_backup');
        return arr ? Uint8Array.from(arr) : null;
    },

    async saveCurrentBackup(bytes) {
        await invoke<void>('save_current_backup', { bytes: Array.from(bytes) });
    },

    async startQrScan() {
        await invoke<void>('start_qr_scan');
    },

    async stopQrScan() {
        await invoke<void>('stop_qr_scan');
    },
};

/**
 * Browser / hc-spin mock. The purpose is to exercise the UI code paths
 * without pretending the underlying Holochain operations have succeeded:
 * each method either returns a useful approximation or logs a warning so
 * the developer knows a real conductor action was skipped.
 */
const mockImpl: TauriBridge = {
    isTauri: () => false,
    runtime: () => 'browser',

    async startupState() {
        // In dev we treat every launch as "installed": hc-spin has already
        // provisioned the agent and the UI proceeds to normal boot.
        return { kind: 'installed', app_id: 'edet' } as StartupState;
    },

    async getAppWebsocketAuth(): Promise<{ port: number; token: number[] }> {
        // Not meaningful in the browser/hc-spin path — callers should branch
        // on isTauri() and use plain AppWebsocket.connect() instead.
        throw new Error('getAppWebsocketAuth: unavailable outside the Tauri shell');
    },

    async dumpSourceChain() {
        console.warn('[tauriBridge:mock] dumpSourceChain is approximate in dev mode');
        return [];
    },

    async graftSourceChain(_cellId, records) {
        console.warn(
            `[tauriBridge:mock] graftSourceChain skipped (${records.length} records) — only the Tauri shell can graft`,
        );
    },

    async importLairSeed() {
        // In dev we cannot re-seed lair; the conductor has already generated
        // its own agent key. The caller should fall back to the live pubkey.
        throw new Error('importLairSeed: unavailable outside the Tauri shell');
    },

    async installAppWithAgentKey() {
        throw new Error('installAppWithAgentKey: unavailable outside the Tauri shell');
    },

    async exportBackupFile(bytes, suggestedName) {
        // Browser-friendly download fallback. The cast silences a newer-
        // TypeScript-lib `Uint8Array<ArrayBufferLike>` vs `BlobPart` mismatch
        // that does not affect runtime behaviour.
        const blob = new Blob([bytes as BlobPart], { type: 'application/octet-stream' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = suggestedName;
        document.body.appendChild(a);
        a.click();
        a.remove();
        URL.revokeObjectURL(url);
        return suggestedName;
    },

    async importBackupFile() {
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
                resolve(new Uint8Array(await file.arrayBuffer()));
            };
            input.oncancel = () => resolve(null);
            input.click();
        });
    },

    // Browser path keeps the "current backup" in IndexedDB via backupStorage;
    // here we return null so that backupStorage falls through to its own
    // IndexedDB-backed read.
    async getCurrentBackup() {
        return null;
    },

    async saveCurrentBackup() {
        // No-op: backupStorage handles IndexedDB writes directly in dev.
    },

    async startQrScan(): Promise<void> {
        throw new Error('startQrScan: unavailable outside the Tauri shell');
    },

    async stopQrScan(): Promise<void> {
        // no-op in browser
    },
};

export const tauriBridge: TauriBridge = hasTauriGlobals() ? tauriImpl : mockImpl;
