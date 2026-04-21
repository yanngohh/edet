/**
 * Shared connection-state store.
 *
 * App.svelte owns the actual connect / reconnect lifecycle; this module
 * exposes its state as a tiny writable so other components (onboarding
 * steps, banners) can react to connectivity changes without needing to
 * receive props from the root.
 *
 * The state has three fields:
 *   - `connected`: `true` once we have a live AppWebsocket.
 *   - `lost`: `true` when the connection dropped after at least one
 *     successful connect. Drives the red "Connection lost" banner.
 *   - `reconnecting`: `true` while the exponential-backoff retry is in
 *     flight. Used to flip the banner's icon / message.
 */

import { writable } from 'svelte/store';

export interface ConnectionState {
    connected: boolean;
    lost: boolean;
    reconnecting: boolean;
}

const initial: ConnectionState = {
    connected: false,
    lost: false,
    reconnecting: false,
};

export const connectionState = writable<ConnectionState>(initial);

export function markConnected(): void {
    connectionState.set({ connected: true, lost: false, reconnecting: false });
}

export function markLost(): void {
    connectionState.set({ connected: false, lost: true, reconnecting: false });
}

export function markReconnecting(): void {
    connectionState.update((s) => ({ ...s, reconnecting: true }));
}

export function markRestored(): void {
    connectionState.set({ connected: true, lost: false, reconnecting: false });
}
