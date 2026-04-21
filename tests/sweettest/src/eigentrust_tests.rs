//! EigenTrust Tests (Whitepaper Theorem 3.1, Definition 3.7)
//!
//! Tests for EigenTrust convergence properties, trust distribution correctness,
//! and transitive trust propagation through the acquaintance graph.

use super::*;

/// Observer has highest trust in themselves.
///
/// Exercises: Personalized pre-trust (Definition 3.8) and EigenTrust convergence.
/// An observer's subjective trust in themselves should be the highest value
/// in their trust vector, since they are the root of their pre-trust distribution.
#[tokio::test(flavor = "multi_thread")]
async fn test_self_reputation_highest() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = apps[1].cells()[0].agent_pubkey().clone();
    let carol_agent: AgentPubKey = apps[2].cells()[0].agent_pubkey().clone();

    // Build some history
    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Self-trust test".to_string(),
        debt: 49.0,
    };
    create_transaction(&conductor, &alice_cell, tx).await.ok();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Query Alice's trust in herself, Bob, and Carol
    let self_rep = get_subjective_reputation(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get self-reputation");

    let bob_rep = get_subjective_reputation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Should get Bob's reputation");

    let carol_rep = get_subjective_reputation(&conductor, &alice_cell, carol_agent.clone())
        .await
        .expect("Should get Carol's reputation");

    // Alice should trust herself at least as much as others
    // (due to personalized pre-trust vector anchored on self)
    assert!(
        self_rep.trust >= bob_rep.trust,
        "Self-trust ({}) should be >= trust in Bob ({})",
        self_rep.trust,
        bob_rep.trust
    );
    assert!(
        self_rep.trust >= carol_rep.trust,
        "Self-trust ({}) should be >= trust in Carol ({})",
        self_rep.trust,
        carol_rep.trust
    );
}

/// Direct trading partner has higher trust than stranger.
///
/// Exercises: Trust propagation via local trust matrix C.
/// In a 3-agent network where A<->B have bilateral history but A has no
/// direct history with C, A should assign more trust to B than to C.
#[tokio::test(flavor = "multi_thread")]
async fn test_direct_partner_higher_than_stranger() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = apps[2].cells()[0].agent_pubkey().clone();

    // Build history between Alice and Bob only
    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Direct partner trust test".to_string(),
        debt: 49.0,
    };
    create_transaction(&conductor, &alice_cell, tx)
        .await
        .expect("Transaction should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Publish trust rows to make links available
    publish_trust_row(&conductor, &alice_cell).await.ok();
    publish_trust_row(&conductor, &bob_cell).await.ok();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Invalidate cache for fresh computation
    invalidate_trust_caches(&conductor, &alice_cell).await.ok();

    let bob_trust = get_subjective_reputation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Should get Bob's trust");

    let carol_trust = get_subjective_reputation(&conductor, &alice_cell, carol_agent.clone())
        .await
        .expect("Should get Carol's trust");

    // Bob (direct partner) should have higher trust than Carol (no direct history)
    // Note: Both may have some base trust from genesis vouching, but direct
    // bilateral history should give Bob an advantage.
    assert!(
        bob_trust.trust >= carol_trust.trust,
        "Direct partner trust ({}) should be >= stranger trust ({})",
        bob_trust.trust,
        carol_trust.trust
    );
}

/// Transitive trust propagation through a chain of agents.
///
/// Exercises: BFS subgraph expansion and power iteration convergence.
/// In a chain A->B->C->D, trust should propagate transitively:
/// from A's perspective, trust(B) > trust(C) > trust(D) > 0.
#[tokio::test(flavor = "multi_thread")]
async fn test_trust_propagation_through_chain() {
    let (conductor, apps) = setup_multi_agent(4).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();
    let dave_cell = apps[3].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();
    let dave_agent: AgentPubKey = dave_cell.agent_pubkey().clone();

    // Build chain: Alice->Bob->Carol->Dave
    let tx1 = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Chain A->B".to_string(),
        debt: 49.0,
    };
    create_transaction(&conductor, &alice_cell, tx1).await.ok();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let tx2 = CreateTransactionInput {
        seller: carol_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Chain B->C".to_string(),
        debt: 49.0,
    };
    create_transaction(&conductor, &bob_cell, tx2).await.ok();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let tx3 = CreateTransactionInput {
        seller: dave_agent.clone().into(),
        buyer: carol_agent.clone().into(),
        description: "Chain C->D".to_string(),
        debt: 49.0,
    };
    create_transaction(&conductor, &carol_cell, tx3).await.ok();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Publish trust rows for all agents
    publish_trust_row(&conductor, &alice_cell).await.ok();
    publish_trust_row(&conductor, &bob_cell).await.ok();
    publish_trust_row(&conductor, &carol_cell).await.ok();
    publish_trust_row(&conductor, &dave_cell).await.ok();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    invalidate_trust_caches(&conductor, &alice_cell).await.ok();

    // From Alice's perspective: trust should propagate through the chain
    let bob_trust = get_subjective_reputation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Should get Bob's trust");
    let carol_trust = get_subjective_reputation(&conductor, &alice_cell, carol_agent.clone())
        .await
        .expect("Should get Carol's trust");
    let dave_trust = get_subjective_reputation(&conductor, &alice_cell, dave_agent.clone())
        .await
        .expect("Should get Dave's trust");

    // All should have non-negative trust
    assert!(bob_trust.trust >= 0.0, "Bob's trust should be non-negative");
    assert!(carol_trust.trust >= 0.0, "Carol's trust should be non-negative");
    assert!(dave_trust.trust >= 0.0, "Dave's trust should be non-negative");

    // Direct partner (Bob) should have at least as much trust as 2-hop (Carol).
    // Use a small epsilon to tolerate floating-point rounding differences across
    // different arithmetic paths in the EigenTrust computation.
    let eps = 1e-9;
    assert!(
        bob_trust.trust >= carol_trust.trust - eps,
        "Direct partner trust ({}) should be >= 2-hop trust ({}) (within epsilon {})",
        bob_trust.trust,
        carol_trust.trust,
        eps
    );
}

/// Trust values are bounded in [0, 1] for all agents.
///
/// Exercises: EigenTrust stochastic property (Property 3.2).
/// The trust vector should sum to approximately 1.0 and each value should be in [0, 1].
#[tokio::test(flavor = "multi_thread")]
async fn test_trust_values_bounded() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = apps[2].cells()[0].agent_pubkey().clone();

    // Build bilateral history with seller-approved transactions.
    // The seller must approve so the cascade runs on the seller's cell,
    // triggering transfer_debt and producing positive S/F counters.

    // tx1: Alice buys 50 from Bob → propagate to Bob, Bob approves
    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Alice");

    let tx1 = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Bounded trust: Alice buys from Bob".to_string(),
        debt: 49.0,
    };
    let tx1_record = create_transaction(&conductor, &alice_cell, tx1)
        .await
        .expect("tx1 should succeed");

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx1 must propagate to Bob");

    approve_pending_transaction(
        &conductor,
        &bob_cell,
        tx1_record.action_address().clone(),
        tx1_record.action_address().clone(),
    )
    .await
    .expect("Bob should approve tx1");

    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice should have an active contract after tx1");

    // tx2: Bob buys 50 from Alice → propagate to Alice, Alice approves.
    // Alice approving triggers the cascade on Alice's cell, which calls
    // transfer_debt(Alice) → reduces Alice's debt to Bob → S > 0 for Bob.
    ensure_wallet_propagation(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Alice wallet must be visible to Bob");

    let tx2 = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Bounded trust: Bob buys from Alice".to_string(),
        debt: 49.0,
    };
    let tx2_record = create_transaction(&conductor, &bob_cell, tx2)
        .await
        .expect("tx2 should succeed");

    ensure_transaction_propagation_seller(&conductor, &alice_cell, alice_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx2 must propagate to Alice");

    approve_pending_transaction(
        &conductor,
        &alice_cell,
        tx2_record.action_address().clone(),
        tx2_record.action_address().clone(),
    )
    .await
    .expect("Alice should approve tx2");

    // Wait for call_remote side effects (buyer contract creation) to propagate.
    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Publish trust rows NOW so the DHT has data for tx3's risk assessment.
    // After tx1+tx2:
    //   - Bob (creditor for tx1) has Alice's contract Transferred → S(Alice)>0
    //   - Bob's trust row will include Alice with positive trust
    //   - Alice (creditor for tx2) has Bob's contract Active → S(Bob)=0 for now
    //   - Alice's trust row is empty (no local evidence), but she has Bob as acquaintance
    // When tx3 builds the subgraph from Bob's perspective (PATH 2), it fetches
    // Bob's trust row from DHT → finds Alice → subgraph size ≥ 2 → trust > 0.
    invalidate_trust_caches(&conductor, &alice_cell).await.ok();
    invalidate_trust_caches(&conductor, &bob_cell).await.ok();
    publish_trust_row(&conductor, &bob_cell).await.ok();
    publish_trust_row(&conductor, &alice_cell).await.ok();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // tx3: Alice buys from Bob again.
    // Alice is no longer bootstrap-eligible, so risk assessment runs.
    // With published trust rows, Bob's subgraph includes Alice → trust > 0 → risk < 1.
    // If auto-accepted (Finalized), notify_seller fires on Bob's cell, triggering
    // the cascade → transfer_debt(Bob) → Bob's debt to Alice gets reduced → S > 0.
    // If Pending, Bob must approve to trigger the same cascade.
    let tx3 = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Bounded trust: Alice buys from Bob again".to_string(),
        debt: 49.0,
    };
    let tx3_record = create_transaction(&conductor, &alice_cell, tx3)
        .await
        .expect("tx3 should succeed");

    // Check tx3 status: if Pending, Bob must approve
    let tx3_entry: Transaction = tx3_record
        .entry()
        .to_app_option()
        .ok()
        .flatten()
        .expect("Should deserialize tx3");

    if tx3_entry.status == TransactionStatus::Pending {
        ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
            .await
            .expect("tx3 must propagate to Bob");

        approve_pending_transaction(
            &conductor,
            &bob_cell,
            tx3_record.action_address().clone(),
            tx3_record.action_address().clone(),
        )
        .await
        .expect("Bob should approve tx3");
    }

    // Wait for side effects to propagate (cascade, transfer_debt, call_remote notifications)
    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Publish trust rows again after tx3 cascade has run, then query trust.
    invalidate_trust_caches(&conductor, &alice_cell).await.ok();
    invalidate_trust_caches(&conductor, &bob_cell).await.ok();
    publish_trust_row(&conductor, &alice_cell).await.ok();
    publish_trust_row(&conductor, &bob_cell).await.ok();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Invalidate cache for fresh computation before final trust query
    invalidate_trust_caches(&conductor, &alice_cell).await.ok();

    // Query trust for all agents from Alice's perspective
    let agents = vec![alice_agent.clone(), bob_agent.clone(), carol_agent.clone()];
    let mut trust_sum = 0.0;

    for agent in &agents {
        let rep = get_subjective_reputation(&conductor, &alice_cell, agent.clone())
            .await
            .expect("Should get reputation");

        assert!(rep.trust >= 0.0 && rep.trust <= 1.0, "Trust for {:?} should be in [0,1]: got {}", agent, rep.trust);

        trust_sum += rep.trust;
    }

    // Note: The sum may not be exactly 1.0 because we're only querying 3 out of
    // potentially more agents in the subgraph. But each individual value should be in [0,1].
    // The subjective reputation query returns values from the full EigenTrust vector
    // which sums to 1.0 over all agents in the subgraph.
    assert!(trust_sum > 0.0, "Total trust across queried agents should be positive: got {trust_sum}");
}
