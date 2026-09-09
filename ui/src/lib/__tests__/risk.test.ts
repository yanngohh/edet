/**
 * The client's risk score must mirror `crates/kernel/src/risk.rs` term for
 * term. These fixtures are the same ones the kernel asserts, so the two cannot
 * drift — and drift here is not cosmetic: the wallet's acceptance policy reads
 * this score to decide what to sign without asking, so a client scoring
 * differently from the kernel automates a decision the protocol never made.
 */
import { describe, expect, it } from 'vitest';

import { confidence, headroom, lambdaB, memberRisk, riskOf, riskScore, V_BASE_FALLBACK } from '../risk';

const K = 0.75;
const V = V_BASE_FALLBACK;

describe('the score mirrors the kernel', () => {
    it('gives maximum risk to an account nobody has backed', () => {
        expect(memberRisk({ capacity: 0, debt: 0, d_in: 0, d_out: 0 }, K, V)).toBe(1);
    });

    /// The property the client's cold-start rule exists for: the score cannot
    /// tell "nothing is known" from "everything is spoken for".
    it('cannot tell a fresh key from a fully drawn ceiling', () => {
        const fresh = memberRisk({ capacity: 0, debt: 0, d_in: 0, d_out: 0 }, K, V);
        const drawn = memberRisk({ capacity: 5000, debt: 5000, d_in: 0, d_out: 0 }, K, V);
        expect(fresh).toBe(drawn);
    });

    it('scores a well-backed, unencumbered member low', () => {
        expect(memberRisk({ capacity: 50_000, debt: 0, d_in: 0, d_out: 0 }, K, V)).toBeLessThan(0.05);
    });

    it('lowers risk as backing rises — capacity is what confidence reads', () => {
        const modest = memberRisk({ capacity: 2_500, debt: 0, d_in: 0, d_out: 0 }, K, V);
        const ample = memberRisk({ capacity: 25_000, debt: 0, d_in: 0, d_out: 0 }, K, V);
        expect(ample).toBeLessThan(modest);
    });

    /// A default does not release the flow it committed, so the defaulter's
    /// capacity stays consumed and the score follows with no term of its own.
    it('raises risk for an unreleased default without a term for it', () => {
        const clear = memberRisk({ capacity: 2_500, debt: 0, d_in: 0, d_out: 0 }, K, V);
        const defaulted = memberRisk({ capacity: 2_500, debt: 2_400, d_in: 0, d_out: 0 }, K, V);
        expect(defaulted).toBeGreaterThan(clear);
    });

    it('discounts a pure accumulator', () => {
        expect(lambdaB(100, 0)).toBe(0.5);
        expect(lambdaB(100, 100)).toBe(1);
        expect(lambdaB(0, 0)).toBe(1);
    });

    /// The scale is K·V_base, so a re-denomination leaves every score
    /// unchanged. A bare Cap/(Cap+K) would re-price the whole community's risk
    /// the first time the unit changed — silently, as a change of unit.
    it('survives a re-denomination unchanged', () => {
        const pi = 100;
        const before = memberRisk({ capacity: 2_500, debt: 500, d_in: 10, d_out: 5 }, K, V);
        const after = memberRisk(
            { capacity: 2_500 * pi, debt: 500 * pi, d_in: 10 * pi, d_out: 5 * pi },
            K,
            V * pi,
        );
        expect(after).toBeCloseTo(before, 12);
    });
});

describe('the pieces', () => {
    it('clamps confidence and returns zero without capacity', () => {
        expect(confidence(0, K, V)).toBe(0);
        expect(confidence(-5, K, V)).toBe(0);
        expect(confidence(1e12, K, V)).toBeLessThanOrEqual(1);
    });

    it('clamps headroom and returns zero without capacity', () => {
        expect(headroom(0, 0)).toBe(0);
        expect(headroom(100, 150)).toBe(0);
        expect(headroom(100, 25)).toBeCloseTo(0.75, 12);
    });

    it('clamps the assembled score into [0, 1]', () => {
        expect(riskScore(2, 2, 2)).toBe(0);
        expect(riskScore(-1, -1, -1)).toBe(1);
    });
});

describe('riskOf', () => {
    /// Authenticated reads: an unauthenticated caller gets NO risk
    /// fields at all. A missing input must read as "unknown" and never as a
    /// manufactured zero-risk score, or an anonymous read would score every
    /// stranger as safe.
    it('returns null when the row omits its risk fields', () => {
        expect(riskOf({ id: 1, capacity: 10, debt: 0 } as never, null)).toBeNull();
        expect(riskOf(null, null)).toBeNull();
        expect(riskOf(undefined, null)).toBeNull();
    });

    it('scores a complete row', () => {
        const r = riskOf({ id: 1, capacity: 2_500, debt: 0, d_in: 0, d_out: 0 } as never, null);
        expect(r).not.toBeNull();
        expect(r as number).toBeGreaterThan(0);
        expect(r as number).toBeLessThan(1);
    });
});
