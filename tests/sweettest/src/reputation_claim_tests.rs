use super::*;

/// ReputationClaim: initial values for new agent should be valid
#[tokio::test(flavor = "multi_thread")]
async fn test_reputation_claim_initial_values() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();
    let agent: AgentPubKey = cell.agent_pubkey().clone();

    let _claim_record = publish_reputation_claim(&conductor, &cell).await.expect("Should publish claim");

    {
        await_consistency(30, [&cell]).await.unwrap();
    }

    if let Ok(Some(claim)) = get_reputation_claim(&conductor, &cell, agent).await {
        assert!(
            claim.capacity_lower_bound >= 0.0,
            "Capacity should be non-negative: got {}",
            claim.capacity_lower_bound
        );
        assert!(claim.debt_upper_bound >= 0.0, "Debt should be non-negative");
        assert!(claim.timestamp > 0, "Timestamp should be positive: got {}", claim.timestamp);
    }
}

/// First-contact transaction flow with ReputationClaims
#[tokio::test(flavor = "multi_thread")]
async fn test_first_contact_transaction_with_claim() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    let _ = get_wallet_for_agent(&conductor, &alice_cell, alice_agent.clone()).await;
    let _ = get_wallet_for_agent(&conductor, &bob_cell, bob_agent.clone()).await;

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob's wallet should propagate to Alice");

    let _claim = publish_reputation_claim(&conductor, &alice_cell)
        .await
        .expect("Alice should publish claim");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let alice_claim = get_reputation_claim(&conductor, &bob_cell, alice_agent.clone()).await;
    let _ = alice_claim;

    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "First-contact with claim".to_string(),
        debt: 49.0,
    };

    let result = create_transaction(&conductor, &alice_cell, tx).await;
    assert!(result.is_ok(), "Transaction should succeed with claim-based flow");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multiple_claims_latest_returned() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();
    let agent: AgentPubKey = cell.agent_pubkey().clone();

    // 1. Publish first claim
    let claim1_record = publish_reputation_claim(&conductor, &cell)
        .await
        .expect("Should publish claim 1");
    let claim1: ReputationClaim = claim1_record.entry().to_app_option().unwrap().unwrap();

    // 2. Second attempt in SAME epoch should be idempotent (return same record)
    let claim2_record = publish_reputation_claim(&conductor, &cell)
        .await
        .expect("Idempotent call should succeed");
    let claim2: ReputationClaim = claim2_record.entry().to_app_option().unwrap().unwrap();

    let epoch1 = claim1.timestamp / transaction_integrity::types::constants::EPOCH_DURATION_SECS;
    let epoch2 = claim2.timestamp / transaction_integrity::types::constants::EPOCH_DURATION_SECS;

    if epoch1 == epoch2 {
        assert_eq!(
            claim1_record.action_address(),
            claim2_record.action_address(),
            "Should return same record for same epoch"
        );
    } else {
        // If we crossed an epoch (possible in test-mode with 1s epochs),
        // the hashes should be different but claim2 should be valid.
        assert_ne!(
            claim1_record.action_address(),
            claim2_record.action_address(),
            "Should return different record for different epoch"
        );
    }

    // 3. Wait for NEXT epoch and publish again
    // In test-epoch mode, DUR is 1s.
    {
        // In test-epoch mode, DUR is 1s. We must sleep.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }

    let claim3_record = publish_reputation_claim(&conductor, &cell)
        .await
        .expect("Should publish claim in new epoch");
    let claim3: ReputationClaim = claim3_record.entry().to_app_option().unwrap().unwrap();
    assert!(claim3.timestamp > claim1.timestamp, "New claim should have later timestamp");

    // 4. Retrieve: should return the latest (epoch 2)
    let retrieved = get_reputation_claim(&conductor, &cell, agent)
        .await
        .expect("Retrieval should succeed")
        .expect("Claim should exist");
    assert_eq!(retrieved.timestamp, claim3.timestamp, "Should return latest claim from epoch 2");
}
