//! Multi-Conductor DHT Propagation Tests
//!
//! These tests exercise the real Holochain DHT gossip layer by running each
//! agent on its own conductor process. They validate that entries and links
//! written on one conductor become visible to agents on separate conductors
//! after DHT sync.
//!
//! All tests use `SweetConductorBatch::from_standard_config_rendezvous` so
//! conductors discover each other automatically via a shared local rendezvous
//! server (kitsune2_bootstrap_srv). `await_consistency` is used to wait for
//! full DHT sync before asserting cross-conductor visibility.
//!
//! Tests:
//! 1. test_wallet_visible_across_conductors
//! 2. test_transaction_propagates_across_conductors
//! 3. test_failure_observation_propagates_across_conductors
//! 4. test_vouch_visible_across_conductors

use std::time::Duration;

use holochain::sweettest::await_consistency;

use super::*;

// ============================================================================
//  Multi-conductor setup
// ============================================================================

/// How long to wait for initial gossip to settle after conductor setup.
const MULTI_CONDUCTOR_INIT_MS: u64 = 3000;

/// `await_consistency` timeout in seconds (generous for CI environments).
const CONSISTENCY_TIMEOUT_S: u64 = 30;

/// Create N conductors, each hosting one agent, connected via a shared
/// rendezvous server. Each agent's wallet is initialised. Returns the batch
/// and one `SweetApp` per conductor (index i → conductor i → agent i).
async fn setup_conductors(num: usize) -> (SweetConductorBatch, Vec<SweetApp>) {
    let mut conductors = SweetConductorBatch::from_standard_config_rendezvous(num).await;
    let dna = SweetDnaFile::from_bundle(&dna_path()).await.expect("Failed to load DNA");

    let mut apps = Vec::with_capacity(num);
    for i in 0..num {
        let app = conductors[i]
            .setup_app(&format!("edet-{i}"), std::slice::from_ref(&dna))
            .await
            .expect("Failed to setup app");
        // Trigger wallet init.
        let cell = app.cells()[0].clone();
        let agent = cell.agent_pubkey().clone();
        let _: Option<(Option<ActionHash>, Option<Record>)> = conductors[i]
            .call_fallible(&cell.zome("transaction"), "get_wallet_for_agent", agent)
            .await
            .ok();
        apps.push(app);
    }

    // Allow conductors to discover each other and perform initial gossip.
    tokio::time::sleep(Duration::from_millis(MULTI_CONDUCTOR_INIT_MS)).await;

    (conductors, apps)
}

/// Genesis-vouch every agent for every other agent across conductors.
/// Sponsor `i` calls `genesis_vouch` for entrant `j` on conductor `i`.
/// After all vouches are written we wait for DHT consistency.
async fn cross_conductor_genesis_vouching(conductors: &SweetConductorBatch, apps: &[SweetApp]) {
    let vouch_amount = 500.0;
    let n = apps.len();
    for i in 0..n {
        let sponsor_cell = apps[i].cells()[0].clone();
        let sponsor_agent: AgentPubKey = sponsor_cell.agent_pubkey().clone();
        for (j, app) in apps.iter().enumerate().take(n) {
            if i == j {
                continue;
            }
            let entrant_agent: AgentPubKey = app.cells()[0].agent_pubkey().clone();
            let input = CreateVouchInput {
                sponsor: sponsor_agent.clone().into(),
                entrant: entrant_agent.into(),
                amount: vouch_amount,
            };
            let _: Record = conductors[i]
                .call_fallible(&sponsor_cell.zome("transaction"), "genesis_vouch", input)
                .await
                .unwrap_or_else(|e| panic!("genesis_vouch {i}->{j} failed: {e:?}"));
        }
    }

    // Wait for all vouch entries to reach every conductor.
    let cells: Vec<SweetCell> = apps.iter().map(|a| a.cells()[0].clone()).collect();
    await_consistency(CONSISTENCY_TIMEOUT_S, &cells)
        .await
        .expect("vouch consistency");
}

// ============================================================================
//  1. test_wallet_visible_across_conductors
// ============================================================================
//
// A Wallet entry created during init on conductor 0 must propagate via DHT
// gossip and be retrievable by an agent on conductor 1.

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn test_wallet_visible_across_conductors() {
    let (conductors, apps) = setup_conductors(2).await;

    let alice_cell = apps[0].cells()[0].clone(); // conductor 0
    let bob_cell = apps[1].cells()[0].clone(); // conductor 1
    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();

    // Alice's wallet was created during setup on conductor 0.
    // Verify it is retrievable from Alice's own conductor first.
    let alice_wallet_local: (Option<ActionHash>, Option<Record>) = conductors[0]
        .call_fallible(&alice_cell.zome("transaction"), "get_wallet_for_agent", alice_agent.clone())
        .await
        .expect("alice wallet on own conductor");
    assert!(alice_wallet_local.0.is_some(), "Alice's wallet must exist on her own conductor");

    // Wait for full DHT consistency.
    await_consistency(CONSISTENCY_TIMEOUT_S, [&alice_cell, &bob_cell])
        .await
        .expect("alice-bob consistency");

    // Bob's conductor queries Alice's wallet — must be visible via DHT.
    let alice_wallet_remote: (Option<ActionHash>, Option<Record>) = conductors[1]
        .call_fallible(&bob_cell.zome("transaction"), "get_wallet_for_agent", alice_agent.clone())
        .await
        .expect("alice wallet from bob's conductor");

    assert!(alice_wallet_remote.0.is_some(), "Alice's wallet must be visible to Bob's conductor after DHT sync");
    assert!(alice_wallet_remote.1.is_some(), "Alice's wallet record must be retrievable from Bob's conductor");
}

// ============================================================================
//  2. test_transaction_propagates_across_conductors
// ============================================================================
//
// A Transaction entry created by the buyer (conductor 0) must propagate to the
// seller's conductor (conductor 1) so the seller can discover and moderate it.

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn test_transaction_propagates_across_conductors() {
    let (conductors, apps) = setup_conductors(2).await;
    cross_conductor_genesis_vouching(&conductors, &apps).await;

    let alice_cell = apps[0].cells()[0].clone(); // buyer
    let bob_cell = apps[1].cells()[0].clone(); // seller
    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice creates a trial transaction pointing to Bob as seller.
    // Debt must be strictly < 50.0 (TRIAL_FRACTION * BASE_CAPACITY) to be a trial.
    // Alice's conductor resolves Bob's wallet via DHT.
    let tx_record: Record = conductors[0]
        .call_fallible(&alice_cell.zome("transaction"), "create_transaction", {
            let mut tx = Transaction::default();
            tx.buyer.pubkey = alice_agent.clone().into();
            tx.seller.pubkey = bob_agent.clone().into();
            tx.debt = 30.0;
            tx.description = "cross-conductor trial".to_string();
            tx.status = TransactionStatus::Pending;
            tx
        })
        .await
        .expect("alice creates transaction");

    let tx_hash = tx_record.action_address().clone();
    let tx: Transaction = tx_record.entry().to_app_option().unwrap().unwrap();
    assert!(tx.is_trial, "30 debt must be a trial (threshold is < 50.0)");
    assert_eq!(tx.status, TransactionStatus::Pending);

    // Wait for the transaction to reach Bob's conductor.
    await_consistency(CONSISTENCY_TIMEOUT_S, [&alice_cell, &bob_cell])
        .await
        .expect("transaction consistency");

    // Bob queries for pending transactions on his conductor using the existing
    // get_transactions_for_seller helper (avoids direct GetTransactionsCursor
    // serialization issues with msgpack / SerializedBytes encoding).
    let bob_pending =
        get_transactions_for_seller(&conductors[1], &bob_cell, bob_agent.clone(), TransactionStatusTag::Pending)
            .await
            .expect("bob gets transactions for seller");

    let found = bob_pending.iter().any(|r| r.action_address() == &tx_hash);
    assert!(found, "Alice's trial transaction must be visible in Bob's pending list on his own conductor");
}

// ============================================================================
//  3. test_failure_observation_propagates_across_conductors
// ============================================================================
//
// Whitepaper Definition 3.5 (Witness-Based Contagion): failure observations are
// published as DHT links. An observer on a third conductor must be able to query
// a debtor's failure witnesses, seeing creditors from other conductors.
//
// Setup: Alice defaults on a contract with Bob (Alice's conductor).
// Carol (conductor 2) queries `get_failure_witnesses(alice)` — must see Bob.

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn test_failure_observation_propagates_across_conductors() {
    let (conductors, apps) = setup_conductors(3).await;
    cross_conductor_genesis_vouching(&conductors, &apps).await;

    let alice_cell = apps[0].cells()[0].clone(); // debtor
    let bob_cell = apps[1].cells()[0].clone(); // creditor
    let carol_cell = apps[2].cells()[0].clone(); // third-party observer
    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Create a trial-sized Active contract on Alice's chain (Alice = debtor, Bob = creditor).
    // Debt must be strictly < 50.0 (TRIAL_FRACTION * BASE_CAPACITY) to be a trial.
    // Use the direct-creation helper to avoid call_remote unreliability.
    let tx_input = {
        let mut tx = Transaction::default();
        tx.buyer.pubkey = alice_agent.clone().into();
        tx.seller.pubkey = bob_agent.clone().into();
        tx.debt = 30.0;
        tx.description = "cross-conductor expiry test".to_string();
        tx.status = TransactionStatus::Pending;
        tx
    };
    let tx_record: Record = conductors[0]
        .call_fallible(&alice_cell.zome("transaction"), "create_transaction", tx_input)
        .await
        .expect("alice creates trial tx");
    let tx_hash = tx_record.action_address().clone();

    // Create the DebtContract directly on Alice's chain.
    let _contract_record: Record = conductors[0]
        .call_fallible(
            &alice_cell.zome("transaction"),
            "create_debt_contract",
            CreateDebtContractInput {
                amount: 30.0,
                creditor: bob_agent.clone().into(),
                debtor: alice_agent.clone().into(),
                transaction_hash: tx_hash,
                is_trial: true,
            },
        )
        .await
        .expect("create_debt_contract on alice's chain");

    // Wait for the contract to propagate to all conductors.
    await_consistency(CONSISTENCY_TIMEOUT_S, [&alice_cell, &bob_cell, &carol_cell])
        .await
        .expect("contract consistency");

    // Sleep past MIN_MATURITY (3 epochs + safety margin).
    let total_ms = EPOCH_SLEEP_MS * MATURITY_EPOCHS;
    tokio::time::sleep(Duration::from_millis(total_ms)).await;

    // Alice processes expirations: this publishes a FailureObservation DHT link
    // (creditor=Bob, debtor=Alice) on Alice's conductor.
    let exp_result: ExpirationResult = conductors[0]
        .call_fallible(&alice_cell.zome("transaction"), "process_contract_expirations", ())
        .await
        .expect("process_contract_expirations");
    assert!(exp_result.total_expired > 0.0, "trial contract must have expired; result: {exp_result:?}");

    // Allow failure observation links to propagate across all conductors.
    await_consistency(CONSISTENCY_TIMEOUT_S, [&alice_cell, &bob_cell, &carol_cell])
        .await
        .expect("failure observation consistency");

    // Carol (conductor 2) queries failure witnesses for Alice.
    // Must see Bob as a witness, even though the observation was published on conductor 0.
    let witnesses: Vec<AgentPubKeyB64> = conductors[2]
        .call_fallible(&carol_cell.zome("transaction"), "get_failure_witnesses", alice_agent.clone())
        .await
        .expect("carol gets failure witnesses for alice");

    let bob_b64: AgentPubKeyB64 = bob_agent.clone().into();
    assert!(
        witnesses.contains(&bob_b64),
        "Bob must appear as a failure witness for Alice on Carol's conductor; witnesses: {witnesses:?}"
    );

    // Verify get_aggregate_witness_rate works cross-conductor.
    // With only 1 witness (Bob), rate should be 0.0 (below MIN_CONTAGION_WITNESSES=3).
    let agg_rate: f64 = conductors[2]
        .call_fallible(&carol_cell.zome("transaction"), "get_aggregate_witness_rate", alice_agent.clone())
        .await
        .expect("carol gets aggregate witness rate for alice");
    assert!(
        agg_rate.abs() < 1e-9,
        "Aggregate witness rate should be 0.0 with only 1 witness on Carol's conductor; got {agg_rate}"
    );
}

// ============================================================================
//  4. test_vouch_visible_across_conductors
// ============================================================================
//
// Whitepaper Definition 2.4 (Vouch Transaction): vouch entries and their
// associated links (EntrantToVouch, SponsorToVouch) are DHT data. An agent on
// one conductor must be able to discover vouches written by an agent on another.

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn test_vouch_visible_across_conductors() {
    let (conductors, apps) = setup_conductors(2).await;

    let alice_cell = apps[0].cells()[0].clone(); // sponsor
    let bob_cell = apps[1].cells()[0].clone(); // entrant
    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Alice (conductor 0) creates a genesis vouch for Bob (conductor 1).
    let _vouch_record: Record = conductors[0]
        .call_fallible(
            &alice_cell.zome("transaction"),
            "genesis_vouch",
            CreateVouchInput { sponsor: alice_agent.clone().into(), entrant: bob_agent.clone().into(), amount: 500.0 },
        )
        .await
        .expect("alice creates genesis vouch for bob");

    // Wait for the vouch entry and links to propagate to Bob's conductor.
    await_consistency(CONSISTENCY_TIMEOUT_S, [&alice_cell, &bob_cell])
        .await
        .expect("vouch consistency");

    // Bob queries his vouchers on his own conductor — must see Alice.
    let bob_vouchers: Vec<AgentPubKey> = conductors[1]
        .call_fallible(&bob_cell.zome("transaction"), "get_vouchers_for_agent", bob_agent.clone())
        .await
        .expect("bob gets his vouchers");

    assert!(
        bob_vouchers.contains(&alice_agent),
        "Alice must appear as a voucher for Bob on Bob's own conductor; vouchers: {bob_vouchers:?}"
    );

    // Bob's vouched capacity must reflect Alice's vouch.
    let bob_capacity: f64 = conductors[1]
        .call_fallible(&bob_cell.zome("transaction"), "get_vouched_capacity", bob_agent.clone())
        .await
        .expect("bob gets vouched capacity");

    assert!(
        bob_capacity >= 400.0, // 500 - rounding tolerance
        "Bob's vouched capacity must reflect Alice's 500-unit vouch; got {bob_capacity}"
    );

    // Alice can also verify she has vouched for Bob (SponsorToVouch link check).
    let alice_vouched_for_bob: bool = conductors[0]
        .call_fallible(&alice_cell.zome("transaction"), "get_my_vouched_for_agent", bob_agent.clone())
        .await
        .expect("alice checks if she vouched for bob");

    assert!(alice_vouched_for_bob, "Alice must see herself as having vouched for Bob");
}
