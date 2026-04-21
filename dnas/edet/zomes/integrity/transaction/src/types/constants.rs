/// Default risk score thresholds for the wallet.
/// Risk score is in [0, 1] where 0 = no risk, 1 = maximum risk.
/// See whitepaper Section 5.3, Definition (Transaction Risk Score).
pub const WALLET_DEFAULT_AUTO_REJECT_THRESHOLD: f64 = 0.80f64;
pub const WALLET_DEFAULT_AUTO_ACCEPT_THRESHOLD: f64 = 0.40f64;

/// Maximum size of an agent's acquaintance set (Dunbar-style cap).
/// When exceeded, the acquaintance with the lowest bilateral satisfaction S_ij is evicted.
/// Eviction affects only the pre-trust vector p^(i); S/F counters are retained.
/// Matches sim/config.py: max_acquaintances = 150.
/// See whitepaper: Acquaintance cap paragraph (Appendix B).
pub const MAX_ACQUAINTANCES: usize = 150;

/// K_claim: Sigmoid steepness for the claim-based risk score (Whitepaper Eq. claim_risk).
/// hat_t_claim = n_S / (n_S + K_claim), where n_S is the number of successfully transferred
/// contracts from the ReputationClaim. K_claim = 20 means a buyer with 20 successful contracts
/// reaches hat_t_claim = 0.5; with 100 contracts, hat_t_claim ≈ 0.83.
pub const K_CLAIM_SIGMOID: f64 = 20.0;

/// Sigmoid steepness for risk score normalization.
/// The risk formula uses rel_trust = rel_rep / (rel_rep + K) where
/// rel_rep = trust / t_baseline. K controls the discrimination threshold:
/// nodes with rel_rep > K get rel_trust > 0.5 (trending toward accept),
/// nodes with rel_rep < K get rel_trust < 0.5 (trending toward reject).
/// See whitepaper Remark (Scale Invariance of Risk Score).
pub const RISK_SIGMOID_K: f64 = 0.75;

// =========================================================================
//  Protocol Parameters (Whitepaper Appendix B / sim/config.py)
// =========================================================================

/// EigenTrust mixing factor alpha. Balances subjective baseline p against
/// the weight of global history. t^(k+1) = (1-alpha)*C^T*t^(k) + alpha*p
pub const EIGENTRUST_ALPHA: f64 = 0.08;

/// EigenTrust convergence threshold. Power iteration stops when
/// ||t^(k+1) - t^(k)||_1 < epsilon.
pub const EIGENTRUST_EPSILON: f64 = 0.001;

/// Maximum number of EigenTrust power iterations per computation.
/// With alpha = 0.08, the spectral radius bound is (1-alpha)^K = 0.92^K.
/// K = 84 gives 0.92^84 ≈ 9.5e-4 < epsilon = 0.001, guaranteeing
/// convergence even on adversarial / sparse graphs.  Well-connected
/// graphs converge in 10-15 iterations; the early-exit check
/// (diff < EIGENTRUST_EPSILON) ensures no unnecessary work.
pub const EIGENTRUST_MAX_ITERATIONS: u32 = 84;

/// V_base: Denomination constant for trial transactions and capacity bounds.
/// Not a free capacity floor: new nodes receive capacity V_staked from their vouchers.
/// Used as: trial threshold = eta * V_base; attack bounds in security analysis.
pub const BASE_CAPACITY: f64 = 1000.0;

/// Beta: Scaling constant for the logarithmic credit capacity function.
/// Cap_i = V_staked + beta * ln(max(1, t_i / t_baseline)) * (1 - exp(-n / n0))
pub const CAPACITY_BETA: f64 = 5000.0;

/// n0: Acquaintance saturation constant for the capacity ramp.
/// The saturation factor (1 - exp(-n / ACQ_SATURATION)) rises from 0 at n=0
/// to ~0.63 at n=n0, ~0.87 at n=2*n0, and asymptotes to 1 for large n.
/// This enforces a gradual capacity ramp with acquaintance count:
///   n=1   → cap ~1,242  (just above the vouched base)
///   n=10  → cap ~3,194
///   n=50  → cap ~8,650
///   n=150 → cap ~12,499 (near ceiling for Dunbar-bounded networks)
pub const ACQ_SATURATION: f64 = 50.0;

/// Tau: Failure tolerance threshold. Nodes with failure rate >= tau are
/// fully excluded from the trust graph (Theorem 6 / Definition 5).
pub const FAILURE_TOLERANCE: f64 = 0.12;

/// Gamma: Penalty sharpness exponent for the trust attenuation function.
/// phi(r) = max(0, 1 - (r/tau)^gamma). Higher gamma concentrates penalty
/// near the threshold.
pub const PENALTY_SHARPNESS: f64 = 4.0;

/// K: Recent failure rate window size in epochs.
/// r_recent = F_last_K / (S_last_K + F_last_K) over the last K epochs.
#[cfg(not(feature = "test-epoch"))]
pub const RECENT_WINDOW_K: u64 = 10;
#[cfg(feature = "test-epoch")]
pub const RECENT_WINDOW_K: u64 = 3;

/// w_r: Recent failure rate amplification factor.
/// r_eff = max(r_cumul, RECENT_WEIGHT * r_recent).
/// At w_r=2.0: a node defaulting on ≥50% of recent transactions gets phi=0
/// even if cumulative r is low due to a prior honest build phase.
pub const RECENT_WEIGHT: f64 = 2.0;

/// M_min: Minimum contract maturity in epochs.
/// Reduces the attacker's extraction horizon: a build-then-betray attacker who
/// takes on maximum debt can enjoy goods/services for at most M_min epochs before
/// any failure is recorded. Lower = less extraction time; higher = more tolerance
/// for slow-paying debtors. 30 epochs (≈ 30 days) matches standard net-30 terms.
///
/// Changed from 50 → 30 to reduce the adversarial extraction window by 40%
/// while remaining compatible with normal commercial payment timelines.
#[cfg(not(feature = "test-epoch"))]
pub const MIN_MATURITY: u64 = 30;
#[cfg(feature = "test-epoch")]
pub const MIN_MATURITY: u64 = 10;

/// Eta: Trial transaction threshold as fraction of V_base. Transactions
/// with amount < eta * V_base are accepted regardless of buyer reputation.
pub const TRIAL_FRACTION: f64 = 0.05;

/// Slashing multiplier applied to a sponsor when a vouched identity defaults.
/// Ensures the economic punishment outweighs the extracted value.
pub const VOUCH_SLASHING_MULTIPLIER: f64 = 3.0;

/// Maximum number of trial transactions a merchant will accept per epoch.
/// Mitigates Sybil trial flood attacks.
pub const TRIAL_VELOCITY_LIMIT_PER_EPOCH: u32 = 5;

/// Distributed epoch duration in seconds (24 hours).
/// E = floor(unix_timestamp / EPOCH_DURATION_SECS)
#[cfg(not(feature = "test-epoch"))]
pub const EPOCH_DURATION_SECS: u64 = 86400;
/// Test-epoch mode: 1-second epochs for fast sweettest integration tests.
#[cfg(feature = "test-epoch")]
pub const EPOCH_DURATION_SECS: u64 = 1;

/// `genesis_vouch` is a test-only bootstrap mechanism and does not exist in
/// production builds.  The constant is only compiled with the `test-epoch`
/// feature so production code cannot reference it.
#[cfg(feature = "test-epoch")]
pub const GENESIS_VOUCH_CUTOFF_EPOCH: u64 = u64::MAX;

/// Maximum depth for the Subjective Local Expansion graph walk.
/// Peers fetch trust links to this depth from their acquaintance set.
pub const SUBGRAPH_MAX_DEPTH: u32 = 4;

/// Maximum depth of the support cascade recursion.
///
/// The cascade uses a visited-set for cycle detection, guaranteeing finite
/// termination in any finite graph. However, without a depth limit a long
/// acyclic support chain (A→B→C→…→Z) fires O(depth) remote calls in series,
/// creating unbounded resource consumption per transaction on relay cells.
///
/// A depth of 20 allows up to 20 hops in the support tree before the remaining
/// uncleared amount becomes genesis debt on the buyer. In practice, support
/// breakdowns rarely exceed 3–4 hops; 20 is a generous safety ceiling that
/// prevents pathological long chains without affecting normal operation.
pub const MAX_CASCADE_DEPTH: u32 = 20;

/// Hard backstop on the integrity-layer dynamic capacity ceiling.
///
/// The integrity zome computes cap_integrity(n) from the agent's net acquaintance
/// count n on their source chain (see debt_contract.rs).  MAX_THEORETICAL_CAPACITY
/// is the clamp applied to that result — it fires only when n ≥ MAX_ACQUAINTANCES,
/// i.e. when the agent already has the maximum allowed acquaintance set.
///
/// Derivation (n = MAX_ACQUAINTANCES = 150, t = 1−α = 0.92):
///
///   Cap_max = 1000 + 5000 × ln(0.92 / (0.08/150)) × (1 − e^{−150/50})
///           = 1000 + 5000 × ln(1725) × 0.9502 ≈ 36,410  →  37,000
///
/// A modified conductor can pre-populate fake AgentToAcquaintance links to push
/// n toward MAX_ACQUAINTANCES, but the ceiling never exceeds this constant
/// regardless of how many fake links are written.
pub const MAX_THEORETICAL_CAPACITY: f64 = 37_000.0;

pub mod wallet_validation_error {
    pub const OWNER_ASSOCIATION_EXISTS: &str = "EV100000";
    pub const INVALID_INIT_STATE: &str = "EV100001";
    pub const ACTION_AUTHOR_NOT_OWNER: &str = "EV100002";
    pub const INVALID_THRESHOLDS: &str = "EV100003";
    pub const OWNER_IMMUTABLE: &str = "EV100004";
    pub const WALLETS_NOT_DELETABLE: &str = "EV100005";
}

pub mod owner_to_wallet_validation_error {
    pub const TARGET_NOT_ON_CREATE_WALLET_ACTION: &str = "EV101000";
    pub const AUTHOR_NOT_WALLET_OWNER: &str = "EV101001";
}

pub mod transaction_validation_error {
    pub const BUYER_MUST_CREATE_TRANSACTION: &str = "EV200000";
    pub const TRANSACTION_STATUS_INCOHERENT: &str = "EV200001";
    pub const TRANSACTION_TIMESTAMP_NON_MONOTONOUS: &str = "EV200002";
    pub const SELLER_WALLET_OBSOLETE: &str = "EV200003";
    pub const BUYER_WALLET_OBSOLETE: &str = "EV200004";
    pub const SELLER_LAST_TRANSACTION_OBSOLETE: &str = "EV200005";
    pub const BUYER_LAST_TRANSACTION_OBSOLETE: &str = "EV200006";
    pub const TRANSACTION_SELLER_WALLET_MISSING: &str = "EV200008";
    pub const TRANSACTION_BUYER_WALLET_MISSING: &str = "EV200009";
    pub const TRANSACTION_NOT_DELETABLE: &str = "EV200010";
    /// Only the seller can approve (Pending -> Accepted) a transaction.
    pub const ONLY_SELLER_CAN_APPROVE: &str = "EV200011";
    /// Only the seller can reject (Pending -> Rejected) a transaction.
    pub const ONLY_SELLER_CAN_REJECT: &str = "EV200012";
    /// Only the buyer can cancel (Pending -> Canceled) a transaction.
    pub const ONLY_BUYER_CAN_CANCEL: &str = "EV200013";
    /// Status transition is only allowed from Pending status.
    pub const INVALID_STATUS_TRANSITION_SOURCE: &str = "EV200014";
    /// Buyer already has an open (Pending) trial transaction with this seller.
    /// A new trial cannot be created until the existing one is resolved.
    pub const DUPLICATE_OPEN_TRIAL: &str = "EV200015";
    /// Drain transaction created with an invalid status (must be Pending or Accepted).
    pub const DRAIN_INVALID_CREATION_STATUS: &str = "EV200016";
    /// Trial transaction created with an invalid status (must be Pending).
    pub const TRIAL_INVALID_CREATION_STATUS: &str = "EV200017";
    /// Transaction debt must be a finite positive number (NaN, Infinity, zero, and
    /// negative values are rejected at the integrity layer as well as the coordinator).
    pub const DEBT_NOT_POSITIVE: &str = "EV200018";
    /// Trial transaction amount exceeds the protocol-defined cap of η · V_base
    /// (= 0.05 · 1000 = 50 units). Whitepaper §2 Bootstrap / Theorem 2.3.
    /// Enforced at the integrity layer so that a modified conductor cannot
    /// publish a `is_trial=true` transaction with arbitrary amount.
    pub const TRIAL_AMOUNT_EXCEEDS_CAP: &str = "EV200019";
    /// Approving this trial transaction would exceed the per-seller per-epoch
    /// trial velocity limit (L_trial = 5). Whitepaper §2.3.
    /// Enforced at the integrity layer (in addition to coordinator EC200022)
    /// so that a modified conductor cannot mass-approve trials.
    pub const TRIAL_VELOCITY_EXCEEDED: &str = "EV200020";
}

pub mod wallet_to_transaction_validation_error {
    pub const INVALID_TRANSACTION_DATA: &str = "EV201000";
    pub const BUYER_MUST_PERFORM_ASSOCIATION: &str = "EV201001";
    pub const SELLER_MUST_PERFORM_ASSOCIATION: &str = "EV201002";
    pub const FINALIZED_TRANSACTION_ASSOCIATION_NOT_DELETABLE: &str = "EV201003";
}

pub mod debt_contract_validation_error {
    pub const AMOUNT_NOT_POSITIVE: &str = "EV400000";
    pub const MATURITY_TOO_LOW: &str = "EV400001";
    pub const CREDITOR_IS_DEBTOR: &str = "EV400002";
    pub const AUTHOR_NOT_DEBTOR: &str = "EV400003";
    pub const AMOUNT_INCREASED: &str = "EV400004";
    pub const INVALID_STATUS_TRANSITION: &str = "EV400005";
    pub const CONTRACT_NOT_DELETABLE: &str = "EV400006";
    pub const CREDITOR_CHANGED: &str = "EV400007";
    pub const DEBTOR_CHANGED: &str = "EV400008";
    pub const MATURITY_CHANGED: &str = "EV400009";
    pub const START_EPOCH_CHANGED: &str = "EV400010";
    pub const DEBT_EXCEEDS_CAPACITY: &str = "EV400011";
    /// Contract cannot be marked as Expired before maturity (start_epoch + maturity).
    pub const PREMATURE_EXPIRATION: &str = "EV400012";
    /// transaction_hash field is immutable after creation.
    pub const TRANSACTION_HASH_CHANGED: &str = "EV400013";
    /// co_signers field is immutable after creation.
    pub const CO_SIGNERS_CHANGED: &str = "EV400014";
    /// is_trial field is immutable after creation.
    pub const IS_TRIAL_CHANGED: &str = "EV400015";
    /// original_amount field is immutable after creation.
    pub const ORIGINAL_AMOUNT_CHANGED: &str = "EV400016";
    /// original_amount must equal amount at contract creation.
    pub const ORIGINAL_AMOUNT_MISMATCH_ON_CREATE: &str = "EV400017";
    /// Trial debt contract amount exceeds the protocol cap of η · V_base = 50.
    /// Whitepaper §2.3, Theorem `thm:bootstrap`.
    pub const TRIAL_AMOUNT_EXCEEDS_CAP: &str = "EV400018";
}

pub mod trust_link_validation_error {
    pub const AUTHOR_NOT_BASE_AGENT: &str = "EV500000";
    pub const TRUST_VALUE_OUT_OF_RANGE: &str = "EV500001";
}

// =========================================================================
//  Scalability Constants
// =========================================================================

/// Trust cache time-to-live within an epoch (seconds).
/// Cached trust data is valid for this duration before requiring DHT refresh.
/// Set to 5 minutes to balance freshness with performance.
#[cfg(not(feature = "test-epoch"))]
pub const TRUST_CACHE_TTL_SECS: u64 = 300;
/// Test-epoch mode: 1-second TTL matches 1-second epoch duration.
#[cfg(feature = "test-epoch")]
pub const TRUST_CACHE_TTL_SECS: u64 = 1;

/// Maximum clock skew (Δ_drift) allowed between a transaction author and a validator node.
/// When a node's wall-clock time is within Δ_drift seconds of an epoch boundary, the
/// validator accepts the action if the author assigned it to either of the two adjacent
/// epochs. This prevents spurious PREMATURE_EXPIRATION rejections caused by clock drift
/// near midnight boundaries.
///
/// Requirement: Δ_drift < EPOCH_DURATION_SECS / 2 (satisfied: 300 ≪ 43200).
/// Matches Whitepaper Property (Epoch Unambiguity) and Appendix B parameter table.
pub const CLOCK_DRIFT_MAX_SECS: u64 = 300;

/// Maximum number of agents in a trust subgraph.
/// Acts as a circuit breaker to prevent memory explosion in highly-connected networks.
/// BFS stops expanding when this limit is reached, effectively reducing depth adaptively.
pub const MAX_SUBGRAPH_NODES: usize = 50_000;

/// Maximum number of DHT trust rows cached per agent.
/// When exceeded, oldest entries are evicted (LRU-like via epoch + timestamp).
pub const MAX_DHT_TRUST_ROWS_CACHED: usize = 100_000;

/// Number of epochs after which transferred/expired contracts can be archived.
/// Archived contracts are excluded from active scans but preserved for history.
pub const ARCHIVE_AFTER_EPOCHS: u64 = 30;

/// Interval between chain checkpoints (in epochs).
/// Checkpoints allow validators to skip historical validation.
pub const CHECKPOINT_INTERVAL_EPOCHS: u64 = 100;

/// Interval between chain checkpoints (in source chain entries).
/// Whichever threshold is reached first triggers a checkpoint.
pub const CHECKPOINT_INTERVAL_ENTRIES: u64 = 1000;

// =========================================================================
//  Bilateral Volume-Scaled Tolerance (Whitepaper Definition 7)
// =========================================================================

/// Tau_0: Failure tolerance for brand-new bilateral relationships (n_ij = 0).
/// A creditor who has never interacted with a debtor applies a strict 5%
/// failure threshold. As bilateral interaction volume grows, the tolerance
/// ramps logarithmically to the full FAILURE_TOLERANCE (tau).
///
/// This bounds Sybil economic impact without restricting network membership:
/// honest newcomers (0% failure) are unaffected, while attackers face 2.4x
/// stricter exclusion in new relationships (tau/tau_0 = 0.12/0.05 = 2.4).
///
/// tau_eff(n_ij) = TAU_NEWCOMER + (TAU - TAU_NEWCOMER) * min(ln(1 + n_ij) / ln(1 + N_mat), 1)
pub const TAU_NEWCOMER: f64 = 0.05;

/// N_mat: Bilateral interaction volume at which full failure tolerance (tau) is
/// reached. The logarithmic ramp provides diminishing returns: the first few
/// transactions matter most, preventing gaming via many small Sybil transactions.
/// With N_mat = 1000 and max_volume_per_epoch = 100, a creditor reaches full
/// tolerance after ~10 active epochs of trading with a debtor.
pub const VOLUME_MATURATION_THRESHOLD: f64 = 1000.0;

/// Limit the amount of successful volume S that can be accrued per epoch per
/// counterparty relationship. This enforces "Time-Weighted" maturation, preventing
/// wash trading from bypassing the maturation period in a single day.
pub const MAX_VOLUME_PER_EPOCH: f64 = 100.0;

/// f_bank: Trust Banking Bound Fraction for local trust multiplier.
/// Bounds the maximum extractable absolute leverage (capacity) an attacker
/// can gain from an honest edge. The maximum score for an edge is capped at
/// N_mat * f_bank preventing nodes from amassing unlimited trust to absorb
/// massive defaults later.
/// With f_bank = 0.25 and N_mat = 1000, max score per edge is 250.
pub const TRUST_BANKING_BOUND_FRACTION: f64 = 0.25;

/// k: Contagion witness factor for tau_eff penalty.
/// Each independent witness who has observed a node default contributes to
/// a stricter tolerance: tau_eff' = tau_eff / (1 + k * num_witnesses)
/// With k = 0.25, 4 witnesses reduce tau_eff by 50%.
///
/// This mechanism addresses selective defaulting attacks: an attacker who
/// defaults on some creditors but trades honestly with others would normally
/// retain trust from the honest partners. With contagion, failure observations
/// propagate via DHT, causing stricter tolerance even without direct evidence.
pub const CONTAGION_WITNESS_FACTOR: f64 = 0.25;

/// d_w: Discount factor for the aggregate witness rate floor.
/// The median bilateral F/(S+F) across failure witnesses is multiplied by d_w
/// before being used as a floor for the observer's effective rate r_eff.
/// 0.5 = hearsay counts at half weight compared to direct bilateral evidence.
/// This closes the selective defaulting gap where phi(0/tau')=1.0 regardless
/// of how tight tau' becomes — by injecting a nonzero numerator from community
/// observations when the observer has zero bilateral failures.
pub const WITNESS_DISCOUNT: f64 = 0.5;

/// n_min: Minimum number of failure witnesses required before applying
/// the aggregate witness rate floor. Prevents noise from 1-2 isolated
/// observations from triggering imputed rates in legitimate dispute scenarios.
pub const MIN_CONTAGION_WITNESSES: u32 = 3;

/// Number of epochs over which witness contagion observations are considered
/// relevant (Whitepaper §3 Witness-Based Contagion, implementation detail).
///
/// Witnesses older than WITNESS_RELEVANCE_EPOCHS epochs are silently filtered
/// out of the contagion witness count in `contagion.rs`. This prevents
/// permanently penalising reformed agents for defaults from many epochs ago,
/// consistent with the whitepaper's intent that reputation tracks RECENT
/// economic behaviour.
///
/// The whitepaper Def 3.4 does not specify a numeric age cutoff; this value
/// (100 epochs ≈ 100 days at EPOCH_DURATION_SECS = 86400) is an operational
/// constant documented here for transparency.
pub const WITNESS_RELEVANCE_EPOCHS: u64 = 100;

/// VOUCH_TRUST_BASE: Fixed trust-score contribution a single Active vouch adds to the
/// sponsor's local trust row for their vouchee (Whitepaper §5.1, vouch-as-trust-edge).
///
/// Derivation: a vouch guarantees V_staked capacity to the entrant, which is bounded
/// above by V_base = 1000. The trust contribution should represent one "maturation unit"
/// of economic engagement, calibrated to the newcomer extraction bound:
///   VOUCH_TRUST_BASE = TAU_NEWCOMER × V_base = 0.05 × 1000 = 50.0
///
/// This means a single vouch contributes the same trust mass as an honest newcomer
/// who has traded exactly up to their per-identity extraction bound — a conservative
/// starting point that grows with actual transaction history. The attenuation function
/// φ is then applied so that a defaulting vouchee immediately reduces this contribution.
///
/// NOTE: TAU_NEWCOMER = 0.05 and BASE_CAPACITY (V_base) = 1000.0, so this evaluates
/// to 50.0. The expression is written as a product to make the derivation explicit and
/// to keep the value consistent if either parameter is ever changed.
pub const VOUCH_TRUST_BASE: f64 = TAU_NEWCOMER * BASE_CAPACITY; // = 50.0

// =========================================================================
//  ReputationClaim Constants
// =========================================================================

/// Maximum staleness for a ReputationClaim in seconds. A claim is considered
/// fresh if timestamp >= current_timestamp - MAX_CLAIM_STALENESS_SECONDS.
/// A short window (e.g., 15 minutes) prevents "Flash Loan" double-spend attacks limit bypasses.
pub const MAX_CLAIM_STALENESS_SECONDS: u64 = 900;

pub mod vouch_validation_error {
    pub const AUTHOR_NOT_SPONSOR: &str = "EV700000";
    pub const AMOUNT_NOT_POSITIVE: &str = "EV700001";
    pub const AMOUNT_EXCEEDS_MAXIMUM: &str = "EV700002";
    pub const SELF_VOUCH_NOT_ALLOWED: &str = "EV700003";
    pub const DELETE_NOT_AUTHORIZED: &str = "EV700004";
    pub const NEW_VOUCH_STATUS_NOT_ACTIVE: &str = "EV700005";
    pub const NEW_VOUCH_SLASHED_NOT_ZERO: &str = "EV700006";
    pub const UPDATE_AUTHOR_NOT_SPONSOR: &str = "EV700007";
    pub const SPONSOR_CHANGED: &str = "EV700008";
    pub const ENTRANT_CHANGED: &str = "EV700009";
    pub const AMOUNT_CHANGED: &str = "EV700010";
    pub const SLASH_MUST_INCREASE: &str = "EV700011";
    pub const SLASH_EXCEEDS_AMOUNT: &str = "EV700012";
    pub const SLASH_CANNOT_DECREASE: &str = "EV700013";
    pub const INVALID_STATUS_TRANSITION: &str = "EV700014";
    pub const VOUCH_NOT_DELETABLE: &str = "EV700015";
    /// Genesis vouch created after the founding epoch cutoff. Test-epoch only.
    #[cfg(feature = "test-epoch")]
    pub const GENESIS_VOUCH_AFTER_CUTOFF: &str = "EV700016";
    /// is_genesis field cannot be changed after creation.
    pub const IS_GENESIS_CHANGED: &str = "EV700017";
    /// Debtor-initiated slash must supply an expired_contract_hash as proof.
    pub const SLASH_MISSING_PROOF: &str = "EV700018";
    /// The expired_contract_hash supplied as slash proof was not found on the DHT.
    pub const SLASH_PROOF_CONTRACT_NOT_FOUND: &str = "EV700019";
    /// The expired_contract_hash does not point to a DebtContract entry.
    pub const SLASH_PROOF_NOT_CONTRACT: &str = "EV700020";
    /// The contract referenced as slash proof is not Expired or Archived.
    pub const SLASH_PROOF_CONTRACT_NOT_EXPIRED: &str = "EV700021";
    /// The debtor on the referenced contract does not match the vouch entrant.
    pub const SLASH_PROOF_DEBTOR_MISMATCH: &str = "EV700022";
    /// Sponsor lacks sufficient capacity to back the new vouch.
    /// The total of (existing live vouches as sponsor + active debt as debtor +
    /// new vouch amount) exceeds the integrity-zome capacity ceiling for the
    /// sponsor's acquaintance count. Whitepaper Theorem 2.2 requires every
    /// vouch be backed by real staked capacity.
    pub const SPONSOR_CAPACITY_INSUFFICIENT: &str = "EV700023";
}

pub mod reputation_claim_validation_error {
    pub const AUTHOR_NOT_AGENT: &str = "EV600000";
    pub const EPOCH_IN_FUTURE: &str = "EV600001";
    pub const EPOCH_TOO_OLD: &str = "EV600002";
    pub const CAPACITY_BELOW_MINIMUM: &str = "EV600003";
    pub const CAPACITY_ABOVE_MAXIMUM: &str = "EV600004";
    pub const DEBT_NEGATIVE: &str = "EV600005";
    pub const DEBT_EXCEEDS_CAPACITY: &str = "EV600006";
    pub const DUPLICATE_EPOCH: &str = "EV600007";
    pub const UPDATE_NOT_ALLOWED: &str = "EV600008";
    pub const UPDATED_ENTRY_TYPE_MISMATCH: &str = "EV600014";
    pub const PREV_CLAIM_NOT_FOUND: &str = "EV600010";
    pub const PREV_CLAIM_TIMESTAMP_INVALID: &str = "EV600011";
    pub const CUMULATIVE_STATS_MISMATCH: &str = "EV600012";
    pub const EVIDENCE_HASH_MISMATCH: &str = "EV600013";
    pub const DELETE_NOT_ALLOWED: &str = "EV600009";
}

// =========================================================================
//  Link Validation Errors (EV3xxxxx)
// =========================================================================

pub mod link_validation_error {
    pub const AUTHOR_NOT_LINK_BASE: &str = "EV300000";
    pub const INVALID_REPUTATION_CLAIM_LINK: &str = "EV300001";
    pub const INVALID_CHECKPOINT_LINK: &str = "EV300002";
    pub const INVALID_DEBT_BALANCE_LINK: &str = "EV300003";
    pub const INVALID_EPOCH_BUCKET_LINK: &str = "EV300004";
    pub const ENTRANT_VOUCH_TARGET_NOT_ACTION: &str = "EV300005";
    pub const ENTRANT_VOUCH_BASE_MISMATCH: &str = "EV300006";
    pub const ENTRANT_VOUCH_AUTHOR_NOT_SPONSOR: &str = "EV300007";
    pub const ENTRANT_VOUCH_TARGET_NOT_VOUCH: &str = "EV300008";
    pub const ENTRANT_VOUCH_BASE_NOT_AGENT: &str = "EV300009";
    pub const SPONSOR_VOUCH_AUTHOR_MISMATCH: &str = "EV300010";
    pub const SPONSOR_VOUCH_BASE_NOT_AGENT: &str = "EV300011";
    pub const VOUCH_UPDATE_AUTHOR_NOT_SPONSOR: &str = "EV300012";
    pub const VOUCH_UPDATE_BASE_NOT_VOUCH: &str = "EV300013";
    pub const VOUCH_UPDATE_BASE_NOT_ACTION: &str = "EV300014";
    pub const VOUCH_LINK_DELETE_NOT_CREATOR: &str = "EV300015";
    pub const VOUCH_UPDATE_LINK_NOT_DELETABLE: &str = "EV300016";
    pub const CONTRACT_LINK_NOT_DELETABLE: &str = "EV300017";
    pub const UPDATE_ORIGINAL_NOT_CREATE: &str = "EV300018";
    pub const UPDATE_TYPE_MISMATCH: &str = "EV300019";
    pub const DELETE_ORIGINAL_NOT_CREATE: &str = "EV300020";
    pub const DELETE_RECORD_NO_ENTRY: &str = "EV300021";
    pub const DELETE_UNKNOWN_ENTRY_TYPE: &str = "EV300022";
    pub const DELETE_ACTION_NOT_CREATE: &str = "EV300023";
    pub const FAILURE_OBS_TARGET_NOT_AGENT: &str = "EV300024";
    pub const TYPE_MISMATCH: &str = "EV300025";
    pub const CREATE_AGENT_PREV_NOT_AVP: &str = "EV300026";
    pub const CONTRACT_UPDATE_ORIGINAL_NOT_DEBT_CONTRACT: &str = "EV300027";
    pub const VOUCH_UPDATE_ORIGINAL_NOT_VOUCH: &str = "EV300028";
    pub const BLOCKED_TRIAL_AUTHOR_NOT_DEBTOR: &str = "EV300029";
    pub const BLOCKED_TRIAL_TARGET_NOT_AGENT: &str = "EV300030";
    pub const BLOCKED_TRIAL_BASE_NOT_AGENT: &str = "EV300031";
    pub const BLOCKED_TRIAL_LINK_NOT_DELETABLE: &str = "EV300032";
    pub const CONTRACT_DEBTOR_AUTHOR_MISMATCH: &str = "EV300033";
    pub const CONTRACT_CREDITOR_LINK_NOT_DEBTOR: &str = "EV300034";
    pub const CONTRACT_UPDATE_LINK_NOT_DEBTOR: &str = "EV300035";
    pub const FAILURE_OBS_TAG_MALFORMED: &str = "EV300036";
    pub const FAILURE_OBS_CONTRACT_NOT_FOUND: &str = "EV300037";
    pub const FAILURE_OBS_NOT_CONTRACT: &str = "EV300038";
    pub const FAILURE_OBS_CONTRACT_NOT_EXPIRED: &str = "EV300039";
    pub const FAILURE_OBS_DEBTOR_MISMATCH: &str = "EV300040";
    pub const FAILURE_OBS_AUTHOR_NOT_CREDITOR: &str = "EV300041";
    pub const FAILURE_OBS_BASE_NOT_AGENT: &str = "EV300042";
    pub const EXPECTED_ENTRY_CREATION_ACTION: &str = "EV300044";
    pub const EXPECTED_ENTRY_TYPE: &str = "EV300045";
    pub const ORIGINAL_APP_ENTRY_NOT_DEFINED: &str = "EV300046";
    pub const STORE_RECORD_AUTHOR_MISMATCH: &str = "EV300047";
    pub const STORE_RECORD_SPONSOR_MISMATCH: &str = "EV300048";
    pub const CONTRACT_LINK_DELETE_NOT_ARCHIVED: &str = "EV300049";
}

// =========================================================================
//  Coordinator Errors (EC prefix)
//  These are functional errors from coordinator zome operations.
// =========================================================================

pub mod coordinator_transaction_error {
    pub const BUYER_WALLET_NOT_FOUND: &str = "EC200000";
    pub const SELLER_WALLET_NOT_FOUND: &str = "EC200001";
    pub const CAPACITY_EXCEEDED: &str = "EC200002";
    pub const CREATED_TX_NOT_FOUND: &str = "EC200003";
    pub const MALFORMED_GET_DETAILS: &str = "EC200004";
    pub const ORIGINAL_TX_NOT_FOUND: &str = "EC200005";
    pub const ORIGINAL_ENTRY_HASH_NOT_FOUND: &str = "EC200006";
    pub const UPDATED_TX_NOT_FOUND: &str = "EC200007";
    pub const UPDATED_ENTRY_HASH_NOT_FOUND: &str = "EC200008";
    pub const TX_CONVERSION_FAILED: &str = "EC200009";
    pub const SELLER_WALLET_RESOLVE_FAILED: &str = "EC200010";
    pub const APPROVE_TX_NOT_FOUND: &str = "EC200011";
    pub const APPROVE_INVALID_ENTRY: &str = "EC200012";
    pub const APPROVE_NOT_SELLER: &str = "EC200013";
    pub const APPROVE_NOT_PENDING: &str = "EC200014";
    pub const REJECT_TX_NOT_FOUND: &str = "EC200015";
    pub const REJECT_INVALID_ENTRY: &str = "EC200016";
    pub const REJECT_NOT_SELLER: &str = "EC200017";
    pub const REJECT_NOT_PENDING: &str = "EC200018";
    /// The remote caller of create_buyer_debt_contract is not the buyer on this transaction.
    /// This is a security check to prevent a third party from triggering contract creation.
    pub const CREATE_CONTRACT_CALLER_NOT_BUYER: &str = "EC200021";
    /// Seller attempted to approve a trial transaction but is already at the velocity limit
    /// for the current epoch. The approval is rejected to prevent exceeding TRIAL_VELOCITY_LIMIT_PER_EPOCH.
    pub const APPROVE_TRIAL_VELOCITY_EXCEEDED: &str = "EC200022";
    /// A trial transaction was attempted while an existing trial contract between
    /// this buyer and seller is still Active (not yet Transferred via repayment).
    /// The trial slot is released only when the buyer successfully repays (Transferred).
    /// Expiry/default does NOT release the slot — the buyer must pay to earn a new trial.
    pub const OPEN_TRIAL_EXISTS: &str = "EC200019";
    /// A trial transaction was attempted but the (buyer, seller) pair is permanently blocked.
    /// This block is written when a trial contract between this buyer and seller expires/defaults.
    /// It cannot be lifted — the pair is barred from trials permanently.
    pub const TRIAL_PAIR_PERMANENTLY_BLOCKED: &str = "EC200020";
    /// Transaction debt amount must be a finite positive number.
    /// Triggered when debt is zero, negative, NaN, or Infinity.
    pub const DEBT_MUST_BE_POSITIVE: &str = "EC200023";
    /// Buyer and seller must be different agents.
    /// Self-dealing transactions create circular debt with no economic meaning.
    pub const BUYER_IS_SELLER: &str = "EC200024";
}

pub mod coordinator_vouch_error {
    pub const INSUFFICIENT_CAPACITY: &str = "EC700000";
    pub const CREATED_VOUCH_NOT_FOUND: &str = "EC700001";
    pub const RELEASE_VOUCH_NOT_FOUND: &str = "EC700002";
    pub const RELEASE_INVALID_ENTRY: &str = "EC700003";
    pub const RELEASE_NOT_SPONSOR: &str = "EC700004";
    pub const RELEASE_INVALID_STATUS: &str = "EC700005";
    pub const RELEASE_UPDATED_NOT_FOUND: &str = "EC700006";
    /// Sponsor cannot release a vouch while the entrant still has active debt contracts.
    /// Releasing while active contracts exist would allow a sponsor to escape slash
    /// liability for defaults on those contracts. The release must be deferred until
    /// the entrant's active contracts are fully transferred, expired, or archived.
    pub const RELEASE_ENTRANT_HAS_ACTIVE_CONTRACTS: &str = "EC700008";
    /// genesis_vouch was called — test-epoch mode only.
    #[cfg(feature = "test-epoch")]
    pub const GENESIS_VOUCH_EPOCH_EXPIRED: &str = "EC700007";
}

pub mod coordinator_contract_error {
    pub const CREATED_CONTRACT_NOT_FOUND: &str = "EC400000";
}

pub mod coordinator_wallet_error {
    /// The newly created Wallet record was not found after creation.
    pub const CREATED_WALLET_NOT_FOUND: &str = "EC100000";
    /// The newly updated Wallet record was not found after update.
    pub const UPDATED_WALLET_NOT_FOUND: &str = "EC100001";
    /// No action hash associated with the wallet link.
    pub const WALLET_LINK_NO_ACTION_HASH: &str = "EC100002";
    /// Malformed get_details response for wallet resolution.
    pub const WALLET_GET_DETAILS_MALFORMED: &str = "EC100003";
}

pub mod coordinator_support_error {
    /// The newly created SupportBreakdown record was not found after creation.
    pub const CREATED_BREAKDOWN_NOT_FOUND: &str = "EC800001";
    /// The newly updated SupportBreakdown record was not found after update.
    pub const UPDATED_BREAKDOWN_NOT_FOUND: &str = "EC800002";
    /// No action hash associated with the support breakdown link.
    pub const BREAKDOWN_LINK_NO_ACTION_HASH: &str = "EC800003";
    /// Malformed get_details response for support breakdown resolution.
    pub const BREAKDOWN_GET_DETAILS_MALFORMED: &str = "EC800004";
    /// No entry in support breakdown record.
    pub const BREAKDOWN_NO_ENTRY: &str = "EC800005";
}

pub mod coordinator_checkpoint_error {
    /// No actions found on source chain (chain is empty).
    pub const CHAIN_EMPTY: &str = "EC900000";
}

pub mod link_resolution_error {
    /// Failed to fetch the CreateLink action for a DeleteLink.
    pub const FETCH_CREATE_LINK_FAILED: &str = "EC010000";
    /// CreateLink action expected but not found.
    pub const CREATE_LINK_NOT_FOUND: &str = "EC010001";
    /// Bad ranking index component.
    pub const BAD_RANKING_COMPONENT: &str = "EC010002";
}

pub mod type_resolution_error {
    /// Agent is not a party (buyer or seller) on the transaction.
    pub const AGENT_NOT_TRANSACTION_PARTY: &str = "EC020000";
    /// Could not resolve wallet entry hash.
    pub const WALLET_ENTRY_HASH_RESOLVE_FAILED: &str = "EC020001";
    /// No action hash associated with link.
    pub const LINK_NO_ACTION_HASH: &str = "EC020002";
    /// Linked action must reference an entry.
    pub const LINK_NO_ENTRY: &str = "EC020003";
    /// Base must be an agent public key.
    pub const BASE_NOT_AGENT: &str = "EC020004";
}

pub mod coordinator_cascade_error {
    /// The caller of handle_drain_request is not listed as a supporter
    /// (i.e. does not have a SupportBreakdown that lists this agent as a beneficiary).
    pub const REQUESTER_NOT_SUPPORTER: &str = "EC800000";
}

pub mod coordinator_trust_error {
    pub const CONTRACT_RECORD_NOT_FOUND: &str = "EC500000";
    pub const REPUTATION_CLAIM_NOT_FOUND: &str = "EC600000";
}

// =========================================================================
//  Chain Scan Limits
// =========================================================================

/// DUST_THRESHOLD: Minimum meaningful amount for debt operations.
///
/// Amounts at or below this threshold are treated as economically zero:
/// contracts with residual debt ≤ DUST_THRESHOLD are considered fully
/// transferred, and transfer/slash iterations stop when the remaining
/// amount drops to or below this value.
///
/// Chosen as 0.01 to be negligible relative to V_base = 1000 (0.001%)
/// while avoiding exact floating-point zero comparisons that can fail
/// due to accumulated rounding error in multi-hop cascades.
pub const DUST_THRESHOLD: f64 = 0.01;

/// Maximum number of source chain entries to scan per reputation claim validation.
/// This bounds validator work for a single claim while being large enough to
/// accommodate high-frequency traders. An agent who creates more contracts than
/// this between consecutive claims must publish claims more frequently.
///
/// With MAX_VOLUME_PER_EPOCH = 100 per counterparty and 150 max acquaintances,
/// worst-case new contracts per epoch ≈ 150 * 100 = 15,000. At one claim per
/// epoch, 50,000 provides ample headroom.
pub const MAX_CONTRACTS_PER_CLAIM_SCAN: u64 = 50_000;
