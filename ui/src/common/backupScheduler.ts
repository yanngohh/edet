/**
 * Orchestrates when the encrypted identity backup bundle is (re-)generated.
 *
 * Rules (per §12 of PLAN.md):
 *
 *  - Debounce window: 30 s. Multiple triggers within the window collapse
 *    into a single backup write, so a flurry of profile edits doesn't cause
 *    a flurry of disk writes.
 *  - Daily scheduled trigger: if more than 24 h have elapsed since the last
 *    successful backup we re-run at app open and every hour while the app is
 *    running.
 *  - Explicit triggers: profile change, wallet change, new debtor contract,
 *    and the user-invoked "export now" button.
 *
 * The scheduler does NOT produce the bundle itself — it only decides when to
 * ask the caller for a fresh dump + encrypted write. The actual encryption
 * and persistence live in the onboarding / settings code paths.
 */

import { readBackupMetadata } from './backupStorage';

/** Reasons a backup was requested. Kept small and enumerable for log lines. */
export type BackupTriggerReason =
    | 'initial'
    | 'daily'
    | 'profile-updated'
    | 'wallet-updated'
    | 'contract-created'
    | 'user-requested';

export type BackupHandler = (reason: BackupTriggerReason) => Promise<void>;

const DEBOUNCE_MS = 30_000;
const DAILY_MS = 24 * 60 * 60 * 1_000;
const DAILY_CHECK_INTERVAL_MS = 60 * 60 * 1_000; // re-check every hour while running

interface SchedulerState {
    handler: BackupHandler | null;
    pendingReason: BackupTriggerReason | null;
    debounceTimer: ReturnType<typeof setTimeout> | null;
    dailyTimer: ReturnType<typeof setInterval> | null;
    lastWriteAt: number | null;
    running: boolean;
}

const state: SchedulerState = {
    handler: null,
    pendingReason: null,
    debounceTimer: null,
    dailyTimer: null,
    lastWriteAt: null,
    running: false,
};

/** Bind the scheduler to a concrete bundle producer. Idempotent; clears any previous handler. */
export function startBackupScheduler(handler: BackupHandler): void {
    stopBackupScheduler();
    state.handler = handler;
    state.running = true;

    // Seed `lastWriteAt` from on-disk metadata so a fresh app launch with a
    // recent backup doesn't immediately trigger "daily".
    void readBackupMetadata().then((meta) => {
        if (meta?.createdAt) state.lastWriteAt = meta.createdAt;
        evaluateDaily();
    });

    state.dailyTimer = setInterval(evaluateDaily, DAILY_CHECK_INTERVAL_MS);
}

/** Release all timers. Safe to call when the scheduler is not running. */
export function stopBackupScheduler(): void {
    state.running = false;
    state.handler = null;
    state.pendingReason = null;
    if (state.debounceTimer !== null) {
        clearTimeout(state.debounceTimer);
        state.debounceTimer = null;
    }
    if (state.dailyTimer !== null) {
        clearInterval(state.dailyTimer);
        state.dailyTimer = null;
    }
}

/** Enqueue a backup for the given reason; triggers the handler after DEBOUNCE_MS of quiet. */
export function requestBackup(reason: BackupTriggerReason): void {
    if (!state.running || !state.handler) return;

    // Preserve the highest-priority reason seen during a debounce window, so
    // if both a "profile-updated" and a "user-requested" land the resulting
    // log line reflects the user's explicit action.
    state.pendingReason = mergeReason(state.pendingReason, reason);

    if (state.debounceTimer !== null) clearTimeout(state.debounceTimer);
    state.debounceTimer = setTimeout(flush, DEBOUNCE_MS);
}

/** Flush the pending backup immediately, skipping the debounce window. */
export async function forceBackup(reason: BackupTriggerReason = 'user-requested'): Promise<void> {
    if (!state.running || !state.handler) return;
    if (state.debounceTimer !== null) {
        clearTimeout(state.debounceTimer);
        state.debounceTimer = null;
    }
    state.pendingReason = reason;
    await flush();
}

async function flush() {
    if (!state.handler || !state.pendingReason) return;
    const reason = state.pendingReason;
    state.pendingReason = null;
    state.debounceTimer = null;
    try {
        await state.handler(reason);
        state.lastWriteAt = Date.now();
    } catch (e) {
        console.error('[backupScheduler] handler failed', { reason, error: e });
        // Requeue so we retry on the next trigger, but don't retry in a hot
        // loop: the next tick will debounce again.
        state.pendingReason = reason;
    }
}

function evaluateDaily() {
    if (!state.running) return;
    // The "daily" cadence is relative to the last successful backup. Before
    // onboarding finishes there is no `lastWriteAt`, and we do NOT want the
    // scheduler to fire in that window — onboarding is producing the first
    // backup itself.
    const last = state.lastWriteAt;
    if (last === null) return;
    if (Date.now() - last > DAILY_MS) requestBackup('daily');
}

// Priority: user > daily > initial > contract > wallet > profile.
// (User-initiated actions win; passive triggers yield to richer reasons.)
const PRIORITY: Record<BackupTriggerReason, number> = {
    'user-requested': 5,
    daily: 4,
    initial: 3,
    'contract-created': 2,
    'wallet-updated': 1,
    'profile-updated': 0,
};

function mergeReason(
    current: BackupTriggerReason | null,
    next: BackupTriggerReason,
): BackupTriggerReason {
    if (current === null) return next;
    return PRIORITY[next] >= PRIORITY[current] ? next : current;
}

/** Exposed for tests / debugging. Not part of the stable API. */
export function __getSchedulerState(): Readonly<SchedulerState> {
    return state;
}
