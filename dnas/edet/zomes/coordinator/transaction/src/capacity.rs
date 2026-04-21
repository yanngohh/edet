use transaction_integrity::types::constants::*;

/// Compute credit capacity for an agent given their trust, acquaintance count, and base (vouched) capacity.
///
/// Cap_i = base_capacity + beta * ln(max(1, t_i / t_baseline)) * (1 - exp(-n / n0))
/// where t_baseline = alpha / num_acquaintances
///       n0 = ACQ_SATURATION
///
/// This is a pure mathematical function — it does not perform any DHT queries.
/// It can be called from any module without creating circular dependencies.
pub fn compute_credit_capacity(trust: f64, num_acquaintances: usize, base_capacity: f64) -> f64 {
    // Note: unvouched agents (base_capacity == 0.0) still receive reputation-based
    // capacity as they build a positive transaction history (S/F counters).
    // This allows natural "graduation" from trials to Path 1/2.

    if num_acquaintances == 0 {
        return base_capacity;
    }
    let n = num_acquaintances as f64;
    let t_baseline = EIGENTRUST_ALPHA / n;
    let rel_rep = if t_baseline > 0.0 { trust / t_baseline } else { 1.0 };
    let saturation = 1.0 - (-n / ACQ_SATURATION).exp();
    base_capacity + CAPACITY_BETA * rel_rep.max(1.0).ln() * saturation
}
