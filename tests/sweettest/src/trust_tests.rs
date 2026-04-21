use super::*;

/// Reputation claim: published claim has valid structure after transaction
#[tokio::test(flavor = "multi_thread")]
async fn test_reputation_claim_valid_structure() {
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

    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Trust test transaction".to_string(),
        debt: 25.0,
    };
    create_transaction(&conductor, &alice_cell, tx)
        .await
        .expect("Transaction should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let claim_record = publish_reputation_claim(&conductor, &alice_cell)
        .await
        .expect("Reputation claim should be published");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let retrieved_claim = get_reputation_claim(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("get_reputation_claim call should succeed")
        .expect("Bob should be able to retrieve Alice's published claim");

    assert!(retrieved_claim.capacity_lower_bound >= 0.0, "Capacity should be non-negative");
    assert!(retrieved_claim.debt_upper_bound >= 0.0, "Debt bound should be non-negative");
    assert!(retrieved_claim.timestamp > 0, "Timestamp should be positive");
}

/// Subjective reputation: returns valid result for known agent
#[tokio::test(flavor = "multi_thread")]
async fn test_subjective_reputation_valid_result() {
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

    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Reputation query test".to_string(),
        debt: 15.0,
    };
    create_transaction(&conductor, &alice_cell, tx)
        .await
        .expect("Transaction should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let rep = get_subjective_reputation(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Reputation result should not be null");

    assert!(rep.trust >= 0.0 && rep.trust <= 1.0, "Trust should be in [0,1]: got {}", rep.trust);
}

/// Trust cache: invalidation clears cached data
#[tokio::test(flavor = "multi_thread")]
async fn test_trust_cache_invalidation() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();

    let initial_stats = get_trust_cache_stats(&conductor, &cell)
        .await
        .expect("Cache stats should be accessible");
    let _ = initial_stats;

    invalidate_trust_caches(&conductor, &cell)
        .await
        .expect("Cache invalidation should succeed");

    let post_stats = get_trust_cache_stats(&conductor, &cell)
        .await
        .expect("Cache stats should still be accessible after invalidation");
    assert_eq!(post_stats.num_cached_reputations, 0, "Cached reputations should be 0 after invalidation");
    assert_eq!(post_stats.num_cached_dht_trust_rows, 0, "Cached DHT trust rows should be 0 after invalidation");
}

/// Credit capacity: new agent has base capacity
#[tokio::test(flavor = "multi_thread")]
async fn test_new_agent_has_base_capacity() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();

    let _ = publish_reputation_claim(&conductor, &alice_cell).await;

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let claim = get_reputation_claim(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("get_reputation_claim call should succeed")
        .expect("Bob should be able to retrieve Alice's published claim — if this fails, publish_reputation_claim above silently failed");

    assert!(
        claim.capacity_lower_bound >= 400.0,
        "New agent capacity should be >= 400 (vouched=500 minus rounding): got {}",
        claim.capacity_lower_bound
    );
}

/// Vouch trust: sponsor sees vouchee as trusted via AgentToLocalTrust
#[tokio::test(flavor = "multi_thread")]
async fn test_trust_flows_via_vouch() {
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice vouches for Bob
    let vouch_input =
        CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: bob_agent.clone().into(), amount: 250.0 };
    genesis_vouch(&conductor, &alice_cell, vouch_input)
        .await
        .expect("Genesis vouch should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // Alice publishes her trust row (should now include Bob)
    publish_trust_row(&conductor, &alice_cell)
        .await
        .expect("Publish trust row should succeed");

    // Verify Bob is Alice's acquaintance
    let acqs = get_acquaintances(&conductor, &alice_cell)
        .await
        .expect("Acquaintances check should succeed");
    assert!(acqs.contains(&bob_agent), "Bob should be in Alice's acquaintance list: got {acqs:?}");

    // Verify Bob has vouched capacity
    let cap = get_vouched_capacity(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Capacity check should succeed");
    assert_eq!(cap, 250.0, "Bob should have 250 units from Alice: got {cap}");

    // Alice checks Bob's reputation from her perspective
    let rep = get_subjective_reputation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Reputation check should succeed");

    assert!(
        rep.trust > 0.0,
        "Sponsor should see vouchee as trusted: got {}, acqs={}",
        rep.trust,
        rep.acquaintance_count
    );
}

/// Repayment reputation: buyer gains self-reputation after paying back creditor
#[tokio::test(flavor = "multi_thread")]
async fn test_reputation_after_trial_repayment() {
    let (conductor, apps) = setup_multi_agent(3).await; // Alice=0, Bob=1, Charlie=2
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let charlie_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let charlie_agent: AgentPubKey = charlie_cell.agent_pubkey().clone();

    // 1. Bob (new) buys trial from Alice (creditor)
    let tx_input = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Repayment test trial".to_string(),
        debt: 20.0,
    };
    // Buyer (Bob) creates the transaction
    let tx_record = create_transaction(&conductor, &bob_cell, tx_input)
        .await
        .expect("Trial transaction should succeed");

    // Seller (Alice) approves it
    approve_pending_transaction(
        &conductor,
        &alice_cell,
        tx_record.action_address().clone(),
        tx_record.action_address().clone(),
    )
    .await
    .expect("Approval should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // Alice publishes her trust row (so Bob can see her in his subgraph)
    publish_trust_row(&conductor, &alice_cell)
        .await
        .expect("Alice publish should succeed");

    // 2. Bob sells to Charlie to pay back Alice
    // Charlie is the buyer. Charlie must initiate.
    let tx2_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: charlie_agent.clone().into(),
        description: "Repayment sale".to_string(),
        debt: 25.0,
    };
    let tx2_record = create_transaction(&conductor, &charlie_cell, tx2_input)
        .await
        .expect("Sale transaction should succeed");

    // Bob (seller) approves it
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        tx2_record.action_address().clone(),
        tx2_record.action_address().clone(),
    )
    .await
    .expect("Sale approval should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // 3. Bob publishes trust row and checks self-reputation
    // He should now have Satisfaction for Alice (his witness)
    publish_trust_row(&conductor, &bob_cell)
        .await
        .expect("Publish trust row should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
        // Give it a bit more time for DHT links to settle
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
    }

    let self_rep = get_subjective_reputation(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Self reputation check should succeed");

    assert!(self_rep.trust > 0.0, "Agent should gain self-reputation after repayment: got {}", self_rep.trust);
}
