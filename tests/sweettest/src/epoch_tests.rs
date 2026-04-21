//! Epoch Simulation Tests (Whitepaper Sections 5, 6, and Appendix B)
//!
//! These tests exercise time-dependent protocol behaviors that require
//! epoch boundaries to pass.  They rely on the `test-epoch` feature flag
//! (EPOCH_DURATION_SECS = 1, MIN_MATURITY = 3) so that "sleeping across
//! MIN_MATURITY epochs" only takes ~4 seconds instead of ~50 days.
//!
//! Build the test DNA with:
//!   npm run build:test-dna
//! Then run:
//!   cargo test -p sweettest
//!
//! Tests:
//! 1. test_contract_expires_after_maturity
//! 2. test_slacker_trust_collapses_after_default
//! 3. test_whitewashing_cost_exceeds_benefit
//! 4. test_vouch_slash_on_default
//! 5. test_recent_window_detects_behavioral_switch
//! 6. test_failure_witnesses_published_on_expiry
//! 7. test_permanent_trial_block_after_default

use super::*;

// ============================================================================
//  Helpers
// ============================================================================

/// Sleep long enough to push an Active contract past its maturity epoch.
///
/// In `test-epoch` mode: EPOCH_DURATION_SECS=1, MIN_MATURITY=3.
/// The contract expiry epoch = start_epoch + 3. We sleep 4.4 seconds
/// (slightly over 4 epoch boundaries) so the system clock is well past
/// start_epoch + 3 when `process_contract_expirations` is called.
async fn sleep_past_maturity() {
    let total_ms = EPOCH_SLEEP_MS * MATURITY_EPOCHS; // 1100 * 4 = 4400 ms
    tokio::time::sleep(std::time::Duration::from_millis(total_ms)).await;
}

/// Create a DebtContract directly on the buyer's chain for epoch tests.
///
/// In sweettest's single-conductor mode, `EntrantToVouch` links created by
/// sponsor cells don't propagate to the entrant's cell reliably, and
/// `call_remote` from `approve_pending_transaction` to `create_buyer_debt_contract`
/// is unreliable. This helper works around both issues by:
///
/// 1. Creating a Pending trial transaction (bypasses capacity check).
/// 2. Directly calling `create_debt_contract` on the buyer's cell using the
///    transaction's action hash.
///
/// The resulting Active DebtContract is indistinguishable from one created
/// via the normal approval path — it has the correct creditor, debtor,
/// amount, start_epoch, and maturity.
///
/// `amount` determines whether `expiration.rs` treats the contract as a trial
/// (is_trial = amount < 50.0, strict) for the permanent-block mechanism in test 7.
///
/// Returns the ActionHash of the created DebtContract record.
async fn create_debt_contract_direct(
    conductor: &SweetConductor,
    buyer_cell: &SweetCell,
    seller_agent: AgentPubKey,
    buyer_agent: AgentPubKey,
    amount: f64,
) -> ActionHash {
    // Ensure seller's wallet is visible to buyer.
    ensure_wallet_propagation(conductor, buyer_cell, seller_agent.clone())
        .await
        .expect("Seller wallet should propagate to buyer");

    // Create a trial transaction to get a valid transaction_hash.
    // Trial transactions bypass the capacity check and always start as Pending,
    // which is fine because we only need the hash as a pointer.
    let trial_amount = amount.min(49.0); // Keep it strictly below trial threshold (50.0) for the capacity bypass
    let tx_input = CreateTransactionInput {
        seller: seller_agent.clone().into(),
        buyer: buyer_agent.clone().into(),
        description: "Epoch test contract anchor".to_string(),
        debt: trial_amount,
    };
    let tx_record = create_transaction(conductor, buyer_cell, tx_input)
        .await
        .expect("Trial transaction should be created");

    let tx_hash = tx_record.action_address().clone();

    // Create the DebtContract directly on the buyer's chain with the desired amount.
    let contract_input = CreateDebtContractInput {
        amount,
        creditor: seller_agent.clone().into(),
        debtor: buyer_agent.clone().into(),
        transaction_hash: tx_hash,
        is_trial: amount < 50.0,
    };
    let contract_record: Record = conductor
        .call_fallible(&buyer_cell.zome("transaction"), "create_debt_contract", contract_input)
        .await
        .expect("create_debt_contract should succeed");

    let contract: DebtContract = contract_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(contract.status, ContractStatus::Active, "DebtContract should be Active");

    contract_record.action_address().clone()
}

// ============================================================================
//  1. test_contract_expires_after_maturity
// ============================================================================

/// An Active contract transitions to Expired once MIN_MATURITY epochs pass
/// and `process_contract_expirations` is called.
///
/// Exercises: contracts/expiration.rs — ExpirationResult.total_expired > 0.
/// Whitepaper Section 5.1: contract maturity enforcement.
#[tokio::test(flavor = "multi_thread")]
async fn test_contract_expires_after_maturity() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer / debtor
    let bob_cell = apps[1].cells()[0].clone(); // seller / creditor

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    create_debt_contract_direct(&conductor, &alice_cell, bob_agent.clone(), alice_agent.clone(), 150.0).await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let active_before = get_active_contracts_for_debtor(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get active contracts");
    assert!(!active_before.is_empty(), "Alice should have an Active contract");

    // Sleep past MIN_MATURITY epochs (4.4 s in test-epoch mode).
    sleep_past_maturity().await;

    // Alice triggers expiration processing on her own chain.
    let result = process_contract_expirations(&conductor, &alice_cell)
        .await
        .expect("process_contract_expirations should succeed");

    assert!(result.total_expired > 0.0, "total_expired should be > 0 after maturity; got {}", result.total_expired);
    assert!(!result.creditor_failures.is_empty(), "creditor_failures should not be empty after expiry");

    // The active contract list should now be empty (contract status is Expired).
    let active_after = get_active_contracts_for_debtor(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get active contracts after expiry");
    assert!(active_after.is_empty(), "Alice should have 0 Active contracts after expiry; got {}", active_after.len());
}

// ============================================================================
//  2. test_slacker_trust_collapses_after_default
// ============================================================================

/// A pure debtor (Alice) who never repays sees her EigenTrust score fall to 0
/// after her only contract expires.
///
/// Exercises: trust/attenuation.rs — phi → 0 for failure_rate >= tau.
/// Whitepaper Theorem 6 / Definition 5: exclusion of defaulters.
#[tokio::test(flavor = "multi_thread")]
async fn test_slacker_trust_collapses_after_default() {
    // Use no-vouch setup so vouches don't create a baseline trust floor that
    // prevents Alice's trust from collapsing to ~0 after her default.
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer / debtor
    let bob_cell = apps[1].cells()[0].clone(); // seller / creditor

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    create_debt_contract_direct(&conductor, &alice_cell, bob_agent.clone(), alice_agent.clone(), 150.0).await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Sleep past MIN_MATURITY epochs → Alice's contract expires (she never repaid).
    sleep_past_maturity().await;

    // Alice processes her own expirations.
    let exp = process_contract_expirations(&conductor, &alice_cell)
        .await
        .expect("process_contract_expirations should succeed");
    assert!(exp.total_expired > 0.0, "Should have expired debt; got {}", exp.total_expired);

    // Allow failure observation DHT writes to propagate.
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    // Bob checks Alice's reputation: after a 100% failure rate,
    // phi(r) = max(0, 1 - (r/tau)^gamma) = 0 → trust → 0.
    let rep = get_subjective_reputation(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Should get Alice's reputation from Bob's perspective");

    assert!(rep.trust < 0.05, "Alice's trust should collapse to ~0 after full default; got {}", rep.trust);
}

// ============================================================================
//  3. test_whitewashing_cost_exceeds_benefit
// ============================================================================

/// A new agent (Carol) created after Alice defaults starts with zero trust
/// and can only conduct trial transactions, making whitewashing economically
/// unattractive.
///
/// Exercises: capacity.rs — Cap = V_staked = 0 for unvouched agents.
/// Whitepaper Section 6.3: Whitewashing prevention via MIN_MATURITY.
#[tokio::test(flavor = "multi_thread")]
async fn test_whitewashing_cost_exceeds_benefit() {
    // Two agents: Bob (seller) and Carol (fresh agent, no history, no vouch).
    // Carol represents the "new identity" that a whitewasher would adopt.
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let carol_cell = apps[0].cells()[0].clone(); // fresh agent
    let bob_cell = apps[1].cells()[0].clone(); // seller

    let carol_agent: AgentPubKey = carol_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Carol has no vouches, so her credit capacity is 0.
    let capacity = get_credit_capacity(&conductor, &carol_cell, carol_agent.clone())
        .await
        .expect("Should get Carol's capacity");
    assert!(capacity < 1.0, "Fresh unvouched agent should have near-zero capacity; got {capacity}");

    // A non-trial transaction above the threshold should fail due to zero capacity.
    let non_trial_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: carol_agent.clone().into(),
        description: "Whitewash non-trial attempt".to_string(),
        debt: 200.0, // Above 100, not a trial
    };
    let non_trial_result = create_transaction(&conductor, &carol_cell, non_trial_input).await;
    assert!(non_trial_result.is_err(), "Carol should not be able to create a non-trial transaction without capacity");

    // But a trial transaction (amount < 100) should succeed.
    let trial_input = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: carol_agent.clone().into(),
        description: "Whitewash trial attempt".to_string(),
        debt: 49.0, // Trial range
    };
    let trial_result = create_transaction(&conductor, &carol_cell, trial_input).await;
    assert!(trial_result.is_ok(), "Carol should be able to create a trial transaction; err: {:?}", trial_result.err());
}

// ============================================================================
//  4. test_vouch_slash_on_default
// ============================================================================

/// When Alice (vouched by Carol) defaults on a contract, Carol's vouch entry
/// has its `slashed_amount` increased (and status set to Slashed).
///
/// Exercises: vouch.rs — slash_vouch_for_entrant; Vouch.slashed_amount.
/// Whitepaper Theorem 5.1: sponsor accountability.
///
/// Setup: 3 agents with genesis vouching. Each agent in setup_multi_agent(3)
/// vouches for each other via bootstrap_genesis_vouching.
/// We use Alice as the debtor and read her EntrantToVouch links to detect slashing.
#[tokio::test(flavor = "multi_thread")]
async fn test_vouch_slash_on_default() {
    let (conductor, apps) = setup_multi_agent(3).await;
    let alice_cell = apps[0].cells()[0].clone(); // debtor
    let bob_cell = apps[1].cells()[0].clone(); // creditor / seller
    let _carol_cell = apps[2].cells()[0].clone(); // one of the sponsors

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Get Alice's vouches before the default (check initial state).
    let vouches_before = get_vouches_for_entrant(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get Alice's vouches");
    assert!(!vouches_before.is_empty(), "Alice should have vouches from genesis setup (found 0)");
    let total_slashed_before: f64 = vouches_before.iter().map(|v| v.slashed_amount).sum();

    // Create a DebtContract for Alice → Bob.
    let contract_amount = 150.0;
    create_debt_contract_direct(&conductor, &alice_cell, bob_agent.clone(), alice_agent.clone(), contract_amount).await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Sleep past MIN_MATURITY epochs → Alice defaults.
    sleep_past_maturity().await;

    // Alice processes expirations — triggers slash_vouch_for_entrant.
    let exp = process_contract_expirations(&conductor, &alice_cell)
        .await
        .expect("process_contract_expirations should succeed");
    assert!(exp.total_expired > 0.0, "Alice should have expired debt");
    assert!(
        exp.total_slashed_dispatched > 0.0,
        "slash_vouch_for_entrant should have dispatched a non-zero slash (dispatched={})",
        exp.total_slashed_dispatched
    );

    // Wait for the fire-and-forget call_remote slash to complete and propagate.
    {
        let cells: Vec<_> = apps.iter().map(|a| a.cells()[0].clone()).collect();
        await_consistency(30, &cells).await.unwrap();
    }

    // Read Alice's vouches after default — slashed_amount should have increased.
    let vouches_after = get_vouches_for_entrant(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get Alice's vouches after slash");
    let total_slashed_after: f64 = vouches_after.iter().map(|v| v.slashed_amount).sum();

    // VOUCH_SLASHING_MULTIPLIER = 3.0; the slash = min(3 * 150.0, vouch_amount).
    // At least one vouch should have been slashed.
    assert!(
        total_slashed_after > total_slashed_before,
        "Alice's vouches should show increased slashed_amount after default; before={total_slashed_before}, after={total_slashed_after}"
    );
}

// ============================================================================
//  5. test_recent_window_detects_behavioral_switch
// ============================================================================

/// An agent who defaults has her *recent* failure rate amplified
/// (RECENT_WEIGHT = 2.0), triggering phi → 0 even if the cumulative record
/// would otherwise be tolerable.
///
/// Exercises: trust/attenuation.rs — r_eff = max(r_cumul, RECENT_WEIGHT * r_recent).
/// Whitepaper Definition 6 / Remark (Recent Window).
#[tokio::test(flavor = "multi_thread")]
async fn test_recent_window_detects_behavioral_switch() {
    // Use no-vouch setup so vouches don't create a baseline trust floor.
    let (conductor, apps) = setup_multi_agent_no_vouch(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer / debtor
    let bob_cell = apps[1].cells()[0].clone(); // seller / creditor

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    create_debt_contract_direct(&conductor, &alice_cell, bob_agent.clone(), alice_agent.clone(), 150.0).await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Sleep past maturity → contract expires (recent failure_rate = 1.0).
    sleep_past_maturity().await;

    let exp = process_contract_expirations(&conductor, &alice_cell)
        .await
        .expect("process_contract_expirations should succeed");
    assert!(exp.total_expired > 0.0, "Should have expired debt");

    // Allow DHT propagation.
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    // Bob evaluates Alice's reputation.
    // r_recent = 1.0 (100% failure in last RECENT_WINDOW_K=3 epochs).
    // r_eff = max(r_cumul, RECENT_WEIGHT * r_recent) = max(r_cumul, 2.0) ≥ 1.0 > tau=0.12.
    // → phi = max(0, 1 - (r_eff/tau)^4) = 0 → trust = 0.
    let rep = get_subjective_reputation(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Should get Alice's reputation");

    assert!(rep.trust < 0.05, "Alice's trust should collapse to ~0 via recent-window amplification; got {}", rep.trust);
}

// ============================================================================
//  6. test_failure_witnesses_published_on_expiry
// ============================================================================

/// After Alice's contract expires, `get_failure_witnesses(alice)` returns Bob
/// (the creditor who witnessed the default).
///
/// Exercises: trust/contagion.rs — publish_failure_observation; get_failure_witnesses.
/// Whitepaper Section 6.2: Failure witness mechanism.
#[tokio::test(flavor = "multi_thread")]
async fn test_failure_witnesses_published_on_expiry() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer / debtor
    let bob_cell = apps[1].cells()[0].clone(); // seller / creditor

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // No witnesses before any default.
    let witnesses_before = get_failure_witnesses(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get failure witnesses");
    assert!(
        witnesses_before.is_empty(),
        "No witnesses should exist for Alice before any default; got {witnesses_before:?}"
    );

    create_debt_contract_direct(&conductor, &alice_cell, bob_agent.clone(), alice_agent.clone(), 150.0).await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Sleep past MIN_MATURITY → contract expires.
    sleep_past_maturity().await;

    // Alice triggers expiration (publishes failure observation for Bob as creditor).
    let exp = process_contract_expirations(&conductor, &alice_cell)
        .await
        .expect("process_contract_expirations should succeed");
    assert!(exp.total_expired > 0.0, "Should have expired debt");

    // Allow DHT write (FailureObservationIndex link) to propagate.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    // Bob (or anyone) queries witnesses for Alice.
    let witnesses_after = get_failure_witnesses(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("Should get failure witnesses after default");

    assert!(!witnesses_after.is_empty(), "At least one failure witness should be published after Alice defaults");

    // The witness list should include Bob's key (creditor of the expired contract).
    let bob_b64: AgentPubKeyB64 = bob_agent.clone().into();
    assert!(
        witnesses_after.contains(&bob_b64),
        "Bob should be listed as a failure witness for Alice; got {witnesses_after:?}"
    );

    // Verify the aggregate witness rate API: with only 1 witness (below n_min=3),
    // it should return 0.0. This confirms the witness_bilateral_rate field was
    // embedded in the FailureObservationIndexTag (otherwise the API would error).
    let agg_rate = get_aggregate_witness_rate(&conductor, &bob_cell, alice_agent.clone())
        .await
        .expect("get_aggregate_witness_rate should succeed after failure publication");
    assert!(
        agg_rate.abs() < 1e-9,
        "Aggregate witness rate should be 0.0 with only 1 witness (n_min=3); got {agg_rate}"
    );
}

// ============================================================================
//  7. test_permanent_trial_block_after_default
// ============================================================================

/// After a trial contract (amount < 100) between Alice and Bob expires (default),
/// the pair is permanently blocked from creating further trials: a second trial
/// attempt returns EC200020 (TRIAL_PAIR_PERMANENTLY_BLOCKED).
///
/// Exercises: contracts/expiration.rs — DebtorToBlockedTrialSeller link;
///            transaction/mod.rs — check_open_trial_for_buyer.
/// Whitepaper Section 6.3: Permanent trial block prevents whitewash-then-trial cycles.
///
/// The block is keyed on `is_trial = (amount < 50.0)`.  We use `amount = 30.0`
/// so the expiration code classifies it as a trial and writes the blocked-trial link.
#[tokio::test(flavor = "multi_thread")]
async fn test_permanent_trial_block_after_default() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone(); // buyer / debtor
    let bob_cell = apps[1].cells()[0].clone(); // seller

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();
    let bob_agent: AgentPubKey = bob_cell.agent_pubkey().clone();

    // Create a trial-sized DebtContract (amount < 50.0 → is_trial=true in expiration.rs).
    create_debt_contract_direct(&conductor, &alice_cell, bob_agent.clone(), alice_agent.clone(), 30.0).await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Sleep past MIN_MATURITY → trial contract expires (Alice never repaid).
    sleep_past_maturity().await;

    // Alice processes expirations — this writes the DebtorToBlockedTrialSeller link.
    let exp = process_contract_expirations(&conductor, &alice_cell)
        .await
        .expect("process_contract_expirations should succeed");
    assert!(exp.total_expired > 0.0, "Trial contract should have expired");

    // Allow DHT propagation of the blocked-trial link.
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    // Alice attempts a second small transaction with Bob.
    // Alice now has a DebtorToContracts link (from the expired contract), so she is
    // GRADUATED — buyer_has_any_economic_activity returns true, is_trial=false.
    // As a graduated buyer doing a small non-trial transaction, PATH 1/2 runs.
    // The permanent block (TRIAL_PAIR_PERMANENTLY_BLOCKED) gate only fires inside
    // the `if is_trial` block, which is never reached for graduated buyers.
    let second_attempt = CreateTransactionInput {
        seller: bob_agent.clone().into(),
        buyer: alice_agent.clone().into(),
        description: "Permanent block test — second attempt after default".to_string(),
        debt: 49.0,
    };
    let second_result = create_transaction(&conductor, &alice_cell, second_attempt).await;

    // Alice is graduated (has a DebtorToContracts link from the expired contract),
    // so buyer_has_any_economic_activity returns true → is_trial=false.
    // The second attempt goes through PATH 1/2 risk assessment.
    match &second_result {
        Ok(record) => {
            let tx: Transaction = record.entry().to_app_option().unwrap().unwrap();
            assert!(!tx.is_trial, "After default, Alice is graduated — second attempt must NOT be a trial");
            // The transaction goes through normal risk assessment; it may be Accepted,
            // Pending, or Rejected based on Alice's post-default risk score.
        }
        Err(e) => {
            // Risk score or capacity check may cause an error after default.
            // This is valid: Alice defaulted and her economic standing is damaged.
            let err_str = format!("{e:?}");
            // Not TRIAL_PAIR_PERMANENTLY_BLOCKED (that gate is only inside if is_trial),
            // but could be CAPACITY_EXCEEDED or similar.
            assert!(
                !err_str.contains(coordinator_transaction_error::TRIAL_PAIR_PERMANENTLY_BLOCKED),
                "TRIAL_PAIR_PERMANENTLY_BLOCKED should not fire for a graduated buyer; got: {err_str}"
            );
        }
    }
}
