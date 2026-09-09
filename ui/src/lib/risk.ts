/**
 * The advisory risk score, client side.
 *
 *   confidence = Cap / (Cap + K·V_base)
 *   headroom   = max(0, (Cap − Debt) / Cap)
 *   λ_b        = ½ + ½·min(1, D_out / D_in)
 *   R          = 1 − confidence · headroom · λ_b
 *
 * Mirrors `crates/kernel/src/risk.rs` term for term (the tests pin the same
 * fixtures the kernel asserts, so the two cannot drift). Consensus enforces
 * the cut bound; this score is advisory — it is what the wallet's acceptance
 * policy reads to decide accept / hold / reject on an incoming request, which
 * is exactly why a client scoring differently from the kernel would be
 * automating a decision the protocol never described.
 *
 * It reads only quantities this model produces. A trust mass `τ` and a
 * governor brake `g`, and both went with the
 * trust-fixed-point machinery — leaving a score whose inputs the node no
 * longer serves, so every request scored `NaN` and the wallet held everything.
 * Fail-safe, and still broken.
 *
 * Capacity carries what `τ` was reaching for and carries it better, because it
 * is a cut rather than a fixed point: it cannot be raised by minting
 * identities or by trading with oneself. The brake needs no successor — it
 * throttled a model that no longer exists, and a cut is its own ceiling.
 */

import type { MemberSummary, ParamsView } from './api';

/**
 * Confidence: what the community has actually put behind this account, on a
 * scale set by `K·V_base` rather than by a bare constant — so it survives
 * re-denomination unchanged, where `Cap/(Cap+K)` would silently re-price every
 * score in the community the first time the unit changed.
 *
 * Zero capacity gives zero confidence, so a fresh key scores maximum risk.
 * That says the community has said NOTHING about this account — not that
 * something is known against it — which is why `decideBand` holds a cold start
 * for a human instead of auto-declining it.
 */
export function confidence(capacity: number, k: number, vBase: number): number {
    const scale = Math.max(0, k * vBase);
    if (!(capacity > 0)) return 0;
    const denom = capacity + scale;
    if (!(denom > 0)) return 0;
    return Math.min(1, Math.max(0, capacity / denom));
}

/** Free headroom as a fraction: max(0, (cap − debt)/cap), zero when cap is. */
export function headroom(cap: number, debt: number): number {
    if (!(cap > 0)) return 0;
    return Math.min(1, Math.max(0, (cap - debt) / cap));
}

/** Debt-velocity factor λ_b; 1 when nothing was acquired this window (a pure
 *  accumulator is discounted to ½). */
export function lambdaB(dIn: number, dOut: number): number {
    if (!(dIn > 0)) return 1;
    return 0.5 + 0.5 * Math.min(1, dOut / dIn);
}

/** R = 1 − confidence · headroom · λ_b, clamped to [0, 1].
 *
 *  The headroom argument was called `brakedHeadroom` while a community-wide
 *  governor multiplied it. Nothing brakes it now — a cut is its own ceiling —
 *  so it is what is left of the member's own capacity and nothing else. */
export function riskScore(conf: number, headroom: number, lambda: number): number {
    const c = Math.min(1, Math.max(0, conf));
    const h = Math.min(1, Math.max(0, headroom));
    const l = Math.min(1, Math.max(0, lambda));
    return Math.min(1, Math.max(0, 1 - c * h * l));
}

/** The risk inputs a member row carries (all served by /members). */
export interface RiskInputs {
    capacity: number;
    debt: number;
    d_in: number;
    d_out: number;
}

/** Score a member as the party who would take on the debt. */
export function memberRisk(m: RiskInputs, riskK: number, vBase: number): number {
    return riskScore(confidence(m.capacity, riskK, vBase), headroom(m.capacity, m.debt), lambdaB(m.d_in, m.d_out));
}

/** K, the reference constant of the score — a governed parameter. The
 *  fallback mirrors the kernel genesis value (kernel::constants::RISK_K) and
 *  is used only until /params has answered once. */
export const RISK_K_FALLBACK = 0.75;
/** V_base, the denomination the score's scale is expressed in. Mirrors
 *  `kernel::constants::BASE_CAPACITY`, and likewise only a fallback. */
export const V_BASE_FALLBACK = 1000;

export function riskK(params: ParamsView | null): number {
    const v = params?.governed.find((p) => p.key === 'RiskK')?.value;
    return typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : RISK_K_FALLBACK;
}

export function vBase(params: ParamsView | null): number {
    const v = params?.v_base;
    return typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : V_BASE_FALLBACK;
}

/** Convenience: score a member row straight off the members list. Returns
 *  null both when there is no row AND when the row's risk fields are
 *  absent (an anonymous/unauthenticated read
 *  omits them entirely) — a missing input must read as "unknown", never as
 *  a manufactured zero-risk score. */
export function riskOf(m: MemberSummary | null | undefined, params: ParamsView | null): number | null {
    if (!m) return null;
    if (
        m.capacity === undefined ||
        m.debt === undefined ||
        m.d_in === undefined ||
        m.d_out === undefined
    ) {
        return null;
    }
    return memberRisk(m as RiskInputs, riskK(params), vBase(params));
}
