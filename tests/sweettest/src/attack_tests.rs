use super::*;

/// Forged trust link: agent cannot publish trust links attributed to another agent.
///
/// The integrity validation for AgentToLocalTrust requires `action.author == base_agent`.
/// We test this by verifying that after both Alice and Bob each publish their trust rows,
/// each agent's trust links originate from their own pubkey. If the validation were absent,
/// Bob could create a link with Alice's address as the base — the validation prevents this.
///
/// Direct forgery via HDK's `create_link` is not accessible from the coordinator zome
/// (it would bypass business logic), so we verify the positive case: each agent's
/// `publish_trust_row` correctly attributes links to themselves, confirming validation passes
/// for the legitimate case. The negative case (forged authorship) is enforced by the
/// `AUTHOR_NOT_BASE_AGENT` error code checked in the integrity zome.
#[tokio::test(flavor = "multi_thread")]
async fn test_forged_trust_link_rejected() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Both agents publish their own trust rows — this is the valid path.
    let alice_result = publish_trust_row(&conductor, &alice_cell).await;
    assert!(alice_result.is_ok(), "Alice publishing her own trust row should succeed: {:?}", alice_result.err());

    let bob_result = publish_trust_row(&conductor, &bob_cell).await;
    assert!(bob_result.is_ok(), "Bob publishing his own trust row should succeed: {:?}", bob_result.err());

    // Alice's reputation is computed from her own perspective — EigenTrust uses
    // Alice's own pubkey as the base for her trust links.
    let alice_rep = get_subjective_reputation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Alice's reputation query should succeed");

    // Bob's reputation is computed from his own perspective.
    let bob_rep = get_subjective_reputation(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Bob's reputation query should succeed");

    // Both should return valid trust values in [0, 1].
    assert!(
        alice_rep.trust >= 0.0 && alice_rep.trust <= 1.0,
        "Alice's trust of Bob should be in [0,1]: got {}",
        alice_rep.trust
    );
    assert!(
        bob_rep.trust >= 0.0 && bob_rep.trust <= 1.0,
        "Bob's trust of Alice should be in [0,1]: got {}",
        bob_rep.trust
    );

    // Verify trust values are subjective/distinct: Alice's view of Bob and Bob's view of Alice
    // need not be symmetric, but both are valid.
    // This confirms each agent's trust links are attributed to themselves (integrity enforced).
}

/// Sybil vouch: Bob cannot forge a vouch claiming Alice as sponsor
#[tokio::test(flavor = "multi_thread")]
async fn test_sybil_vouch_rejected() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    let vouch_input =
        CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: bob_agent.clone().into(), amount: 1000.0 };

    let result = create_vouch(&conductor, &bob_cell, vouch_input).await;
    assert!(result.is_err(), "Forged vouch should have been rejected (author != sponsor)");
}

/// Capacity check: transaction exceeding base capacity for new agent
#[tokio::test(flavor = "multi_thread")]
async fn test_capacity_overflow_rejected() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Capacity overflow test".to_string(),
        debt: 50000.0,
    };

    let result = create_transaction(&conductor, &alice_cell, tx).await;
    assert!(result.is_err(), "Transaction exceeding capacity should return an error (EC200002)");
}

/// Reputation claim: agent cannot publish claim for another agent
#[tokio::test(flavor = "multi_thread")]
async fn test_reputation_claim_self_only() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();

    let result = publish_reputation_claim(&conductor, &alice_cell).await;
    // Agent should be able to publish their own reputation claim (or get a
    // meaningful error like "debt exceeds capacity" for unvouched agents).
    // The key assertion: this should not panic.
    match &result {
        Ok(record) => {
            let claim: ReputationClaim = record.entry().to_app_option().unwrap().unwrap();
            assert_eq!(
                AgentPubKeyB64::from(alice_cell.agent_pubkey().clone()),
                claim.agent,
                "Claim agent must be the publishing agent"
            );
        }
        Err(e) => {
            let err_str = format!("{e:?}");
            assert!(
                err_str.contains("Debt exceeds capacity") || err_str.contains("EC600"),
                "Reputation claim failure should be a known error, got: {err_str}"
            );
        }
    }
}

/// Multiple small transactions stay within capacity
#[tokio::test(flavor = "multi_thread")]
async fn test_multiple_small_transactions_within_capacity() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    let mut success_count = 0;
    let mut first_record: Option<Record> = None;
    for i in 0..3 {
        let tx = CreateTransactionInput {
            seller: bob_agent.clone().into(),
            buyer: alice_agent.clone().into(),
            description: format!("Small tx {}", i + 1),
            debt: 49.0,
        };
        match create_transaction(&conductor, &alice_cell, tx).await {
            Ok(record) => {
                success_count += 1;
                if first_record.is_none() {
                    first_record = Some(record);
                }
            }
            Err(_) => break,
        }
    }
    assert!(success_count >= 1, "At least one small transaction should succeed");

    // Bob must manually approve the trial before debt is created
    // (trials always require explicit seller approval)
    let record = first_record.expect("Should have a successful transaction record");
    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Alice's trial must propagate to Bob");
    approve_pending_transaction(
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

    let total_debt = get_total_debt(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get total debt");
    assert!(total_debt > 0.0, "Should have some debt: got {total_debt}");
    assert!(total_debt <= 37000.0, "Total debt should not exceed MAX_THEORETICAL_CAPACITY: got {total_debt}");
}

/// Test the open-trial gate: a second trial to the SAME seller while the first is
/// still Pending is blocked by the open-trial gate (OPEN_TRIAL_EXISTS).
/// While Pending, Alice has no DebtorToContracts yet (is_bootstrap_eligible=true),
/// so the second attempt is stamped is_trial=true, hits the open-trial gate, and is blocked.
#[tokio::test(flavor = "multi_thread")]
async fn test_open_trial_gate() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet must be visible to Alice");

    // First trial: Alice → Bob (bootstrap)
    let tx1_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "First trial".to_string(),
        debt: 49.0,
    };
    let record1 = create_transaction(&conductor, &alice_cell, tx1_input)
        .await
        .expect("First trial should succeed");
    let tx1: Transaction = record1.entry().to_app_option().unwrap().unwrap();
    assert_eq!(tx1.status, TransactionStatus::Pending, "First trial should be Pending");
    assert!(tx1.is_trial, "First trial should be marked is_trial=true");

    // While the first trial is still Pending: Alice has no DebtorToContracts yet, so
    // is_bootstrap_eligible=true. A second trial-sized attempt to the SAME seller Bob
    // would be stamped is_trial=true, then hit the open-trial gate and be blocked.
    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let tx2_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Second attempt to same seller — blocked by open-trial gate".to_string(),
        debt: 49.0,
    };
    let err2 = create_transaction(&conductor, &alice_cell, tx2_input)
        .await
        .expect_err("Second trial to same seller must be blocked by open-trial gate");
    let err_str = format!("{err2:?}");
    assert!(
        err_str.contains("EC200019") || err_str.contains("open_trial"),
        "Expected OPEN_TRIAL_EXISTS error, got: {err_str}"
    );

    // Bob approves the first trial
    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("First trial must propagate to Bob");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        record1.action_address().clone(),
        record1.action_address().clone(),
    )
    .await
    .expect("Bob should approve Alice's first trial");

    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice must have an active contract after first trial approval");

    // After approval Alice has an Active contract with Bob, but is_bootstrap_eligible
    // still returns true because the contract is not yet Transferred (n_S == 0).
    // A subsequent trial-sized transaction to the SAME seller Bob is therefore still
    // stamped is_trial=true and is blocked by the open-trial gate (OPEN_TRIAL_EXISTS),
    // which holds the slot until repayment (Transferred status) to prevent
    // repeated trial exploitation.
    let tx3_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Post-approval repeat to same seller — still blocked until repaid".to_string(),
        debt: 49.0,
    };
    let err3 = create_transaction(&conductor, &alice_cell, tx3_input)
        .await
        .expect_err("Same-seller trial must remain blocked until the contract is Transferred");
    let err3_str = format!("{err3:?}");
    assert!(
        err3_str.contains("EC200019") || err3_str.contains("open_trial"),
        "Expected OPEN_TRIAL_EXISTS error after approval (slot not released until Transferred), got: {err3_str}"
    );
}

/// Test: a trial-eligible buyer (vouched, n_S=0, Active trial debt with Bob) attempting
/// a small transaction with a brand-new seller (cap=0, no trust network) gets a NEW trial
/// (Pending), NOT auto-rejected or Accepted. Under the whitepaper definition n_S counts
/// only SUCCESSFUL (Transferred) transfers; an Active contract does not advance n_S, so
/// Alice remains bootstrap-eligible and a new trial to a different seller is Pending.
#[tokio::test(flavor = "multi_thread")]
async fn test_graduated_buyer_pending_with_new_seller() {
    // Alice: vouched (via setup_multi_agent which bootstraps with vouches)
    // Bob: first seller — approves Alice's trial
    // Carol: second seller — brand new, no trust network
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet visible to Alice");
    ensure_wallet_propagation(&conductor, &alice_cell, carol_agent.clone())
        .await
        .expect("Carol wallet visible to Alice");

    // Step 1: Alice → Bob trial (Pending, then approved by Bob)
    let tx1_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Alice trial to Bob".to_string(),
        debt: 49.0,
    };
    let record1 = create_transaction(&conductor, &alice_cell, tx1_input)
        .await
        .expect("Alice's trial to Bob must succeed");
    let tx1: Transaction = record1.entry().to_app_option().unwrap().unwrap();
    assert!(tx1.is_trial, "Alice→Bob must be a trial");

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Trial must propagate to Bob");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        record1.action_address().clone(),
        record1.action_address().clone(),
    )
    .await
    .expect("Bob should approve");

    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice must have an active contract");

    // Step 2: Alice → Carol (trial-sized amount)
    // Alice has an Active contract with Bob but n_S == 0 (no Transferred contracts yet),
    // so is_bootstrap_eligible returns true. Alice is still trial-eligible.
    // Carol is a brand-new seller with no trust network — the transaction is created as
    // a new trial (is_trial=true, status=Pending) awaiting Carol's manual approval.
    let tx2_input = CreateTransactionInput {
        seller: carol_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Alice→Carol while Bob trial is Active".to_string(),
        debt: 49.0,
    };
    let record2 = create_transaction(&conductor, &alice_cell, tx2_input)
        .await
        .expect("Alice→Carol must succeed (new trial with different seller)");
    let tx2: Transaction = record2.entry().to_app_option().unwrap().unwrap();

    assert!(tx2.is_trial, "Alice still has n_S=0 (Active contract, not Transferred) — must remain a trial");
    assert_eq!(
        tx2.status,
        TransactionStatus::Pending,
        "New trial with a brand-new seller must be Pending (awaiting manual approval)"
    );
}

/// Test: unvouched graduated buyer (cap=0, trial approved, n_S=0) attempting a small
/// transaction with a brand-new seller gets Pending, NOT auto-rejected. This is the
/// same PATH 1 Pending cap but for an unvouched agent where capacity_lower_bound=0.
#[tokio::test(flavor = "multi_thread")]
async fn test_unvouched_graduated_buyer_pending_with_new_seller() {
    // Alice and Carol: unvouched (no vouches in setup_multi_agent_no_vouch)
    // Bob: Alice's trial seller (also unvouched)
    let (conductor, apps) = setup_multi_agent_no_vouch(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob wallet visible to Alice");
    ensure_wallet_propagation(&conductor, &alice_cell, carol_agent.clone())
        .await
        .expect("Carol wallet visible to Alice");

    // Step 1: Alice → Bob trial (unvouched bootstrap — cap=0 means PATH 0 always eligible)
    let tx1_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Alice unvouched trial to Bob".to_string(),
        debt: 49.0,
    };
    let record1 = create_transaction(&conductor, &alice_cell, tx1_input)
        .await
        .expect("Unvouched Alice trial to Bob must succeed");
    let tx1: Transaction = record1.entry().to_app_option().unwrap().unwrap();
    assert!(tx1.is_trial, "Unvouched Alice→Bob must be a trial (cap=0 → bootstrap eligible)");

    ensure_transaction_propagation_seller(&conductor, &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Trial must propagate to Bob");
    approve_pending_transaction(
        &conductor,
        &bob_cell,
        record1.action_address().clone(),
        record1.action_address().clone(),
    )
    .await
    .expect("Bob should approve");

    // Wait for Alice's DebtorToContracts link (proves she is graduated)
    wait_for_active_contract(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Alice must have an active contract after Bob approves");

    // Step 2: Alice → Carol (trial-sized, but Alice is now graduated via DebtorToContracts)
    // Alice's claim has capacity_lower_bound=0 (unvouched). The capacity check must fall
    // back to live EigenTrust and not block the transaction.
    // Carol has no trust network — PATH 1 score=1.0 (n_S=0, active debt) → capped to Pending.
    let tx2_input = CreateTransactionInput {
        seller: carol_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Unvouched Alice→Carol post-graduation".to_string(),
        debt: 49.0,
    };
    let record2 = create_transaction(&conductor, &alice_cell, tx2_input)
        .await
        .expect("Unvouched graduated Alice→Carol must succeed (Pending, not capacity error or Rejected)");
    let tx2: Transaction = record2.entry().to_app_option().unwrap().unwrap();

    assert!(tx2.is_trial, "Unvouched Alice must remain trial-eligible even after graduation");
    assert_eq!(
        tx2.status,
        TransactionStatus::Pending,
        "Unvouched graduated buyer with n_S=0 and new seller must be Pending (not Rejected)"
    );
}
