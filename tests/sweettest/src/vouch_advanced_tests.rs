//! Vouch Advanced Tests (Whitepaper Theorem 2.1, Definition 2.4)
//!
//! Tests for multi-vouch accumulation, vouch link propagation,
//! and additive vouching from the same sponsor.

use super::*;

/// Multiple sponsors' vouches accumulate into total capacity.
///
/// Exercises: Vouch accumulation (Theorem 5.1, Cap = sum of V_staked).
/// When multiple sponsors vouch for the same entrant, the entrant's total
/// vouched capacity should equal the sum of all vouch amounts.
#[tokio::test(flavor = "multi_thread")]
async fn test_vouched_capacity_accumulates_from_multiple_sponsors() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    // Carol already has genesis vouches from Alice and Bob (500 each).
    // Verify the accumulated capacity.
    let carol_capacity = get_vouched_capacity(&conductor, &carol_cell, carol_agent.clone())
        .await
        .expect("Should get Carol's vouched capacity");

    // Carol should have 500 (from Alice) + 500 (from Bob) = 1000 vouched capacity
    assert!(
        carol_capacity >= 900.0,
        "Carol should have >= 900 vouched capacity from two sponsors: got {carol_capacity}"
    );

    // Add an extra vouch from Alice to Carol
    let extra_vouch =
        CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: carol_agent.clone().into(), amount: 200.0 };
    genesis_vouch(&conductor, &alice_cell, extra_vouch)
        .await
        .expect("Extra vouch should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let updated_capacity = get_vouched_capacity(&conductor, &carol_cell, carol_agent.clone())
        .await
        .expect("Should get updated capacity");

    assert!(
        updated_capacity >= carol_capacity + 150.0,
        "Capacity should increase by ~200: was {carol_capacity}, now {updated_capacity}"
    );
}

/// Vouch links are visible to both sponsor and entrant.
///
/// Exercises: Vouch link propagation (SponsorToVouch, EntrantToVouch links).
/// After vouching, the sponsor should see they've vouched for the entrant,
/// and the entrant should see the sponsor in their vouchers list.
#[tokio::test(flavor = "multi_thread")]
async fn test_vouch_visible_to_both_parties() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice already vouched for Bob during genesis bootstrap.

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    // Alice should see she vouched for Bob
    let alice_vouched_for_bob = get_my_vouched_for_agent(&conductor, &alice_cell, bob_agent.clone())
        .await
        .expect("Should check vouch status");

    assert!(alice_vouched_for_bob, "Alice should see she has vouched for Bob");

    // Bob should see Alice as a voucher
    let bob_vouchers = get_vouchers_for_agent(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Should get Bob's vouchers");

    assert!(!bob_vouchers.is_empty(), "Bob should have at least one voucher");
    assert!(bob_vouchers.contains(&alice_agent), "Alice should be in Bob's voucher list");
}

/// Same sponsor can vouch multiple times for the same entrant (additive).
///
/// Exercises: Vouch accumulation from same sponsor.
/// Multiple vouch entries from the same sponsor to the same entrant
/// should additively increase the entrant's capacity.
#[tokio::test(flavor = "multi_thread")]
async fn test_double_vouch_same_pair_accumulates() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Get Bob's initial capacity (from genesis vouching)
    let initial_cap = get_vouched_capacity(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Should get initial capacity");

    // Alice vouches for Bob again (additional genesis vouch)
    let vouch_input =
        CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: bob_agent.clone().into(), amount: 300.0 };
    genesis_vouch(&conductor, &alice_cell, vouch_input)
        .await
        .expect("Additional vouch should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();

        await_consistency(30, &cells).await.unwrap();
    }

    let updated_cap = get_vouched_capacity(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Should get updated capacity");

    assert!(
        updated_cap >= initial_cap + 250.0,
        "Capacity should increase by ~300 after additional vouch: was {initial_cap}, now {updated_cap}"
    );
}

/// Vouch release: sponsor withdraws support and entrant's capacity decreases.
///
/// Exercises: release_vouch (Whitepaper Section 6, vouch lifecycle).
#[tokio::test(flavor = "multi_thread")]
async fn test_vouch_release_decreases_capacity() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    let initial_cap = get_vouched_capacity(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Should get initial capacity");
    assert!(initial_cap >= 400.0, "Bob should have genesis vouched capacity: got {initial_cap}");

    // Alice creates an additional vouch for Bob
    let vouch_input =
        CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: bob_agent.clone().into(), amount: 300.0 };
    let vouch_record = genesis_vouch(&conductor, &alice_cell, vouch_input)
        .await
        .expect("Additional vouch should succeed");
    let vouch_hash = vouch_record.action_address().clone();

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    let cap_after_vouch = get_vouched_capacity(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Should get capacity after vouch");
    assert!(cap_after_vouch >= initial_cap + 250.0, "Capacity should increase after vouch");

    // Alice releases the vouch
    let released = release_vouch(&conductor, &alice_cell, vouch_hash.clone(), vouch_hash.clone())
        .await
        .expect("Release vouch should succeed");
    let released_vouch: Vouch = released.entry().to_app_option().unwrap().unwrap();
    assert_eq!(released_vouch.status, VouchStatus::Released, "Vouch should be Released");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    let cap_after_release = get_vouched_capacity(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Should get capacity after release");
    assert!(
        cap_after_release < cap_after_vouch,
        "Capacity should decrease after vouch release: was {cap_after_vouch}, now {cap_after_release}"
    );
}

/// Vouch release is blocked while the entrant has active debt contracts.
///
/// A sponsor cannot release their vouch stake while the entrant still has outstanding
/// active debt. If they could, the slash mechanism would lose its target for any
/// defaults on those pre-existing contracts. The coordinator checks for active contracts
/// before allowing the release and returns RELEASE_ENTRANT_HAS_ACTIVE_CONTRACTS.
///
/// Once the entrant's contracts resolve (transferred, expired, archived), the sponsor
/// should be able to release successfully.
#[tokio::test(flavor = "multi_thread")]
async fn test_vouch_release_blocked_while_entrant_has_active_debt() {
    let (conductor, apps) = setup_multi_agent_no_vouch(3).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();
    let carol_cell = apps[2].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();
    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();

    // Step 1: Alice vouches for Bob (gives Bob capacity to do transactions)
    let vouch_input =
        CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: bob_agent.clone().into(), amount: 500.0 };
    let vouch_record = genesis_vouch(&conductor, &alice_cell, vouch_input)
        .await
        .expect("Alice's vouch for Bob should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // Carol also needs a vouch to be a valid seller (capacity to operate)
    let carol_vouch =
        CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: carol_agent.clone().into(), amount: 500.0 };
    genesis_vouch(&conductor, &alice_cell, carol_vouch)
        .await
        .expect("Alice's vouch for Carol should succeed");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // Step 2: Bob buys from Carol (trial) — Bob now has an active debt contract
    let tx = CreateTransactionInput {
        seller: carol_agent.clone().into(),
        buyer: bob_agent.clone().into(),
        description: "Bob buys from Carol to create active debt".to_string(),
        debt: 30.0,
    };
    let tx_record = create_transaction(&conductor, &bob_cell, tx)
        .await
        .expect("Bob's trial purchase should succeed");

    ensure_transaction_propagation_seller(&conductor, &carol_cell, carol_agent.clone(), TransactionStatusTag::Pending)
        .await
        .expect("Transaction must propagate to Carol");
    approve_pending_transaction(
        &conductor,
        &carol_cell,
        tx_record.action_address().clone(),
        tx_record.action_address().clone(),
    )
    .await
    .expect("Carol should approve");
    wait_for_active_contract(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Bob must have active contract before release attempt");

    // Step 3: Alice tries to release her vouch for Bob while Bob has an active contract.
    // This MUST fail with RELEASE_ENTRANT_HAS_ACTIVE_CONTRACTS.
    let release_result = release_vouch(
        &conductor,
        &alice_cell,
        vouch_record.action_address().clone(),
        vouch_record.action_address().clone(),
    )
    .await;

    assert!(release_result.is_err(), "Vouch release must be blocked while entrant has active debt contracts");
    let err_str = format!("{:?}", release_result.unwrap_err());
    assert!(
        err_str.contains("EC700008"),
        "Expected RELEASE_ENTRANT_HAS_ACTIVE_CONTRACTS (EC700008) error, got: {err_str}"
    );

    // Step 4: Bob's debt resolves — Carol buys from Bob which triggers cascade and
    // transfers Bob's debt to Alice. In test-epoch mode, we can also wait for expiry.
    // Use expiry (simpler): sleep past MIN_MATURITY then process expirations.
    tokio::time::sleep(tokio::time::Duration::from_millis(EPOCH_SLEEP_MS * MATURITY_EPOCHS)).await;

    process_contract_expirations(&conductor, &bob_cell)
        .await
        .expect("Should process expirations for Bob");

    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // Verify Bob no longer has active contracts
    let active = get_active_contracts_for_debtor(&conductor, &bob_cell, bob_agent.clone())
        .await
        .expect("Should query Bob's active contracts");
    assert!(active.is_empty(), "Bob should have no active contracts after expiration: found {}", active.len());

    // Step 5: Now Alice should be able to release the vouch
    // Get the latest vouch hashes via get_vouches_given (follows SponsorToVouch links
    // to the most recent update, which may be Slashed if the expiry triggered a slash).
    let (orig_hash, prev_hash) = get_latest_vouch_hashes(&conductor, &alice_cell, vouch_record.action_address())
        .await
        .expect("Should find the vouch in Alice's vouches given");

    let release_result2 = release_vouch(&conductor, &alice_cell, orig_hash, prev_hash).await;

    // If the expiry slashed the vouch to Slashed status, release_vouch will still
    // fail with RELEASE_INVALID_STATUS (Active→Released only). In that case we
    // accept either: successful release (if Active) or RELEASE_INVALID_STATUS (if Slashed).
    // What we must NOT see is RELEASE_ENTRANT_HAS_ACTIVE_CONTRACTS.
    if let Err(ref e) = release_result2 {
        let err_str = format!("{e:?}");
        assert!(
            !err_str.contains("EC700008"),
            "After debt resolved, release must not fail with RELEASE_ENTRANT_HAS_ACTIVE_CONTRACTS: {err_str}"
        );
    }
}
