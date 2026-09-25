/**
 * The waterfill share editor's math, mirroring the kernel's
 * EditSupportBreakdown: a set of sliders whose shares always total 100%.
 * Dragging one slider shifts the others uniformly; the dragged slider
 * closes the sum, clamped to [0, 1]; everything rounds to 4 decimals with
 * the largest share absorbing the rounding remainder.
 */

export const SHARE_PRECISION = 10_000;
const SUM_TOLERANCE_LOWER = 0.99999999;

function round4(x: number): number {
    return Math.round(x * SHARE_PRECISION) / SHARE_PRECISION;
}

/** Normalize arbitrary positive weights into shares summing to 1
 *  (equal shares when the total is zero or empty input). */
export function normalizeShares(weights: number[]): number[] {
    if (weights.length === 0) return [];
    const total = weights.reduce((a, b) => a + b, 0);
    const shares = total > 0 ? weights.map((w) => w / total) : weights.map(() => 1 / weights.length);
    return closeSum(shares.map(round4));
}

/**
 * Rebalance after slider `changed` moved: spread the excess/deficit
 * uniformly over all shares, then pin the changed slider to close the sum
 * (clamped to [0, 1]), round, and let the largest share absorb rounding.
 */
export function rebalanceShares(shares: number[], changed: number): number[] {
    const count = shares.length;
    if (count === 0) return shares;
    if (count === 1) return [1];
    let out = [...shares];

    let total = out.reduce((a, b) => a + b, 0);
    if (total < SUM_TOLERANCE_LOWER || total > 1) {
        out = out.map((c) => Math.max(0, Math.min(1, c + (1 - total) / count)));
    }
    total = out.reduce((a, b) => a + b, 0);
    const othersSum = total - out[changed];
    out[changed] = Math.max(0, Math.min(1, 1 - othersSum));

    return closeSum(out.map(round4));
}

/** Remove index `removed` and hand its share out equally to the rest. */
export function removeShare(shares: number[], removed: number): number[] {
    const gone = shares[removed] ?? 0;
    const rest = shares.filter((_, i) => i !== removed);
    if (rest.length === 0) return [];
    return closeSum(rest.map((c) => round4(c + gone / rest.length)));
}

/** Append a new entry at 0% (the user raises its slider afterwards). A
 *  first entry starts at 100%. */
export function appendShare(shares: number[]): number[] {
    return shares.length === 0 ? [1] : closeSum([...shares.map(round4), 0]);
}

/** Make the rounded shares total exactly 1.0 via the largest entry. */
function closeSum(shares: number[]): number[] {
    if (shares.length === 0) return shares;
    const out = [...shares];
    const sum = out.reduce((a, b) => a + b, 0);
    if (Math.abs(sum - 1.0) > 1e-12) {
        const maxIndex = out.indexOf(Math.max(...out));
        out[maxIndex] = round4(out[maxIndex] + (1.0 - sum));
    }
    return out;
}
