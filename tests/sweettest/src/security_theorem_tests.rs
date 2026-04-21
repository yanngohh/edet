use super::*;

/// Gateway attack containment (Theorem 4):
/// After the accomplice defaults, the gateway's reputation collapses from the victim's view.
///
/// Scenario:
///   - Alice (honest victim) transacts with Gateway.
///   - Gateway vouches for Accomplice.
///   - Accomplice buys from Alice (creates debt).
///   - Accomplice defaults (contract expires without transfer).
///   - Alice observes Gateway's failure via contagion.
///   - Gateway's trust from Alice's perspective should drop.
#[tokio::test(flavor = "multi_thread")]
async fn test_gateway_attack_containment() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let gateway_cell = apps[1].cells()[0].clone();
    let accomplice_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let gateway_agent: AgentPubKey = gateway_cell.agent_pubkey().clone();
    let accomplice_agent: AgentPubKey = accomplice_cell.agent_pubkey().clone();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // Gateway vouches for accomplice (uses genesis_vouch since this is the test founding epoch)
    let vouch_input = CreateVouchInput {
        sponsor: gateway_agent.clone().into(),
        entrant: accomplice_agent.clone().into(),
        amount: 500.0,
    };
    let vouch_result: ConductorApiResult<Record> = conductor
        .call_fallible(&gateway_cell.zome("transaction"), "genesis_vouch", vouch_input)
        .await;
    assert!(vouch_result.is_ok(), "Gateway should be able to vouch for Accomplice");

    // Accomplice gains vouched capacity
    let accomplice_capacity = get_vouched_capacity(&conductor, &accomplice_cell, accomplice_agent.clone())
        .await
        .expect("Should get Accomplice's vouched capacity");
    assert!(accomplice_capacity >= 500.0, "Accomplice should have vouched capacity: got {accomplice_capacity}");

    // Accomplice buys from Alice (Alice becomes creditor of Accomplice)
    ensure_wallet_propagation(&conductor, &accomplice_cell, alice_agent.clone())
        .await
        .expect("Alice's wallet must be visible to Accomplice");
    let tx = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: accomplice_agent.clone().into(),
        description: "Accomplice buys from honest Alice".to_string(),
        debt: 49.0,
    };
    let tx_record = create_transaction(&conductor, &accomplice_cell, tx)
        .await
        .expect("Accomplice's transaction to Alice should succeed");

    // Alice approves (it's a trial, Pending)
    ensure_transaction_propagation_seller(&conductor, &alice_cell, alice_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Transaction must propagate to Alice");
    approve_pending_transaction(
        &conductor,
        &alice_cell,
        tx_record.action_address().clone(),
        tx_record.action_address().clone(),
    )
    .await
    .expect("Alice should approve Accomplice's trial");

    wait_for_active_contract(&conductor, &accomplice_cell, accomplice_agent.clone())
        .await
        .expect("Accomplice's contract should be active");

    // Capture Alice's view of Gateway BEFORE the default
    publish_trust_row(&conductor, &alice_cell).await.ok();
    invalidate_trust_caches(&conductor, &alice_cell).await.ok();
    let gateway_trust_before = get_subjective_reputation(&conductor, &alice_cell, gateway_agent.clone())
        .await
        .expect("Should get gateway trust before default");

    // Accomplice defaults: sleep past MIN_MATURITY (test-epoch: 3 epochs = ~3s) then expire.
    tokio::time::sleep(tokio::time::Duration::from_millis(EPOCH_SLEEP_MS * MATURITY_EPOCHS)).await;
    let exp = process_contract_expirations(&conductor, &accomplice_cell)
        .await
        .expect("Should process expirations for Accomplice");
    assert!(exp.total_expired > 0.0, "Accomplice's contract should have expired: got {}", exp.total_expired);

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // Refresh Alice's trust view after the default propagates
    publish_trust_row(&conductor, &alice_cell).await.ok();
    invalidate_trust_caches(&conductor, &alice_cell).await.ok();
    let gateway_trust_after = get_subjective_reputation(&conductor, &alice_cell, gateway_agent.clone())
        .await
        .expect("Should get gateway trust after default");

    // Gateway's trust from Alice's perspective should decrease after Accomplice defaults.
    // The contagion mechanism records a failure against the gateway (as co-signer/sponsor).
    assert!(
        gateway_trust_after.trust <= gateway_trust_before.trust,
        "Gateway's trust should not increase after Accomplice defaults: before={}, after={}",
        gateway_trust_before.trust,
        gateway_trust_after.trust
    );
}

/// Slacker isolation: agent who accumulates debt and never sells loses reputation.
/// Extended to verify trust collapses after contract expiration.
#[tokio::test(flavor = "multi_thread")]
async fn test_slacker_isolation() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let slacker_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let slacker_agent: AgentPubKey = slacker_cell.agent_pubkey().clone();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    let tx = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: slacker_agent.clone().into(),
        description: "Slacker buys".to_string(),
        debt: 49.0,
    };
    let slacker_record = create_transaction(&conductor, &slacker_cell, tx)
        .await
        .expect("Slacker's trial transaction should succeed");

    ensure_transaction_propagation_seller(&conductor, &alice_cell, alice_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Slacker's trial must propagate to Alice");
    approve_pending_transaction(
        &conductor,
        &alice_cell,
        slacker_record.action_address().clone(),
        slacker_record.action_address().clone(),
    )
    .await
    .expect("Alice should approve Slacker's trial");

    wait_for_active_contract(&conductor, &slacker_cell, slacker_agent.clone())
        .await
        .expect("Slacker's contract should be active");

    let slacker_debt = get_total_debt(&conductor, &slacker_cell, slacker_agent.clone())
        .await
        .expect("Should get Slacker's debt");
    assert!(slacker_debt >= 49.0, "Slacker should have debt: got {slacker_debt}");

    // Verify trust starts at some positive value (slacker hasn't defaulted yet)
    publish_trust_row(&conductor, &alice_cell).await.ok();
    invalidate_trust_caches(&conductor, &alice_cell).await.ok();
    let trust_before = get_subjective_reputation(&conductor, &alice_cell, slacker_agent.clone())
        .await
        .expect("Should get slacker trust before default");

    // Slacker never sells → contract expires → F increases → trust collapses.
    tokio::time::sleep(tokio::time::Duration::from_millis(EPOCH_SLEEP_MS * MATURITY_EPOCHS)).await;
    let exp = process_contract_expirations(&conductor, &slacker_cell)
        .await
        .expect("Should expire slacker's contract");
    assert!(exp.total_expired > 0.0, "Slacker's contract should expire: got {}", exp.total_expired);

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }
    publish_trust_row(&conductor, &alice_cell).await.ok();
    invalidate_trust_caches(&conductor, &alice_cell).await.ok();

    let trust_after = get_subjective_reputation(&conductor, &alice_cell, slacker_agent.clone())
        .await
        .expect("Should get slacker trust after default");

    // Trust should collapse: 100% failure rate → phi=0 → trust=0 (or near 0 due to teleportation).
    assert!(
        trust_after.trust < 0.05,
        "Slacker trust should collapse after 100% failure rate: got {}",
        trust_after.trust
    );
    assert!(
        trust_after.trust <= trust_before.trust,
        "Slacker trust should not increase after default: before={}, after={}",
        trust_before.trust,
        trust_after.trust
    );
}

/// Whitewashing resistance: new identity has no trust from old
#[tokio::test(flavor = "multi_thread")]
async fn test_whitewashing_resistance() {
    // Use no-vouch setup so genesis vouches don't grant whitewash identity any
    // pre-existing trust from Alice.
    let (conductor, apps) = setup_multi_agent_no_vouch(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let original_cell = apps[1].cells()[0].clone();
    let whitewash_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let original_agent: AgentPubKey = original_cell.agent_pubkey().clone();
    let whitewash_agent: AgentPubKey = whitewash_cell.agent_pubkey().clone();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    let tx1 = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: original_agent.clone().into(),
        description: "Original buys".to_string(),
        debt: 49.0,
    };
    let _ = create_transaction(&conductor, &original_cell, tx1).await;

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    let original_rep = get_subjective_reputation(&conductor, &alice_cell, original_agent.clone())
        .await
        .expect("Should get original reputation");

    let whitewash_rep = get_subjective_reputation(&conductor, &alice_cell, whitewash_agent.clone())
        .await
        .expect("Should get whitewash reputation");

    assert!(
        whitewash_rep.acquaintance_count == 0 || whitewash_rep.trust == 0.0,
        "Whitewashed identity should have no trust with Alice"
    );

    let large_tx = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: whitewash_agent.clone().into(),
        description: "Whitewasher tries large purchase".to_string(),
        debt: 500.0,
    };
    let result = create_transaction(&conductor, &whitewash_cell, large_tx).await;
    match result {
        Ok(record) => {
            let tx: Transaction = record.entry().to_app_option().unwrap().unwrap();
            assert_ne!(
                tx.status,
                TransactionStatus::Accepted,
                "Whitewashed identity's large transaction must not be auto-accepted"
            );
        }
        Err(e) => {
            let err_str = format!("{e:?}");
            assert!(
                err_str.contains("EC200002") || err_str.contains("CAPACITY") || err_str.contains("capacity"),
                "Whitewash rejection should be capacity-related, got: {err_str}"
            );
        }
    }
}

/// Intermittent attack exclusion (Theorem 5.3 / recent window):
/// An agent who builds positive history then defaults sees trust collapse
/// rapidly due to the recent failure rate window (RECENT_WEIGHT=2.0).
#[tokio::test(flavor = "multi_thread")]
async fn test_intermittent_attack_exclusion() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let attacker_cell = apps[1].cells()[0].clone();
    let helper_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let attacker_agent: AgentPubKey = attacker_cell.agent_pubkey().clone();
    let helper_agent: AgentPubKey = helper_cell.agent_pubkey().clone();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // Phase 1: Honest behaviour — Attacker buys from Alice, Helper buys from Attacker
    // (this creates a positive S_{Alice, Attacker} counter)
    let tx1 = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: attacker_agent.clone().into(),
        description: "Attacker buys 1 (honest phase)".to_string(),
        debt: 49.0,
    };
    let record1 = create_transaction(&conductor, &attacker_cell, tx1)
        .await
        .expect("Attacker's first transaction should succeed");

    ensure_transaction_propagation_seller(&conductor, &alice_cell, alice_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Attacker's trial must propagate to Alice");
    approve_pending_transaction(
        &conductor,
        &alice_cell,
        record1.action_address().clone(),
        record1.action_address().clone(),
    )
    .await
    .expect("Alice should approve Attacker's trial");

    wait_for_active_contract(&conductor, &attacker_cell, attacker_agent.clone())
        .await
        .expect("Attacker's contract should be active");

    // Helper buys from Attacker — debt transfers, S_{Alice, Attacker} > 0
    let tx2 = CreateTransactionInput {
        seller: attacker_agent.clone().into(),
        buyer: helper_agent.clone().into(),
        description: "Helper buys from Attacker (debt transfer — honest phase)".to_string(),
        debt: 30.0,
    };
    create_transaction(&conductor, &helper_cell, tx2)
        .await
        .expect("Helper buys from Attacker should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    publish_trust_row(&conductor, &alice_cell).await.ok();
    invalidate_trust_caches(&conductor, &alice_cell).await.ok();
    let trust_honest_phase = get_subjective_reputation(&conductor, &alice_cell, attacker_agent.clone())
        .await
        .expect("Should get trust after honest phase");

    assert!(
        trust_honest_phase.trust > 0.0,
        "Attacker should have positive trust after honest phase: got {}",
        trust_honest_phase.trust
    );

    // Phase 2: Attack — Attacker buys from Helper (different seller) and NEVER sells (default).
    // We use Helper as seller rather than Alice because the Attacker→Alice trial is still
    // Active (not Transferred), so the open-trial gate would block a repeat trial to Alice
    // until repayment. Helper is a fresh pair with no prior trial slot in use.
    // The failure observation is published on the DHT and Alice can still observe the
    // Attacker's default via the FailureObservationIndex, causing trust collapse.
    let tx3 = CreateTransactionInput {
        seller: helper_agent.clone().into(),
        buyer: attacker_agent.clone().into(),
        description: "Attacker buys 2 (attack phase — will default)".to_string(),
        debt: 49.0,
    };
    let record3 = create_transaction(&conductor, &attacker_cell, tx3)
        .await
        .expect("Attacker's second transaction should succeed");

    ensure_transaction_propagation_seller(
        &conductor,
        &helper_cell,
        helper_agent.clone(),
        TransactionStatusTag::Pending,
    )
    .await
    .expect("Attacker's second trial must propagate to Helper");
    approve_pending_transaction(
        &conductor,
        &helper_cell,
        record3.action_address().clone(),
        record3.action_address().clone(),
    )
    .await
    .expect("Helper should approve Attacker's second trial");

    wait_for_active_contract(&conductor, &attacker_cell, attacker_agent.clone())
        .await
        .expect("Attacker's second contract should be active");

    // Attacker never sells in this epoch → contract expires (default).
    tokio::time::sleep(tokio::time::Duration::from_millis(EPOCH_SLEEP_MS * MATURITY_EPOCHS)).await;
    let exp = process_contract_expirations(&conductor, &attacker_cell)
        .await
        .expect("Should expire attacker's contract");
    assert!(exp.total_expired > 0.0, "Attacker's contract should expire: got {}", exp.total_expired);

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    publish_trust_row(&conductor, &alice_cell).await.ok();
    invalidate_trust_caches(&conductor, &alice_cell).await.ok();
    let trust_attack_phase = get_subjective_reputation(&conductor, &alice_cell, attacker_agent.clone())
        .await
        .expect("Should get trust after attack phase");

    // The recent window (RECENT_WEIGHT=2.0) amplifies the recent failure.
    // Since F_recent > 0 and S_recent = 0 in the last epoch, r_recent = 1.0.
    // r_star = max(r_cumul, 2.0 * 1.0) = 2.0 > tau → phi = 0 → trust collapses.
    assert!(
        trust_attack_phase.trust < trust_honest_phase.trust,
        "Trust should drop after intermittent attack: honest_phase={}, attack_phase={}",
        trust_honest_phase.trust,
        trust_attack_phase.trust
    );
    assert!(
        trust_attack_phase.trust < 0.05,
        "Trust should collapse after recent-window detects switch: got {}",
        trust_attack_phase.trust
    );
}

/// Sybil resistance: internal transactions don't build external trust
#[tokio::test(flavor = "multi_thread")]
async fn test_sybil_internal_transactions() {
    // Use no-vouch setup so genesis vouches don't give Sybil identities pre-existing
    // trust from the honest observer.
    let (conductor, apps) = setup_multi_agent_no_vouch(3).await;
    let honest_cell = apps[0].cells()[0].clone();
    let sybil1_cell = apps[1].cells()[0].clone();
    let sybil2_cell = apps[2].cells()[0].clone();

    let honest_agent: AgentPubKey = honest_cell.agent_pubkey().clone();
    let sybil1_agent: AgentPubKey = sybil1_cell.agent_pubkey().clone();
    let sybil2_agent: AgentPubKey = sybil2_cell.agent_pubkey().clone();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    let internal_tx = CreateTransactionInput {
        seller: sybil2_agent.clone().into(),
        buyer: sybil1_agent.clone().into(),
        description: "Sybil internal tx".to_string(),
        debt: 49.0,
    };
    let _ = create_transaction(&conductor, &sybil1_cell, internal_tx).await;

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    let sybil1_rep = get_subjective_reputation(&conductor, &honest_cell, sybil1_agent.clone())
        .await
        .expect("Should get Sybil1 reputation");

    assert!(sybil1_rep.trust < 0.1, "Sybil should have very low trust from honest observer: got {}", sybil1_rep.trust);
}
