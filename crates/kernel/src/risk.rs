//! The advisory risk score. Consensus enforces the cut bound; this score only
//! drives client accept / hold / reject presentation.
//!
//! It is mirrored term for term by `ui/src/lib/risk.ts`, and the two must not
//! drift: the wallet's acceptance policy reads it to decide what to sign
//! without asking, so a client scoring differently from the kernel would be
//! automating a decision the protocol never described.
//!
//! It reads only quantities this model produces. A score built on a trust mass
//! `τ` and a governor brake `g` reads inputs no node serves, so every request
//! scores `NaN` and the wallet holds everything — fail-safe, and still broken.
//!
//! What it reads instead:
//!
//! ```text
//! confidence = Cap / (Cap + K·V_base)      how much the community has said
//! headroom   = max(0, (Cap − Debt) / Cap)  how much of that is unspoken for
//! λ          = ½ + ½·min(1, D_out / D_in)  a pure accumulator is discounted
//! R          = 1 − confidence · headroom · λ
//! ```
//!
//! Capacity carries what `τ` was reaching for and carries it better, because
//! it is a cut rather than a fixed point: it cannot be raised by minting
//! identities or by trading with oneself. The brake needs no successor — it
//! was a community-wide throttle on a model that no longer has one, and a cut
//! is its own ceiling.
//!
//! An open default needs no term of its own either, and this is the part worth
//! noticing. A default does not release the flow it committed, so a defaulter's
//! capacity STAYS consumed by the debt they did not pay — `Cap − Debt` closes
//! toward zero on its own, and the score follows without being told.

/// Confidence: what the community has actually put behind this account, on a
/// scale set by `K·V_base` rather than by a bare constant.
///
/// `K` is dimensionless and governed, `V_base` is the denomination, so the
/// ratio survives re-denomination unchanged — which a bare `Cap/(Cap+K)` would
/// not, since it would silently re-price every score the first time `rescale`
/// ran.
///
/// Zero capacity gives zero confidence, so a fresh key scores maximum risk.
/// That is correct and must not be read as an accusation: it says the
/// community has said nothing about this account, which is exactly the
/// cold-start case a client should HOLD for a human rather than auto-decline.
pub fn confidence(capacity: f64, k: f64, v_base: f64) -> f64 {
    let scale = (k * v_base).max(0.0);
    if capacity <= 0.0 {
        return 0.0;
    }
    if capacity + scale <= 0.0 {
        return 0.0;
    }
    (capacity / (capacity + scale)).clamp(0.0, 1.0)
}

/// Free headroom as a fraction: `max(0, (cap − debt)/cap)`, zero when cap is.
pub fn headroom(cap: f64, debt: f64) -> f64 {
    if cap > 0.0 {
        ((cap - debt) / cap).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Debt-velocity factor λ_b = ½ + ½·min(1, out/in); 1 when nothing was
/// acquired this window (a pure accumulator is discounted to ½).
pub fn lambda_b(d_in: f64, d_out: f64) -> f64 {
    if d_in > 0.0 {
        0.5 + 0.5 * (d_out / d_in).min(1.0)
    } else {
        1.0
    }
}

/// R = 1 − confidence · headroom · λ_b, clamped to [0, 1].
pub fn risk_score(confidence: f64, headroom: f64, lambda: f64) -> f64 {
    (1.0 - confidence.clamp(0.0, 1.0) * headroom.clamp(0.0, 1.0) * lambda.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// The whole score for one member, from the figures a view serves.
pub fn member_risk(capacity: f64, debt: f64, d_in: f64, d_out: f64, k: f64, v_base: f64) -> f64 {
    risk_score(confidence(capacity, k, v_base), headroom(capacity, debt), lambda_b(d_in, d_out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{BASE_CAPACITY, RISK_K};

    #[test]
    fn no_capacity_means_maximum_risk() {
        assert_eq!(member_risk(0.0, 0.0, 0.0, 0.0, RISK_K, BASE_CAPACITY), 1.0);
    }

    /// And that is a statement about ignorance, not about evidence: it is the
    /// same score a well-backed member at their ceiling gets, which is why the
    /// client holds a cold start rather than declining it.
    #[test]
    fn a_fresh_key_and_a_drawn_ceiling_are_indistinguishable_to_the_score() {
        let fresh = member_risk(0.0, 0.0, 0.0, 0.0, RISK_K, BASE_CAPACITY);
        let drawn = member_risk(5000.0, 5000.0, 0.0, 0.0, RISK_K, BASE_CAPACITY);
        assert_eq!(fresh, drawn, "the score cannot tell them apart, so the client must not either");
    }

    #[test]
    fn well_backed_and_clear_scores_low() {
        let r = member_risk(50_000.0, 0.0, 0.0, 0.0, RISK_K, BASE_CAPACITY);
        assert!(r < 0.05, "r = {r}");
    }

    /// Capacity is what confidence reads, so being backed MORE lowers risk —
    /// carried by a cut, which cannot be manufactured.
    #[test]
    fn more_backing_lowers_risk() {
        let k = RISK_K;
        let modest = member_risk(2_500.0, 0.0, 0.0, 0.0, k, BASE_CAPACITY);
        let ample = member_risk(25_000.0, 0.0, 0.0, 0.0, k, BASE_CAPACITY);
        assert!(ample < modest, "{ample} vs {modest}");
    }

    /// A default does not release the flow it committed, so a defaulter's
    /// capacity stays consumed and the score follows with no term of its own.
    #[test]
    fn an_unreleased_default_raises_risk_without_a_term_for_it() {
        let clear = member_risk(2_500.0, 0.0, 0.0, 0.0, RISK_K, BASE_CAPACITY);
        let defaulted = member_risk(2_500.0, 2_400.0, 0.0, 0.0, RISK_K, BASE_CAPACITY);
        assert!(defaulted > clear);
    }

    #[test]
    fn accumulator_discount() {
        assert_eq!(lambda_b(100.0, 0.0), 0.5);
        assert_eq!(lambda_b(100.0, 100.0), 1.0);
        assert_eq!(lambda_b(0.0, 0.0), 1.0);
    }

    /// The scale is `K·V_base`, so a re-denomination leaves every score
    /// unchanged. A bare `Cap/(Cap+K)` would re-price the whole community's
    /// risk the first time `rescale` ran, silently, as a change of unit.
    #[test]
    fn the_score_survives_a_redenomination() {
        let pi = 100.0;
        let before = member_risk(2_500.0, 500.0, 10.0, 5.0, RISK_K, BASE_CAPACITY);
        let after = member_risk(2_500.0 * pi, 500.0 * pi, 10.0 * pi, 5.0 * pi, RISK_K, BASE_CAPACITY * pi);
        assert!((before - after).abs() < 1e-12, "{before} vs {after}");
    }
}
