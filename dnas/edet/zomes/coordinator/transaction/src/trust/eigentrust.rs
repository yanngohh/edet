use hdk::prelude::*;
use transaction_integrity::types::constants::*;

use crate::trust_cache::CachedTrustSubgraph;

// =========================================================================
//  EigenTrust: Power Iteration (Whitepaper Section 7.1)
//
//  t^(k+1) = (1 - alpha) * C^T * t^(k) + alpha * p
// =========================================================================

/// Run EigenTrust power iteration on the subgraph.
///
/// t^(k+1) = (1 - alpha) * C^T * t^(k) + alpha * p
///
/// Where C is the local trust matrix (row-stochastic) and p is the
/// personalized pre-trust vector.
///
/// Returns `Err` if `pre_trust.len() != subgraph.size()`, avoiding a panic
/// that would crash the zome call.
pub(crate) fn power_iteration(subgraph: &CachedTrustSubgraph, pre_trust: &[f64]) -> ExternResult<Vec<f64>> {
    let n = subgraph.size();
    if n == 0 {
        return Ok(Vec::new());
    }

    // pre_trust must have exactly one entry per subgraph node.
    // A length mismatch would silently truncate via zip, producing incorrect trust values.
    if pre_trust.len() != n {
        return Err(wasm_error!(WasmErrorInner::Guest(format!(
            "EigenTrust: pre_trust length ({}) != subgraph size ({})",
            pre_trust.len(),
            n
        ))));
    }

    let alpha = EIGENTRUST_ALPHA;
    let mut t: Vec<f64> = pre_trust.to_vec();

    // Ensure t sums to 1.0 before the first iteration. pre_trust should already
    // be normalised, but floating-point drift can make dangling_mass negative
    // if mass_distributed slightly exceeds 1.0, breaking the non-negativity invariant.
    let t_sum: f64 = t.iter().sum();
    if t_sum > 0.0 && (t_sum - 1.0).abs() > 1e-12 {
        for v in t.iter_mut() {
            *v /= t_sum;
        }
    }

    for iter_num in 0..EIGENTRUST_MAX_ITERATIONS {
        let mut new_t = vec![0.0f64; n];

        // Compute C^T * t: for each row i in C (agent i's trust row),
        // distribute t[i] to each j that i trusts.
        let mut mass_distributed = 0.0;
        for (i, row) in subgraph.trust_rows.iter().enumerate().take(n) {
            for (&j, &c_ij) in row {
                // C^T[j][i] = c_ij, so new_t[j] += c_ij * t[i]
                new_t[j] += c_ij * t[i];
                mass_distributed += c_ij * t[i];
            }
        }

        // Dangling node mass: trust from nodes with no outgoing links
        // is redistributed to the pre-trust vector
        let dangling_mass = 1.0 - mass_distributed;

        // Apply the EigenTrust formula: (1-alpha) * C^T * t + alpha * p
        // Plus dangling mass redistribution
        let mut total = 0.0;
        for (new_val, p_val) in new_t.iter_mut().zip(pre_trust.iter()).take(n) {
            *new_val = (1.0 - alpha) * *new_val + (alpha + (1.0 - alpha) * dangling_mass) * p_val;
            total += *new_val;
        }

        // Per-iteration renormalization ensures sum == 1 even when dangling-node
        // redistribution and floating-point rounding cause minor drift. In theory,
        // an column-stochastic M = (1-α)C^T + α·p·1^T preserves sum-1 exactly, so
        // this renormalization should be a no-op for well-formed C. In practice it
        // guards against accumulated rounding errors over many iterations and against
        // slightly non-stochastic rows produced by BFS subgraph truncation.
        // This is defensive and correct; the whitepaper does not require it explicitly
        // but it does not contradict the convergence proof either.
        if total > 0.0 {
            for val in new_t.iter_mut().take(n) {
                *val /= total;
            }
        }

        // Check convergence
        let diff: f64 = t.iter().zip(new_t.iter()).map(|(a, b)| (a - b).abs()).sum();
        t = new_t;

        if diff < EIGENTRUST_EPSILON {
            break;
        }

        // Warn if we exhausted all iterations without converging
        if iter_num == EIGENTRUST_MAX_ITERATIONS - 1 {
            warn!(
                "EigenTrust: did not converge after {} iterations (residual={:.6}, epsilon={})",
                EIGENTRUST_MAX_ITERATIONS, diff, EIGENTRUST_EPSILON
            );
        }
    }

    Ok(t)
}
