use super::*;

/// Two-agent transaction: Alice (buyer) creates transaction with Bob (seller)
#[tokio::test(flavor = "multi_thread")]
async fn test_two_agent_transaction() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Trigger Bob's init to create his wallet
    let result = get_wallet_for_agent(&conductor, &bob_cell, bob_agent.clone()).await;
    match result {
        Ok((Some(_), Some(_))) => println!("Bob found his own wallet"),
        Ok((None, None)) => panic!("Bob failed to create/find his own wallet after init trigger"),
        Ok(_) => panic!("Bob found partial wallet data"),
        Err(e) => panic!("Bob's get_wallet_for_agent failed: {e:?}"),
    }

    // Wait for Bob's wallet to be visible to Alice
    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob's wallet should propagate to Alice");

    // Alice (buyer) creates a transaction with Bob (seller)
    let tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Alice buys from Bob".to_string(),
        debt: 49.0,
    };

    let record = create_transaction(&conductor, &alice_cell, tx_input)
        .await
        .expect("Transaction should be created successfully");

    // Allow DHT sync
    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Bob should see the transaction in his seller links
    // Trial transactions are now always Pending (seller must approve manually)
    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Transaction should propagate to Bob as seller");

    let seller_links =
        get_transactions_for_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
            .await
            .expect("Should get seller links");
    assert!(!seller_links.is_empty(), "Bob should have at least one seller link");

    // Alice should see it in her buyer links
    let buyer_links =
        get_transactions_for_buyer(&conductor, &alice_cell, alice_agent.clone(), TransactionStatusTag::Pending)
            .await
            .expect("Should get buyer links");
    assert!(!buyer_links.is_empty(), "Alice should have at least one buyer link");
}

/// Debt balance increases after transaction creation
#[tokio::test(flavor = "multi_thread")]
async fn test_debt_balance_increases() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Check Alice's initial debt
    let initial_debt = get_total_debt(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get initial debt");

    // Alice buys from Bob with debt=50
    let tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Test debt creation".to_string(),
        debt: 49.0,
    };

    create_transaction(&conductor, &alice_cell, tx_input)
        .await
        .expect("Transaction should succeed");

    // Transaction is a trial — must propagate to Bob and be manually approved
    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Trial must propagate to Bob");

    // Since we only have the action hash from the wallet-to-transaction index, we fetch it here
    let pending_txs =
        get_transactions_for_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
            .await
            .expect("Should get pending txs for Bob");
    let tx_record = pending_txs.first().expect("Bob should see pending tx");

    approve_pending_transaction(
        &conductor,
        &bob_cell,
        tx_record.action_address().clone(),
        tx_record.action_address().clone(),
    )
    .await
    .expect("Bob should approve Alice's trial");

    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice should have an active contract");

    // Alice's debt should have increased
    let new_debt = get_total_debt(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get new debt");

    assert!(new_debt >= initial_debt + 49.0, "Debt should have increased by ~50: was {initial_debt}, now {new_debt}");
}

/// Three-agent chain: Alice->Bob->Carol debt propagation
#[tokio::test(flavor = "multi_thread")]
async fn test_three_agent_chain() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    // Trigger init for all agents to ensure wallets are created
    let _ = get_wallet_for_agent(&conductor, &alice_cell, alice_agent.clone()).await;
    let _ = get_wallet_for_agent(&conductor, &bob_cell, bob_agent.clone()).await;
    let _ = get_wallet_for_agent(&conductor, &carol_cell, carol_agent.clone()).await;

    // Ensure wallets are propagated before transacting
    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob's wallet should propagate to Alice");

    ensure_wallet_propagation(&conductor, &bob_cell, carol_agent.clone())
        .await
        .expect("Carol's wallet should propagate to Bob");

    // Step 1: Alice buys from Bob (Alice gets debt, Bob is creditor)
    let tx1 = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Alice buys from Bob".to_string(),
        debt: 30.0,
    };
    let tx1_record = create_transaction(&conductor, &alice_cell, tx1)
        .await
        .expect("First transaction should succeed");

    // Bob manually approves Alice's trial
    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Alice's trial must propagate to Bob");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        tx1_record.action_address().clone(),
        tx1_record.action_address().clone(),
    )
    .await
    .expect("Bob should approve Alice's trial");

    // Wait for Alice's contract to be created (async call_remote from Bob)
    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice contract should be active");

    // Step 2: Bob buys from Carol (Bob gets debt, Carol is creditor)
    let tx2 = CreateTransactionInput {
        seller: carol_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Bob buys from Carol".to_string(),
        debt: 20.0,
    };
    let tx2_record = create_transaction(&conductor, &bob_cell, tx2)
        .await
        .expect("Second transaction should succeed");

    // Carol manually approves Bob's trial
    ensure_transaction_propagation_seller(&conductor, &carol_cell, carol_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Bob's trial must propagate to Carol");
    approve_pending_transaction(
        &conductor,
        &carol_cell,
        tx2_record.action_address().clone(),
        tx2_record.action_address().clone(),
    )
    .await
    .expect("Carol should approve Bob's trial");

    // Wait for Bob's contract to be created
    wait_for_active_contract(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Bob contract should be active");

    // Verify debt balances
    let alice_debt = get_total_debt(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get Alice's debt");
    let bob_debt = get_total_debt(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Should get Bob's debt");

    assert!(alice_debt >= 29.0, "Alice should have ~30 debt: got {alice_debt}");
    assert!(bob_debt >= 19.0, "Bob should have ~20 debt: got {bob_debt}");
}

/// Bilateral history check returns appropriate values
#[tokio::test(flavor = "multi_thread")]
async fn test_bilateral_history_check() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Before transaction: check bilateral history type
    let before_history = check_bilateral_history(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Should get history check result");
    // Just verify it returns a boolean (may be false or may have been set by init)
    let _ = before_history; // Type checked at compile time

    // Create a trial transaction (Alice buys from Bob) and approve it so that
    // a DebtContract is created. check_bilateral_history checks CreditorToContracts
    // links — a Pending transaction with no approved contract does not create
    // bilateral history, so the approval step is required.
    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "History builder".to_string(),
        debt: 10.0,
    };
    let record = create_transaction(&conductor, &alice_cell, tx)
        .await
        .expect("Transaction should succeed");

    // Bob must approve the trial for the DebtContract to be created
    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Transaction must propagate to Bob as Pending");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        record.action_address().clone(),
        record.action_address().clone(),
    )
    .await
    .expect("Bob should approve Alice's trial");

    // Wait for the DebtContract to propagate
    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice must have an active contract after approval");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // After the approved trial, Bob (as creditor) should have bilateral history with Alice (debtor)
    let after_history = check_bilateral_history(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Should get history check result");
    assert!(
        after_history,
        "After Alice's trial is approved, Bob (creditor) should have bilateral history with Alice (debtor)"
    );
}
