//! Utility Function Tests — Tier 3 (Nice-to-have)
//!
//! Tests for simple get/utility functions that are implicitly tested elsewhere
//! but benefit from direct coverage:
//!
//! 1. get_original_transaction      — returns create-time record, not latest update
//! 2. get_agent_last_transaction    — skips canceled/rejected, returns most recent
//! 3. check_bootstrap_eligible      — UI wrapper for trial eligibility
//! 4. get_original_wallet           — returns create-time wallet record
//! 5. get_checkpoint_by_sequence    — checkpoint lookup by sequence number
//! 6. get_all_contracts_as_debtor_resolved — resolves to latest contract version

use super::*;

// ============================================================================
//  1. get_original_transaction
// ============================================================================

/// `get_original_transaction` returns the create-time record (Pending status)
/// even after the transaction has been updated to Accepted.
#[tokio::test(flavor = "multi_thread")]
async fn test_get_original_transaction() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer
    let bob_cell = apps[1].cells()[0].clone(); // seller

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("bob wallet should propagate");

    // Create a trial transaction (stays Pending until manually approved).
    let tx_record = create_transaction(
        &conductor,
        &alice_cell,
        CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: "get original tx test".to_string(),
            debt: 30.0,
        },
    )
    .await
    .expect("create_transaction should succeed");
    let original_hash = tx_record.action_address().clone();

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx should propagate to bob");

    // Approve so it transitions to Accepted.
    approve_pending_transaction(&conductor, &bob_cell, original_hash.clone(), original_hash.clone())
        .await
        .expect("approve should succeed");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Latest transaction should be Accepted.
    let latest: Record = conductor
        .call_fallible(&alice_cell.zome("transaction"), "get_latest_transaction", original_hash.clone())
        .await
        .expect("get_latest_transaction should succeed");
    let latest_tx: Transaction = latest.entry().to_app_option().unwrap().unwrap();
    assert_eq!(latest_tx.status, TransactionStatus::Accepted, "Latest should be Accepted");

    // Original transaction should still show the create-time status.
    let original: Option<Record> = conductor
        .call_fallible(&alice_cell.zome("transaction"), "get_original_transaction", original_hash)
        .await
        .expect("get_original_transaction should succeed");

    let original_tx: Transaction = original
        .expect("Should find original record")
        .entry()
        .to_app_option()
        .unwrap()
        .unwrap();

    // Original record was created with status Initial (before coordinator sets Pending).
    // After coordinator sets status, the initial create is stored as Initial or Pending.
    // The key property: the returned record is the *first* action in the chain for this tx.
    assert!(
        matches!(original_tx.status, TransactionStatus::Pending | TransactionStatus::Initial),
        "Original record should reflect create-time status; got {:?}",
        original_tx.status
    );
}

// ============================================================================
//  2. get_agent_last_transaction
// ============================================================================

/// `get_agent_last_transaction` skips Canceled/Rejected transactions and
/// returns the most recent non-terminal transaction.
#[tokio::test(flavor = "multi_thread")]
async fn test_get_agent_last_transaction() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer
    let bob_cell = apps[1].cells()[0].clone(); // seller

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("bob wallet should propagate");

    // Create a trial transaction and then cancel it.
    let tx_record = create_transaction(
        &conductor,
        &alice_cell,
        CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: "to be canceled".to_string(),
            debt: 30.0,
        },
    )
    .await
    .expect("create_transaction should succeed");
    let canceled_hash = tx_record.action_address().clone();

    cancel_pending_transaction(&conductor, &alice_cell, canceled_hash.clone(), canceled_hash.clone())
        .await
        .expect("cancel should succeed");

    // Create a second transaction (pending, not canceled).
    let tx2_record = create_transaction(
        &conductor,
        &alice_cell,
        CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: "active pending tx".to_string(),
            debt: 25.0,
        },
    )
    .await
    .expect("second create_transaction should succeed");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // get_agent_last_transaction should return the second (non-canceled) transaction.
    let last: Option<Record> = conductor
        .call_fallible(&alice_cell.zome("transaction"), "get_agent_last_transaction", alice_agent)
        .await
        .expect("get_agent_last_transaction should succeed");

    let last_tx: Transaction = last
        .expect("Should return Some transaction")
        .entry()
        .to_app_option()
        .unwrap()
        .unwrap();

    assert!(
        !matches!(last_tx.status, TransactionStatus::Canceled),
        "Last transaction should not be Canceled; got {:?}",
        last_tx.status
    );
}

// ============================================================================
//  3. check_bootstrap_eligible
// ============================================================================

/// `check_bootstrap_eligible` returns `true` for a fresh agent with no
/// DebtContracts, and `false` once a trial transaction is in-flight.
#[tokio::test(flavor = "multi_thread")]
async fn test_check_bootstrap_eligible() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer
    let bob_cell = apps[1].cells()[0].clone(); // seller

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Fresh agent — should be eligible for bootstrap trial.
    let eligible_before: bool = conductor
        .call_fallible(&alice_cell.zome("transaction"), "check_bootstrap_eligible", alice_agent.clone())
        .await
        .expect("check_bootstrap_eligible should succeed");
    assert!(eligible_before, "Fresh agent should be bootstrap eligible");

    // Create a trial transaction (now Alice has a pending trial in-flight).
    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("bob wallet should propagate");
    let tx = create_transaction(
        &conductor,
        &alice_cell,
        CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: "bootstrap eligibility test".to_string(),
            debt: 30.0, // trial amount
        },
    )
    .await
    .expect("create_transaction should succeed");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Should now be ineligible (pending trial in-flight).
    let eligible_after: bool = conductor
        .call_fallible(&alice_cell.zome("transaction"), "check_bootstrap_eligible", alice_agent)
        .await
        .expect("check_bootstrap_eligible should succeed after tx");
    assert!(!eligible_after, "Agent with pending trial should not be bootstrap eligible");
}

// ============================================================================
//  4. get_original_wallet
// ============================================================================

/// `get_original_wallet` returns the create-time wallet record by action hash,
/// even after the wallet has been updated.
#[tokio::test(flavor = "multi_thread")]
async fn test_get_original_wallet() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();
    let agent: AgentPubKey = cell.agent_pubkey().clone();

    // Get the original wallet (created by init).
    let (maybe_original_hash, maybe_record) = get_wallet_for_agent(&conductor, &cell, agent.clone())
        .await
        .expect("get_wallet_for_agent should succeed");
    let original_hash = maybe_original_hash.expect("Should have an original wallet hash");
    let original_wallet: Wallet = maybe_record
        .expect("Should have a wallet record")
        .entry()
        .to_app_option()
        .unwrap()
        .unwrap();

    // Update the wallet (change auto-accept threshold).
    #[derive(Debug, Serialize, Deserialize)]
    struct UpdateWalletInput {
        pub original_wallet_hash: ActionHash,
        pub previous_wallet_hash: ActionHash,
        pub updated_wallet: Wallet,
    }
    let mut updated = original_wallet.clone();
    updated.auto_accept_threshold = 0.3;
    let _: Record = conductor
        .call_fallible(
            &cell.zome("transaction"),
            "update_wallet",
            UpdateWalletInput {
                original_wallet_hash: original_hash.clone(),
                previous_wallet_hash: original_hash.clone(),
                updated_wallet: updated,
            },
        )
        .await
        .expect("update_wallet should succeed");

    // get_original_wallet by hash should return the original (unchanged threshold).
    let fetched: Option<Record> = conductor
        .call_fallible(&cell.zome("transaction"), "get_original_wallet", original_hash)
        .await
        .expect("get_original_wallet should succeed");

    let fetched_wallet: Wallet = fetched
        .expect("Should find original wallet record")
        .entry()
        .to_app_option()
        .unwrap()
        .unwrap();

    assert_eq!(
        fetched_wallet.auto_accept_threshold, original_wallet.auto_accept_threshold,
        "get_original_wallet should return the create-time record, not the updated one"
    );
}

// ============================================================================
//  5. get_checkpoint_by_sequence
// ============================================================================

/// `get_checkpoint_by_sequence` returns the checkpoint with the requested
/// sequence number, and `None` for a sequence that does not exist.
///
/// Note: `create_checkpoint` only creates a checkpoint when the chain has
/// >= CHECKPOINT_INTERVAL_ENTRIES (1000) entries, so a fresh agent always gets
/// `None`. This test verifies the None-for-missing-sequence path.
/// The happy-path (checkpoint found) is covered by test_scalability_functions_integration.
#[tokio::test(flavor = "multi_thread")]
async fn test_get_checkpoint_by_sequence() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();

    #[derive(Debug, Serialize, Deserialize)]
    struct GetCheckpointBySequenceInput {
        pub agent: AgentPubKey,
        pub sequence: u64,
    }

    // For a fresh agent create_checkpoint returns None (chain too short).
    let created: Option<Record> = conductor
        .call_fallible(&cell.zome("transaction"), "create_checkpoint", ())
        .await
        .expect("create_checkpoint should succeed (returns None for fresh agent)");
    // Fresh agent has fewer than CHECKPOINT_INTERVAL_ENTRIES actions → no checkpoint created.
    assert!(created.is_none(), "Fresh agent should not create a checkpoint; got Some");

    // Querying any sequence on an agent with no checkpoints should return None.
    let missing: Option<Record> = conductor
        .call_fallible(
            &cell.zome("transaction"),
            "get_checkpoint_by_sequence",
            GetCheckpointBySequenceInput { agent: cell.agent_pubkey().clone(), sequence: 1 },
        )
        .await
        .expect("get_checkpoint_by_sequence should succeed");

    assert!(missing.is_none(), "Agent with no checkpoints should return None for any sequence");
}

// ============================================================================
//  6. get_all_contracts_as_debtor_resolved
// ============================================================================

/// `get_all_contracts_as_debtor_resolved` resolves each contract to its latest
/// version via the update chain, unlike the base `get_all_contracts_as_debtor`
/// which may return stale create-time records.
#[tokio::test(flavor = "multi_thread")]
async fn test_get_all_contracts_as_debtor_resolved() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer / debtor
    let bob_cell = apps[1].cells()[0].clone(); // seller / creditor

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("bob wallet should propagate");
    // Use a trial-sized amount (strictly < TRIAL_FRACTION * BASE_CAPACITY = 50.0) so the
    // transaction is always Pending (awaiting seller approval), regardless of trust score.
    let debt = 30.0;
    let tx = create_transaction(
        &conductor,
        &alice_cell,
        CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: "resolved contracts test".to_string(),
            debt,
        },
    )
    .await
    .expect("create_transaction should succeed");
    let tx_hash = tx.action_address().clone();

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx should propagate");
    approve_pending_transaction(&conductor, &bob_cell, tx_hash.clone(), tx_hash.clone())
        .await
        .expect("approve should succeed");

    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice should have an Active contract");

    // get_all_contracts_as_debtor_resolved should return Alice's active contract
    // in its latest (current) state.
    let contracts: Vec<Record> = conductor
        .call_fallible(&alice_cell.zome("transaction"), "get_all_contracts_as_debtor_resolved", alice_agent)
        .await
        .expect("get_all_contracts_as_debtor_resolved should succeed");

    assert!(!contracts.is_empty(), "Should return at least one contract");
    let contract: DebtContract = contracts[0].entry().to_app_option().unwrap().unwrap();
    assert_eq!(contract.status, ContractStatus::Active, "Resolved contract should be Active");
    assert!((contract.amount - debt).abs() < 1.0, "Contract amount should be ~{debt}; got {}", contract.amount);
}
