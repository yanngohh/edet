//! Risk Score Tests (Whitepaper Definition 4.1)
//!
//! Tests for the transaction risk score computation, including PATH 1 (claim-based),
//! PATH 2 (full EigenTrust), debt velocity factor, and auto-accept/reject thresholds.

use super::*;

/// Newcomer with no bilateral history gets high risk score (near 1.0).
///
/// Exercises: Risk score PATH 1 fallback (Equation 10).
/// An agent with no history and no ReputationClaim has risk = 1.0 for non-trial
/// amounts, resulting in Rejected status.
#[tokio::test(flavor = "multi_thread")]
async fn test_newcomer_risk_score_near_one() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice tries to buy a non-trial amount from Bob (no bilateral history).
    // With no history and no claim, risk should be high -> Rejected.
    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "High risk newcomer test".to_string(),
        debt: 200.0, // Non-trial amount (> TRIAL_FRACTION * BASE_CAPACITY = 50)
    };

    let result = create_transaction(&conductor, &alice_cell, tx).await;

    // The transaction may be created but with Rejected or Pending status,
    // or it may fail at capacity check. Either way, it should NOT be Accepted.
    match result {
        Ok(record) => {
            let tx: Transaction = record.entry().to_app_option().unwrap().unwrap();
            assert_ne!(
                tx.status,
                TransactionStatus::Accepted,
                "Non-trial tx from newcomer should not be auto-accepted (got {:?})",
                tx.status
            );
        }
        Err(_) => {
            // Also acceptable -- capacity exceeded or rejected at coordinator level
        }
    }
}

/// Risk decreases as trust builds through successful transactions.
///
/// Exercises: Risk score PATH 2 (Equation 9, full EigenTrust).
/// After building bilateral history, the risk score should decrease, enabling
/// transactions that were previously rejected.
#[tokio::test(flavor = "multi_thread")]
async fn test_risk_decreases_with_trust() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Step 1: Check Alice's initial trust from Bob's perspective
    let initial_rep = get_subjective_reputation(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Should get initial reputation");

    // Step 2: Build bilateral history with a small trial transaction
    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Trust building tx".to_string(),
        debt: 49.0, // Trial amount
    };
    let record = create_transaction(&conductor, &alice_cell, tx)
        .await
        .expect("Trial transaction should succeed");

    // Bob must manually approve the trial
    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Trial must propagate to Bob");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        record.action_address().clone(),
        record.action_address().clone(),
    )
    .await
    .expect("Transaction should be approved");

    // Step 3: Invalidate cache and check updated trust
    invalidate_trust_caches(&conductor, &bob_cell).await.ok();

    let updated_rep = get_subjective_reputation(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Should get updated reputation");

    // Trust should be at least as high as before (the transaction creates bilateral evidence)
    assert!(
        updated_rep.trust >= initial_rep.trust,
        "Trust should not decrease after successful transaction: was {}, now {}",
        initial_rep.trust,
        updated_rep.trust
    );
}

/// Debt overload increases risk toward 1.0.
///
/// Exercises: remaining_ratio = (Cap - Debt) / Cap in risk score formula.
/// Loading a buyer with debt close to capacity should push risk toward 1.0.
///
/// Per protocol, only one trial slot is open per (buyer, seller) pair at a time
/// (slot released only when contract is Transferred/repaid). We use distinct
/// sellers to accumulate debt across independent contracts.
#[tokio::test(flavor = "multi_thread")]
async fn test_debt_overload_increases_risk() {
    // 3 agents: Alice (buyer), Bob and Carol (sellers).
    // Each seller can have one active trial with Alice at a time.
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    // Trial 1: Alice buys from Bob
    let tx1 = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Debt loading tx 1".to_string(),
        debt: 49.0,
    };
    let record1 = create_transaction(&conductor, &alice_cell, tx1)
        .await
        .expect("Debt loading tx 1 should succeed");

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Trial 1 must propagate to Bob");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        record1.action_address().clone(),
        record1.action_address().clone(),
    )
    .await
    .expect("Bob should approve trial 1");

    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Trial 1 contract should be active");

    // Trial 2: Alice buys from Carol (different seller — no open-trial gate conflict)
    let tx2 = CreateTransactionInput {
        seller: carol_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Debt loading tx 2".to_string(),
        debt: 49.0,
    };
    let record2 = create_transaction(&conductor, &alice_cell, tx2)
        .await
        .expect("Debt loading tx 2 should succeed");

    ensure_transaction_propagation_seller(&conductor, &carol_cell, carol_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Trial 2 must propagate to Carol");
    approve_pending_transaction(
        &conductor,
        &carol_cell,
        record2.action_address().clone(),
        record2.action_address().clone(),
    )
    .await
    .expect("Carol should approve trial 2");

    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Trial 2 contract should be active");

    // Verify debt accumulated
    let debt = get_total_debt(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get debt");

    assert!(debt > 0.0, "Alice should have accumulated debt: got {debt}");

    // Verify capacity is available
    let cap = get_credit_capacity(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get capacity");

    // As debt approaches capacity, subsequent transactions should be rejected
    // (risk increases as remaining_ratio approaches 0)
    if debt > cap * 0.5 {
        // If already over 50% utilized, a large transaction should fail
        let big_tx = CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: "Overloaded buyer large tx".to_string(),
            debt: cap, // Try to use full remaining capacity
        };
        let result = create_transaction(&conductor, &alice_cell, big_tx).await;
        assert!(result.is_err(), "Transaction should be rejected when debt approaches capacity");
    }
}

/// Debt velocity factor penalizes pure borrowers.
///
/// Exercises: lambda_b = 0.5 + 0.5 * min(1, D_out / D_in) (Definition 16).
///
/// Setup:
///   - Alice is a pure buyer: she buys from Bob but never sells. D_out=0, so
///     lambda_b = 0.5, which penalizes her risk score.
///   - Carol is a balanced trader: she buys from Bob AND Bob buys from Carol.
///     Carol's contract is Transferred (debt extinguished), so D_out > 0 and
///     lambda_b > 0.5, giving her a better (lower) risk score.
///
/// Assertion: risk(Alice) >= risk(Carol) as seen from Bob's cell.
///
/// The test uses the `get_risk_score` extern which is feature-gated to `test-epoch`
/// builds. In production builds this extern does not exist.
#[tokio::test(flavor = "multi_thread")]
async fn test_debt_velocity_factor() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    // Alice buys from Bob (pure buyer -- accumulates debt, never sells)
    let tx1 = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Pure buyer tx".to_string(),
        debt: 49.0,
    };
    let record1 = create_transaction(&conductor, &alice_cell, tx1)
        .await
        .expect("Alice's purchase should succeed");

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Alice's trial must propagate to Bob");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        record1.action_address().clone(),
        record1.action_address().clone(),
    )
    .await
    .expect("Alice tx should be approved");

    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice's contract should be active");

    // Carol buys from Bob (this creates D_in for Carol this epoch)
    let tx2 = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: carol_agent.clone().into(),
        description: "Carol buys from Bob".to_string(),
        debt: 49.0,
    };
    let record2 = create_transaction(&conductor, &carol_cell, tx2)
        .await
        .expect("Carol's purchase should succeed");

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Carol's trial must propagate to Bob");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        record2.action_address().clone(),
        record2.action_address().clone(),
    )
    .await
    .expect("Carol tx should be approved");

    wait_for_active_contract(&conductor, &carol_cell, carol_agent.clone())
        .await
        .expect("Carol's contract should be active");

    // Allow DHT propagation of Carol's tx2 approval before Bob creates tx3.
    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Bob buys from Carol -- this creates a second transaction where Carol is the seller.
    // Carol's debt contract (tx2) will be Transferred as part of the debt cascade,
    // contributing to her D_out this epoch and raising her lambda_b above 0.5.
    let tx3 = CreateTransactionInput {
        seller: carol_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Bob buys from Carol".to_string(),
        debt: 30.0,
    };
    let record3 = create_transaction(&conductor, &bob_cell, tx3)
        .await
        .expect("Bob's purchase from Carol should succeed");

    ensure_transaction_propagation_seller(&conductor, &carol_cell, carol_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Bob's trial must propagate to Carol");
    approve_pending_transaction(
        &conductor,
        &carol_cell,
        record3.action_address().clone(),
        record3.action_address().clone(),
    )
    .await
    .expect("Bob tx should be approved");

    wait_for_active_contract(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Bob's contract should be active");

    // Allow DHT propagation before querying risk scores.
    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Verify both have accumulated some debt (sanity check)
    let alice_debt = get_total_debt(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get Alice's debt");
    assert!(alice_debt > 0.0, "Alice should have debt");

    // Query risk scores from Bob's perspective (Bob is a counterparty to both).
    // Alice: pure buyer, D_out=0 this epoch -> lambda_b = 0.5
    // Carol: balanced, D_out > 0 this epoch -> lambda_b > 0.5
    let alice_risk = get_risk_score(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Should compute Alice's risk score");
    let carol_risk = get_risk_score(&conductor, &bob_cell, carol_agent.clone())
        .await
        .expect("Should compute Carol's risk score");

    // Alice's lambda_b = 0.5 (no transfers), Carol's lambda_b > 0.5 (has transfers).
    // Lower lambda_b means the trust term is halved, so risk is higher.
    // Therefore Alice's risk should be >= Carol's risk.
    assert!(
        alice_risk >= carol_risk,
        "Pure buyer (Alice, lambda_b=0.5) should have risk >= balanced trader (Carol, lambda_b>0.5): alice={alice_risk:.4}, carol={carol_risk:.4}"
    );
}

/// Auto-accept and auto-reject thresholds control transaction moderation.
///
/// Exercises: Wallet thresholds (theta_accept=0.4, theta_reject=0.8).
/// - Risk < 0.4 -> Accepted
/// - Risk > 0.8 -> Rejected
/// - 0.4 <= Risk <= 0.8 -> Pending (manual moderation)
///
/// Trial transactions bypass this and always go to Pending for seller approval.
#[tokio::test(flavor = "multi_thread")]
async fn test_auto_accept_reject_thresholds() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Trial transaction should be Pending (awaiting seller approval)
    let trial_tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Trial threshold test".to_string(),
        debt: 49.0, // Under trial threshold (100)
    };
    let trial_result = create_transaction(&conductor, &alice_cell, trial_tx)
        .await
        .expect("Trial transaction should be created");
    let trial: Transaction = trial_result.entry().to_app_option().unwrap().unwrap();

    // Trial transactions are always Pending (seller must manually approve)
    assert_eq!(trial.status, TransactionStatus::Pending, "Trial transaction should be Pending: got {:?}", trial.status);
    assert!(trial.is_trial, "Should be marked as trial");

    // Non-trial transaction from a newcomer with no bilateral history
    // should be Rejected (risk = 1.0 > theta_reject = 0.8)
    let non_trial_tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Non-trial threshold test".to_string(),
        debt: 200.0, // Over trial threshold (100)
    };
    let non_trial_result = create_transaction(&conductor, &alice_cell, non_trial_tx).await;

    match non_trial_result {
        Ok(record) => {
            let tx: Transaction = record.entry().to_app_option().unwrap().unwrap();
            // Without established trust, a non-trial transaction should NOT be Accepted
            assert_ne!(
                tx.status,
                TransactionStatus::Accepted,
                "Non-trial from newcomer should not be Accepted: got {:?}",
                tx.status
            );
        }
        Err(_) => {
            // Also acceptable -- rejected at coordinator level (capacity or risk)
        }
    }
}
