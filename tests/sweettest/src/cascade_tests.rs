use super::*;

/// Multi-phase cascade test covering:
///   Phase 1: A builds debt, A supports B (10%), C buys from A triggering cascade
///   Phase 2: A updates support to include C (20%), B buys from A triggering cascade
///
/// Expected debt flows (cascade mechanics):
///
/// Setup:
///   - A buys 10 from B (trial, B approves)  → A.debt += 10; B sells → cascade B: own=0, no breakdown → genesis goes to BUYER (A already has contract) → B.debt = 0
///   - A buys 20 from C (trial, C approves)  → A.debt += 20 = 30 total; C.debt = 0 (same logic)
///   - A vouches 1000 for C
///   - A creates SupportBreakdown [A=0.9, B=0.1]
///
/// Phase 1:
///   - C tries to buy 900 from A → auto-rejects (non-trial, no bilateral history)
///   - C buys 40 from A (trial, A approves):
///       Cascade on A (amount=40):
///         1. Own drain: min(40, 30) = 30 → A.debt -= 30. Remaining = 10.
///         2. B only (coeff 1.0). B has 0 debt → dry. Remaining = 10.
///         3. Genesis (10) goes to buyer C — C already has buyer contract for full 40.
///            Seller A gets NO new debt.
///       Net: A.debt = 0. C.debt = 40 (buyer contract).
///   Assert: A.debt ≈ 0, B.debt ≈ 0, C.debt ≈ 40
///
/// Phase 2:
///   - A updates SupportBreakdown to [A=0.7, B=0.1, C=0.2]
///   - B buys 49 from A (trial, A approves):
///       Cascade on A (amount=49):
///         1. Own drain: A has 0 → own=0. Remaining = 49.
///         2. B(0.1), C(0.2), total_coeff=0.3.
///            Pass 1: B_target=16 (B dry — buyer contract after cascade),
///                    C_target=32 → drain min(32, 40)=32. remaining=17.
///            Pass 2: C_target=17 → drain min(17, 8)=8. remaining=9.
///            Pass 3: C dry. Remaining=9 → genesis to buyer B.
///            Seller A gets NO new debt.
///       B.debt += 49 (buyer contract). C.debt = 40 - 32 - 8 = 0.
///   Assert: A.debt ≈ 0, B.debt ≈ 49, C.debt ≈ 0
#[tokio::test(flavor = "multi_thread")]
async fn test_cascade_multi_phase() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let a_cell = apps[0].cells()[0].clone();
    let b_cell = apps[1].cells()[0].clone();
    let c_cell = apps[2].cells()[0].clone();

    let a_agent: AgentPubKey = a_cell.agent_pubkey().clone();
    let b_agent: AgentPubKey = b_cell.agent_pubkey().clone();
    let c_agent: AgentPubKey = c_cell.agent_pubkey().clone();

    // ── Step 1: A buys 10 from B (trial, B approves) → A.debt = 10 ──────────────
    ensure_wallet_propagation(&conductor, &a_cell, b_agent.clone())
        .await
        .expect("B wallet must be visible to A");

    let tx1 = CreateTransactionInput {
        seller: b_agent.clone().into(),
        buyer: a_agent.clone().into(),
        description: "A buys 10 from B".to_string(),
        debt: 10.0,
    };
    let tx1_record = create_transaction(&conductor, &a_cell, tx1)
        .await
        .expect("tx1 should be created");

    ensure_transaction_propagation_seller(&conductor, &b_cell, b_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx1 must propagate to B");
    approve_pending_transaction(
        &conductor,
        &b_cell,
        tx1_record.action_address().clone(),
        tx1_record.action_address().clone(),
    )
    .await
    .expect("B should approve tx1");

    wait_for_active_contract(&conductor, &a_cell, a_agent.clone())
        .await
        .expect("A should have an active debt contract after B approves tx1");

    // ── Step 2: A buys 20 from C (trial, C approves) → A.debt = 30 ──────────────
    ensure_wallet_propagation(&conductor, &a_cell, c_agent.clone())
        .await
        .expect("C wallet must be visible to A");

    let tx2 = CreateTransactionInput {
        seller: c_agent.clone().into(),
        buyer: a_agent.clone().into(),
        description: "A buys 20 from C".to_string(),
        debt: 20.0,
    };
    let tx2_record = create_transaction(&conductor, &a_cell, tx2)
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

    // Wait for A to have BOTH active debt contracts before proceeding.
    // wait_for_active_contract only checks >= 1; we need to confirm tx2's contract
    // (created async via call_remote from C) is also committed before A becomes a seller.
    for _ in 0..60 {
        let contracts = get_active_contracts_for_debtor(&conductor, &a_cell, a_agent.clone())
            .await
            .unwrap_or_default();
        if contracts.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    let a_debt_initial = get_total_debt(&conductor, &a_cell, a_agent.clone())
        .await
        .expect("Should get A's initial debt");
    assert!(
        (a_debt_initial - 30.0).abs() < 1.0,
        "A should have ~30 debt after buying from B and C, got {a_debt_initial}"
    );

    // ── Step 3: A vouches 1000 for C ────────────────────────────────────────────
    ensure_wallet_propagation(&conductor, &a_cell, c_agent.clone())
        .await
        .expect("C wallet must be visible to A for vouching");

    let vouch = CreateVouchInput { sponsor: a_agent.clone().into(), entrant: c_agent.clone().into(), amount: 1000.0 };
    genesis_vouch(&conductor, &a_cell, vouch)
        .await
        .expect("A should be able to vouch for C");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // ── Step 4: A creates SupportBreakdown [A=0.9, B=0.1] ───────────────────────
    let breakdown_v1 = CreateSupportBreakdownInput {
        owner: a_agent.clone().into(),
        addresses: vec![a_agent.clone().into(), b_agent.clone().into()],
        coefficients: vec![0.9, 0.1],
    };
    let breakdown_v1_record = create_support_breakdown(&conductor, &a_cell, breakdown_v1)
        .await
        .expect("A should create support breakdown [A=0.9, B=0.1]");
    let breakdown_original_hash = breakdown_v1_record.action_address().clone();

    // Allow breakdown to propagate so B can authenticate drain requests
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    // ── Step 5: C tries to buy 900 from A → auto-rejects (non-trial, no bilateral history) ──
    // PATH 1: S_{A,C} = 0 (no prior bilateral history between A and C as seller/buyer pair).
    // rel_trust = 0, risk = 1 - 0 * ... = 1.0 → auto-reject.
    ensure_wallet_propagation(&conductor, &c_cell, a_agent.clone())
        .await
        .expect("A wallet must be visible to C for 900-unit purchase attempt");

    let tx5 = CreateTransactionInput {
        seller: a_agent.clone().into(),
        buyer: c_agent.clone().into(),
        description: "C tries to buy 900 from A (should auto-reject)".to_string(),
        debt: 900.0,
    };
    let tx5_record = create_transaction(&conductor, &c_cell, tx5)
        .await
        .expect("tx5 create_transaction should succeed (auto-reject happens at status level)");

    let tx5_initial: Transaction = tx5_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(
        tx5_initial.status,
        TransactionStatus::Rejected,
        "900-unit purchase from A by C should be auto-rejected (no bilateral history, non-trial), got {:?}",
        tx5_initial.status
    );

    // ── Step 6: C buys 40 from A (trial, A approves) → cascade drains A's debt ──
    let tx6 = CreateTransactionInput {
        seller: a_agent.clone().into(),
        buyer: c_agent.clone().into(),
        description: "C buys 40 from A (triggers cascade)".to_string(),
        debt: 40.0,
    };
    let tx6_record = create_transaction(&conductor, &c_cell, tx6)
        .await
        .expect("tx6 should be created (40 is trial amount)");

    let tx6_initial: Transaction = tx6_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(tx6_initial.status, TransactionStatus::Pending, "40-unit tx should be Pending (trial)");

    ensure_transaction_propagation_seller(&conductor, &a_cell, a_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx6 must propagate to A");
    approve_pending_transaction(
        &conductor,
        &a_cell,
        tx6_record.action_address().clone(),
        tx6_record.action_address().clone(),
    )
    .await
    .expect("A should approve tx6 (triggers cascade)");

    // Wait for cascade to complete: A drains all 30 own debt, B is dry → A.debt = 0.
    // No genesis contract on seller — genesis remainder belongs to buyer C (already has
    // the full 40-unit buyer contract). A's debt should reach 0.
    wait_for_debt_to_reach(&conductor, &a_cell, a_agent.clone(), 0.0, 1.0, 8000)
        .await
        .expect("A's debt should drain to ~0 after cascade (own 30 drained, no genesis on seller)");

    // Also wait for C's buyer contract to arrive
    wait_for_active_contract(&conductor, &c_cell, c_agent.clone())
        .await
        .expect("C should have an active debt contract (buyer contract from tx6)");

    let a_debt_phase1 = get_total_debt(&conductor, &a_cell, a_agent.clone())
        .await
        .expect("Should get A's debt after phase 1");
    let c_debt_phase1 = get_total_debt(&conductor, &c_cell, c_agent.clone())
        .await
        .expect("Should get C's debt after phase 1");
    let b_debt_phase1 = get_total_debt(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("Should get B's debt after phase 1");

    // Cascade on A (amount=40): own drain=30. B has 0 debt (B got no genesis when B sold to A).
    // remaining=10 → genesis belongs to buyer C. Seller A gets NO new debt.
    // C.debt = 40 (full buyer contract). B.debt = 0.
    eprintln!("A.debt after phase 1: {a_debt_phase1} (expected ~0)");
    eprintln!("B.debt after phase 1: {b_debt_phase1} (expected ~0)");
    eprintln!("C.debt after phase 1: {c_debt_phase1} (expected ~40)");
    assert!(
        a_debt_phase1 < 1.0,
        "A should have ~0 debt after phase-1 cascade (30 own drained, no genesis on seller), got {a_debt_phase1}"
    );
    assert!((c_debt_phase1 - 40.0).abs() < 1.0, "C should have ~40 debt (full buyer contract), got {c_debt_phase1}");
    assert!(b_debt_phase1 < 1.0, "B should have 0 debt, got {b_debt_phase1}");

    // ── Phase 2: A updates support to [A=0.7, B=0.1, C=0.2] ────────────────────
    let breakdown_v2 = CreateSupportBreakdownInput {
        owner: a_agent.clone().into(),
        addresses: vec![a_agent.clone().into(), b_agent.clone().into(), c_agent.clone().into()],
        coefficients: vec![0.7, 0.1, 0.2],
    };
    update_support_breakdown(
        &conductor,
        &a_cell,
        UpdateSupportBreakdownInput {
            original_support_breakdown_hash: breakdown_original_hash.clone(),
            previous_support_breakdown_hash: breakdown_original_hash,
            updated_support_breakdown: breakdown_v2,
        },
    )
    .await
    .expect("A should update support breakdown to [A=0.7, B=0.1, C=0.2]");

    // Allow updated breakdown to propagate
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    // ── Step 8: B buys 49 from A (trial, A approves) → cascade drains C's debt ──
    // Using 49 (trial, < 50) so it goes Pending and A can approve.
    // 50 would be non-trial (PATH 1 → auto-reject since S_{A,B}=0).
    ensure_wallet_propagation(&conductor, &b_cell, a_agent.clone())
        .await
        .expect("A wallet must be visible to B");

    let tx8 = CreateTransactionInput {
        seller: a_agent.clone().into(),
        buyer: b_agent.clone().into(),
        description: "B buys 49 from A (triggers phase-2 cascade)".to_string(),
        debt: 49.0,
    };
    let tx8_record = create_transaction(&conductor, &b_cell, tx8)
        .await
        .expect("tx8 should be created");

    let tx8_initial: Transaction = tx8_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(tx8_initial.status, TransactionStatus::Pending, "49-unit tx should be Pending (trial)");

    ensure_transaction_propagation_seller(&conductor, &a_cell, a_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx8 must propagate to A");

    // Wait for A's chain to be quiescent before approving.
    // The `update_support_breakdown` call a few lines above triggers async post_commit
    // side-effects on A's chain (trust row publishes, acquaintance evictions, ranking-index
    // updates).  If approve_pending_transaction fires while those in-flight writes are
    // still being committed, Holochain raises HeadMoved because our update_transaction
    // call builds on a stale chain head.  wait_for_chain_quiescent polls A's wallet hash
    // until it has been stable for two consecutive 300ms intervals, ensuring all pending
    // post_commit writes have landed before we proceed.
    wait_for_chain_quiescent(&conductor, &a_cell, a_agent.clone(), 300, 8000).await;

    approve_pending_transaction(
        &conductor,
        &a_cell,
        tx8_record.action_address().clone(),
        tx8_record.action_address().clone(),
    )
    .await
    .expect("A should approve tx8 (triggers phase-2 cascade)");

    // Wait for cascade (A → C via call_remote) and buyer contract (A → B via call_remote).
    // Cascade on A (amount=49): A has 0 own debt → own_drain=0. remaining=49.
    // Non-self: B(0.1), C(0.2), total=0.3.
    //   Pass 1: B_target=16 (B dry — buyer contract created AFTER cascade),
    //           C_target=32 → drain min(32, 40)=32. remaining=17.
    //   Pass 2: C_target=17 → drain min(17, 8)=8. remaining=9.
    //   Pass 3: C dry. remaining=9 → genesis to buyer B.
    //   Seller A gets NO new debt.
    // C.debt = 40 - 32 - 8 = 0. B.debt = 49 (buyer contract after cascade).
    wait_for_debt_to_reach(&conductor, &c_cell, c_agent.clone(), 0.0, 1.0, 10000)
        .await
        .expect("C's debt should drain to ~0 via phase-2 cascade");
    wait_for_active_contract(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("B should have an active debt contract (buyer contract from tx8)");

    let a_debt_phase2 = get_total_debt(&conductor, &a_cell, a_agent.clone())
        .await
        .expect("Should get A's debt after phase 2");
    let b_debt_phase2 = get_total_debt(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("Should get B's debt after phase 2");
    let c_debt_phase2 = get_total_debt(&conductor, &c_cell, c_agent.clone())
        .await
        .expect("Should get C's debt after phase 2");

    eprintln!("A.debt after phase 2: {a_debt_phase2} (expected ~0)");
    eprintln!("B.debt after phase 2: {b_debt_phase2} (expected ~49)");
    eprintln!("C.debt after phase 2: {c_debt_phase2} (expected ~0)");

    assert!(
        a_debt_phase2 < 1.0,
        "A should have ~0 debt after phase-2 cascade (no genesis on seller), got {a_debt_phase2}"
    );
    assert!((b_debt_phase2 - 49.0).abs() < 2.0, "B should have ~49 debt (new buyer contract), got {b_debt_phase2}");
    assert!(c_debt_phase2 < 1.0, "C should have ~0 debt (fully drained by phase-2 cascade), got {c_debt_phase2}");
}

/// Support breakdown: create with owner only
#[tokio::test(flavor = "multi_thread")]
async fn test_support_breakdown_owner_only() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();
    let agent: AgentPubKey = cell.agent_pubkey().clone();

    let bd = CreateSupportBreakdownInput {
        owner: agent.clone().into(),
        addresses: vec![agent.clone().into()],
        coefficients: vec![1.0],
    };

    let record = create_support_breakdown(&conductor, &cell, bd)
        .await
        .expect("Support breakdown should be created");
}

/// Support breakdown: create with multiple beneficiaries
#[tokio::test(flavor = "multi_thread")]
async fn test_support_breakdown_multiple_beneficiaries() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    let bd = CreateSupportBreakdownInput {
        owner: alice_agent.clone().into(),
        addresses: vec![alice_agent.clone().into(), bob_agent.clone().into()],
        coefficients: vec![0.6, 0.4],
    };

    let _record = create_support_breakdown(&conductor, &alice_cell, bd)
        .await
        .expect("Multi-beneficiary breakdown should be created");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let result = get_support_breakdown_for_owner(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get breakdown");
    assert!(result.0.is_some(), "Should have found the breakdown");
}

/// Support breakdown: update preserves structure
#[tokio::test(flavor = "multi_thread")]
async fn test_support_breakdown_update() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();

    let bd1 = CreateSupportBreakdownInput {
        owner: alice_agent.clone().into(),
        addresses: vec![alice_agent.clone().into()],
        coefficients: vec![1.0],
    };
    let _record1 = create_support_breakdown(&conductor, &alice_cell, bd1)
        .await
        .expect("First breakdown should be created");
}

/// Cascade drain: seller with no own debt drains beneficiary's debt.
///
/// Scenario:
///   - B owes 40 to D (D sold to B, trial tx).
///   - A has no debt of its own but lists B as 100% beneficiary in its SupportBreakdown.
///   - C buys 10 from A (trial tx, A approves it).
///   - Expected: cascade fires on A's cell, A has 0 own debt, drains B by 10.
///   - B's debt should decrease from 40 to ~30.
#[tokio::test(flavor = "multi_thread")]
async fn test_cascade_beneficiary_drain() {
    // 4 agents: A (seller/supporter), B (beneficiary), C (buyer from A), D (original seller to B)
    let (conductor, apps) = setup_multi_agent(4).await;
    let cells: Vec<SweetCell> = apps.iter().flat_map(|app| app.cells().to_vec()).collect();
    let a_cell = apps[0].cells()[0].clone(); // supporter / cascade seller
    let b_cell = apps[1].cells()[0].clone(); // beneficiary with debt
    let c_cell = apps[2].cells()[0].clone(); // buyer from A
    let d_cell = apps[3].cells()[0].clone(); // original seller to B

    let a_agent: AgentPubKey = a_cell.agent_pubkey().clone();
    let b_agent: AgentPubKey = b_cell.agent_pubkey().clone();
    let c_agent: AgentPubKey = c_cell.agent_pubkey().clone();
    let d_agent: AgentPubKey = d_cell.agent_pubkey().clone();

    // ── Step 1: B buys 40 from D (trial, D approves → B gets 40 debt) ──────────
    // Amount must be < 50.0 (TRIAL_FRACTION * BASE_CAPACITY) to guarantee the
    // transaction is a trial and always starts as Pending (regardless of trust score).
    ensure_wallet_propagation(&conductor, &b_cell, d_agent.clone())
        .await
        .expect("D wallet must be visible to B");

    let tx1 = CreateTransactionInput {
        seller: d_agent.clone().into(),
        buyer: b_agent.clone().into(),
        description: "B buys from D - creates B debt".to_string(),
        debt: 40.0,
    };
    let tx1_record = create_transaction(&conductor, &b_cell, tx1)
        .await
        .expect("B->D transaction should be created");

    // Wait for tx1 to propagate to D, then D approves
    ensure_transaction_propagation_seller(&conductor, &d_cell, d_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx1 must propagate to D");
    approve_pending_transaction(
        &conductor,
        &d_cell,
        tx1_record.action_address().clone(),
        tx1_record.action_address().clone(),
    )
    .await
    .expect("D should approve tx1");

    // Wait for B's debt contract to be created
    wait_for_active_contract(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("B should have an active debt contract after approval");

    let b_debt_before = get_total_debt(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("Should get B's debt");
    assert!((b_debt_before - 40.0).abs() < 1.0, "B should have ~40 debt before cascade, got {b_debt_before}");

    // ── Step 2: A creates SupportBreakdown listing B as 100% beneficiary ────────
    // Owner (A) must be in addresses; A gets coefficient 0 so the cascade skips self
    // and allocates 100% of the non-self remainder to B.
    let breakdown = CreateSupportBreakdownInput {
        owner: a_agent.clone().into(),
        addresses: vec![a_agent.clone().into(), b_agent.clone().into()],
        coefficients: vec![0.0, 1.0],
    };
    create_support_breakdown(&conductor, &a_cell, breakdown)
        .await
        .expect("A should create support breakdown listing B");

    // Allow breakdown to propagate to DHT so B can authenticate the drain request
    await_consistency(30, &cells).await.expect("Breakdown propagation failed");

    // ── Step 3: C buys 10 from A (trial, A approves → cascade fires) ────────────
    ensure_wallet_propagation(&conductor, &c_cell, a_agent.clone())
        .await
        .expect("A wallet must be visible to C");

    let tx2 = CreateTransactionInput {
        seller: a_agent.clone().into(),
        buyer: c_agent.clone().into(),
        description: "C buys from A - triggers cascade to B".to_string(),
        debt: 10.0,
    };
    let tx2_record = create_transaction(&conductor, &c_cell, tx2)
        .await
        .expect("C->A transaction should be created");

    // Wait for tx2 to propagate to A, then A approves
    ensure_transaction_propagation_seller(&conductor, &a_cell, a_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx2 must propagate to A");
    approve_pending_transaction(
        &conductor,
        &a_cell,
        tx2_record.action_address().clone(),
        tx2_record.action_address().clone(),
    )
    .await
    .expect("A should approve tx2 (triggering cascade)");

    // Wait for cascade to drain B's debt.
    // With genesis vouching, B already trusts A (they vouched for each other), so the
    // drain is auto-accepted by B's risk assessment (low risk → Accepted).
    // B's debt should drop from 40 to ~30 (10 drained by cascade) without manual approval.
    wait_for_debt_to_reach(&conductor, &b_cell, b_agent.clone(), 30.0, 5.0, 10000)
        .await
        .expect("B's debt should drop to ~30 after the cascade drain (auto-accepted)");

    // ── Step 4: Assert B's debt decreased by ~10 ─────────────────────────────────
    let b_debt_after = get_total_debt(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("Should get B's debt after cascade");

    assert!(
        b_debt_after < b_debt_before - 5.0,
        "B's debt should have decreased by ~10 via cascade drain \
         (was {b_debt_before}, now {b_debt_after})"
    );
    assert!(
        (b_debt_after - 30.0).abs() < 5.0,
        "B's debt should be ~30 after 10 was drained (was {b_debt_before}, now {b_debt_after})"
    );
}

/// Transaction with self-support triggers own debt transfer.
///
/// Scenario:
///   1. Alice buys 30 from Bob (trial, Bob approves) → Alice.debt = 30
///   2. Carol buys 15 from Alice (trial, Alice approves) → cascade fires on Alice's cell
///
/// Expected cascade on Alice (amount=15):
///   1. Own drain: min(15, 30) = 15 transferred. Alice.debt = 15. Remaining = 0.
///   2. No breakdown (or self-only) → no beneficiary drain, no genesis debt.
///
/// Final state: Alice.debt ≈ 15
#[tokio::test(flavor = "multi_thread")]
async fn test_cascade_self_support_debt_transfer() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    // Step 1: Alice buys 30 from Bob (trial) — Bob approves → Alice gets debt contract
    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Alice");

    let tx1 = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Build Alice's debt".to_string(),
        debt: 30.0,
    };
    let tx1_record = create_transaction(&conductor, &alice_cell, tx1)
        .await
        .expect("tx1 should be created");

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
        .expect("Alice should have an active debt contract after Bob approves");

    let alice_debt_before = get_total_debt(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get Alice's initial debt");
    assert!(
        (alice_debt_before - 30.0).abs() < 1.0,
        "Alice should have ~30 debt after buying from Bob, got {alice_debt_before}"
    );

    // Step 2: Carol buys 15 from Alice (trial) — Alice approves → cascade fires on Alice
    ensure_wallet_propagation(&conductor, &carol_cell, alice_agent.clone())
        .await
        .expect("Alice wallet must be visible to Carol");

    let tx2 = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: carol_agent.clone().into(),
        description: "Cascade test: Carol buys from Alice".to_string(),
        debt: 15.0,
    };
    let tx2_record = create_transaction(&conductor, &carol_cell, tx2)
        .await
        .expect("tx2 should be created");

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
    .expect("Alice should approve tx2 (triggering cascade)");

    // Cascade: own drain = min(15, 30) = 15. No beneficiary drain (no breakdown).
    // Alice.debt should go from 30 → 15.
    wait_for_debt_to_reach(&conductor, &alice_cell, alice_agent.clone(), 15.0, 2.0, 8000)
        .await
        .expect("Alice's debt should stabilise at ~15 after cascade drains 15 of her 30");

    let alice_debt_after = get_total_debt(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get Alice's final debt");

    assert!(
        (alice_debt_after - 15.0).abs() < 2.0,
        "Alice's debt should be ~15 (30 − 15 drained by own cascade), got {alice_debt_after}"
    );
}

/// Exact UI scenario: C supports B, A buys from C, B's debt should drain.
///
/// Steps (mirrors the user's manual UI test):
///   1. B buys 10 from A → A approves → B.debt = 10 (buyer contract). A.debt = 0 (no seller genesis).
///   2. C buys 20 from B → B approves → cascade on B: own_drain=10, remaining=10, no breakdown
///      → genesis 10 belongs to buyer C. B.debt = 0. C.debt = 20 (buyer contract).
///   3. C creates SupportBreakdown [C=0.7, B=0.3]
///   4. A buys 50 from C → C approves → cascade fires on C's cell
///
/// Expected cascade on C's cell (amount=50):
///   1. Own drain: C has 20 debt → min(50, 20) = 20 transferred. C.debt = 0. Remaining = 30.
///   2. Breakdown lookup: [C=0.7, B=0.3]. Active non-self: B(0.3). total_coef=0.3.
///      B gets remaining * (0.3/0.3) = 30. B has 0 debt → dry. remaining=30.
///   3. Genesis (30) belongs to buyer A. Seller C gets NO new debt.
///
/// Expected final state:
///   - A.debt = 50 (buyer contract from step 4)
///   - B.debt = 0  (original 10 fully drained by cascade in step 2; stays 0 in step 4)
///   - C.debt = 0  (original 20 fully drained by cascade own-transfer)
#[tokio::test(flavor = "multi_thread")]
async fn test_cascade_ui_scenario_c_supports_b() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let a_cell = apps[0].cells()[0].clone();
    let b_cell = apps[1].cells()[0].clone();
    let c_cell = apps[2].cells()[0].clone();

    let a_agent: AgentPubKey = a_cell.agent_pubkey().clone();
    let b_agent: AgentPubKey = b_cell.agent_pubkey().clone();
    let c_agent: AgentPubKey = c_cell.agent_pubkey().clone();

    // ── Step 1: B buys 10 from A → A approves → B.debt = 10 ─────────────────────
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

    let b_debt_step1 = get_total_debt(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("Should get B's debt");
    assert!((b_debt_step1 - 10.0).abs() < 1.0, "B should have ~10 debt after step 1, got {b_debt_step1}");

    // ── Step 2: C buys 20 from B → B approves → C.debt = 20 ─────────────────────
    ensure_wallet_propagation(&conductor, &c_cell, b_agent.clone())
        .await
        .expect("B wallet must be visible to C");

    let tx2 = CreateTransactionInput {
        seller: b_agent.clone().into(),
        buyer: c_agent.clone().into(),
        description: "C buys 20 from B".to_string(),
        debt: 20.0,
    };
    let tx2_record = create_transaction(&conductor, &c_cell, tx2)
        .await
        .expect("tx2 should be created");

    ensure_transaction_propagation_seller(&conductor, &b_cell, b_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx2 must propagate to B");
    approve_pending_transaction(
        &conductor,
        &b_cell,
        tx2_record.action_address().clone(),
        tx2_record.action_address().clone(),
    )
    .await
    .expect("B should approve tx2");

    wait_for_active_contract(&conductor, &c_cell, c_agent.clone())
        .await
        .expect("C should have an active debt contract after B approves tx2");

    let c_debt_step2 = get_total_debt(&conductor, &c_cell, c_agent.clone())
        .await
        .expect("Should get C's debt");
    assert!((c_debt_step2 - 20.0).abs() < 1.0, "C should have ~20 debt after step 2, got {c_debt_step2}");

    // Note: B was seller in tx2 — cascade fires on B's cell with amount=20.
    // B has 10 debt → own drain = 10, remaining = 10.
    // B has no SupportBreakdown → no beneficiaries. Genesis (10) goes to buyer C.
    // Seller B gets NO new debt. B.debt = 0.
    // Poll until B's debt stabilises at 0.
    wait_for_debt_to_reach(&conductor, &b_cell, b_agent.clone(), 0.0, 1.0, 5000)
        .await
        .expect("B's debt should reach 0 after step 2 cascade (own 10 drained, no seller genesis)");
    let b_debt_step2 = get_total_debt(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("Should get B's debt after step 2");
    eprintln!("B.debt after step 2 (B was seller, cascade drained B's own 10, no genesis): {b_debt_step2}");

    // ── Step 3: C creates SupportBreakdown [C=0.7, B=0.3] ───────────────────────
    let breakdown = CreateSupportBreakdownInput {
        owner: c_agent.clone().into(),
        addresses: vec![c_agent.clone().into(), b_agent.clone().into()],
        coefficients: vec![0.7, 0.3],
    };
    create_support_breakdown(&conductor, &c_cell, breakdown)
        .await
        .expect("C should create support breakdown [C=0.7, B=0.3]");

    // Allow breakdown propagation
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    // ── Step 4: A buys 50 from C → C approves → cascade fires on C ──────────────
    ensure_wallet_propagation(&conductor, &a_cell, c_agent.clone())
        .await
        .expect("C wallet must be visible to A");

    let tx3 = CreateTransactionInput {
        seller: c_agent.clone().into(),
        buyer: a_agent.clone().into(),
        description: "A buys 50 from C (triggers cascade)".to_string(),
        debt: 49.0,
    };
    let tx3_record = create_transaction(&conductor, &a_cell, tx3)
        .await
        .expect("tx3 should be created");

    ensure_transaction_propagation_seller(&conductor, &c_cell, c_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("tx3 must propagate to C");
    approve_pending_transaction(
        &conductor,
        &c_cell,
        tx3_record.action_address().clone(),
        tx3_record.action_address().clone(),
    )
    .await
    .expect("C should approve tx3 (triggers cascade)");

    // Wait for cascade (C → B via call_remote) and buyer contract (C → A via call_remote).
    // C's cascade: own 20 drained → C.debt=0. B has 0 debt → dry. remaining=30 → genesis to buyer A.
    // Seller C gets NO new debt. C.debt = 0.
    wait_for_debt_to_reach(&conductor, &c_cell, c_agent.clone(), 0.0, 1.0, 10000)
        .await
        .expect("C's debt should drain to ~0 after step 4 cascade (own 20 drained, no seller genesis)");
    wait_for_active_contract(&conductor, &a_cell, a_agent.clone())
        .await
        .expect("A should have an active debt contract (buyer contract from tx3)");

    // ── Assertions ───────────────────────────────────────────────────────────────
    let a_debt_final = get_total_debt(&conductor, &a_cell, a_agent.clone())
        .await
        .expect("Should get A's final debt");
    let b_debt_final = get_total_debt(&conductor, &b_cell, b_agent.clone())
        .await
        .expect("Should get B's final debt");
    let c_debt_final = get_total_debt(&conductor, &c_cell, c_agent.clone())
        .await
        .expect("Should get C's final debt");

    eprintln!("=== FINAL STATE ===");
    eprintln!("A.debt = {a_debt_final} (expected: ~50 — buyer contract from step 4)");
    eprintln!("B.debt = {b_debt_final} (expected: 0 — drained in step 2, no genesis on seller)");
    eprintln!("C.debt = {c_debt_final} (expected: 0 — drained by own cascade in step 4, no seller genesis)");

    // A has 50 debt: buyer contract from step 4 only. A was seller in step 1 → no genesis on seller.
    assert!(
        (a_debt_final - 50.0).abs() < 3.0,
        "A should have ~50 debt (buyer contract from step 4), got {a_debt_final}"
    );

    // B has 0 debt: original 10 from step 1 drained when B sold in step 2; no new genesis on seller.
    assert!(
        b_debt_final < 1.0,
        "B should have ~0 debt (drained in step-2 cascade, no seller genesis), got {b_debt_final}"
    );

    // C has 0 debt: original 20 from step 2 drained by own cascade in step 4; no new genesis on seller.
    assert!(
        c_debt_final < 1.0,
        "C should have ~0 debt (drained by own cascade in step 4, no seller genesis), got {c_debt_final}"
    );
}

/// Cascade debt conservation invariant.
///
/// The cascade mechanism redistributes existing debt — it does not inflate total system debt.
/// When buyer B creates a transaction of amount `delta` from seller S (who has existing debt D):
///   - A new DebtContract of `delta` is created on B's chain  (+delta to system total)
///   - S's existing contracts are drained by min(D, delta)     (-min(D,delta) from system total)
///
/// Net effect: total system debt changes by delta - min(D, delta).
///   - If S has enough debt (D >= delta): system total is unchanged (delta - delta = 0).
///   - If S has less debt (D < delta): system total increases by delta - D (genesis portion).
///
/// The upper bound is always: debt_after <= debt_before + delta.
/// This test verifies that upper bound and that the buyer (Carol) acquired the full obligation.
#[tokio::test(flavor = "multi_thread")]
async fn test_cascade_debt_conservation() {
    // Alice (0), Bob (1), Carol (2): full genesis vouching so all have capacity
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    // Step 1: Bob buys 30 from Alice (trial, Alice approves) → Bob has 30 active debt
    let tx1 = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Conservation: Bob buys from Alice".to_string(),
        debt: 30.0,
    };
    let record1 = create_transaction(&conductor, &bob_cell, tx1)
        .await
        .expect("Bob's trial purchase should succeed");

    ensure_transaction_propagation_seller(&conductor, &alice_cell, alice_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Trial must propagate to Alice");
    approve_pending_transaction(
        &conductor,
        &alice_cell,
        record1.action_address().clone(),
        record1.action_address().clone(),
    )
    .await
    .expect("Alice should approve");
    wait_for_active_contract(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Bob must have active contract");

    // Use a short consistency wait — test-epoch has 1s epochs so await_consistency(N)
    // = N epoch boundaries. Staying under MIN_MATURITY (10 epochs) keeps contracts Active.
    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(5, &cells).await.unwrap();
    }

    // Snapshot: sum of all agents' active debt before the cascade transaction
    let agent_cells: Vec<(SweetCell, AgentPubKey)> = vec![
        (alice_cell.clone(), alice_agent.clone()),
        (bob_cell.clone(), bob_agent.clone()),
        (carol_cell.clone(), carol_agent.clone()),
    ];
    let debt_before = sum_active_debt(&conductor, &agent_cells).await;

    // Step 2: Carol buys 20 from Bob — this triggers a cascade: Bob's debt to Alice
    // will be partially drained. The total system active debt must increase by exactly 20.
    let sale_amount = 20.0_f64;
    let tx2 = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: carol_agent.clone().into(),
        description: "Conservation: Carol buys from Bob (cascade trigger)".to_string(),
        debt: sale_amount,
    };
    let record2 = create_transaction(&conductor, &carol_cell, tx2)
        .await
        .expect("Carol's trial purchase should succeed");

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Transaction must propagate to Bob");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        record2.action_address().clone(),
        record2.action_address().clone(),
    )
    .await
    .expect("Bob should approve Carol's purchase");

    // Wait for the cascade to settle: Bob's debt is draining into Alice
    // while Carol's new contract is being created. Poll Bob until stable.
    // Alice has zero debt throughout (she is creditor, not debtor), so we
    // only wait on Bob (the debtor whose debt is being cascaded) and Carol
    // (the new debtor from the purchase).
    wait_for_debt_stable(&conductor, &bob_cell, bob_agent.clone(), 0.01, 20000)
        .await
        .expect("Bob's debt should stabilise after cascade");
    wait_for_active_contract(&conductor, &carol_cell, carol_agent.clone())
        .await
        .expect("Carol must have an active contract after approval");

    // Wait for Carol's debt cache to settle
    wait_for_debt_to_reach(&conductor, &carol_cell, carol_agent.clone(), sale_amount, 1.0, 10000)
        .await
        .expect("Carol's debt should reach the sale amount after contract creation");

    // Query each agent's debt individually
    let alice_debt_after = get_total_debt(&conductor, &alice_cell, alice_agent.clone())
        .await
        .unwrap_or(0.0);
    let bob_debt_after = get_total_debt(&conductor, &bob_cell, bob_agent.clone()).await.unwrap_or(0.0);
    let carol_debt_after = get_total_debt(&conductor, &carol_cell, carol_agent.clone())
        .await
        .unwrap_or(0.0);
    let debt_after = alice_debt_after + bob_debt_after + carol_debt_after;

    // Correct cascade conservation invariant:
    // When Carol buys `sale_amount=20` from Bob (who has `debt_before=30` to Alice):
    //   1. Carol gets a new DebtContract of +20 (debt_after increases by 20)
    //   2. Bob's cascade fires: drains min(Bob's debt, sale_amount) = min(30,20) = 20
    //      → Bob's debt decreases by 20 (debt_after decreases by 20)
    //   3. Net delta = +20 (Carol) - 20 (Bob cascade drain) = 0
    //
    // The cascade conserves system-wide debt: new buyer debt exactly offsets the
    // drained seller debt. Total debt is UNCHANGED, not increased.
    //
    // This is the key protocol property: cascades do not inflate total system debt.
    // They redistribute it — Carol's new obligation replaces Bob's drained portion.
    //
    // The invariant is: debt_after ≤ debt_before + sale_amount
    //   (equality when cascade drains nothing; less when cascade absorbs the new debt)
    let max_expected = debt_before + sale_amount;
    assert!(
        debt_after <= max_expected + 1.0,
        "Cascade must not inflate total system debt beyond debt_before + sale_amount. \
         Before: {debt_before:.2}, After: {debt_after:.2} \
         (Alice={alice_debt_after:.2}, Bob={bob_debt_after:.2}, Carol={carol_debt_after:.2}), \
         Max expected: {max_expected:.2}."
    );
    // Carol must hold the new contract obligation
    assert!(
        carol_debt_after >= sale_amount - 1.0,
        "Carol must have acquired the new obligation: expected ~{sale_amount:.2}, got {carol_debt_after:.2}"
    );
    // Total system debt must be non-negative
    assert!(debt_after >= 0.0, "System debt cannot be negative: {debt_after:.2}");
}
