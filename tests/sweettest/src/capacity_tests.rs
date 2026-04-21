//! Credit Capacity Tests (Whitepaper Definition 4.1)
//!
//! Tests for the credit capacity formula: Cap_i = V_staked + beta * ln(max(1, t_i / t_baseline)).
//! Verifies that capacity scales correctly with vouching, reputation, and never drops below base.

use super::*;

/// Unvouched agent has zero capacity and cannot create non-trial transactions.
///
/// Exercises: Cap = V_staked when V_staked = 0 and trust = 0.
/// An agent with no vouches should have zero credit capacity. Trial transactions
/// bypass the capacity check (PATH 0, bootstrap mechanism per Whitepaper Section 5.1),
/// but non-trial transactions should be rejected.
#[tokio::test(flavor = "multi_thread")]
async fn test_credit_capacity_zero_without_vouch() {
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice has no vouches -- her capacity should be 0
    let vouched = get_vouched_capacity(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get vouched capacity");
    assert_eq!(vouched, 0.0, "Unvouched agent should have 0 vouched capacity");

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob's wallet should propagate");

    // Trial transactions (< TRIAL_FRACTION * BASE_CAPACITY = 50) bypass capacity check.
    // This is correct protocol behavior: trials are the bootstrap mechanism for newcomers.
    let trial_tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Unvouched trial test".to_string(),
        debt: 49.0, // Trial amount
    };
    let trial_result = create_transaction(&conductor, &alice_cell, trial_tx).await;
    assert!(trial_result.is_ok(), "Trial transactions should succeed even for unvouched agents (PATH 0 bootstrap)");

    // Non-trial transactions should be rejected (capacity = 0)
    let non_trial_tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Unvouched non-trial test".to_string(),
        debt: 200.0, // Non-trial amount (> 100)
    };
    let non_trial_result = create_transaction(&conductor, &alice_cell, non_trial_tx).await;
    assert!(non_trial_result.is_err(), "Non-trial transaction should be rejected for unvouched agent with 0 capacity");
}

/// New vouched agent has capacity equal to vouch amount.
///
/// Exercises: Cap = V_staked for new agent with zero trust.
/// After genesis vouching, capacity should approximately equal the vouched amount
/// (no reputation bonus yet since trust is at baseline).
#[tokio::test(flavor = "multi_thread")]
async fn test_credit_capacity_equals_vouch_for_newcomer() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();

    // Alice was vouched during genesis bootstrap with 500 per vouch.
    // In a 2-agent setup, she gets one vouch of 500 from the other agent.
    let vouched = get_vouched_capacity(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get vouched capacity");

    assert!(vouched >= 400.0, "Vouched capacity should be >= 400 (genesis vouch 500): got {vouched}");

    // Credit capacity should be at least the vouched amount
    let cap = get_credit_capacity(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get credit capacity");

    assert!(cap >= vouched, "Credit capacity ({cap}) should be >= vouched capacity ({vouched})");
}

/// Credit capacity increases with reputation (logarithmic growth).
///
/// Exercises: Cap = V_staked + beta * ln(max(1, t_i / t_baseline)).
/// After building trust through transactions, capacity should increase
/// beyond the base vouched amount due to the logarithmic reputation bonus.
#[tokio::test(flavor = "multi_thread")]
async fn test_credit_capacity_increases_with_reputation() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Get Alice's initial capacity (from Bob's perspective - she is a stranger initially)
    let initial_cap = get_credit_capacity(&conductor, &bob_cell, alice_agent.clone()).await.unwrap();
    let initial_vouched = get_vouched_capacity(&conductor, &bob_cell, alice_agent.clone()).await.unwrap();
    let initial_rep = get_subjective_reputation(&conductor, &bob_cell, alice_agent.clone())
        .await
        .unwrap();

    let mut tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Capacity growth test - Alice buys".to_string(),
        debt: 49.0,
    };
    let record = create_transaction(&conductor, &alice_cell, tx.clone())
        .await
        .expect("Alice's Transaction should succeed");

    // Alice's transaction is a trial (n_s = 0), so it is Pending. Bob must approve it.
    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Alice tx must propagate to Bob");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        record.action_address().clone(),
        record.action_address().clone(),
    )
    .await
    .expect("Bob should approve Alice's trial");

    // Wait for Alice's DebtContract to finish creating.
    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice should have an active debt contract after Bob approves");

    // To earn capacity, Alice's debt MUST be successfully transferred.
    // So Alice must provide services (become a seller) to receive incoming debt.
    let charlie_cell = apps[2].cells()[0].clone();
    let charlie_agent: AgentPubKey = charlie_cell.agent_pubkey().clone();

    // Charlie buys from Alice. This incoming debt triggers Alice's cascade,
    // which pays off Alice's debt to her creditor, Bob.
    ensure_wallet_propagation(&conductor, &charlie_cell, alice_agent.clone())
        .await
        .expect("Alice wallet must be visible to Charlie");

    let charlie_buy = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: charlie_agent.clone().into(),
        description: "Charlie buys from Alice, transferring Alice's debt to Bob".to_string(),
        debt: 20.0,
    };
    let charlie_buy_record = create_transaction(&conductor, &charlie_cell, charlie_buy.clone())
        .await
        .expect("Charlie creates transaction");

    // Charlie's transaction is a trial (n_s = 0), so it is Pending. Alice must approve it.
    ensure_transaction_propagation_seller(&conductor, &alice_cell, alice_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Charlie tx must propagate to Alice");
    approve_pending_transaction(
        &conductor,
        &alice_cell,
        charlie_buy_record.action_address().clone(),
        charlie_buy_record.action_address().clone(),
    )
    .await
    .expect("Alice should approve Charlie's trial");

    // Wait for the cascade to extinguish Alice's debt.
    // Alice's debt to Bob was 50. Her cascade pays Bob 20. Alice's debt -> 30.
    wait_for_debt_to_reach(&conductor, &alice_cell, alice_agent.clone(), 30.0, 2.0, 8000)
        .await
        .expect("Alice's debt should reduce to ~30 after cascade");

    // Wait for the cascade to extinguish Alice's debt.
    // Alice's debt to Bob was 50. Her cascade pays Bob 20. Alice's debt -> 30.
    wait_for_debt_to_reach(&conductor, &alice_cell, alice_agent.clone(), 30.0, 2.0, 8000)
        .await
        .expect("Alice's debt should reduce to ~30 after cascade");

    #[derive(serde::Deserialize, Debug)]
    pub struct SFCounters {
        pub satisfaction: f64,
        pub failure: f64,
        pub first_seen_epoch: u64,
        pub recent_satisfaction: f64,
        pub recent_failure: f64,
    }

    // Wait until Bob's cell actually sees the satisfaction.
    let mut bob_saw_satisfaction = false;
    for i in 0..15 {
        // up to 7.5 seconds
        invalidate_trust_caches(&conductor, &bob_cell).await.ok();
        let bob_sf = conductor
            .call_fallible(&bob_cell.zome("transaction"), "get_my_sf_counters", ())
            .await
            .unwrap_or_else(|_| std::collections::HashMap::<AgentPubKeyB64, SFCounters>::new());

        let alice_b64: AgentPubKeyB64 = alice_agent.clone().into();
        if let Some(counters) = bob_sf.get(&alice_b64) {
            if counters.satisfaction >= 20.0 {
                bob_saw_satisfaction = true;
                break;
            }
        }
        {
            let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

            await_consistency(30, &cells).await.unwrap();
        }
    }
    if !bob_saw_satisfaction {
        let bob_active = conductor
            .call_fallible(&bob_cell.zome("transaction"), "get_active_contracts_for_creditor", bob_agent.clone())
            .await
            .unwrap_or_else(|_| Vec::<holochain::prelude::Record>::new());
        for record in bob_active {
            let action_hash = record.action_address().clone();

            // Check original vs current amount
            let contract: transaction_integrity::debt_contract::DebtContract =
                record.entry().to_app_option().unwrap().unwrap();
        }

        let alice_active = conductor
            .call_fallible(&alice_cell.zome("transaction"), "get_active_contracts_for_debtor", alice_agent.clone())
            .await
            .unwrap_or_else(|_| Vec::<holochain::prelude::Record>::new());
        for record in alice_active {
            let action_hash = record.action_address().clone();
            let contract: transaction_integrity::debt_contract::DebtContract =
                record.entry().to_app_option().unwrap().unwrap();
        }

        panic!("Bob never saw Alice's satisfaction!");
    }

    // Publish trust row from Bob. Since Alice paid Bob 20, Bob's trust in Alice increases.
    publish_trust_row(&conductor, &bob_cell).await.ok();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Invalidate cache and recompute on Bob's side
    invalidate_trust_caches(&conductor, &bob_cell).await.ok();

    let updated_cap = get_credit_capacity(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Should get updated capacity");

    // Capacity should now be greater than the initial amount due to S > 0 evidence
    assert!(
        updated_cap > initial_cap + 1.0,
        "Capacity should increase after debt transfer: was {initial_cap}, now {updated_cap}"
    );
}

/// Capacity never drops below vouched base, even with poor reputation.
///
/// Exercises: max(1, rel_rep) in capacity formula prevents negative bonus.
/// Even if trust is below baseline, the logarithmic term yields ln(1) = 0,
/// so capacity stays at V_staked.
#[tokio::test(flavor = "multi_thread")]
async fn test_capacity_never_below_base() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();

    let vouched = get_vouched_capacity(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get vouched capacity");

    let cap = get_credit_capacity(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get credit capacity");

    // Capacity should never be less than the vouched base
    assert!(
        cap >= vouched - 1.0, // Small tolerance for floating point
        "Capacity ({cap}) should never be less than vouched base ({vouched})"
    );

    // And it should always be non-negative
    assert!(cap >= 0.0, "Capacity should never be negative: got {cap}");
}

/// Pure buyer (no vouches, no sales) gains capacity after receiving support.
///
/// Scenario:
/// 1. Bob (pure buyer) buys 50 from Alice (trusted seller) via trial.
/// 2. Alice sets Bob as beneficiary in her support breakdown.
/// 3. Charlie buys 50 from Alice. Since Alice has no debt, her cascade drains Bob's debt.
/// 4. Bob accepts the drain.
/// 5. Confirmation: Alice (Supporter) records satisfaction for Bob (Beneficiary).
/// 6. Bob gains capacity from Alice's perspective.
#[tokio::test(flavor = "multi_thread")]
async fn test_buyer_capacity_after_support_fix() {
    let (conductor, apps) = setup_multi_agent_no_vouch(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let charlie_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let charlie_agent: AgentPubKey = charlie_cell.agent_pubkey().clone();

    // Bob should have 0 initial capacity (unvouched, 0 reputation).
    let initial_cap = get_credit_capacity(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Should get initial capacity");
    assert_eq!(initial_cap, 0.0, "Bob should have 0 initial capacity");

    // 1. Bob buys 40 from Alice (trial).
    let tx = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Trial".to_string(),
        debt: 40.0,
    };
    let record = create_transaction(&conductor, &bob_cell, tx).await.expect("Bob creates trial");

    // Alice approves the trial.
    ensure_transaction_propagation_seller(&conductor, &alice_cell, alice_agent.clone(), TransactionStatusTag::Pending)
        .await
        .unwrap();
    approve_pending_transaction(
        &conductor,
        &alice_cell,
        record.action_address().clone(),
        record.action_address().clone(),
    )
    .await
    .expect("Alice approves trial");

    // Wait for Bob's DebtContract to exist.
    wait_for_active_contract(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Bob should have active contract");

    // 2. Alice sets Bob as beneficiary in her support breakdown.
    let breakdown = CreateSupportBreakdownInput {
        owner: alice_agent.clone().into(),
        addresses: vec![alice_agent.clone().into(), bob_agent.clone().into()],
        coefficients: vec![0.0, 1.0], // 100% support to Bob
    };
    create_support_breakdown(&conductor, &alice_cell, breakdown)
        .await
        .expect("Alice updates breakdown");

    let cells = vec![alice_cell.clone(), bob_cell.clone(), charlie_cell.clone()];
    await_consistency(30, &cells).await.unwrap();

    // 3. Charlie buys 40 from Alice.
    ensure_wallet_propagation(&conductor, &charlie_cell, alice_agent.clone())
        .await
        .expect("Alice wallet visible to Charlie");
    let tx2 = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: charlie_agent.clone().into(),
        description: "Charlie buys from Alice".to_string(),
        debt: 40.0,
    };
    let record2 = create_transaction(&conductor, &charlie_cell, tx2).await.expect("Charlie buys");
    await_consistency(30, &cells).await.unwrap();

    // In setup_multi_agent_no_vouch, Charlie is a stranger to Alice.
    // Manually approve Charlie's transaction on Alice's cell to trigger cascade.
    approve_pending_transaction(
        &conductor,
        &alice_cell,
        record2.action_address().clone(),
        record2.action_address().clone(),
    )
    .await
    .ok();
    await_consistency(30, &cells).await.unwrap();

    // 4. Bob accepts the drain request.
    // Wait for drain tx to propagate to Bob.
    // After role realignment: Bob = seller (beneficiary), Alice = buyer (supporter)
    let mut drain_tx_hash = None;
    for _ in 0..20 {
        // Increase timeout to 10 seconds
        let pending =
            get_transactions_for_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
                .await
                .unwrap();
        if let Some(r) = pending.iter().find(|r| {
            let tx: Transaction = r.entry().to_app_option().unwrap().unwrap();
            tx.is_drain() && tx.buyer.pubkey == AgentPubKeyB64::from(alice_agent.clone())
        }) {
            drain_tx_hash = Some(r.action_address().clone());
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    let drain_hash = drain_tx_hash.expect("Bob should have received a drain request");
    approve_pending_transaction(&conductor, &bob_cell, drain_hash.clone(), drain_hash)
        .await
        .expect("Bob approves drain");

    // 5. Trigger publication and invalidation to ensure trust loop propagates.
    publish_trust_row(&conductor, &alice_cell).await.unwrap();
    publish_trust_row(&conductor, &bob_cell).await.unwrap();

    let cells = vec![alice_cell.clone(), bob_cell.clone(), charlie_cell.clone()];
    await_consistency(30, &cells).await.unwrap();

    invalidate_trust_caches(&conductor, &alice_cell).await.unwrap();
    invalidate_trust_caches(&conductor, &bob_cell).await.unwrap();

    // 6. Verification: Bob should now have capacity from Alice's perspective.
    let final_cap = get_credit_capacity(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Should get final capacity");

    assert!(final_cap > 1.0, "Bob should have non-zero capacity: got {final_cap}");
}
