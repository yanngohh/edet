/**
 * Protocol constants — mirrors transaction_integrity/types/constants.rs.
 *
 * Keep in sync when the Rust constants change. Every numeric protocol
 * threshold used in the UI must live here so there is a single place
 * to update.
 */

/** V_base: denomination constant used as the reference capacity unit.
 *  Rust: BASE_CAPACITY */
export const BASE_CAPACITY = 1000.0;

/** Eta: fraction of V_base below which a transaction is a "trial".
 *  Rust: TRIAL_FRACTION */
export const TRIAL_FRACTION = 0.05;

/** Derived trial threshold: transactions with amount < TRIAL_THRESHOLD
 *  are treated as trials by the backend (always Pending, seller approves).
 *  Rust: TRIAL_FRACTION * BASE_CAPACITY */
export const TRIAL_THRESHOLD = TRIAL_FRACTION * BASE_CAPACITY; // 50.0

/** Default vouch amount when sponsoring a new entrant (= V_base).
 *  Rust: BASE_CAPACITY used as the canonical vouch unit */
export const DEFAULT_VOUCH_AMOUNT = BASE_CAPACITY; // 1000.0

/** n0: Acquaintance saturation constant for the capacity ramp.
 *  Saturation factor = (1 - exp(-n / ACQ_SATURATION)).
 *  n=1 → ~1,242; n=10 → ~3,194; n=50 → ~8,650; n=150 → ~12,499.
 *  Rust: ACQ_SATURATION */
export const ACQ_SATURATION = 50.0;

/** Maximum number of trial transactions a seller can approve per epoch.
 *  Rust: TRIAL_VELOCITY_LIMIT_PER_EPOCH */
export const TRIAL_VELOCITY_LIMIT_PER_EPOCH = 5;

/** Maximum amount a single vouch can stake (= V_base).
 *  Rust: MAX_VOUCH_AMOUNT = BASE_CAPACITY (integrity/transaction/src/vouch.rs) */
export const MAX_VOUCH_AMOUNT = BASE_CAPACITY; // 1000.0

/** Vouch slashing multiplier X. When a vouchee defaults by δ, the sponsor
 *  loses min(X × δ, vouch_amount) from their stake. This is the primary
 *  disincentive against Sybil rings — the ring destroys X times what it extracts.
 *  Rust: VOUCH_SLASHING_MULTIPLIER (integrity/transaction/src/types/constants.rs) */
export const VOUCH_SLASHING_MULTIPLIER = 3.0;

/** Default auto-accept threshold for new wallets.
 *  Transactions with risk score ≤ this are auto-accepted.
 *  Rust: WALLET_DEFAULT_AUTO_ACCEPT_THRESHOLD (integrity/transaction/src/types/constants.rs) */
export const WALLET_DEFAULT_AUTO_ACCEPT_THRESHOLD = 0.40;

/** Default auto-reject threshold for new wallets.
 *  Transactions with risk score ≥ this are auto-rejected.
 *  Rust: WALLET_DEFAULT_AUTO_REJECT_THRESHOLD (integrity/transaction/src/types/constants.rs) */
export const WALLET_DEFAULT_AUTO_REJECT_THRESHOLD = 0.80;

/** Maximum depth of the support cascade recursion.
 *  Rust: MAX_CASCADE_DEPTH (integrity/transaction/src/types/constants.rs) */
export const MAX_CASCADE_DEPTH = 20;

/** Minimum amount considered non-dust. Values below this are ignored in
 *  S/F accumulation and debt transfer.
 *  Rust: DUST_THRESHOLD (integrity/transaction/src/types/constants.rs) */
export const DUST_THRESHOLD = 0.01;

/** Milliseconds per epoch (= EPOCH_DURATION_SECS × 1000).
 *  Rust: EPOCH_DURATION_SECS = 86400 (one calendar day at production cadence).
 *  Note: test-epoch builds use 1-second epochs — this constant must not be
 *  hardcoded in test harnesses.
 *  Rust: EPOCH_DURATION_SECS (integrity/transaction/src/types/constants.rs) */
export const EPOCH_DURATION_MS = 86_400_000;

/** Failure tolerance threshold τ. A bilateral failure rate above this
 *  triggers trust attenuation φ(r) → 0.
 *  Rust: FAILURE_TOLERANCE (integrity/transaction/src/types/constants.rs) */
export const FAILURE_TOLERANCE = 0.12;

/** EigenTrust mixing parameter α. Pre-trust fraction per iteration.
 *  Rust: EIGENTRUST_ALPHA (integrity/transaction/src/types/constants.rs) */
export const EIGENTRUST_ALPHA = 0.08;

/** Breakdown coefficient sum tolerance bounds (lower/upper).
 *  Matches the integrity zome's SupportBreakdown coefficient-sum band.
 *  Rust: integrity/support/src/support_breakdown.rs:36 */
export const BREAKDOWN_SUM_TOLERANCE_LOWER = 0.99999999;
export const BREAKDOWN_SUM_TOLERANCE_UPPER = 1.00000001;

/** Precision factor for breakdown coefficient rounding (4 decimal places). */
export const BREAKDOWN_ROUNDING_PRECISION = 10000;
