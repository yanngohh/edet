use super::*;

/// Archive old contracts: returns result for new agent
#[tokio::test(flavor = "multi_thread")]
async fn test_archive_old_contracts() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();

    let result: ConductorApiResult<ArchivalResult> = conductor
        .call_fallible(&cell.zome("transaction"), "archive_old_contracts", ())
        .await;

    assert!(result.is_ok(), "Archive should succeed");
    let archival = result.unwrap();
    assert_eq!(archival.archived_count, 0, "Should have no contracts to archive");
}

/// Get archived contracts: returns empty for new agent
#[tokio::test(flavor = "multi_thread")]
async fn test_archived_contracts_empty_for_new_agent() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();
    let agent: AgentPubKey = cell.agent_pubkey().clone();

    let archived: ConductorApiResult<Vec<Record>> = conductor
        .call_fallible(&cell.zome("transaction"), "get_archived_contracts", agent)
        .await;

    assert!(archived.is_ok(), "Should get archived contracts");
    assert!(archived.unwrap().is_empty(), "New agent should have no archived contracts");
}

/// Batch trust row fetching: works via reputation query
#[tokio::test(flavor = "multi_thread")]
async fn test_batch_trust_row_fetching() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    let _ = get_wallet_for_agent(&conductor, &alice_cell, alice_agent.clone()).await;
    let _ = get_wallet_for_agent(&conductor, &bob_cell, bob_agent.clone()).await;
    let _ = get_wallet_for_agent(&conductor, &carol_cell, carol_agent.clone()).await;

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob's wallet should propagate to Alice");
    ensure_wallet_propagation(&conductor, &bob_cell, carol_agent.clone())
        .await
        .expect("Carol's wallet should propagate to Bob");

    let tx1 = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Batch test 1".to_string(),
        debt: 20.0,
    };
    create_transaction(&conductor, &alice_cell, tx1)
        .await
        .expect("First tx should succeed");

    let tx2 = CreateTransactionInput {
        seller: carol_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Batch test 2".to_string(),
        debt: 15.0,
    };
    create_transaction(&conductor, &bob_cell, tx2)
        .await
        .expect("Second tx should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let rep = get_subjective_reputation(&conductor, &alice_cell, carol_agent)
        .await
        .expect("Should get reputation");

    assert!(rep.trust >= 0.0 && rep.trust <= 1.0, "Trust should be in [0,1]");
}

/// Create checkpoint for new agent
#[tokio::test(flavor = "multi_thread")]
async fn test_create_checkpoint() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();

    let result: ConductorApiResult<Option<Record>> = conductor
        .call_fallible(&cell.zome("transaction"), "create_checkpoint", ())
        .await;

    assert!(result.is_ok(), "Checkpoint call should succeed");
}

/// Get latest checkpoint: returns None for new agent without checkpoint
#[tokio::test(flavor = "multi_thread")]
async fn test_get_latest_checkpoint() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();
    let agent: AgentPubKey = cell.agent_pubkey().clone();

    let checkpoint: ConductorApiResult<Option<(ActionHash, Record)>> = conductor
        .call_fallible(&cell.zome("transaction"), "get_latest_checkpoint", agent)
        .await;

    assert!(checkpoint.is_ok(), "Should be able to query checkpoint");
}

/// Verify checkpoint consistency
#[tokio::test(flavor = "multi_thread")]
async fn test_verify_checkpoint_consistency() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();

    let _checkpoint: Option<Record> = conductor
        .call_fallible(&cell.zome("transaction"), "create_checkpoint", ())
        .await
        .expect("Checkpoint call should succeed");

    {
        await_consistency(30, [&cell]).await.unwrap();
    }

    let consistent: ConductorApiResult<bool> = conductor
        .call_fallible(&cell.zome("transaction"), "verify_checkpoint_consistency", ())
        .await;

    assert!(consistent.is_ok(), "Consistency check should succeed");
    assert!(consistent.unwrap(), "Checkpoint should be consistent for new agent");
}

/// Debt balance: returns 0 and initializes lazily for new agent
#[tokio::test(flavor = "multi_thread")]
async fn test_debt_balance_lazy_init() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();
    let agent: AgentPubKey = cell.agent_pubkey().clone();

    let debt = get_total_debt(&conductor, &cell, agent).await.expect("Should get debt");

    assert_eq!(debt, 0.0, "New agent should have 0 debt");
}

/// Scalability functions work together with multiple agents
#[tokio::test(flavor = "multi_thread")]
async fn test_scalability_functions_integration() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    ensure_wallet_propagation(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Bob's wallet should propagate to Alice");

    let tx = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Integration test".to_string(),
        debt: 49.0,
    };
    create_transaction(&conductor, &alice_cell, tx)
        .await
        .expect("Transaction should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let _claim = publish_reputation_claim(&conductor, &alice_cell)
        .await
        .expect("Claim should be published");

    let _checkpoint: Option<Record> = conductor
        .call_fallible(&alice_cell.zome("transaction"), "create_checkpoint", ())
        .await
        .expect("Checkpoint call should succeed");

    let archival: ArchivalResult = conductor
        .call_fallible(&alice_cell.zome("transaction"), "archive_old_contracts", ())
        .await
        .expect("Archive should succeed");
    assert_eq!(archival.archived_count, 0, "New contracts shouldn't be archived");

    let stats = get_trust_cache_stats(&conductor, &alice_cell)
        .await
        .expect("Cache stats should be accessible");
    let _ = stats;

    let rep = get_subjective_reputation(&conductor, &bob_cell, alice_agent)
        .await
        .expect("Should get reputation");
    assert!(rep.trust >= 0.0, "Trust should be non-negative");
}
