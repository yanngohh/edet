use super::*;

/// Wallet: update by non-owner should be rejected
#[tokio::test(flavor = "multi_thread")]
async fn test_wallet_update_by_non_owner_rejected() {
    let (conductor, apps) = setup_multi_agent(2).await;
    let alice_cell = apps[0].cells()[0].clone();
    let bob_cell = apps[1].cells()[0].clone();

    let alice_agent: AgentPubKey = alice_cell.agent_pubkey().clone();

    let (original_hash, wallet_record) = get_wallet_for_agent(&conductor, &alice_cell, alice_agent.clone())
        .await
        .expect("Should get Alice's wallet");

    if let (Some(orig_hash), Some(_record)) = (original_hash, wallet_record) {
        let update_input = UpdateWalletInput {
            original_wallet_hash: orig_hash.clone(),
            previous_wallet_hash: orig_hash,
            updated_wallet: Wallet {
                owner: alice_agent.into(),
                auto_accept_threshold: 0.1,
                auto_reject_threshold: 0.9,
                total_slashed_as_sponsor: 0.0,
                trial_tx_count: 0,
                last_trial_epoch: 0,
            },
        };

        let result: ConductorApiResult<Record> = conductor
            .call_fallible(&bob_cell.zome("transaction"), "update_wallet", update_input)
            .await;

        assert!(result.is_err(), "Bob updating Alice's wallet should be rejected");
    }
}

/// Debt contract: creditor-is-debtor should be rejected
#[tokio::test(flavor = "multi_thread")]
async fn test_debt_contract_creditor_is_debtor_rejected() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();
    let agent: AgentPubKey = cell.agent_pubkey().clone();

    let contract_input = CreateDebtContractInput {
        amount: 50.0,
        creditor: agent.clone().into(),
        debtor: agent.clone().into(),
        transaction_hash: ActionHash::from_raw_36(vec![0u8; 36]),
        is_trial: false,
    };

    let result: ConductorApiResult<Record> = conductor
        .call_fallible(&cell.zome("transaction"), "create_debt_contract", contract_input)
        .await;

    assert!(result.is_err(), "Self-referential debt contract should be rejected");
}
