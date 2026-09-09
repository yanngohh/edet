//! Pure allocation math for the support cascade and the sealing bucket.

/// Waterfilling allocation: distribute `total` across targets with (need,
/// weight), proportionally by weight, clamped at each need, redistributing
/// the surplus among still-unsatisfied targets until stable. Returns the
/// per-target allocations (each ≤ its need; sum ≤ total).
pub fn waterfill(targets: &[(f64, f64)], total: f64) -> Vec<f64> {
    let n = targets.len();
    let mut alloc = vec![0.0; n];
    if n == 0 || total.is_nan() || total <= 0.0 {
        return alloc;
    }
    let mut remaining = total;
    let mut open: Vec<usize> = (0..n).filter(|&i| targets[i].0 > 0.0 && targets[i].1 > 0.0).collect();
    // Each round satisfies at least one target or ends; ≤ n rounds.
    for _ in 0..n {
        if open.is_empty() || remaining <= 0.0 {
            break;
        }
        let weight_sum: f64 = open.iter().map(|&i| targets[i].1).sum();
        if weight_sum <= 0.0 {
            break;
        }
        let mut next_open = Vec::new();
        let mut spent = 0.0;
        for &i in &open {
            let share = remaining * targets[i].1 / weight_sum;
            let room = targets[i].0 - alloc[i];
            let take = share.min(room);
            alloc[i] += take;
            spent += take;
            if alloc[i] < targets[i].0 - 1e-12 {
                next_open.push(i);
            }
        }
        remaining -= spent;
        if next_open.len() == open.len() {
            // Nobody saturated: proportional split is final.
            break;
        }
        open = next_open;
    }
    alloc
}

/// Waterfilling in MINOR UNITS: the same allocation over integer needs and an
/// integer total, with the relative weights still real numbers.
///
/// Needs and the pot are amounts, so they are integers — the ledger holds no
/// other kind — while a weight is a ratio and stays an `f64`, because nothing
/// ever compares a weight to a stored quantity.
///
/// Each round's share is FLOORED, which is what makes the result a partition
/// the book can hold: `Σ floor(share_i) ≤ remaining`, so no round can
/// over-allocate and no allocation can exceed its need. The floors leave up to
/// one minor unit per target unallocated; in the cascade that surplus is the
/// genesis remainder the sale already routes back to the seller, so it is
/// neither lost nor minted.
pub fn waterfill_minor(targets: &[(u64, f64)], total: u64) -> Vec<u64> {
    let n = targets.len();
    let mut alloc = vec![0u64; n];
    if n == 0 || total == 0 {
        return alloc;
    }
    let mut remaining = total;
    let mut open: Vec<usize> = (0..n)
        .filter(|&i| targets[i].0 > 0 && targets[i].1 > 0.0 && targets[i].1.is_finite())
        .collect();
    // Each round satisfies at least one target or ends; ≤ n rounds.
    for _ in 0..n {
        if open.is_empty() || remaining == 0 {
            break;
        }
        let weight_sum: f64 = open.iter().map(|&i| targets[i].1).sum();
        if !weight_sum.is_finite() || weight_sum <= 0.0 {
            break;
        }
        let mut next_open = Vec::new();
        let mut spent: u64 = 0;
        for &i in &open {
            let share = libm::floor(remaining as f64 * targets[i].1 / weight_sum);
            let share = if share.is_finite() && share > 0.0 { share as u64 } else { 0 };
            let room = targets[i].0 - alloc[i];
            let take = share.min(room).min(remaining - spent);
            alloc[i] += take;
            spent += take;
            if alloc[i] < targets[i].0 {
                next_open.push(i);
            }
        }
        remaining -= spent;
        if next_open.len() == open.len() {
            // Nobody saturated: the proportional split is final.
            break;
        }
        open = next_open;
    }
    alloc
}

/// Power-of-two magnitude bucket for sealed-amount display: the smallest
/// power of two at or above `x` (0 for non-positive input).
pub fn pow2_bucket(x: f64) -> f64 {
    if !x.is_finite() || x <= 0.0 {
        return 0.0;
    }
    let e = libm::ceil(libm::log2(x));
    libm::exp2(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proportional_when_nobody_saturates() {
        let a = waterfill(&[(100.0, 2.0), (100.0, 1.0)], 30.0);
        assert!((a[0] - 20.0).abs() < 1e-9);
        assert!((a[1] - 10.0).abs() < 1e-9);
    }

    #[test]
    fn surplus_redistributes() {
        // Target 0 needs only 5; its surplus flows to target 1.
        let a = waterfill(&[(5.0, 1.0), (100.0, 1.0)], 40.0);
        assert!((a[0] - 5.0).abs() < 1e-9);
        assert!((a[1] - 35.0).abs() < 1e-9);
    }

    #[test]
    fn never_exceeds_needs_or_total() {
        let a = waterfill(&[(3.0, 1.0), (4.0, 5.0)], 100.0);
        assert!((a[0] - 3.0).abs() < 1e-9);
        assert!((a[1] - 4.0).abs() < 1e-9);
        let b = waterfill(&[(50.0, 1.0)], 20.0);
        assert!((b[0] - 20.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_inputs() {
        assert!(waterfill(&[], 10.0).is_empty());
        assert_eq!(waterfill(&[(10.0, 1.0)], 0.0), vec![0.0]);
        assert_eq!(waterfill(&[(0.0, 1.0), (10.0, 0.0)], 10.0), vec![0.0, 0.0]);
    }

    #[test]
    fn buckets() {
        assert_eq!(pow2_bucket(0.0), 0.0);
        assert_eq!(pow2_bucket(1.0), 1.0);
        assert_eq!(pow2_bucket(3.0), 4.0);
        assert_eq!(pow2_bucket(4.0), 4.0);
        assert_eq!(pow2_bucket(700.0), 1024.0);
        assert_eq!(pow2_bucket(f64::NAN), 0.0);
    }
}
