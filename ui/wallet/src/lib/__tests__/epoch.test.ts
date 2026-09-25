/**
 * An epoch, as the day it is.
 *
 * The conversion is exact — an epoch is `unix_secs / EPOCH_SECS`, so epoch N
 * begins at `N × EPOCH_SECS` UTC — which is the whole reason the client may
 * show a date at all rather than an estimate. What can go wrong is the LENGTH:
 * hardcode it and a client goes on dating every maturity against a clock the
 * chain no longer runs, silently and plausibly. So the node's own
 * `epoch_secs` is what the conversion uses, and the constant here is only for
 * the window before the first poll has answered.
 */
import { describe, expect, it } from 'vitest';

import { EPOCH_SECS_FALLBACK, epochSecs, epochStartMs, formatEpoch } from '../epoch';
import type { NetworkView } from '../api';

const net = (over: Partial<NetworkView> = {}): NetworkView => ({ epoch_secs: 86_400, ...over }) as NetworkView;

describe('the epoch clock', () => {
    it('is the kernel constant, and one day', () => {
        expect(EPOCH_SECS_FALLBACK).toBe(86_400);
    });

    /**
     * **The length is the node's answer, not a constant in the client.** A
     * chain whose epoch is an hour must date against an hour: reading the
     * fallback instead would put every maturity 24× too far out, with nothing
     * on screen to say so.
     */
    it('reads its length from the node', () => {
        expect(epochSecs(net({ epoch_secs: 3_600 }))).toBe(3_600);
        expect(epochStartMs(2, 3_600)).toBe(2 * 3_600 * 1000);
    });

    it('falls back only where the node has not answered, or answered nonsense', () => {
        expect(epochSecs(null)).toBe(EPOCH_SECS_FALLBACK);
        expect(epochSecs(net({ epoch_secs: 0 }))).toBe(EPOCH_SECS_FALLBACK);
        expect(epochSecs(net({ epoch_secs: -1 }))).toBe(EPOCH_SECS_FALLBACK);
        expect(epochSecs(net({ epoch_secs: NaN }))).toBe(EPOCH_SECS_FALLBACK);
        expect(epochSecs(net({ epoch_secs: 'day' as unknown as number }))).toBe(EPOCH_SECS_FALLBACK);
    });

    /**
     * An epoch is ABSOLUTE — days since the Unix epoch — so the day it names
     * is arithmetic and not a guess. Asserted against the same arithmetic in
     * UTC rather than a written-out date, because the formatter renders in the
     * member's own timezone and format preference.
     */
    it('names the day the epoch begins', () => {
        const epoch = 20_704;
        const iso = new Date(epoch * 86_400 * 1000).toISOString().slice(0, 10);
        const [y, m, d] = iso.split('-');
        const shown = formatEpoch(epoch, net());
        for (const part of [y, m, d]) expect(shown).toContain(part);
    });

    /**
     * A number that is not an epoch comes back as itself. Rendering it as a
     * plausible date would have the summary describing a different obligation
     * than the one on screen — the failure a fallback date cannot be seen to
     * have made.
     */
    it('does not invent a date for what is not an epoch', () => {
        expect(formatEpoch(NaN, net())).toBe('NaN');
        expect(formatEpoch(-1, net())).toBe('-1');
        expect(formatEpoch(Infinity, net())).toBe('Infinity');
    });
});
