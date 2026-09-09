import { describe, expect, it } from 'vitest';

import { appendShare, normalizeShares, rebalanceShares, removeShare } from '../waterfill';

const sum = (xs: number[]) => xs.reduce((a, b) => a + b, 0);

describe('normalizeShares', () => {
    it('turns raw weights into shares totalling exactly 1', () => {
        const s = normalizeShares([2, 1, 1]);
        expect(sum(s)).toBeCloseTo(1, 12);
        expect(s[0]).toBeCloseTo(0.5, 4);
    });

    it('splits equally when the total is zero', () => {
        expect(normalizeShares([0, 0])).toEqual([0.5, 0.5]);
        expect(normalizeShares([])).toEqual([]);
    });
});

describe('rebalanceShares', () => {
    it('keeps the sum at exactly 1 after a drag', () => {
        // User dragged slider 0 from 0.5 to 0.9: others must give way.
        const s = rebalanceShares([0.9, 0.25, 0.25], 0);
        expect(sum(s)).toBeCloseTo(1, 12);
        expect(s[1]).toBeLessThan(0.25);
        expect(s[2]).toBeLessThan(0.25);
    });

    it('never drives a share negative', () => {
        const s = rebalanceShares([1, 0.6, 0.6], 0);
        expect(sum(s)).toBeCloseTo(1, 12);
        for (const x of s) expect(x).toBeGreaterThanOrEqual(0);
    });

    it('a single entry is always 100%', () => {
        expect(rebalanceShares([0.3], 0)).toEqual([1]);
    });
});

describe('add/remove', () => {
    it('appendShare starts newcomers at 0% (first entry at 100%)', () => {
        expect(appendShare([])).toEqual([1]);
        const s = appendShare([0.6, 0.4]);
        expect(s).toHaveLength(3);
        expect(s[2]).toBe(0);
        expect(sum(s)).toBeCloseTo(1, 12);
    });

    it('removeShare hands the departed share out equally', () => {
        const s = removeShare([0.5, 0.3, 0.2], 0);
        expect(s).toHaveLength(2);
        expect(sum(s)).toBeCloseTo(1, 12);
        expect(s[0]).toBeCloseTo(0.55, 4);
        expect(s[1]).toBeCloseTo(0.45, 4);
        expect(removeShare([1], 0)).toEqual([]);
    });
});
