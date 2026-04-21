//! EDET Integration Tests using Holochain Sweettest
//!
//! This crate contains Rust-based integration tests for the EDET Holochain zomes.
//! These tests replace the TypeScript/Tryorama tests which have version incompatibility
//! issues with Holochain 0.6.x.
//!
//! Test categories:
//! - Lifecycle tests: Multi-agent transaction workflows
//! - Validation tests: Negative tests for integrity validation
//! - Trust tests: EigenTrust computation and reputation
//! - Attack tests: Security scenarios and attack resistance
//! - Cascade tests: Support cascade functionality
//! - Additional validation tests: Wallet and debt contract edge cases
//! - ReputationClaim tests: Claim lifecycle and first-contact flow
//! - Scalability tests: Archival, checkpoints, batch fetching
//! - Security theorem tests: Formal property verification
//! - Alignment tests: Trial velocity and co-signer penalties
//! - Attenuation tests: Trust attenuation function and volume tolerance
//! - Risk score tests: Risk computation paths and thresholds
//! - Capacity tests: Credit capacity formula verification
//! - Contagion tests: Witness-based failure contagion
//! - EigenTrust tests: Trust distribution and convergence properties
//! - Contract lifecycle tests: Contract creation, transfer, and co-signers
//! - Vouch advanced tests: Multi-vouch accumulation and link propagation
//! - Epoch tests: Contract maturity, expiration, and test-epoch gate
//! - Moderation tests: Seller approve/reject flows and access control
//!
//! - Flash loan / circular trading tests: Simulation-level attack coverage ported to sweettest
//! - Multi-conductor tests: DHT propagation across separate conductor processes
//!
//! Total: 113 tests (82 existing + 3 reconciliation + 9 query/utility + 6 concurrent + 13 query tier-3)
#![allow(unused)]
use holochain::conductor::api::error::ConductorApiResult;
use holochain::prelude::*;
use holochain::sweettest::{await_consistency, SweetApp, SweetCell, SweetConductor, SweetConductorBatch, SweetDnaFile};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// Re-export types from transaction_integrity for convenience
pub use transaction_integrity::{
    debt_contract::{ContractStatus, DebtContract},
    types::{
        constants::{coordinator_transaction_error, transaction_validation_error},
        TransactionStatusTag,
    },
    vouch::VouchStatus,
    ReputationClaim, Transaction, TransactionStatus, Vouch, Wallet,
};
// AgentPubKeyB64 comes from holo_hash
pub use holo_hash::AgentPubKeyB64;

// ============================================================================
//  Test Modules
// ============================================================================

#[cfg(test)]
mod additional_validation_tests;
#[cfg(test)]
mod alignment_tests;
#[cfg(test)]
mod attack_tests;
#[cfg(test)]
mod cascade_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod reputation_claim_tests;
#[cfg(test)]
mod scalability_tests;
#[cfg(test)]
mod security_theorem_tests;
#[cfg(test)]
mod trust_tests;
#[cfg(test)]
mod validation_tests;

// New test modules
#[cfg(test)]
mod attenuation_tests;
#[cfg(test)]
mod capacity_tests;
#[cfg(test)]
mod contagion_tests;
#[cfg(test)]
mod contract_lifecycle_tests;
#[cfg(test)]
mod eigentrust_tests;
#[cfg(test)]
mod epoch_tests;
#[cfg(test)]
mod risk_score_tests;
#[cfg(test)]
mod support_drain_tests;
#[cfg(test)]
mod vouch_advanced_tests;

// Moderation tests
#[cfg(test)]
mod moderation_tests;

// Stress tests
#[cfg(test)]
mod stress_tests;

// Flash loan and circular trading tests (simulation-level coverage ported to sweettest)
#[cfg(test)]
mod flash_loan_circular_tests;

// Multi-conductor DHT propagation tests
#[cfg(test)]
mod multi_conductor_tests;

// Reconciliation tests (Tier 1 — critical data integrity recovery)
#[cfg(test)]
mod reconciliation_tests;

// Query function tests (Tier 2 — moderate priority read functions)
#[cfg(test)]
mod query_tests;

// Utility function tests (Tier 3 — simple get/read utilities)
#[cfg(test)]
mod utility_tests;

// Concurrent operation and boundary value tests
#[cfg(test)]
mod concurrent_tests;

// ============================================================================
//  Test Setup Helpers
// ============================================================================

/// How long to sleep to cross one epoch boundary in test-epoch mode.
///
/// With `test-epoch` feature: EPOCH_DURATION_SECS=1, MIN_MATURITY=3.
/// Sleeping for `EPOCH_SLEEP_MS * (MIN_MATURITY + 1)` epochs is enough to
/// trigger contract expiration in process_contract_expirations().
pub const EPOCH_SLEEP_MS: u64 = 1100; // 1.1 seconds -- slightly over 1-second epoch boundary

/// How many epochs to sleep to cross MIN_MATURITY=10 (test-epoch mode).
pub const MATURITY_EPOCHS: u64 = 11; // 10 (MIN_MATURITY) + 1 safety margin

/// Get the path to the compiled DNA bundle.
/// Uses the test DNA (built with `--features test-epoch`) when running
/// `npm test` / `npm run build:test-dna`, which outputs to edet-test.dna.
pub fn dna_path() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let base = PathBuf::from(manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("dnas/edet/workdir");

    // Prefer the test DNA (built with test-epoch feature) when available.
    // Fall back to the standard DNA for environments that build it directly.
    let test_dna = base.join("edet-test.dna");
    if test_dna.exists() {
        test_dna
    } else {
        base.join("edet.dna")
    }
}

/// Set up a single conductor with one agent
pub async fn setup_single_agent() -> (SweetConductor, SweetApp) {
    let mut conductor = SweetConductor::from_standard_config().await;
    let dna = SweetDnaFile::from_bundle(&dna_path()).await.expect("Failed to load DNA");
    let app = conductor.setup_app("edet", &[dna]).await.expect("Failed to setup app");

    // Trigger init to ensure wallet is created
    let cell = app.cells()[0].clone();
    let agent = cell.agent_pubkey().clone();
    let _ = get_wallet_for_agent(&conductor, &cell, agent).await;

    await_consistency(30, [&cell]).await.unwrap();
    (conductor, app)
}

/// Set up a conductor with multiple agents for multi-agent tests
pub async fn setup_multi_agent(num_agents: usize) -> (SweetConductor, Vec<SweetApp>) {
    let mut conductor = SweetConductor::from_standard_config().await;
    let dna = SweetDnaFile::from_bundle(&dna_path()).await.expect("Failed to load DNA");
    let mut apps = Vec::new();

    for i in 0..num_agents {
        let app = conductor
            .setup_app(&format!("edet-{i}"), std::slice::from_ref(&dna))
            .await
            .expect("Failed to setup app");
        apps.push(app);
    }

    // Trigger init for all agents to ensure wallets are created
    for app in &apps {
        let cell = app.cells()[0].clone();
        let agent = cell.agent_pubkey().clone();
        let _ = get_wallet_for_agent(&conductor, &cell, agent).await;
    }

    let cells: Vec<SweetCell> = apps.iter().map(|a| a.cells()[0].clone()).collect();
    await_consistency(30, &cells).await.unwrap();

    // Bootstrap genesis vouching: all agents vouch for each other.
    // Mirrors simulation's _setup_genesis_vouching (verify_theory.py:54-80).
    bootstrap_genesis_vouching(&conductor, &apps).await;

    (conductor, apps)
}

/// Set up a conductor with multiple agents but NO genesis vouching.
/// Useful for testing unvouched behavior and capacity edge cases.
pub async fn setup_multi_agent_no_vouch(num_agents: usize) -> (SweetConductor, Vec<SweetApp>) {
    let mut conductor = SweetConductor::from_standard_config().await;
    let dna = SweetDnaFile::from_bundle(&dna_path()).await.expect("Failed to load DNA");
    let mut apps = Vec::new();

    for i in 0..num_agents {
        let app = conductor
            .setup_app(&format!("edet-{i}"), std::slice::from_ref(&dna))
            .await
            .expect("Failed to setup app");
        apps.push(app);
    }

    // Trigger init for all agents to ensure wallets are created
    for app in &apps {
        let cell = app.cells()[0].clone();
        let agent = cell.agent_pubkey().clone();
        let _ = get_wallet_for_agent(&conductor, &cell, agent).await;
    }

    let cells: Vec<SweetCell> = apps.iter().map(|a| a.cells()[0].clone()).collect();
    await_consistency(30, &cells).await.unwrap();

    (conductor, apps)
}

// ============================================================================
//  Zome Call Input/Output Types
// ============================================================================

/// Input for creating a transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTransactionInput {
    pub seller: AgentPubKeyB64,
    pub buyer: AgentPubKeyB64,
    pub description: String,
    #[serde(default)]
    pub debt: f64,
}

/// Input for creating a vouch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVouchInput {
    pub sponsor: AgentPubKeyB64,
    pub entrant: AgentPubKeyB64,
    pub amount: f64,
}

/// Input for creating a debt contract
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDebtContractInput {
    pub amount: f64,
    pub creditor: AgentPubKeyB64,
    pub debtor: AgentPubKeyB64,
    pub transaction_hash: ActionHash,
    pub is_trial: bool,
}

/// Input for approving or rejecting a pending transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerateTransactionInput {
    pub original_transaction_hash: ActionHash,
    pub previous_transaction_hash: ActionHash,
    pub transaction: Transaction,
}

/// Input for updating a wallet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateWalletInput {
    pub original_wallet_hash: ActionHash,
    pub previous_wallet_hash: ActionHash,
    pub updated_wallet: Wallet,
}

/// Input for creating a support breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSupportBreakdownInput {
    pub owner: AgentPubKeyB64,
    pub addresses: Vec<AgentPubKeyB64>,
    pub coefficients: Vec<f64>,
}

/// Subjective reputation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectiveReputation {
    pub trust: f64,
    pub acquaintance_count: u64,
}

/// Trust cache stats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustCacheStats {
    pub num_cached_reputations: u64,
    pub num_cached_dht_trust_rows: u64,
    #[serde(default)]
    pub num_cached_witness_contagion: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ExpirationResult {
    pub creditor_failures: Vec<(AgentPubKeyB64, f64)>,
    pub total_expired: f64,
    pub total_slashed_dispatched: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum GetRankingDirection {
    Ascendent,
    Descendent,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum DrainFilterMode {
    IncludeAll,
    ExcludeAll,
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PaginatedTransactionsResult {
    pub records: Vec<Record>,
    pub next_cursor: Option<i64>,
}

/// Archive result type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivalResult {
    pub archived_count: u32,
    pub archived_amount: f64,
}

/// Checkpoint type (for deserialization)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainCheckpointOutput {
    pub agent: AgentPubKeyB64,
    pub epoch: u64,
    pub sequence: u64,
}

// ============================================================================
//  Test Helper Functions
// ============================================================================

/// Ensure that a wallet for the target agent is visible to the source cell
/// This prevents race conditions in multi-agent tests where gossip hasn't propagated yet
pub async fn ensure_wallet_propagation(
    conductor: &SweetConductor,
    source_cell: &SweetCell,
    target_agent: AgentPubKey,
) -> Result<(), String> {
    let mut retries = 0;
    loop {
        let result = get_wallet_for_agent(conductor, source_cell, target_agent.clone()).await;
        if let Ok((Some(_), Some(_))) = result {
            return Ok(());
        }

        retries += 1;
        if retries > 60 {
            // Wait up to 30 seconds
            return Err(format!("Timeout waiting for wallet propagation for agent {target_agent}"));
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Ensure that a transaction for the seller is visible to the seller's cell
pub async fn ensure_transaction_propagation_seller(
    conductor: &SweetConductor,
    cell: &SweetCell,
    seller: AgentPubKey,
    tag: TransactionStatusTag,
) -> Result<(), String> {
    let mut retries = 0;
    loop {
        let result = get_transactions_for_seller(conductor, cell, seller.clone(), tag.clone()).await;
        if let Ok(records) = result {
            if !records.is_empty() {
                return Ok(());
            }
        }

        retries += 1;
        if retries > 60 {
            return Err(format!("Timeout waiting for transaction propagation for seller {seller}"));
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Create a transaction via zome call
pub async fn create_transaction(
    conductor: &SweetConductor,
    cell: &SweetCell,
    input: CreateTransactionInput,
) -> ConductorApiResult<Record> {
    let mut transaction = Transaction::default();
    transaction.buyer.pubkey = input.buyer;
    transaction.seller.pubkey = input.seller;
    transaction.debt = input.debt;
    transaction.description = input.description;
    transaction.status = TransactionStatus::Pending;

    conductor
        .call_fallible(&cell.zome("transaction"), "create_transaction", transaction)
        .await
}

/// Update a transaction via zome call
pub async fn update_transaction(
    conductor: &SweetConductor,
    cell: &SweetCell,
    transaction: Transaction,
    original_action_hash: ActionHash,
) -> ConductorApiResult<Record> {
    #[derive(Debug, Serialize, Deserialize)]
    struct UpdateTransactionInput {
        pub original_transaction_hash: ActionHash,
        pub previous_transaction_hash: ActionHash,
        pub updated_transaction: Transaction,
    }

    let input = UpdateTransactionInput {
        original_transaction_hash: original_action_hash.clone(),
        previous_transaction_hash: original_action_hash,
        updated_transaction: transaction,
    };

    conductor
        .call_fallible(&cell.zome("transaction"), "update_transaction", input)
        .await
}

/// Approve a pending transaction (seller action).
pub async fn approve_pending_transaction(
    conductor: &SweetConductor,
    cell: &SweetCell,
    original_hash: ActionHash,
    previous_hash: ActionHash,
) -> ConductorApiResult<Record> {
    #[derive(Debug, Serialize, Deserialize)]
    struct ModerateTransactionInput {
        pub original_transaction_hash: ActionHash,
        pub previous_transaction_hash: ActionHash,
        pub transaction: Transaction,
    }

    let record: Record = conductor
        .call(&cell.zome("transaction"), "get_latest_transaction", original_hash.clone())
        .await;
    let transaction: Transaction = record.entry().to_app_option().unwrap().unwrap();

    let input = ModerateTransactionInput {
        original_transaction_hash: original_hash,
        previous_transaction_hash: previous_hash,
        transaction,
    };
    conductor
        .call_fallible(&cell.zome("transaction"), "approve_pending_transaction", input)
        .await
}

/// Create a vouch via zome call
pub async fn create_vouch(
    conductor: &SweetConductor,
    cell: &SweetCell,
    input: CreateVouchInput,
) -> ConductorApiResult<Record> {
    conductor.call_fallible(&cell.zome("transaction"), "create_vouch", input).await
}

/// Genesis vouch (no capacity check) -- for founding cohort bootstrap.
/// Mirrors simulation's _setup_genesis_vouching (verify_theory.py:54-80).
pub async fn genesis_vouch(
    conductor: &SweetConductor,
    cell: &SweetCell,
    input: CreateVouchInput,
) -> ConductorApiResult<Record> {
    conductor.call_fallible(&cell.zome("transaction"), "genesis_vouch", input).await
}

/// Bootstrap genesis vouching: every agent vouches for every other agent.
/// Mirrors the simulation's `_setup_genesis_vouching()` which ensures
/// all genesis nodes have staked capacity before the first transaction.
/// Per the whitepaper (Theorem 5.1): unvouched nodes have Cap = V_staked,
/// and V_staked = 0 without vouches.
pub async fn bootstrap_genesis_vouching(conductor: &SweetConductor, apps: &[SweetApp]) {
    let vouch_amount = 500.0; // Half of MAX_VOUCH_AMOUNT, leaves room for test-specific vouches
    for (i, app_i) in apps.iter().enumerate() {
        let sponsor_cell = app_i.cells()[0].clone();
        let sponsor_agent: AgentPubKey = sponsor_cell.agent_pubkey().clone();
        for (j, app_j) in apps.iter().enumerate() {
            if i == j {
                continue;
            }
            let entrant_agent: AgentPubKey = app_j.cells()[0].agent_pubkey().clone();
            let input = CreateVouchInput {
                sponsor: sponsor_agent.clone().into(),
                entrant: entrant_agent.clone().into(),
                amount: vouch_amount,
            };
            genesis_vouch(conductor, &sponsor_cell, input)
                .await
                .unwrap_or_else(|e| panic!("Genesis vouch {i} -> {j} failed: {e:?}"));
        }
    }
    // Allow vouch link propagation on DHT
    let cells: Vec<SweetCell> = apps.iter().map(|a| a.cells()[0].clone()).collect();
    await_consistency(30, &cells).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
}

pub async fn get_wallet_for_agent(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<(Option<ActionHash>, Option<Record>)> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_wallet_for_agent", agent)
        .await
}

/// Get total debt for an agent
pub async fn get_total_debt(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<f64> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_total_debt", agent)
        .await
}

/// Get vouched capacity for an agent
pub async fn get_vouched_capacity(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<f64> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_vouched_capacity", agent)
        .await
}

/// Check bilateral history between current agent and another
pub async fn check_bilateral_history(
    conductor: &SweetConductor,
    cell: &SweetCell,
    other: AgentPubKey,
) -> ConductorApiResult<bool> {
    conductor
        .call_fallible(&cell.zome("transaction"), "check_bilateral_history", other)
        .await
}

/// Get transactions for a seller
pub async fn get_transactions_for_seller(
    conductor: &SweetConductor,
    cell: &SweetCell,
    seller: AgentPubKey,
    tag: TransactionStatusTag,
) -> ConductorApiResult<Vec<Record>> {
    let cursor = GetTransactionsCursor {
        from_timestamp: 0,
        tag,
        count: 100,
        direction: GetRankingDirection::Descendent,
        drain_filter: DrainFilterMode::IncludeAll,
    };
    let result: PaginatedTransactionsResult = conductor
        .call_fallible(&cell.zome("transaction"), "get_transactions", cursor)
        .await?;
    let records = result.records;

    Ok(records
        .into_iter()
        .filter(|r| {
            if let Ok(Some(tx)) = r.entry().to_app_option::<Transaction>() {
                let tx_seller: AgentPubKey = tx.seller.pubkey.into();
                tx_seller == seller
            } else {
                false
            }
        })
        .collect())
}

/// Get transactions for a buyer
pub async fn get_transactions_for_buyer(
    conductor: &SweetConductor,
    cell: &SweetCell,
    buyer: AgentPubKey,
    tag: TransactionStatusTag,
) -> ConductorApiResult<Vec<Record>> {
    let cursor = GetTransactionsCursor {
        from_timestamp: 0,
        tag,
        count: 100,
        direction: GetRankingDirection::Descendent,
        drain_filter: DrainFilterMode::IncludeAll,
    };
    let result: PaginatedTransactionsResult = conductor
        .call_fallible(&cell.zome("transaction"), "get_transactions", cursor)
        .await?;
    let records = result.records;

    Ok(records
        .into_iter()
        .filter(|r| {
            if let Ok(Some(tx)) = r.entry().to_app_option::<Transaction>() {
                let tx_buyer: AgentPubKey = tx.buyer.pubkey.into();
                tx_buyer == buyer
            } else {
                false
            }
        })
        .collect())
}

/// Publish a reputation claim
pub async fn publish_reputation_claim(conductor: &SweetConductor, cell: &SweetCell) -> ConductorApiResult<Record> {
    conductor
        .call_fallible(&cell.zome("transaction"), "publish_reputation_claim", ())
        .await
}

/// Get reputation claim for an agent
pub async fn get_reputation_claim(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<Option<ReputationClaim>> {
    let result: ConductorApiResult<Option<(ActionHash, ReputationClaim)>> = conductor
        .call_fallible(&cell.zome("transaction"), "get_reputation_claim", agent)
        .await;
    Ok(result?.map(|(_, claim)| claim))
}

/// Get subjective reputation for an agent
pub async fn get_subjective_reputation(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<SubjectiveReputation> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_subjective_reputation", agent)
        .await
}

/// Get trust cache stats
pub async fn get_trust_cache_stats(
    conductor: &SweetConductor,
    cell: &SweetCell,
) -> ConductorApiResult<TrustCacheStats> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_trust_cache_stats", ())
        .await
}

/// Invalidate trust caches
pub async fn invalidate_trust_caches(conductor: &SweetConductor, cell: &SweetCell) -> ConductorApiResult<()> {
    conductor
        .call_fallible(&cell.zome("transaction"), "invalidate_trust_caches", ())
        .await
}

/// Publish trust row
pub async fn publish_trust_row(conductor: &SweetConductor, cell: &SweetCell) -> ConductorApiResult<()> {
    conductor
        .call_fallible(&cell.zome("transaction"), "publish_trust_row", ())
        .await
}

/// Input for updating a support breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSupportBreakdownInput {
    pub original_support_breakdown_hash: ActionHash,
    pub previous_support_breakdown_hash: ActionHash,
    pub updated_support_breakdown: CreateSupportBreakdownInput,
}

/// Create a support breakdown
pub async fn create_support_breakdown(
    conductor: &SweetConductor,
    cell: &SweetCell,
    input: CreateSupportBreakdownInput,
) -> ConductorApiResult<Record> {
    conductor
        .call_fallible(&cell.zome("support"), "create_support_breakdown", input)
        .await
}

/// Update a support breakdown
pub async fn update_support_breakdown(
    conductor: &SweetConductor,
    cell: &SweetCell,
    input: UpdateSupportBreakdownInput,
) -> ConductorApiResult<Record> {
    conductor
        .call_fallible(&cell.zome("support"), "update_support_breakdown", input)
        .await
}

/// Get support breakdown for owner
pub async fn get_support_breakdown_for_owner(
    conductor: &SweetConductor,
    cell: &SweetCell,
    owner: AgentPubKey,
) -> ConductorApiResult<(Option<ActionHash>, Option<Record>)> {
    conductor
        .call_fallible(&cell.zome("support"), "get_support_breakdown_for_owner", owner)
        .await
}

/// Get all contracts as debtor for an agent
pub async fn get_all_contracts_as_debtor(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<Vec<Record>> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_all_contracts_as_debtor", agent)
        .await
}

/// Get active contracts as debtor for an agent
pub async fn get_active_contracts_for_debtor(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<Vec<Record>> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_active_contracts_for_debtor", agent)
        .await
}

/// Get active contracts as creditor for an agent
pub async fn get_active_contracts_for_creditor(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<Vec<Record>> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_active_contracts_for_creditor", agent)
        .await
}

/// Process expired contracts via zome call
pub async fn process_contract_expirations(
    conductor: &SweetConductor,
    cell: &SweetCell,
) -> ConductorApiResult<ExpirationResult> {
    conductor
        .call_fallible(&cell.zome("transaction"), "process_contract_expirations", ())
        .await
}

/// Poll until the agent's total debt reaches `expected` within `tolerance`, or time out.
///
/// Used instead of fixed `tokio::time::sleep` durations after cascade operations
/// to avoid test flakiness under load.  The cascade fires asynchronously via
/// `call_remote`; this helper keeps retrying the debt query until the value
/// converges to the expected amount.
///
/// `timeout_ms` is the total wait budget in milliseconds (default suggestion: 8000).
pub async fn wait_for_debt_to_reach(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
    expected: f64,
    tolerance: f64,
    timeout_ms: u64,
) -> Result<f64, String> {
    let poll_interval = std::time::Duration::from_millis(300);
    let max_attempts = (timeout_ms / 300).max(1);
    let mut last_debt = f64::NAN;
    for _ in 0..max_attempts {
        match get_total_debt(conductor, cell, agent.clone()).await {
            Ok(debt) => {
                last_debt = debt;
                if (debt - expected).abs() <= tolerance {
                    return Ok(debt);
                }
            }
            Err(e) => {
                eprintln!("wait_for_debt_to_reach: zome call error: {e:?}");
            }
        }
        tokio::time::sleep(poll_interval).await;
    }
    Err(format!(
        "Timeout waiting for debt to reach {expected} ± {tolerance} for agent {agent}; last value: {last_debt}"
    ))
}

/// Poll until the agent's total debt stabilises (two consecutive reads agree within tolerance).
///
/// Useful after cascade operations where the final value is not known in advance.
pub async fn wait_for_debt_stable(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
    tolerance: f64,
    timeout_ms: u64,
) -> Result<f64, String> {
    let poll_interval = std::time::Duration::from_millis(500);
    let max_attempts = (timeout_ms / 500).max(1);
    let mut prev = f64::NAN;
    for _ in 0..max_attempts {
        match get_total_debt(conductor, cell, agent.clone()).await {
            Ok(debt) => {
                if (debt - prev).abs() <= tolerance {
                    return Ok(debt);
                }
                prev = debt;
            }
            Err(e) => {
                eprintln!("wait_for_debt_stable: zome call error: {e:?}");
            }
        }
        tokio::time::sleep(poll_interval).await;
    }
    Err(format!("Timeout waiting for debt to stabilise for agent {agent}; last value: {prev}"))
}

///
/// After `approve_pending_transaction` the seller sends `call_remote` to the
/// buyer to create the DebtContract.  That remote call is asynchronous, so we
/// must wait for it rather than sleeping a fixed duration.
pub async fn wait_for_active_contract(
    conductor: &SweetConductor,
    cell: &SweetCell,
    debtor: AgentPubKey,
) -> Result<(), String> {
    let mut retries = 0;
    loop {
        let contracts = get_active_contracts_for_debtor(conductor, cell, debtor.clone())
            .await
            .unwrap_or_default();
        if !contracts.is_empty() {
            return Ok(());
        }
        retries += 1;
        if retries > 60 {
            return Err(format!("Timeout waiting for Active contract to appear for debtor {debtor}"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Wait for a specific wallet state (trial count) to propagate
pub async fn wait_for_wallet_state(
    conductor: &SweetConductor,
    source_cell: &SweetCell,
    target_agent: AgentPubKey,
    expected_count: u32,
) -> Result<(), String> {
    let mut retries = 0;
    loop {
        let result = get_wallet_for_agent(conductor, source_cell, target_agent.clone()).await;
        if let Ok((Some(_), Some(record))) = result {
            let wallet: Wallet = record.entry().to_app_option().unwrap().unwrap();
            if wallet.trial_tx_count >= expected_count {
                return Ok(());
            }
        }

        retries += 1;
        if retries > 30 {
            // Wait up to 15 seconds
            return Err(format!("Timeout waiting for trial count {expected_count} for agent {target_agent}"));
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Wait for a transaction to reach a specific status
pub async fn wait_for_transaction_status(
    conductor: &SweetConductor,
    cell: &SweetCell,
    hash: ActionHash,
    status: TransactionStatus,
) -> Result<Record, String> {
    for _ in 0..50 {
        let record_opt: Option<Record> = conductor
            .call(&cell.zome("transaction"), "get_latest_transaction", hash.clone())
            .await;
        if let Some(record) = record_opt {
            if let Ok(Some(tx)) = record.entry().to_app_option::<Transaction>() {
                if tx.status == status {
                    return Ok(record);
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err(format!("Timeout waiting for transaction status {status:?} for hash {hash}"))
}

/// Poll until the given agent's vouched capacity exceeds `min_capacity`, or time out.
///
/// Genesis vouches are written by sponsor cells and must propagate via DHT
/// to the entrant's cell before `get_vouched_capacity` (which reads the
/// entrant's `EntrantToVouch` links) returns a non-zero value.
pub async fn wait_for_vouched_capacity(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
    min_capacity: f64,
) -> Result<(), String> {
    let mut retries = 0;
    loop {
        let cap = get_vouched_capacity(conductor, cell, agent.clone()).await.unwrap_or(0.0);
        if cap >= min_capacity {
            return Ok(());
        }
        retries += 1;
        if retries > 60 {
            return Err(format!(
                "Timeout waiting for vouched capacity >= {min_capacity} for agent {agent}; last value: {cap}"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Get credit capacity for an agent
pub async fn get_credit_capacity(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<f64> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_credit_capacity", agent)
        .await
}

/// Get failure witnesses for a debtor
pub async fn get_failure_witnesses(
    conductor: &SweetConductor,
    cell: &SweetCell,
    debtor: AgentPubKey,
) -> ConductorApiResult<Vec<AgentPubKeyB64>> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_failure_witnesses", debtor)
        .await
}

/// Get aggregate witness rate (median bilateral failure rate across witnesses) for a debtor.
/// Returns 0.0 if fewer than MIN_CONTAGION_WITNESSES (3) witnesses exist.
pub async fn get_aggregate_witness_rate(
    conductor: &SweetConductor,
    cell: &SweetCell,
    debtor: AgentPubKey,
) -> ConductorApiResult<f64> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_aggregate_witness_rate", debtor)
        .await
}

/// Get acquaintances for the calling agent
pub async fn get_acquaintances(conductor: &SweetConductor, cell: &SweetCell) -> ConductorApiResult<Vec<AgentPubKey>> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_acquaintances", ())
        .await
}

/// Check if calling agent has vouched for target agent
pub async fn get_my_vouched_for_agent(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<bool> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_my_vouched_for_agent", agent)
        .await
}

/// Get all sponsors (vouchers) for an agent
pub async fn get_vouchers_for_agent(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<Vec<AgentPubKey>> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_vouchers_for_agent", agent)
        .await
}

/// Get all vouch records for which `agent` is the entrant (includes slashed/released).
pub async fn get_vouches_for_entrant(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
) -> ConductorApiResult<Vec<Vouch>> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_vouches_for_entrant", agent)
        .await
}

/// Mirror of the coordinator's `TransactionSimulationResult`.
/// Defined locally because the coordinator crate targets WASM and cannot be
/// imported as a native dependency.
#[derive(Deserialize, Debug, Clone)]
pub struct TransactionSimulationResult {
    pub status: TransactionStatus,
    pub is_trial: bool,
}

/// Get transaction status from simulation (dry-run risk assessment)
pub async fn get_transaction_status_from_simulation(
    conductor: &SweetConductor,
    cell: &SweetCell,
    transaction: Transaction,
) -> ConductorApiResult<TransactionSimulationResult> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_transaction_status_from_simulation", transaction)
        .await
}

/// Compute the full risk score for `buyer` as seen by the calling cell (observer = cell).
///
/// Only available in `test-epoch` builds (calls the feature-gated `get_risk_score` extern).
/// Returns a value in [0, 1]: 0 = zero risk, 1 = maximum risk.
pub async fn get_risk_score(
    conductor: &SweetConductor,
    cell: &SweetCell,
    buyer: AgentPubKey,
) -> ConductorApiResult<f64> {
    conductor
        .call_fallible(&cell.zome("transaction"), "get_risk_score", buyer)
        .await
}

/// Wait for a specific count of transactions with a given status tag for an agent.
/// Retries up to 60 times at 500ms intervals (30 seconds total).
pub async fn wait_for_transactions_count(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
    tag: TransactionStatusTag,
    count: usize,
) -> Result<Vec<Record>, String> {
    let mut retries = 0;
    loop {
        let results = if tag == TransactionStatusTag::Pending {
            // Check both buyer and seller side
            let mut all = get_transactions_for_buyer(conductor, cell, agent.clone(), tag.clone())
                .await
                .unwrap_or_default();
            let seller_side = get_transactions_for_seller(conductor, cell, agent.clone(), tag.clone())
                .await
                .unwrap_or_default();
            for r in seller_side {
                if !all.iter().any(|existing| existing.action_address() == r.action_address()) {
                    all.push(r);
                }
            }
            all
        } else {
            get_transactions_for_buyer(conductor, cell, agent.clone(), tag.clone())
                .await
                .unwrap_or_default()
        };

        if results.len() >= count {
            return Ok(results);
        }

        retries += 1;
        if retries > 60 {
            return Err(format!("Timeout waiting for {count} transactions with tag {tag:?} for agent {agent}"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Release a vouch (sponsor withdraws support from entrant).
pub async fn release_vouch(
    conductor: &SweetConductor,
    cell: &SweetCell,
    original_vouch_hash: ActionHash,
    previous_vouch_hash: ActionHash,
) -> ConductorApiResult<Record> {
    #[derive(Debug, Serialize, Deserialize)]
    struct ReleaseVouchInput {
        pub original_vouch_hash: ActionHash,
        pub previous_vouch_hash: ActionHash,
    }
    let input = ReleaseVouchInput { original_vouch_hash, previous_vouch_hash };
    conductor.call_fallible(&cell.zome("transaction"), "release_vouch", input).await
}

/// Fetch the latest `(original_hash, previous_hash)` for a vouch given by the sponsor cell.
///
/// Uses `get_vouches_given` which follows SponsorToVouch links and returns the latest
/// version of each vouch. Returns the entry matching `original_vouch_hash`, or an error
/// if not found.
pub async fn get_latest_vouch_hashes(
    conductor: &SweetConductor,
    cell: &SweetCell,
    original_vouch_hash: &ActionHash,
) -> Result<(ActionHash, ActionHash), String> {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct VouchRecord {
        pub original_hash: ActionHash,
        pub previous_hash: ActionHash,
    }
    let records: Vec<VouchRecord> = conductor
        .call_fallible(&cell.zome("transaction"), "get_vouches_given", ())
        .await
        .map_err(|e| format!("get_vouches_given failed: {e:?}"))?;
    records
        .into_iter()
        .find(|r| &r.original_hash == original_vouch_hash)
        .map(|r| (r.original_hash, r.previous_hash))
        .ok_or_else(|| format!("Vouch with original_hash {original_vouch_hash} not found in get_vouches_given"))
}

/// Cancel a pending transaction (buyer action).
pub async fn cancel_pending_transaction(
    conductor: &SweetConductor,
    cell: &SweetCell,
    original_hash: ActionHash,
    previous_hash: ActionHash,
) -> ConductorApiResult<Record> {
    #[derive(Debug, Serialize, Deserialize)]
    struct ModerateTransactionInput {
        pub original_transaction_hash: ActionHash,
        pub previous_transaction_hash: ActionHash,
        pub transaction: Transaction,
    }

    let record: Record = conductor
        .call(&cell.zome("transaction"), "get_latest_transaction", original_hash.clone())
        .await;
    let transaction: Transaction = record.entry().to_app_option().unwrap().unwrap();

    let input = ModerateTransactionInput {
        original_transaction_hash: original_hash,
        previous_transaction_hash: previous_hash,
        transaction,
    };
    conductor
        .call_fallible(&cell.zome("transaction"), "cancel_pending_transaction", input)
        .await
}

/// Compute the sum of all debt across a set of agents using the cached DebtBalanceTag.
///
/// Uses `get_total_debt` (the cached DebtBalanceTag path) which is updated atomically
/// when a DebtContract is created/modified. This is more reliable in test-epoch mode
/// than scanning `get_active_contracts_for_debtor` because the balance cache is updated
/// on the same chain write that creates the contract.
///
/// Conservation invariant: for a sale of `amount` from seller S to buyer B:
///   debt_after <= debt_before + amount
/// (equality when cascade drains nothing; less when cascade absorbs the new debt).
/// See `test_cascade_debt_conservation` for the detailed invariant.
///
/// Returns the total as f64. Tolerates cells where the debtor has no debt (returns 0).
pub async fn sum_active_debt(conductor: &SweetConductor, agent_cells: &[(SweetCell, AgentPubKey)]) -> f64 {
    let mut total = 0.0;
    for (cell, agent) in agent_cells {
        let debt = get_total_debt(conductor, cell, agent.clone()).await.unwrap_or(0.0);
        total += debt;
    }
    total
}

/// Wait until an agent's source chain has stopped receiving new writes.
///
/// The Holochain conductor runs `post_commit` asynchronously after every zome
/// call, so a caller that immediately makes another mutating zome call can race
/// with those in-flight `post_commit` writes and get `HeadMoved`.
///
/// This helper polls the agent's current wallet action hash (a cheap local read).
/// Once the hash has been stable for two consecutive polls spaced `poll_ms` apart
/// we consider the chain quiescent and return.
///
/// Use this before any mutating zome call that follows a series of writes that
/// may still have pending `post_commit` side-effects (trust row publishes,
/// acquaintance evictions, ranking-index updates, etc.).
pub async fn wait_for_chain_quiescent(
    conductor: &SweetConductor,
    cell: &SweetCell,
    agent: AgentPubKey,
    poll_ms: u64,
    timeout_ms: u64,
) {
    let poll = std::time::Duration::from_millis(poll_ms);
    let max_polls = ((timeout_ms / poll_ms) + 1) as usize;
    let mut prev_hash: Option<ActionHash> = None;
    let mut stable_count = 0usize;

    for _ in 0..max_polls {
        tokio::time::sleep(poll).await;
        let current_hash = get_wallet_for_agent(conductor, cell, agent.clone())
            .await
            .ok()
            .and_then(|(hash, _)| hash);

        if current_hash == prev_hash {
            stable_count += 1;
            if stable_count >= 2 {
                return;
            }
        } else {
            stable_count = 0;
            prev_hash = current_hash;
        }
    }
    // Timed out — proceed anyway; the caller will surface any resulting error.
    eprintln!("wait_for_chain_quiescent: timed out after {timeout_ms}ms for agent {agent}");
}
