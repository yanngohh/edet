import { describe, expect, it, vi, beforeEach } from 'vitest';

import {
    __getSchedulerState,
    forceBackup,
    requestBackup,
    startBackupScheduler,
    stopBackupScheduler,
    type BackupTriggerReason,
} from '../backupScheduler';

// backupScheduler reads metadata through backupStorage → localforage. For
// unit tests we stub localforage so the scheduler initialises synchronously
// without a real IndexedDB. `writeCurrentBackup` is not invoked here.
vi.mock('localforage', () => {
    const store = new Map<string, unknown>();
    return {
        default: {
            createInstance: () => ({
                getItem: (k: string) => Promise.resolve(store.get(k) ?? null),
                setItem: (k: string, v: unknown) => {
                    store.set(k, v);
                    return Promise.resolve(v);
                },
                removeItem: (k: string) => {
                    store.delete(k);
                    return Promise.resolve();
                },
            }),
        },
    };
});

// tauriBridge reads globalThis; default to the browser-mode fallback so we
// never attempt real Tauri IPC from inside a test.
vi.mock('../tauriBridge', () => ({
    tauriBridge: {
        isTauri: () => false,
        runtime: () => 'browser',
        getCurrentBackup: () => Promise.resolve(null),
        saveCurrentBackup: () => Promise.resolve(),
    },
}));

describe('backupScheduler', () => {
    beforeEach(() => {
        stopBackupScheduler();
    });

    it('debounces multiple requests into a single handler call', async () => {
        vi.useFakeTimers();
        const reasons: BackupTriggerReason[] = [];
        const handler = vi.fn(async (reason: BackupTriggerReason) => {
            reasons.push(reason);
        });

        startBackupScheduler(handler);

        requestBackup('profile-updated');
        requestBackup('wallet-updated');
        requestBackup('contract-created');

        // Before the debounce window elapses, nothing should have fired.
        await vi.advanceTimersByTimeAsync(29_000);
        expect(handler).not.toHaveBeenCalled();

        // After the debounce window, exactly one call with the priority-merged reason.
        await vi.advanceTimersByTimeAsync(2_000);
        expect(handler).toHaveBeenCalledTimes(1);
        expect(reasons[0]).toBe('contract-created');

        stopBackupScheduler();
        vi.useRealTimers();
    });

    it('forceBackup runs immediately regardless of debounce window', async () => {
        vi.useFakeTimers();
        const handler = vi.fn(async (_reason: BackupTriggerReason) => { /* no-op */ });
        startBackupScheduler(handler);

        requestBackup('profile-updated');
        await forceBackup('user-requested');

        // Handler fired once with `user-requested` (the priority-promoted reason).
        expect(handler).toHaveBeenCalledTimes(1);
        expect(handler).toHaveBeenCalledWith('user-requested');

        stopBackupScheduler();
        vi.useRealTimers();
    });

    it('stopBackupScheduler clears pending work', async () => {
        vi.useFakeTimers();
        const handler = vi.fn();
        startBackupScheduler(handler);
        requestBackup('profile-updated');
        stopBackupScheduler();
        await vi.advanceTimersByTimeAsync(40_000);
        expect(handler).not.toHaveBeenCalled();
        expect(__getSchedulerState().running).toBe(false);
        vi.useRealTimers();
    });
});
