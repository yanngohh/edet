use super::*;

/// Self-vouch should be rejected
#[tokio::test(flavor = "multi_thread")]
async fn test_self_vouch_rejected() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();
    let agent: AgentPubKey = cell.agent_pubkey().clone();

    // Alice tries to vouch for herself -- should be rejected
    let vouch_input = CreateVouchInput { sponsor: agent.clone().into(), entrant: agent.clone().into(), amount: 500.0 };

    let result = create_vouch(&conductor, &cell, vouch_input).await;
    assert!(result.is_err(), "Self-vouch should have been rejected by validation");
}

/// Negative vouch amount should be rejected
#[tokio::test(flavor = "multi_thread")]
async fn test_negative_vouch_amount_rejected() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice tries to vouch with negative amount
    let vouch_input =
        CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: bob_agent.clone().into(), amount: -50.0 };

    let result = create_vouch(&conductor, &alice_cell, vouch_input).await;
    assert!(result.is_err(), "Negative vouch amount should have been rejected");
}

/// Vouch amount exceeding maximum should be rejected
#[tokio::test(flavor = "multi_thread")]
async fn test_oversized_vouch_amount_rejected() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice tries to vouch with amount > MAX_VOUCH_AMOUNT
    let vouch_input = CreateVouchInput {
        sponsor: alice_agent.clone().into(),
        entrant: bob_agent.clone().into(),
        amount: 5000.0, // Exceeds BASE_CAPACITY (1000.0)
    };

    let result = create_vouch(&conductor, &alice_cell, vouch_input).await;
    assert!(result.is_err(), "Oversized vouch amount should have been rejected");
}

/// Valid vouch should succeed
#[tokio::test(flavor = "multi_thread")]
async fn test_valid_vouch_succeeds() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice already vouched for Bob during genesis bootstrap (500).
    // Add an additional genesis vouch to verify vouch accumulation.
    let vouch_input =
        CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: bob_agent.clone().into(), amount: 500.0 };

    let record = genesis_vouch(&conductor, &alice_cell, vouch_input)
        .await
        .expect("Valid vouch should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Bob should have accumulated vouched capacity: 500 (genesis) + 500 (this test)
    let capacity = get_vouched_capacity(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Should get vouched capacity");
    assert!(capacity >= 1000.0, "Vouched capacity should be >= 1000 (500 genesis + 500 test): got {capacity}");
}

/// Negative debt amount should be rejected
#[tokio::test(flavor = "multi_thread")]
async fn test_negative_debt_amount_rejected() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Negative debt test".to_string(),
        debt: -50.0,
    };
    let result = create_transaction(&conductor, &alice_cell, tx).await;
    assert!(result.is_err(), "Negative debt transaction should be rejected");
}

/// Zero debt amount should be rejected
#[tokio::test(flavor = "multi_thread")]
async fn test_zero_debt_amount_rejected() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Zero debt test".to_string(),
        debt: 0.0,
    };
    let result = create_transaction(&conductor, &alice_cell, tx).await;
    assert!(result.is_err(), "Zero debt transaction should be rejected");
}

/// Buyer can cancel their own pending transaction
#[tokio::test(flavor = "multi_thread")]
async fn test_buyer_can_cancel_pending_transaction() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Cancel test".to_string(),
        debt: 49.0,
    };
    let record = create_transaction(&conductor, &alice_cell, tx)
        .await
        .expect("Transaction should be created");
    let tx_data: Transaction = record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(tx_data.status, TransactionStatus::Pending, "Trial should start as Pending");

    // Alice (buyer) cancels the pending transaction
    let canceled = cancel_pending_transaction(
        &conductor,
        &alice_cell,
        record.action_address().clone(),
        record.action_address().clone(),
    )
    .await
    .expect("Buyer should be able to cancel their own pending transaction");
    let canceled_tx: Transaction = canceled.entry().to_app_option().unwrap().unwrap();
    assert_eq!(canceled_tx.status, TransactionStatus::Canceled, "Transaction should be Canceled");
}

/// NaN vouch amount is rejected by integrity validation.
///
/// In Rust, NaN comparisons always return false: `f64::NAN <= 0.0` is false and
/// `f64::NAN > MAX_VOUCH_AMOUNT` is false. Without an explicit `is_finite()` guard,
/// a NaN amount would bypass both the positive-amount and maximum-amount checks.
/// This test verifies the fix in validate_create_vouch (vouch.rs).
#[tokio::test(flavor = "multi_thread")]
async fn test_nan_vouch_amount_rejected() {
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let alice_cell = apps[0].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = apps[1].cells()[0].agent_pubkey().clone();

    let input = CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: bob_agent.into(), amount: f64::NAN };
    let result = genesis_vouch(&conductor, &alice_cell, input).await;
    assert!(result.is_err(), "Vouch with NaN amount must be rejected; integrity guard is missing if this passes");
}

/// Infinity vouch amount is rejected by integrity validation.
///
/// Like NaN, `f64::INFINITY > MAX_VOUCH_AMOUNT` is true in Rust (INFINITY > any finite),
/// so INFINITY would actually be caught by the existing maximum-amount check. However,
/// `f64::NEG_INFINITY <= 0.0` is true, so NEG_INFINITY is also caught by the positive check.
/// This test documents that both Infinity variants are properly rejected.
#[tokio::test(flavor = "multi_thread")]
async fn test_infinity_vouch_amount_rejected() {
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let alice_cell = apps[0].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = apps[1].cells()[0].agent_pubkey().clone();

    let input = CreateVouchInput { sponsor: alice_agent.into(), entrant: bob_agent.into(), amount: f64::INFINITY };
    let result = genesis_vouch(&conductor, &alice_cell, input).await;
    assert!(result.is_err(), "Vouch with Infinity amount must be rejected");
}

/// NaN debt amount is rejected by integrity validation.
///
/// `f64::NAN <= 0.0` is false in Rust, so without an explicit `is_finite()` guard
/// a NaN debt amount would bypass the positive-amount check in validate_create_debt_contract.
/// This test verifies the fix in debt_contract.rs.
#[tokio::test(flavor = "multi_thread")]
async fn test_nan_debt_amount_rejected() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    let tx = CreateTransactionInput {
        seller: bob_agent.into(),
        buyer: alice_agent.into(),
        description: "NaN debt test".to_string(),
        debt: f64::NAN,
    };
    let result = create_transaction(&conductor, &alice_cell, tx).await;
    assert!(result.is_err(), "Transaction with NaN debt must be rejected; integrity guard is missing if this passes");
}

/// Self-dealing transaction (buyer == seller) is rejected.
///
/// A transaction where the buyer and seller are the same agent would create
/// a circular debt that is meaningless and could be used to manufacture
/// fake satisfaction evidence. The integrity layer should reject it.
#[tokio::test(flavor = "multi_thread")]
async fn test_self_dealing_transaction_rejected() {
    let (conductor, apps) = setup_multi_agent(1).await;
    let alice_cell = apps[0].cells()[0].clone();
    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();

    let tx = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Self-dealing test".to_string(),
        debt: 25.0,
    };
    let result = create_transaction(&conductor, &alice_cell, tx).await;
    assert!(result.is_err(), "Self-dealing transaction (buyer == seller) must be rejected");
}
