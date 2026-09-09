/**
 * The wallet's acceptance rule — the client policy the paper leaves to the
 * client (§ Risk and acceptance): an incoming signature request scoring at
 * or below `accept` is signed automatically, at or above `reject` is
 * declined automatically, and anything in between is held for the member to
 * decide in the Requests inbox.
 *
 * The thresholds live on this device only. They are policy, not ledger
 * state: no one else can read or enforce them, and the signature they
 * produce is made here, by the seed in this device's vault. Like the locale
 * and theme they are a device preference, so they are not part of the
 * encrypted identity backup (which carries only what cannot be recreated).
 */

import { writable } from 'svelte/store';

import { lsGet, lsSet } from '../common/safeStorage';

const KEY = 'edet-acceptance-policy';

export interface AcceptancePolicy {
    /** Master switch: with it off, every request waits for a human. */
    auto: boolean;
    /** R ≤ accept → sign automatically. */
    accept: number;
    /** R ≥ reject → decline automatically. */
    reject: number;
    /**
     * Largest amount this device will sign without being asked. A request
     * above it is HELD, whatever it scores.
     *
     * The score prices the counterparty; it says nothing about the size of
     * what is being granted. Without a ceiling the only bound on an automatic
     * signature was the counterparty's own ledger capacity — so a wallet could
     * extend its whole willingness to one buyer in a single request that the
     * member never saw.
     */
    maxAmount: number;
    /**
     * Keep applying the rule while the app is in the background.
     *
     * The rule signs in the WebView, with the seed this device holds, so it
     * decides for exactly as long as that page is running. On Android the
     * platform pauses a backgrounded WebView, and on the desktop closing the
     * window ends the process; with this on, the client re-resumes the page
     * behind a foreground service (Android) and hides to the tray instead of
     * closing (desktop). It never reaches past the app: a killed process, a
     * locked vault or a device that is off signs nothing, whatever this says.
     *
     * Separate from `auto` because it is a separate decision — one about how
     * much of the device's battery and attention the rule may take — and it
     * has no meaning without `auto`, which is why `armed()` reads both.
     */
    background: boolean;
}

/**
 * Paper thresholds: R ≤ 0.40 accept, R ≥ 0.80 reject, hold band between.
 *
 * `auto` is OFF. Automation signs financial obligations on the member's
 * behalf, and that is a decision to make rather than one to discover having
 * been made: on-by-default meant a wallet began signing the moment it was
 * created, before its owner had seen a single request. `maxAmount` is 0 until
 * the member sets one, which the acceptance engine reads as "nothing" rather
 * than "no limit" — the safe direction for an unset value. `background` is OFF
 * for the same reason and one of its own: it spends the device's battery and
 * puts a permanent notification in the member's shade, and neither is
 * something to discover having been chosen.
 */
export const DEFAULT_POLICY: AcceptancePolicy = {
    auto: false,
    accept: 0.4,
    reject: 0.8,
    maxAmount: 0,
    background: false,
};

/** Clamp to a rule that is meaningful: 0 ≤ accept ≤ reject ≤ 1. A collapsed
 *  band (accept == reject) would leave no room to review anything, so the
 *  thresholds are separated by at least one slider step. */
export function normalizePolicy(p: Partial<AcceptancePolicy> | null | undefined): AcceptancePolicy {
    const num = (v: unknown, fallback: number): number =>
        typeof v === 'number' && Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : fallback;
    let accept = num(p?.accept, DEFAULT_POLICY.accept);
    let reject = num(p?.reject, DEFAULT_POLICY.reject);
    if (accept > reject) [accept, reject] = [reject, accept];
    // `auto` and `maxAmount` both fall back to their DEFAULT when absent or
    // malformed, rather than to the permissive reading. A stored policy from
    // an older build has neither field, and inheriting "on, unlimited" from
    // silence is exactly the failure this pair exists to prevent.
    const auto = p?.auto === true;
    // Same reading, for the same reason: a stored policy written before this
    // field existed must not inherit "and keep running in the background"
    // from its silence.
    const background = p?.background === true;
    const maxAmount =
        typeof p?.maxAmount === 'number' && Number.isFinite(p.maxAmount) && p.maxAmount >= 0
            ? p.maxAmount
            : DEFAULT_POLICY.maxAmount;
    return { auto, accept, reject, maxAmount, background };
}

function load(): AcceptancePolicy {
    const raw = lsGet(KEY);
    if (!raw) return { ...DEFAULT_POLICY };
    try {
        return normalizePolicy(JSON.parse(raw));
    } catch {
        return { ...DEFAULT_POLICY };
    }
}

export const acceptancePolicy = writable<AcceptancePolicy>(load());

acceptancePolicy.subscribe((p) => lsSet(KEY, JSON.stringify(p)));

/** Persist a new rule (normalized). */
export function setPolicy(p: Partial<AcceptancePolicy>): AcceptancePolicy {
    const next = normalizePolicy(p);
    acceptancePolicy.set(next);
    return next;
}

export function resetPolicy(): AcceptancePolicy {
    return setPolicy(DEFAULT_POLICY);
}

export type Band = 'accept' | 'hold' | 'reject';

/**
 * Which band a score falls in. Both comparisons are inclusive, so a score
 * exactly on a threshold maps to the named decision rather than silently
 * landing in the hold band — "accept at or below 40% risk" means what it
 * says.
 */
export function classify(r: number, p: AcceptancePolicy): Band {
    if (r <= p.accept) return 'accept';
    if (r >= p.reject) return 'reject';
    return 'hold';
}
