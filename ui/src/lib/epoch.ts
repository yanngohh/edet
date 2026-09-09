/**
 * The ledger's clock, as a date.
 *
 * **An epoch is an absolute day**, not a count from anywhere: it is
 * `unix_secs / EPOCH_SECS` (`kernel::constants`), so epoch N begins at
 * `N × EPOCH_SECS` UTC and the conversion is exact rather than an estimate.
 * That is what makes this worth doing — "due at epoch 20734" is a number
 * nobody can place, and it names one specific day.
 *
 * **The length comes from the node, not from a constant here.** It is served
 * on `/network` as `epoch_secs`, so a re-denomination of the clock cannot
 * leave the client quietly dating everything against the old one. The fallback
 * below is only for the window before the first poll has answered, and it
 * mirrors the kernel the way `risk.ts`'s do.
 *
 * Only ABSOLUTE epochs become dates. A maturity of "30 epochs" is a duration
 * the member types and the ledger reads back in the same unit, and turning
 * that into a date at one end of the form and not the other would be worse
 * than leaving it alone.
 */

import { derived } from 'svelte/store';

import { formatDate } from '../common/functions';
import { networkView } from './node';
import type { NetworkView } from './api';

/** `kernel::constants::EPOCH_SECS`, and only until `/network` has answered. */
export const EPOCH_SECS_FALLBACK = 86_400;

export function epochSecs(net: NetworkView | null): number {
    const v = net?.epoch_secs;
    return typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : EPOCH_SECS_FALLBACK;
}

/** The millisecond at which `epoch` begins. */
export function epochStartMs(epoch: number, secs: number): number {
    return epoch * secs * 1000;
}

/**
 * The day an epoch is, formatted to the member's own date preference.
 *
 * A number that cannot be read as an epoch comes back as itself: a summary
 * that silently rendered an unparseable maturity as a plausible date would be
 * describing a different obligation than the one on screen.
 */
export function formatEpoch(epoch: number, net: NetworkView | null): string {
    if (typeof epoch !== 'number' || !Number.isFinite(epoch) || epoch < 0) return String(epoch);
    return formatDate(epochStartMs(epoch, epochSecs(net)));
}

/** Reactive: `$epochDate(20734)` → the date that epoch begins. */
export const epochDate = derived(networkView, ($net) => (epoch: number): string => formatEpoch(epoch, $net));
