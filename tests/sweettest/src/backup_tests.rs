//! Backup + recovery round-trip tests.
//!
//! These tests validate invariants of the identity backup flow described in
//! PLAN.md §9 and §15:
//!
//!   1. The list of source-chain records produced by a live conductor can
//!      be serialised through the msgpack layer the UI uses.
//!   2. The `graft_records_onto_source_chain` API accepts a valid chain
//!      dumped from our DNA. This doubles as proof that the edet integrity
//!      zomes don't depend on wall-clock timing (see §3 validator audit).
//!   3. The graft API rejects tampered chains (negative case).
//!
//! The "restore on a different device" end-to-end scenario (two conductor
//! processes sharing the same ed25519 seed) is exercised by the Tauri smoke
//! test; sweettest does not expose the cross-process key-handoff wiring.

use std::sync::Arc;

use holochain::conductor::Conductor;
use holochain::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{create_transaction, setup_multi_agent, setup_single_agent, CreateTransactionInput, Wallet};

/// Minimal mirror of the TypeScript `BackupPayload` msgpack structure. Keeping
/// it here (rather than pulling from a shared crate) keeps the test self-
/// contained and makes schema changes obvious in review.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupPayloadV1 {
    schema_version: u32,
    created_at_us: i64,
    app_id: String,
    dna_hash: Vec<u8>,
    agent_pub_key: Vec<u8>,
    source_chain: Vec<Record>,
}

/// Verify that a freshly-onboarded agent produces a non-empty source chain
/// that survives a round-trip through the backup payload format.
#[tokio::test(flavor = "multi_thread")]
async fn test_backup_payload_roundtrip_msgpack() {
    let (conductor, app) = setup_single_agent().await;
    let cell = app.cells()[0].clone();
    let agent = cell.agent_pubkey().clone();

    // Load the agent's wallet record (authored during zome init).
    let (wallet_hash, wallet_record): (Option<ActionHash>, Option<Record>) = conductor
        .call(&cell.zome("transaction"), "get_wallet_for_agent", agent.clone())
        .await;
    assert!(wallet_hash.is_some() && wallet_record.is_some(), "fresh agent should have a wallet created at init");
    let wallet_record = wallet_record.unwrap();

    // Sanity: the Wallet entry decodes to the right owner.
    let wallet: Wallet = wallet_record.entry().to_app_option().unwrap().unwrap();
    assert_eq!(
        wallet.owner,
        holo_hash::AgentPubKeyB64::from(agent.clone()),
        "wallet owner should match the agent's pubkey",
    );

    let payload = BackupPayloadV1 {
        schema_version: 1,
        created_at_us: 1_700_000_000_000_000,
        app_id: "edet".to_string(),
        dna_hash: cell.dna_hash().get_raw_39().to_vec(),
        agent_pub_key: agent.get_raw_39().to_vec(),
        source_chain: vec![wallet_record.clone()],
    };

    let encoded = rmp_serde::to_vec_named(&payload).expect("msgpack encode");
    let decoded: BackupPayloadV1 = rmp_serde::from_slice(&encoded).expect("msgpack decode round-trips");

    assert_eq!(decoded.schema_version, 1);
    assert_eq!(decoded.app_id, "edet");
    assert_eq!(decoded.agent_pub_key, payload.agent_pub_key);
    assert_eq!(decoded.source_chain.len(), 1, "source chain length should round-trip");
    let decoded_wallet: Wallet = decoded.source_chain[0]
        .entry()
        .to_app_option()
        .unwrap()
        .expect("wallet entry survives msgpack round-trip");
    assert_eq!(decoded_wallet.owner, wallet.owner);
}

/// Guard test: an empty payload encodes/decodes cleanly. This covers the
/// "initial backup" case where onboarding fires before any non-init records
/// exist.
#[tokio::test(flavor = "multi_thread")]
async fn test_backup_payload_empty_chain_roundtrip() {
    let payload = BackupPayloadV1 {
        schema_version: 1,
        created_at_us: 0,
        app_id: "edet".to_string(),
        dna_hash: vec![0x84, 0x24],
        agent_pub_key: vec![0x84, 0x20, 0x24],
        source_chain: vec![],
    };

    let encoded = rmp_serde::to_vec_named(&payload).expect("msgpack encode");
    let decoded: BackupPayloadV1 = rmp_serde::from_slice(&encoded).expect("msgpack decode round-trips");

    assert!(decoded.source_chain.is_empty());
    assert_eq!(decoded.schema_version, 1);
}

/// Full GraftRecords round-trip for a live edet cell.
///
/// Simulates what the Tauri shell does on identity recovery:
///   1. Agent authors a few real actions (wallet + 2 transactions).
///   2. We dump the full authored chain via `SourceChain::query`.
///   3. We re-graft the exact same chain. The graft API should accept it as
///      a no-op (the pivot logic detects existing == incoming).
///   4. Post-graft, the chain is still readable and still queryable.
///
/// This also functions as a regression test for PLAN.md §3: if any edet
/// integrity validator starts consulting wall-clock time, this graft would
/// fail on `validate=true` because the replayed actions carry historic
/// timestamps.
#[tokio::test(flavor = "multi_thread")]
async fn test_graft_records_roundtrip_noop() {
    // Two agents so we have a realistic buyer/seller pair.
    let (mut conductor, apps) = setup_multi_agent(2).await;
    let buyer_cell = apps[0].cells()[0].clone();
    let seller_cell = apps[1].cells()[0].clone();
    let buyer = buyer_cell.agent_pubkey().clone();
    let seller = seller_cell.agent_pubkey().clone();

    // Author a single buyer transaction. Multiple trial transactions with the
    // same seller would hit `OPEN_TRIAL_EXISTS` (EC200019) — for the graft
    // round-trip the shape of the chain (≥ wallet + one real action) is
    // sufficient to exercise the validation path.
    let input = CreateTransactionInput {
        seller: seller.clone().into(),
        buyer: buyer.clone().into(),
        description: "graft round-trip tx".to_string(),
        debt: 1.0,
    };
    create_transaction(&conductor, &buyer_cell, input)
        .await
        .expect("create_transaction");

    // Dump the buyer's authored chain.
    let dna_hash = buyer_cell.dna_hash().clone();
    let source_chain = conductor.get_agent_source_chain(&buyer, &dna_hash).await;
    let records = source_chain.query(ChainQueryFilter::new()).await.expect("chain query");
    assert!(records.len() >= 2, "expected at least 2 records (wallet + 1 tx), got {}", records.len());
    let chain_len_before = records.len();

    // Round-trip the records through the on-wire msgpack format the UI uses
    // for backup bundles, proving the type bounds match.
    let encoded = rmp_serde::to_vec_named(&records).expect("encode records");
    let decoded: Vec<Record> = rmp_serde::from_slice(&encoded).expect("decode records round-trips");
    assert_eq!(decoded.len(), records.len());

    // Graft the exact same chain back. With `validate=true` this exercises
    // the full integrity-zome pipeline on replayed actions, which is the
    // property PLAN.md §3 audited for.
    let cell_id = CellId::new(dna_hash.clone(), buyer.clone());
    let handle: Arc<Conductor> = conductor.raw_handle();
    handle
        .graft_records_onto_source_chain(cell_id, true, decoded)
        .await
        .expect("graft should accept a verbatim re-play of the agent's own chain");

    // Post-graft: chain is still readable and unchanged.
    let source_chain_after = conductor.get_agent_source_chain(&buyer, &dna_hash).await;
    let records_after = source_chain_after
        .query(ChainQueryFilter::new())
        .await
        .expect("post-graft chain query");
    assert_eq!(records_after.len(), chain_len_before, "graft should not have duplicated records");
}

/// Negative: graft must reject records signed with a tampered signature.
///
/// We produce a valid chain, flip a byte inside one of the signatures, and
/// verify that `validate=true` refuses the graft. Catches regressions where
/// someone might relax signature validation.
#[tokio::test(flavor = "multi_thread")]
async fn test_graft_records_rejects_tampered_signature() {
    let (mut conductor, apps) = setup_multi_agent(2).await;
    let buyer_cell = apps[0].cells()[0].clone();
    let seller_cell = apps[1].cells()[0].clone();
    let buyer = buyer_cell.agent_pubkey().clone();
    let seller = seller_cell.agent_pubkey().clone();

    // One real buyer transaction to have something to tamper.
    create_transaction(
        &conductor,
        &buyer_cell,
        CreateTransactionInput {
            seller: seller.clone().into(),
            buyer: buyer.clone().into(),
            description: "tamper test".into(),
            debt: 1.0,
        },
    )
    .await
    .expect("create_transaction");

    let dna_hash = buyer_cell.dna_hash().clone();
    let records = conductor
        .get_agent_source_chain(&buyer, &dna_hash)
        .await
        .query(ChainQueryFilter::new())
        .await
        .expect("chain query");

    // Flip the first byte of the most-recent record's signature.
    let mut tampered = records.clone();
    let last = tampered.last_mut().expect("at least one record");
    let mut sig_bytes = last.signed_action.signature.0.to_vec();
    sig_bytes[0] ^= 0xff;
    let mut fixed = [0u8; 64];
    fixed.copy_from_slice(&sig_bytes);
    last.signed_action.signature = Signature(fixed);

    let cell_id = CellId::new(dna_hash.clone(), buyer.clone());
    let handle: Arc<Conductor> = conductor.raw_handle();
    let res = handle.graft_records_onto_source_chain(cell_id, true, tampered).await;
    assert!(res.is_err(), "graft with a tampered signature should be rejected",);
}
