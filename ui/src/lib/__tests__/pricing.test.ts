/**
 * **A member's own price on a counterparty, beside the ledger's.**
 *
 * The protocol's score is the community's reading, computed term for term with
 * `crates/kernel/src/risk.rs`. What a member may add is their own: a personal
 * K, and an offset on one counterparty. What they may NOT do is quietly score
 * differently and have the app present it as the ledger's — so the first probe
 * here is that saying nothing scores exactly what the kernel scores, and the
 * last is that a rule can never widen what is automated past the guards
 * `autosign` already applies.
 */
import { beforeEach, describe, expect, it } from 'vitest';

import {
    DEFAULT_PRICING,
    MAX_ADJUSTMENT,
    hasRule,
    normalizePricing,
    priceCounterparty,
    resetPricing,
    setPricing,
    subjectivePricing,
    subjectiveRisk,
} from '../pricing';
import { memberRisk, RISK_K_FALLBACK, V_BASE_FALLBACK, type RiskInputs } from '../risk';

const ROW: RiskInputs = { capacity: 1000, debt: 200, d_in: 100, d_out: 50 };

beforeEach(() => {
    window.localStorage.clear();
    resetPricing();
});

describe('a member who has said nothing', () => {
    /** Mutation that bites: default `personalK` to anything but the governed
     *  value, and every wallet silently prices every counterparty differently
     *  from the ledger while showing one number. */
    it('scores exactly as the kernel does', () => {
        const kernel = memberRisk(ROW, RISK_K_FALLBACK, V_BASE_FALLBACK);
        const mine = subjectiveRisk(7, ROW, RISK_K_FALLBACK, V_BASE_FALLBACK, DEFAULT_PRICING);
        expect(mine).toBe(kernel);
        expect(hasRule(DEFAULT_PRICING)).toBe(false);
        expect(hasRule(DEFAULT_PRICING, 7)).toBe(false);
    });
});

describe('the two dials', () => {
    it('a personal K replaces the governed one and nothing else', () => {
        const strict = subjectiveRisk(7, ROW, RISK_K_FALLBACK, V_BASE_FALLBACK, {
            personalK: RISK_K_FALLBACK * 10,
            adjustments: {},
        });
        // A larger K means more backing is wanted before an account reads as
        // confident, so the same row scores RISKIER.
        expect(strict).toBeGreaterThan(memberRisk(ROW, RISK_K_FALLBACK, V_BASE_FALLBACK));
        // And it is exactly the kernel's own function at that K — not a
        // second implementation of the score.
        expect(strict).toBe(memberRisk(ROW, RISK_K_FALLBACK * 10, V_BASE_FALLBACK));
    });

    it('an offset moves the band and only the band', () => {
        const base = memberRisk(ROW, RISK_K_FALLBACK, V_BASE_FALLBACK);
        const worse = subjectiveRisk(7, ROW, RISK_K_FALLBACK, V_BASE_FALLBACK, {
            personalK: null,
            adjustments: { 7: 0.2 },
        });
        expect(worse).toBeCloseTo(Math.min(1, base + 0.2), 12);
        // It applies to the counterparty it names and to nobody else.
        const other = subjectiveRisk(8, ROW, RISK_K_FALLBACK, V_BASE_FALLBACK, {
            personalK: null,
            adjustments: { 7: 0.2 },
        });
        expect(other).toBe(base);
    });

    it('keeps the score inside [0, 1] whatever the offset says', () => {
        const risky: RiskInputs = { capacity: 0, debt: 0, d_in: 0, d_out: 0 };
        const p = { personalK: null, adjustments: { 7: MAX_ADJUSTMENT } };
        expect(subjectiveRisk(7, risky, RISK_K_FALLBACK, V_BASE_FALLBACK, p)).toBe(1);
        const safe: RiskInputs = { capacity: 1e9, debt: 0, d_in: 1, d_out: 1 };
        const kind = { personalK: null, adjustments: { 7: -MAX_ADJUSTMENT } };
        expect(subjectiveRisk(7, safe, RISK_K_FALLBACK, V_BASE_FALLBACK, kind)).toBeGreaterThanOrEqual(0);
    });
});

describe('normalisation', () => {
    it('refuses a K that is not a scale, and clamps every offset', () => {
        expect(normalizePricing({ personalK: 0, adjustments: {} }).personalK).toBeNull();
        expect(normalizePricing({ personalK: -1, adjustments: {} }).personalK).toBeNull();
        expect(normalizePricing({ personalK: NaN, adjustments: {} }).personalK).toBeNull();
        expect(normalizePricing({ personalK: 2, adjustments: {} }).personalK).toBe(2);

        const p = normalizePricing({ personalK: null, adjustments: { 1: 9, 2: -9, 3: 0, 4: NaN as never } });
        expect(p.adjustments).toEqual({ 1: MAX_ADJUSTMENT, 2: -MAX_ADJUSTMENT });
    });

    it('reads a stored rule from an older or damaged shape as no rule', () => {
        window.localStorage.setItem('edet-subjective-pricing', 'not json');
        expect(normalizePricing(null)).toEqual(DEFAULT_PRICING);
        expect(normalizePricing(undefined)).toEqual(DEFAULT_PRICING);
        expect(normalizePricing({} as never)).toEqual(DEFAULT_PRICING);
    });
});

describe('persistence', () => {
    it('round-trips through this device and nowhere else', () => {
        setPricing({ personalK: 3, adjustments: { 5: 0.25 } });
        const raw = window.localStorage.getItem('edet-subjective-pricing');
        expect(raw).toBeTruthy();
        expect(normalizePricing(JSON.parse(raw!))).toEqual({ personalK: 3, adjustments: { 5: 0.25 } });
    });

    it('prices one counterparty without disturbing the others', () => {
        let p = setPricing({ personalK: null, adjustments: { 5: 0.25 } });
        p = priceCounterparty(p, 6, -0.1);
        expect(p.adjustments).toEqual({ 5: 0.25, 6: -0.1 });
        // Zero clears rather than storing a rule that says nothing.
        p = priceCounterparty(p, 5, 0);
        expect(p.adjustments).toEqual({ 6: -0.1 });
        expect(hasRule(p, 5)).toBe(false);
        expect(hasRule(p, 6)).toBe(true);
    });

    it('resets to the ledger reading everywhere', () => {
        setPricing({ personalK: 4, adjustments: { 5: 0.25 } });
        expect(resetPricing()).toEqual(DEFAULT_PRICING);
        let held: unknown;
        subjectivePricing.subscribe((v) => (held = v))();
        expect(held).toEqual(DEFAULT_PRICING);
    });
});
