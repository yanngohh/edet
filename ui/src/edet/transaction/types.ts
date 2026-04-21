import type {
  ActionHash,
  AgentPubKey,
  AgentPubKeyB64,
  Create,
  CreateLink,
  Delete,
  DeleteLink,
  DnaHash,
  EntryHash,
  EntryHashB64,
  Record,
  SignedActionHashed,
  Update,
} from "@holochain/client";

export type TransactionSignal = {
  type: "EntryCreated";
  action: SignedActionHashed<Create>;
  app_entry: EntryTypes;
} | {
  type: "EntryUpdated";
  action: SignedActionHashed<Update>;
  app_entry: EntryTypes;
  original_app_entry: EntryTypes;
} | {
  type: "EntryDeleted";
  action: SignedActionHashed<Delete>;
  original_app_entry: EntryTypes;
} | {
  type: "LinkCreated";
  action: SignedActionHashed<CreateLink>;
  link_type: string;
} | {
  type: "LinkDeleted";
  action: SignedActionHashed<DeleteLink>;
  link_type: string;
};

/* dprint-ignore-start */
export type EntryTypes =
  | ({ type: 'Transaction'; } & Transaction)
  | ({ type: 'Wallet'; } & Wallet)
  | ({ type: 'DebtContract'; } & DebtContract);
/* dprint-ignore-end */

export interface Wallet {
  owner: AgentPubKeyB64;

  auto_reject_threshold: number;

  auto_accept_threshold: number;

  /// Cumulative staking capacity permanently consumed by vouchee defaults.
  total_slashed_as_sponsor: number;

  /// Number of trial transactions approved in the current epoch (by this seller).
  trial_tx_count: number;

  /// Epoch in which trial_tx_count was last updated.
  last_trial_epoch: number;
}


export interface TransactionSide {
  type:
  | 'Seller'
  | 'Buyer';
}

export interface TransactionStatus {
  type:
  | 'Pending'
  | 'Accepted'
  | 'Rejected'
  | 'Canceled'
  | 'Testing';
}

export interface Party {
  side: TransactionSide;
  pubkey: AgentPubKeyB64;
  previous_transaction: ActionHash | null;
  wallet: ActionHash;
}


/// Metadata present on support cascade drain transactions.
/// A drain transaction is created by a supporter on the beneficiary's cell,
/// allowing manual moderation of debt drain requests.
export interface DrainMetadata {
  /// ActionHash of the originating buyer→seller transaction
  parent_tx: ActionHash;
  /// Cascade depth: 0 = direct, 1 = second-level, etc.
  cascade_depth: number;
  /// Amount allocated from the cascade waterfilling pass
  allocated_amount: number;
  /// Visited agents for cycle detection
  visited: AgentPubKeyB64[];
}

export interface Transaction {
  seller: Party;

  buyer: Party;

  debt: number;

  description: string;

  status: TransactionStatus;

  /// Immutably set at creation. True iff debt < eta * base_capacity at creation time.
  /// Used for display (trial badge) and gate enforcement.
  is_trial: boolean;

  parent?: EntryHashB64;

  /// If set, this is a cascade drain request (not a purchase transaction).
  drain_metadata?: DrainMetadata;
}

/// Returns true if the transaction is a support cascade drain request.
export function isDrainTransaction(tx: Transaction): boolean {
  return !!tx.drain_metadata;
}

// =========================================================================
//  Debt Contract Types (Whitepaper Definition 1)
// =========================================================================

export interface ContractStatus {
  type:
  | 'Active'
  | 'Transferred'
  | 'Expired'
  | 'Archived';  // Terminal state after ARCHIVE_AFTER_EPOCHS — Rust ContractStatus::Archived
}

export interface DebtContract {
  amount: number;
  /// Immutable original principal at contract creation. Used to compute debt velocity.
  original_amount: number;
  maturity: number;
  start_epoch: number;
  creditor: AgentPubKeyB64;
  debtor: AgentPubKeyB64;
  transaction_hash: ActionHash;
  co_signers?: [AgentPubKeyB64, number][];
  status: ContractStatus;
  is_trial: boolean;
}

// =========================================================================
//  Trust & Reputation Types (Whitepaper Section 7.1)
// =========================================================================

export interface ReputationResult {
  trust: number;
  acquaintance_count: number;
}

/**
 * Community standing metrics for a wallet, showing failure witness
 * count and aggregate witness rate (median bilateral F/(S+F) across
 * witnesses, used for contagion-based trust attenuation).
 */
export interface CommunityStanding {
  /** Number of independent creditors who observed this agent default. */
  witnessCount: number;
  /** Median bilateral failure rate across witnesses (0.0 if < 3 witnesses). */
  aggregateWitnessRate: number;
}

export interface ClaimCumulativeStats {
  total_contracts_processed: number;
  total_amount_transferred: number;
  total_amount_expired: number;
}

export interface ReputationClaim {
  agent: AgentPubKeyB64;
  capacity_lower_bound: number;
  debt_upper_bound: number;
  successful_transfers: number;
  distinct_counterparties: number;
  timestamp: number;
  evidence_hash: Uint8Array;
  last_processed_contract: ActionHash | null;
  cumulative_stats: ClaimCumulativeStats;
  prev_claim_hash: ActionHash | null;
}

export interface DebtTransferResult {
  transferred: number;
  creditor_transfers: [AgentPubKeyB64, number][];
}

export interface SupportCascadeResult {
  own_transferred: number;
  beneficiary_transferred: number;
  genesis_amount: number;
  own_creditor_transfers: [AgentPubKeyB64, number][];
  beneficiary_drains: [AgentPubKeyB64, number][];
}

export interface ExpirationResult {
  creditor_failures: [AgentPubKeyB64, number][];
  total_expired: number;
}

export interface NextDeadline {
  timestamp: number;
  amount: number;
}

// =========================================================================
//  Ranking / Cursor Types
// =========================================================================

export interface TransactionStatusTag {
  type:
  | 'Pending'
  | 'Finalized';
}


export interface GetRankingDirection {
  type:
  | 'Ascendent'
  | 'Descendent';
}

/**
 * Controls how drain (support) transactions are filtered in query results.
 * - IncludeAll: include all drain transactions (for pending moderation, etc.)
 * - ExcludeAll: exclude all drain transactions from results
 * - BeneficiaryOnly: include only drains where the caller is the beneficiary (seller),
 *   hiding them from the supporter's (buyer's) view
 */
export type DrainFilterMode = 'IncludeAll' | 'ExcludeAll' | 'BeneficiaryOnly';

export interface GetTransactionsCursor {
  from_timestamp: number,
  tag: TransactionStatusTag,
  count: number,
  direction: GetRankingDirection,
  drain_filter: DrainFilterMode,
}

/**
 * Paginated result from `get_transactions`.
 * `next_cursor` is the timestamp to use for the next page, or null when no more pages exist.
 */
export interface PaginatedTransactionsResult {
  records: Record[],
  next_cursor: number | null,
}
