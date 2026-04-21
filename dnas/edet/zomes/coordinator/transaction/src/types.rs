use hdk::prelude::Record;
use serde::{Deserialize, Serialize};
use transaction_integrity::types::TransactionStatusTag;
use transaction_integrity::TransactionStatus;

use crate::ranking_index::GetRankingDirection;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TransactionSimulationResult {
    pub status: TransactionStatus,
    pub is_trial: bool,
}

/// Controls how drain (support) transactions are filtered in query results.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DrainFilterMode {
    /// Include all drain transactions (for pending moderation, reputation, etc.)
    IncludeAll,
    /// Exclude all drain transactions from results.
    ExcludeAll,
    /// Include only drains where the caller is the beneficiary (seller).
    /// Hides drains from the supporter's (buyer's) view while keeping them
    /// visible to beneficiaries who need to see their debt relief records.
    BeneficiaryOnly,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GetTransactionsCursor {
    pub from_timestamp: i64,
    pub tag: TransactionStatusTag,
    pub count: usize,
    pub direction: GetRankingDirection,
    pub drain_filter: DrainFilterMode,
}

/// Paginated result for `get_transactions`.
/// `next_cursor` is `Some(timestamp)` when more pages exist, `None` when exhausted.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PaginatedTransactionsResult {
    pub records: Vec<Record>,
    pub next_cursor: Option<i64>,
}
