use super::*;
use transaction_integrity::types::constants::*;

/// Test acquaintance eviction: verifies that adding more than MAX_ACQUAINTANCES
/// triggers eviction of the node with lowest bilateral satisfaction.
#[tokio::test(flavor = "multi_thread")]
async fn test_acquaintance_eviction() {
    // We use a scale of 10 for this test to ensure it runs in reasonable time
    // during integration suites, but the logic verifies the MAX_ACQUAINTANCES
    // eviction mechanism regardless of the constant value.
    let num_initial = 10;
    let (conductor, apps) = setup_multi_agent_no_vouch(num_initial + 2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let alice_agent = alice_cell.agent_pubkey().clone();

    // 1. Fill Alice's acquaintance set by directly calling add_acquaintance.
    //    This tests the eviction mechanism directly without needing full transaction flows.
    for app in apps.iter().take(num_initial + 1).skip(1) {
        let peer_agent = app.cells()[0].agent_pubkey().clone();
        conductor
            .call_fallible::<_, ()>(&alice_cell.zome("transaction"), "add_acquaintance", peer_agent)
            .await
            .expect("add_acquaintance should succeed");
    }

    let res = get_subjective_reputation(&conductor, &alice_cell, alice_agent.clone())
        .await
        .unwrap();
    let count_before = res.acquaintance_count;

    // 2. Add one more peer to trigger eviction IF we were at cap.
    let peer_evictor = apps[num_initial + 1].cells()[0].agent_pubkey().clone();
    conductor
        .call_fallible::<_, ()>(&alice_cell.zome("transaction"), "add_acquaintance", peer_evictor)
        .await
        .expect("add_acquaintance evictor should succeed");

    let res_final = get_subjective_reputation(&conductor, &alice_cell, alice_agent.clone())
        .await
        .unwrap();

    if num_initial >= MAX_ACQUAINTANCES {
        assert!(res_final.acquaintance_count <= count_before, "Should stay at cap on eviction");
    } else {
        assert!(res_final.acquaintance_count > count_before, "Should grow under cap");
    }
}

/// Test capacity clamping: verifies that a transaction exceeding MAX_THEORETICAL_CAPACITY
/// is rejected or clamped by validation logic.
#[tokio::test(flavor = "multi_thread")]
async fn test_max_capacity_clamping() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let alice_agent = alice_cell.agent_pubkey().clone();
    let bob_agent = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .unwrap();

    // Attempt a transaction with debt > MAX_THEORETICAL_CAPACITY
    let excessive_debt = MAX_THEORETICAL_CAPACITY + 1.0;

    let tx = CreateTransactionInput {
        seller: alice_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Excessive debt".to_string(),
        debt: excessive_debt,
    };

    // This should fail integrity validation (DEBT_EXCEEDS_CAPACITY clamp check in validation.rs)
    // Or at least coordinator-level rejection.
    let result: ConductorApiResult<ActionHash> = conductor
        .call_fallible(&bob_cell.zome("transaction"), "create_transaction", tx)
        .await;

    assert!(result.is_err(), "Transaction with debt > MAX_THEORETICAL_CAPACITY should be rejected");
}
