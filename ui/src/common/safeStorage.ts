/**
 * Safe wrappers around the `localStorage` API.
 *
 * Private-browsing modes, disabled cookies, embedded WebViews, or quota
 * exhaustion can all cause `localStorage.getItem` / `setItem` to throw. We
 * wrap the access so callers don't have to try/catch at every site and so
 * the app degrades to a "per-session" behaviour rather than crashing.
 *
 * The first access probes availability once; subsequent calls are
 * zero-overhead. Callers that actually need durable storage should use
 * `backupStorage` / `deviceKey` (IndexedDB via localforage), which has its
 * own, sturdier in-memory fallback.
 */

let cachedAvailable: boolean | null = null;

/**
 * Return `true` iff `localStorage` is usable for both read and write.
 * Result is memoised after the first call; `resetLocalStorageProbe()` can
 * be used in tests to force a re-check.
 */
export function isLocalStorageAvailable(): boolean {
    if (cachedAvailable !== null) return cachedAvailable;
    try {
        if (typeof window === 'undefined') {
            cachedAvailable = false;
            return false;
        }
        const probe = '__edet_probe__';
        window.localStorage.setItem(probe, '1');
        window.localStorage.removeItem(probe);
        cachedAvailable = true;
    } catch {
        cachedAvailable = false;
    }
    return cachedAvailable;
}

/** Test-only: clear the memoised availability so a fresh probe runs. */
export function resetLocalStorageProbe(): void {
    cachedAvailable = null;
}

/** Read `key` from localStorage, returning `null` on any error or absence. */
export function lsGet(key: string): string | null {
    if (!isLocalStorageAvailable()) return null;
    try {
        return window.localStorage.getItem(key);
    } catch {
        return null;
    }
}

/** Write `value` under `key`, silently no-op when storage is unavailable. */
export function lsSet(key: string, value: string): void {
    if (!isLocalStorageAvailable()) return;
    try {
        window.localStorage.setItem(key, value);
    } catch {
        // QuotaExceeded or similar. Best-effort only; callers should never
        // rely on lsSet as a durable persistence layer.
    }
}

/** Remove `key`, silently no-op when storage is unavailable. */
export function lsRemove(key: string): void {
    if (!isLocalStorageAvailable()) return;
    try {
        window.localStorage.removeItem(key);
    } catch {
        // See lsSet: best-effort.
    }
}
