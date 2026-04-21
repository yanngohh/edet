use super::*;

// ============================================================================
//  Helper: reject_pending_transaction (seller action)
// ============================================================================

/// Reject a pending transaction (seller action).
pub async fn reject_pending_transaction(
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
        .call_fallible(&cell.zome("transaction"), "reject_pending_transaction", input)
        .await
}

// ============================================================================
//  Tests
// ============================================================================

/// The seller can successfully reject a pending transaction.
/// Verifies that `reject_pending_transaction` transitions status to Rejected
/// and that neither party ends up with a DebtContract.
#[tokio::test(flavor = "multi_thread")]
async fn test_reject_pending_transaction() {
    // Alice is buyer, Bob is seller.
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice creates a trial transaction to Bob (always Pending).
    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Alice");

    let tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Trial to be rejected".to_string(),
        debt: 10.0, // trial amount
    };
    let tx_record = create_transaction(&conductor, &alice_cell, tx_input)
        .await
        .expect("Create trial transaction");
    let tx: Transaction = tx_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(tx.status, TransactionStatus::Pending, "Trial must be Pending");
    assert!(tx.is_trial, "Must be a trial transaction");

    // Wait for the transaction to propagate to Bob.
    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Transaction must propagate to Bob");

    // Bob rejects.
    let rejected_record = reject_pending_transaction(
        &conductor,
        &bob_cell,
        tx_record.action_address().clone(),
        tx_record.action_address().clone(),
    )
    .await
    .expect("Bob should be able to reject the pending transaction");

    let rejected_tx: Transaction = rejected_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(rejected_tx.status, TransactionStatus::Rejected, "Status must be Rejected after rejection");

    // No DebtContract should exist for Alice (contracts are only created on Accepted).
    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }
    let alice_contracts = get_active_contracts_for_debtor(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should query contracts");
    assert!(alice_contracts.is_empty(), "No DebtContract should exist after rejection");
}

/// Test that rejecting a transaction does not cause an EV200005 error
/// (Seller Last Transaction Obsolete) when approving the next transaction.
/// This verifies the `skip` logic for Rejected transactions in the integrity zome.
#[tokio::test(flavor = "multi_thread")]
async fn test_approve_after_reject_ev200005_fix() {
    let (conductor, apps) = setup_multi_agent_no_vouch(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone(); // Carol is Seller

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, carol_agent.clone())
        .await
        .unwrap();
    ensure_wallet_propagation(&conductor, &bob_cell, carol_agent.clone())
        .await
        .unwrap();

    // Alice creates Tx 1 to Carol
    let tx1_input = CreateTransactionInput {
        seller: carol_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Tx 1 to be rejected".to_string(),
        debt: 10.0,
    };
    let tx1_record = create_transaction(&conductor, &alice_cell, tx1_input).await.unwrap();

    // Bob creates Tx 2 to Carol
    let tx2_input = CreateTransactionInput {
        seller: carol_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Tx 2 to be approved".to_string(),
        debt: 15.0,
    };
    let tx2_record = create_transaction(&conductor, &bob_cell, tx2_input).await.unwrap();

    let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
    await_consistency(30, &cells).await.unwrap();

    // Carol rejects Tx 1
    let rejected_record = reject_pending_transaction(
        &conductor,
        &carol_cell,
        tx1_record.action_address().clone(),
        tx1_record.action_address().clone(),
    )
    .await
    .expect("Carol should be able to reject Tx 1");

    let rejected_tx: Transaction = rejected_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(rejected_tx.status, TransactionStatus::Rejected);

    await_consistency(30, &cells).await.unwrap();

    // Carol approves Tx 2 (this would crash with EV200005 before the fix)
    let approved_record = approve_pending_transaction(
        &conductor,
        &carol_cell,
        tx2_record.action_address().clone(),
        tx2_record.action_address().clone(),
    )
    .await
    .expect("Carol should be able to approve Tx 2 without EV200005 error");

    let approved_tx: Transaction = approved_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(approved_tx.status, TransactionStatus::Accepted);
}

/// A non-seller agent must not be able to reject a pending transaction.
/// Verifies that `reject_pending_transaction` returns an error when called
/// by the buyer (Alice) instead of the seller (Bob).
#[tokio::test(flavor = "multi_thread")]
async fn test_reject_not_seller_fails() {
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Alice");

    let tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Trial for wrong-reject test".to_string(),
        debt: 10.0,
    };
    let tx_record = create_transaction(&conductor, &alice_cell, tx_input)
        .await
        .expect("Create trial transaction");

    // Alice (the buyer) tries to call reject — should fail.
    let result = reject_pending_transaction(
        &conductor,
        &alice_cell, // wrong caller: buyer, not seller
        tx_record.action_address().clone(),
        tx_record.action_address().clone(),
    )
    .await;

    assert!(result.is_err(), "Buyer must not be able to reject a transaction; expected error but got success");
    let err = format!("{result:?}");
    assert!(
        err.contains(coordinator_transaction_error::REJECT_NOT_SELLER),
        "Expected REJECT_NOT_SELLER error, got: {err}"
    );
}

/// Rejecting an already-Accepted transaction must fail.
/// Verifies the "not pending" guard in `reject_pending_transaction`.
#[tokio::test(flavor = "multi_thread")]
async fn test_reject_already_accepted_fails() {
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Alice");

    let tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Trial to accept then attempt re-reject".to_string(),
        debt: 10.0,
    };
    let tx_record = create_transaction(&conductor, &alice_cell, tx_input)
        .await
        .expect("Create trial transaction");

    // Wait for propagation to Bob.
    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Transaction must propagate to Bob");

    // Bob approves first.
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        tx_record.action_address().clone(),
        tx_record.action_address().clone(),
    )
    .await
    .expect("Bob should approve the transaction");

    // Wait for the update to be visible.
    wait_for_transaction_status(&conductor, &bob_cell, tx_record.action_address().clone(), TransactionStatus::Accepted)
        .await
        .expect("Transaction must reach Accepted status");

    // Now Bob tries to reject the already-Accepted transaction.
    let result = reject_pending_transaction(
        &conductor,
        &bob_cell,
        tx_record.action_address().clone(),
        tx_record.action_address().clone(),
    )
    .await;

    // The rejection must fail — the exact error code may vary:
    // - REJECT_NOT_PENDING if the coordinator's guard catches it first
    // - An integrity validation error if the update is rejected at source chain commit
    // Either outcome proves the operation was correctly blocked.
    assert!(result.is_err(), "Rejecting an already-Accepted transaction must fail; expected error but got success");
}

/// The seller can retrieve their pending transactions via `get_pending_transactions_for_seller`.
/// Verifies that only Pending transactions where the caller is the seller are returned.
#[tokio::test(flavor = "multi_thread")]
async fn test_get_pending_transactions_for_seller() {
    let (conductor, apps) = setup_multi_agent_no_vouch(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    // Alice and Carol both create trial transactions to Bob (both Pending).
    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Alice");

    let alice_tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Alice trial to Bob".to_string(),
        debt: 10.0,
    };
    create_transaction(&conductor, &alice_cell, alice_tx_input)
        .await
        .expect("Alice trial to Bob must succeed");

    ensure_wallet_propagation(&conductor, &carol_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Carol");

    let carol_tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: carol_agent.clone().into(),
        description: "Carol trial to Bob".to_string(),
        debt: 10.0,
    };
    create_transaction(&conductor, &carol_cell, carol_tx_input)
        .await
        .expect("Carol trial to Bob must succeed");

    // Wait for both trials to propagate to Bob's cell.
    let mut retries = 0;
    loop {
        let pending: Vec<Record> = conductor
            .call(&bob_cell.zome("transaction"), "get_pending_transactions_for_seller", ())
            .await;
        if pending.len() >= 2 {
            break;
        }
        retries += 1;
        assert!(retries < 60, "Timeout waiting for both pending transactions to appear for Bob");
        {
            let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

            await_consistency(30, &cells).await.unwrap();
        }
    }

    // All returned records must be Pending and Bob must be the seller.
    let pending: Vec<Record> = conductor
        .call(&bob_cell.zome("transaction"), "get_pending_transactions_for_seller", ())
        .await;

    assert!(pending.len() >= 2, "Bob must see at least 2 pending transactions; got {}", pending.len());
    for record in &pending {
        let tx: Transaction = record.entry().to_app_option().unwrap().unwrap();
        assert_eq!(tx.status, TransactionStatus::Pending, "All returned transactions must be Pending");
        let tx_seller: AgentPubKey = tx.seller.pubkey.into();
        assert_eq!(tx_seller, bob_agent, "All returned transactions must have Bob as seller");
    }
}

/// Calling `create_buyer_debt_contract` from an agent who is not the buyer
/// must be rejected with an appropriate error.
#[tokio::test(flavor = "multi_thread")]
async fn test_create_buyer_debt_contract_wrong_caller_fails() {
    let (conductor, apps) = setup_multi_agent_no_vouch(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let eve_cell = apps[2].cells()[0].clone(); // attacker

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice creates a trial transaction to Bob, Bob approves it.
    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Alice");

    let tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Trial for wrong-buyer-contract test".to_string(),
        debt: 10.0,
    };
    let tx_record = create_transaction(&conductor, &alice_cell, tx_input)
        .await
        .expect("Create trial transaction");

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Transaction must propagate to Bob");

    approve_pending_transaction(
        &conductor,
        &bob_cell,
        tx_record.action_address().clone(),
        tx_record.action_address().clone(),
    )
    .await
    .expect("Bob should approve the transaction");

    let accepted_record = wait_for_transaction_status(
        &conductor,
        &bob_cell,
        tx_record.action_address().clone(),
        TransactionStatus::Accepted,
    )
    .await
    .expect("Transaction must reach Accepted status");

    let accepted_tx: Transaction = accepted_record.entry().to_app_option().unwrap().unwrap();

    // Eve (the attacker) tries to call notify_buyer_of_accepted_transaction for
    // Alice's transaction. Eve is not the buyer, so this must fail.
    #[derive(Debug, Serialize, Deserialize)]
    struct NotifyBuyerPayload {
        pub original_transaction_hash: ActionHash,
        pub updated_transaction_hash: ActionHash,
        pub transaction: Transaction,
    }

    let payload = NotifyBuyerPayload {
        original_transaction_hash: tx_record.action_address().clone(),
        updated_transaction_hash: accepted_record.action_address().clone(),
        transaction: accepted_tx,
    };

    let result: ConductorApiResult<()> = conductor
        .call_fallible(&eve_cell.zome("transaction"), "notify_buyer_of_accepted_transaction", payload)
        .await;

    assert!(
        result.is_err(),
        "Eve must not be able to trigger buyer side-effects for Alice's transaction; expected error"
    );
    let err = format!("{result:?}");
    assert!(
        err.contains(coordinator_transaction_error::CREATE_CONTRACT_CALLER_NOT_BUYER),
        "Expected CREATE_CONTRACT_CALLER_NOT_BUYER error, got: {err}"
    );
}
