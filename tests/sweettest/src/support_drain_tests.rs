use super::*;

/// Reproduction of the scenario reported by the user:
/// 1. B buys 10 from A.
/// 2. B buys 20 from C.
/// 3. B has 30 debt.
/// 4. A sets B as beneficiary.
/// 5. C buys 45 from A.
/// 6. A accepts.
/// 7. Verify B's debt is 0.
#[tokio::test(flavor = "multi_thread")]
async fn test_repro_drain_failure_scenario() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let a_cell = apps[0].cells()[0].clone(); // Seller A (Supporter)
    let b_cell = apps[1].cells()[0].clone(); // Buyer B (Beneficiary)
    let c_cell = apps[2].cells()[0].clone(); // Buyer C (Buyer from A)

    let a_agent: AgentPubKey = a_cell.agent_pubkey().clone();
    let b_agent: AgentPubKey = b_cell.agent_pubkey().clone();
    let c_agent: AgentPubKey = c_cell.agent_pubkey().clone();

    // ── Step 1: B buys 10 from A ──────────────
    ensure_wallet_propagation(&conductor, &b_cell, a_agent.clone())
        .await
        .expect("A wallet must be visible to B");

    let tx1 = CreateTransactionInput {
        seller: a_agent.clone().into(),
        buyer: b_agent.clone().into(),
        description: "B buys 10 from A".to_string(),
        debt: 10.0,
    };
    let tx1_record = create_transaction(&conductor, &b_cell, tx1)
        .await
        .expect("tx1 should be created");

    ensure_transaction_propagation_seller(&conductor, &a_cell, a_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx1 must propagate to A");
    approve_pending_transaction(
        &conductor,
        &a_cell,
        tx1_record.action_address().clone(),
        tx1_record.action_address().clone(),
    )
    .await
    .expect("A should approve tx1");

    wait_for_active_contract(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("B should have an active debt contract after A approves tx1");

    // ── Step 2: B buys 20 from C ──────────────
    ensure_wallet_propagation(&conductor, &b_cell, c_agent.clone())
        .await
        .expect("C wallet must be visible to B");

    let tx2 = CreateTransactionInput {
        seller: c_agent.clone().into(),
        buyer: b_agent.clone().into(),
        description: "B buys 20 from C".to_string(),
        debt: 20.0,
    };
    let tx2_record = create_transaction(&conductor, &b_cell, tx2)
        .await
        .expect("tx2 should be created");

    ensure_transaction_propagation_seller(&conductor, &c_cell, c_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx2 must propagate to C");
    approve_pending_transaction(
        &conductor,
        &c_cell,
        tx2_record.action_address().clone(),
        tx2_record.action_address().clone(),
    )
    .await
    .expect("C should approve tx2");

    wait_for_active_contract(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("B should have an active debt contract after C approves tx2");

    let b_debt_initial = get_total_debt(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("Should get B's initial debt");
    assert!(
        (b_debt_initial - 30.0).abs() < 1.0,
        "B should have ~30 debt after buying from A and C, got {b_debt_initial}"
    );

    // ── Step 4: A sets B as beneficiary ──────────────
    let breakdown = CreateSupportBreakdownInput {
        owner: a_agent.clone().into(),
        addresses: vec![a_agent.clone().into(), b_agent.clone().into()],
        coefficients: vec![0.0, 1.0],
    };
    create_support_breakdown(&conductor, &a_cell, breakdown)
        .await
        .expect("A should create support breakdown listing B");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // ── Step 5: C buys 45 from A ──────────────
    ensure_wallet_propagation(&conductor, &c_cell, a_agent.clone())
        .await
        .expect("A wallet must be visible to C");

    let tx3 = CreateTransactionInput {
        seller: a_agent.clone().into(),
        buyer: c_agent.clone().into(),
        description: "C buys 45 from A".to_string(),
        debt: 45.0,
    };
    let tx3_record = create_transaction(&conductor, &c_cell, tx3)
        .await
        .expect("tx3 should be created");

    ensure_transaction_propagation_seller(&conductor, &a_cell, a_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx3 must propagate to A");
    approve_pending_transaction(
        &conductor,
        &a_cell,
        tx3_record.action_address().clone(),
        tx3_record.action_address().clone(),
    )
    .await
    .expect("A should approve tx3 (triggering cascade to B)");

    // ── Step 7: Verify B's debt is drained ──────────────
    // With genesis vouching, B trusts A, so the drain is auto-accepted (low risk → Accepted).
    // B's debt should drain without manual approval.
    wait_for_debt_to_reach(&conductor, &b_cell, b_agent.clone(), 0.0, 1.0, 10000)
        .await
        .expect("B's debt should drain to 0 after the cascade drain (auto-accepted)");

    let b_debt_final = get_total_debt(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("Should get B's final debt");
    assert!(b_debt_final < 1.0, "B's debt should be ~0, got {b_debt_final}");

    // ── Step 8: Verify B's capacity increased (Fix Trial Capacity Release) ──
    // B is unvouched (base_capacity=0), but has now settled a debt (S > 0).
    // The "Vouch Gate" fix in capacity.rs should allow B to gain reputation-based capacity.
    let b_capacity = get_credit_capacity(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("Should get B's capacity");
    assert!(b_capacity > 0.0, "B's capacity should have increased after debt settlement, got {b_capacity}");
    println!("Verified: B's final capacity is {b_capacity}");
}
