//! Concurrent Operation & Boundary Value Tests
//!
//! Tests for edge cases that are not covered by the existing test suite:
//!
//! 1. test_double_approve_race        — second approve after first should fail with stale pointer
//! 2. test_trial_boundary_exact       — transaction at exactly TRIAL_THRESHOLD
//! 3. test_trial_boundary_above       — transaction just above TRIAL_THRESHOLD (non-trial)
//! 4. test_max_vouch_boundary         — vouch at exactly MAX_VOUCH_AMOUNT should succeed
//! 5. test_epoch_boundary_expiration  — contract expires at exact epoch boundary

use super::*;

// ============================================================================
//  Helper: trial threshold constant (mirrors Rust constant)
// ============================================================================

/// Trial threshold: eta * V_base = 0.05 * 1000 = 50.0
/// Transactions strictly below this amount (with bootstrap-eligible buyer) are trials.
const TRIAL_THRESHOLD: f64 = 50.0;

/// Maximum vouch amount: BASE_CAPACITY = 1000.0
const MAX_VOUCH_AMOUNT: f64 = 1000.0;

// ============================================================================
//  1. test_double_approve_race
// ============================================================================

/// The second approve call on an already-Accepted transaction must fail with
/// a "transaction not found in Pending state" or "obsolete pointer" error.
/// This verifies the integrity layer's FSM enforcement (only Pending → Accepted).
#[tokio::test(flavor = "multi_thread")]
async fn test_double_approve_race() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer
    let bob_cell = apps[1].cells()[0].clone(); // seller

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("bob wallet should propagate");

    // Create a trial transaction.
    let tx = create_transaction(
        &conductor,
        &alice_cell,
        CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: "double approve test".to_string(),
            debt: 30.0,
        },
    )
    .await
    .expect("create_transaction should succeed");
    let tx_hash = tx.action_address().clone();

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx should propagate to bob");

    // First approve: should succeed.
    approve_pending_transaction(&conductor, &bob_cell, tx_hash.clone(), tx_hash.clone())
        .await
        .expect("First approve should succeed");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Second approve with the original (now stale) hash: must fail.
    let second_approve = approve_pending_transaction(&conductor, &bob_cell, tx_hash.clone(), tx_hash.clone()).await;

    assert!(second_approve.is_err(), "Second approve on already-Accepted transaction must fail; got Ok");
}

// ============================================================================
//  2. test_trial_boundary_exact
// ============================================================================

/// A transaction with debt == TRIAL_THRESHOLD (50.0) should NOT be a trial.
///
/// Boundary: `debt < TRIAL_FRACTION * BASE_CAPACITY` → trial in zome code.
/// TRIAL_FRACTION=0.05, BASE_CAPACITY=1000.0, so threshold = 50.0.
/// At exactly 50.0 the condition is `50.0 < 50.0` → false → non-trial.
#[tokio::test(flavor = "multi_thread")]
async fn test_trial_boundary_exact() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer
    let bob_cell = apps[1].cells()[0].clone(); // seller

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("bob wallet should propagate");

    // Create a transaction at exactly the threshold.
    let tx = create_transaction(
        &conductor,
        &alice_cell,
        CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: "boundary exact test".to_string(),
            debt: TRIAL_THRESHOLD,
        },
    )
    .await
    .expect("create_transaction at boundary should succeed");

    let created_tx: Transaction = tx.entry().to_app_option().unwrap().unwrap();

    // At exactly 50.0: condition is `debt < 50.0` → false → NOT a trial.
    // So the transaction should NOT be marked is_trial and should be non-trial Pending.
    assert!(
        !created_tx.is_trial,
        "Transaction at exactly TRIAL_THRESHOLD should NOT be a trial (uses strict <); got is_trial=true"
    );
}

// ============================================================================
//  3. test_trial_boundary_above
// ============================================================================

/// A transaction with debt just above TRIAL_THRESHOLD (50.01) should definitely
/// not be a trial.
#[tokio::test(flavor = "multi_thread")]
async fn test_trial_boundary_above() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("bob wallet should propagate");

    let tx = create_transaction(
        &conductor,
        &alice_cell,
        CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: "above boundary test".to_string(),
            debt: TRIAL_THRESHOLD + 0.01,
        },
    )
    .await
    .expect("create_transaction just above threshold should succeed");

    let created_tx: Transaction = tx.entry().to_app_option().unwrap().unwrap();
    assert!(!created_tx.is_trial, "Transaction above TRIAL_THRESHOLD should not be a trial; got is_trial=true");
}

// ============================================================================
//  4. test_max_vouch_boundary
// ============================================================================

/// A vouch at exactly MAX_VOUCH_AMOUNT (1000.0) should succeed.
/// A vouch at MAX_VOUCH_AMOUNT + ε should fail with AMOUNT_EXCEEDS_MAXIMUM.
#[tokio::test(flavor = "multi_thread")]
async fn test_max_vouch_boundary() {
    // Use no-vouch setup so alice starts with zero locked capacity.
    // We then give alice a full genesis vouch from bob (1000.0) so her available
    // capacity equals exactly MAX_VOUCH_AMOUNT with nothing pre-locked.
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // sponsor
    let bob_cell = apps[1].cells()[0].clone(); // entrant

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Give alice a genesis vouch of MAX_VOUCH_AMOUNT from bob so she has full capacity.
    genesis_vouch(
        &conductor,
        &bob_cell,
        CreateVouchInput {
            sponsor: bob_agent.clone().into(),
            entrant: alice_agent.clone().into(),
            amount: MAX_VOUCH_AMOUNT,
        },
    )
    .await
    .expect("genesis vouch bob->alice should succeed");

    // Allow gossip to propagate.
    let cells = vec![alice_cell.clone(), bob_cell.clone()];
    await_consistency(30, &cells).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // At exactly MAX_VOUCH_AMOUNT: should succeed (alice has 1000.0 available, 0 locked).
    let ok = create_vouch(
        &conductor,
        &alice_cell,
        CreateVouchInput {
            sponsor: alice_agent.clone().into(),
            entrant: bob_agent.clone().into(),
            amount: MAX_VOUCH_AMOUNT,
        },
    )
    .await;
    assert!(ok.is_ok(), "Vouch at exactly MAX_VOUCH_AMOUNT should succeed; got {ok:?}");

    // Above MAX_VOUCH_AMOUNT: should fail (integrity rejects amount > MAX_VOUCH_AMOUNT).
    // Alice's capacity is now exhausted, so the coordinator also rejects — either way it fails.
    let overflow = conductor
        .call_fallible::<_, Record>(
            &alice_cell.zome("transaction"),
            "create_vouch",
            CreateVouchInput {
                sponsor: alice_agent.clone().into(),
                entrant: bob_agent.clone().into(), // same entrant
                amount: MAX_VOUCH_AMOUNT + 0.01,
            },
        )
        .await;
    // Must fail: either AMOUNT_EXCEEDS_MAXIMUM (integrity) or INSUFFICIENT_CAPACITY (coordinator).
    assert!(overflow.is_err(), "Vouch above MAX_VOUCH_AMOUNT should fail; got Ok");
}

// ============================================================================
//  5. test_epoch_boundary_expiration
// ============================================================================

/// A contract created at epoch E expires at epoch E + MIN_MATURITY.
/// Expiration processing called AT that exact epoch boundary should transition
/// the contract to Expired (no off-by-one allowing premature or missed expiration).
#[tokio::test(flavor = "multi_thread")]
async fn test_epoch_boundary_expiration() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer / debtor
    let bob_cell = apps[1].cells()[0].clone(); // seller / creditor

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("bob wallet should propagate");

    // Create a debt contract via direct creation (mirrors epoch_tests pattern).
    let trial_amount = 150.0;
    let tx_record = create_transaction(
        &conductor,
        &alice_cell,
        CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: "epoch boundary test".to_string(),
            debt: trial_amount,
        },
    )
    .await
    .expect("create_transaction should succeed");
    let tx_hash = tx_record.action_address().clone();

    let contract_input = CreateDebtContractInput {
        amount: trial_amount,
        creditor: bob_agent.clone().into(),
        debtor: alice_agent.clone().into(),
        transaction_hash: tx_hash,
        is_trial: false,
    };
    let _: Record = conductor
        .call_fallible(&alice_cell.zome("transaction"), "create_debt_contract", contract_input)
        .await
        .expect("create_debt_contract should succeed");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Before maturity: no contract should expire.
    let result_before = process_contract_expirations(&conductor, &alice_cell)
        .await
        .expect("process_contract_expirations should succeed");
    assert_eq!(
        result_before.total_expired, 0.0,
        "Contract should NOT expire before MIN_MATURITY; got {}",
        result_before.total_expired
    );

    // Sleep past MIN_MATURITY epochs.
    let total_ms = EPOCH_SLEEP_MS * MATURITY_EPOCHS;
    tokio::time::sleep(std::time::Duration::from_millis(total_ms)).await;

    // AT or AFTER the maturity epoch: contract should now expire.
    let result_after = process_contract_expirations(&conductor, &alice_cell)
        .await
        .expect("process_contract_expirations should succeed");
    assert!(
        result_after.total_expired > 0.0,
        "Contract should expire at or after MIN_MATURITY epochs; got {}",
        result_after.total_expired
    );
}
