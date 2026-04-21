use super::*;

/// Test that trial velocity limit is enforced.
#[tokio::test(flavor = "multi_thread")]
async fn test_trial_velocity_limit() {
    let (conductor, apps) = setup_multi_agent_no_vouch(3).await;
    let bob_cell = apps[0].cells()[0].clone();
    let alice_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    let (_, bob_wallet_record) = get_wallet_for_agent(&conductor, &bob_cell, bob_agent.clone()).await.unwrap();
    let bob_wallet: Wallet = bob_wallet_record.unwrap().entry().to_app_option().unwrap().unwrap();
    assert_eq!(bob_wallet.trial_tx_count, 0, "Bob's initial trial count should be 0");

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Alice");
    let alice_tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Alice trial".to_string(),
        debt: 10.0,
    };
    let alice_record = create_transaction(&conductor, &alice_cell, alice_tx_input).await.unwrap();
    let alice_tx: Transaction = alice_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(alice_tx.status, TransactionStatus::Pending, "Alice's trial should be Pending");
    assert!(alice_tx.is_trial, "Alice's trial should be marked is_trial=true");

    ensure_wallet_propagation(&conductor, &carol_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Carol");
    let carol_tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: carol_agent.clone().into(),
        description: "Carol trial".to_string(),
        debt: 10.0,
    };
    let carol_record = create_transaction(&conductor, &carol_cell, carol_tx_input).await.unwrap();
    let carol_tx: Transaction = carol_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(carol_tx.status, TransactionStatus::Pending, "Carol's trial should be Pending");
    assert!(carol_tx.is_trial, "Carol's trial should be marked is_trial=true");

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Alice's trial must propagate to Bob");

    // Bob manually approves Alice's trial (trials always require explicit seller approval)
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        alice_record.action_address().clone(),
        alice_record.action_address().clone(),
    )
    .await
    .expect("Bob should approve Alice's trial");

    wait_for_wallet_state(&conductor, &bob_cell, bob_agent.clone(), 1)
        .await
        .expect("Trial count should reach 1 after Alice's approval");

    let large_tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: carol_agent.clone().into(),
        description: "Carol non-trial (large)".to_string(),
        debt: 200.0,
    };
    let large_result = create_transaction(&conductor, &carol_cell, large_tx_input).await;
    assert!(
        large_result.is_err(),
        "Non-trial tx for newcomer with 0 capacity should be rejected with error; got {large_result:?}"
    );
}

/// Test that co-signers are penalized when a contract defaults
#[tokio::test(flavor = "multi_thread")]
async fn test_cosigner_penalty() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let charlie_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let charlie_agent: AgentPubKey = charlie_cell.agent_pubkey().clone();

    let vouch_input =
        CreateVouchInput { sponsor: charlie_agent.clone().into(), entrant: bob_agent.clone().into(), amount: 500.0 };
    let _ = genesis_vouch(&conductor, &charlie_cell, vouch_input).await.unwrap();

    let bd_input = CreateSupportBreakdownInput {
        owner: bob_agent.clone().into(),
        addresses: vec![bob_agent.clone().into(), charlie_agent.clone().into()],
        coefficients: vec![0.5, 0.5],
    };
    let _ = create_support_breakdown(&conductor, &bob_cell, bd_input).await.unwrap();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let tx_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Transaction with co-signer".to_string(),
        debt: 49.0,
    };
    let record = create_transaction(&conductor, &alice_cell, tx_input).await.unwrap();
    let tx: Transaction = record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(tx.status, TransactionStatus::Pending, "Trial tx should be Pending");
    assert!(tx.is_trial, "Trial tx should be marked is_trial=true");

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Alice's trial must propagate to Bob");

    // Bob manually approves Alice's trial (trials always require explicit seller approval)
    let _approved_record = approve_pending_transaction(
        &conductor,
        &bob_cell,
        record.action_address().clone(),
        record.action_address().clone(),
    )
    .await
    .expect("Bob should approve Alice's trial");

    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice must have an active debt contract after Bob's approval");

    let exp_result = process_contract_expirations(&conductor, &alice_cell).await.unwrap();
    assert_eq!(exp_result.total_expired, 0.0);

    let contracts = get_all_contracts_as_debtor(&conductor, &alice_cell, alice_agent.clone())
        .await
        .unwrap();
    let contract_rec = contracts.first().expect("Should have one contract");
    let contract: DebtContract = contract_rec.entry().to_app_option().unwrap().unwrap();

    assert!(contract.co_signers.is_some(), "Contract should have co-signers from Bob's support");
    let co_signers = contract.co_signers.unwrap();
    assert!(co_signers.iter().any(|(addr, _)| *addr == charlie_agent.clone().into()), "Charlie should be a co-signer");
}
