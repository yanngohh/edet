//! Permanent regression guards for the two Critical + one High findings of
//! Each test here
//! is the INVERSE of a proof-of-concept in `doc/adversarial-poc-reference.rs.txt`
//! that PASSED (i.e. the attack succeeded) against the unfixed tree at
//! `7954ee3`: same attack setup, asserting the attack is now refused. If any
//! of these ever fails again, the corresponding exploit is live again.
//!
//! Run in isolation: `cargo test -p edet-node --test regression_adversarial`.

use edet_node::block::{counter_nonce, dev_seed, pubkey_of, sign_tx, Block, DEV_CHAIN_ID};
use edet_node::replica::{Replica, ReplicaError};
use edet_state::errors::{Error, ET_TX_REPLAY};
use edet_state::tx::Tx;
use edet_state::types::Party;
use edet_state::State;

/// Real Ed25519 keys (not placeholder bytes) so `commit_block`'s
/// authenticated path — the one every one of these tests exercises — accepts
/// the constructed transactions instead of refusing them for a reason
/// unrelated to what's under test.
fn genesis(n: u8) -> State {
    let mut st = State::default();
    for i in 0..n {
        let id = st
            .add_underwriter(vec![pubkey_of(&dev_seed(i))], 25_000.0)
            .expect("unique genesis");
        st.set_consensus_key(id, edet_node::block::pubkey_of(&edet_node::block::dev_consensus_seed(id as u8)))
            .expect("consensus key");
        st.set_genesis_validator(id, 1).expect("validator");
    }
    st
}

/// The epoch bomb (
/// poc1_block_timestamp_is_unbounded_and_epoch_advance_is_linear_in_it):
/// a decided block carrying `time_secs = u64::MAX` demands ~2.135e14
/// epoch-closes — ~18.7 years of CPU per PoC1's own measurement — because
/// `State::begin_block` closed one epoch per loop iteration with no bound on
/// how many. If `MAX_EPOCH_ADVANCE_PER_BLOCK` (or the clamp that reads it) is
/// ever removed from `begin_block`, this test stops returning and times out
/// instead of passing.
#[test]
fn a_u64_max_timestamp_returns_promptly() {
    let mut r = Replica::new(genesis(4));
    let app0 = r.app_hash();
    let bomb = Block { height: 1, time_secs: u64::MAX, app_hash: app0, txs: Vec::new() };

    let t0 = std::time::Instant::now();
    let result = r.commit_block(&bomb);
    let elapsed = t0.elapsed();

    assert!(result.is_ok(), "the clamp must bound cost, not reject an otherwise well-formed empty block: {result:?}");
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "REGRESSION: applying time_secs=u64::MAX took {elapsed:?} — the epoch-advance clamp appears to be gone"
    );
}

/// The rewind, from the same construction: "a
/// block may go BACKWARDS"): a block whose `time_secs` is below the last
/// committed block's commits cleanly and rewinds `last_time_secs` unless it is
/// refused. If
/// the monotonicity gate in `Replica::commit_block` is ever removed or
/// weakened, this starts accepting the rewinding block and advancing height.
#[test]
fn a_rewind_below_parent_time_is_refused_and_state_unchanged() {
    let mut r = Replica::new(genesis(2));
    let app0 = r.app_hash();
    r.commit_block(&Block { height: 1, time_secs: 100 * 86_400, app_hash: app0, txs: Vec::new() })
        .expect("forward commit establishes a parent time");
    let height_before = r.height;
    let epoch_before = r.state.epoch;
    let last_time_before = r.last_time_secs;

    let app1 = r.app_hash();
    let back = Block { height: 2, time_secs: 0, app_hash: app1, txs: Vec::new() };
    let err = r
        .commit_block(&back)
        .expect_err("REGRESSION: a block rewinding time_secs below the parent must be refused by commit_block");
    assert!(matches!(err, ReplicaError::NonMonotonicTime { .. }), "unexpected error: {err:?}");

    assert_eq!(r.height, height_before, "height must not advance on a refused rewinding block");
    assert_eq!(r.state.epoch, epoch_before, "epoch must not change on a refused rewinding block");
    assert_eq!(r.last_time_secs, last_time_before, "last_time_secs must not rewind on a refused block");
}

/// The replay (
/// poc2_replaying_a_cosigned_settlement_discharges_four_times_what_was_agreed):
/// a creditor co-signed exactly ONE settlement of 10 against a debt of 40; the
/// unfixed tree let the debtor resubmit the identical signed bytes and
/// discharge the debt in full. If `apply`'s replay check
/// (`state.is_applied`, keyed on the envelope id under the chain id) is ever bypassed, the second
/// commit here starts succeeding instead of returning `ET_TX_REPLAY`.
#[test]
fn a_replayed_cosigned_settle_is_refused_and_outstanding_unchanged() {
    let mut r = Replica::new(genesis(2));
    let app0 = r.app_hash();
    let accept = sign_tx(
        DEV_CHAIN_ID,
        Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        },
        counter_nonce(0),
        30,
        &[dev_seed(0), dev_seed(1)],
    )
    .expect("sign accept");
    r.commit_block(&Block { height: 1, time_secs: 86_400, app_hash: app0, txs: vec![accept] })
        .expect("commit")[0]
        .expect("the obligation is booked");
    let cid = *r.state.contracts.keys().next().expect("one contract");

    // The ONE settlement the creditor actually co-signed.
    let settle = sign_tx(
        DEV_CHAIN_ID,
        Tx::Settle { contract: cid, amount: 10.0 },
        counter_nonce(1),
        30,
        &[dev_seed(0), dev_seed(1)],
    )
    .expect("sign settle");
    let app1 = r.app_hash();
    let first = r
        .commit_block(&Block { height: 2, time_secs: 86_400, app_hash: app1, txs: vec![settle.clone()] })
        .expect("commit");
    first[0].expect("the genuine settlement applies");
    assert_eq!(r.state.contracts[&cid].outstanding, edet_state::State::to_minor(30.0), "one agreed settlement of 10");

    // The debtor resubmits the SAME signed bytes.
    let app2 = r.app_hash();
    let replayed = r
        .commit_block(&Block { height: 3, time_secs: 86_400, app_hash: app2, txs: vec![settle] })
        .expect("commit");
    assert_eq!(
        replayed[0],
        Err(Error(ET_TX_REPLAY)),
        "REGRESSION: replaying the identical co-signed Settle must return ET_TX_REPLAY, not apply"
    );
    assert_eq!(
        r.state.contracts[&cid].outstanding,
        edet_state::State::to_minor(30.0),
        "REGRESSION: outstanding must be unchanged by the replay"
    );
}

/// The in-block replay (
/// poc4_one_block_may_carry_the_same_signed_tx_many_times): a Byzantine
/// proposer needs no mempool cooperation at all — it can pack the same
/// co-signed `Settle` into ONE block four times. The unfixed tree applied all
/// four (40 discharged by a co-signed 10) because `apply_block_to` was a
/// plain map with no in-block dedup. If replay detection is ever narrowed to
/// cross-block only, this test starts seeing all four outcomes as `Ok`.
#[test]
fn four_copies_of_the_same_signed_tx_in_one_block_apply_once() {
    let mut r = Replica::new(genesis(2));
    let app0 = r.app_hash();
    let accept = sign_tx(
        DEV_CHAIN_ID,
        Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        },
        counter_nonce(0),
        30,
        &[dev_seed(0), dev_seed(1)],
    )
    .expect("sign accept");
    r.commit_block(&Block { height: 1, time_secs: 86_400, app_hash: app0, txs: vec![accept] })
        .expect("commit");
    let cid = *r.state.contracts.keys().next().expect("one contract");

    let settle = sign_tx(
        DEV_CHAIN_ID,
        Tx::Settle { contract: cid, amount: 10.0 },
        counter_nonce(1),
        30,
        &[dev_seed(0), dev_seed(1)],
    )
    .expect("sign settle");

    // ONE block, the SAME co-signed settlement four times over.
    let app1 = r.app_hash();
    let bomb = Block {
        height: 2,
        time_secs: 86_400,
        app_hash: app1,
        txs: vec![settle.clone(), settle.clone(), settle.clone(), settle],
    };
    let outcomes = r
        .commit_block(&bomb)
        .expect("the block itself still commits — only the duplicate txs are rejected");

    assert_eq!(outcomes.len(), 4);
    assert!(outcomes[0].is_ok(), "the first copy is the genuine, never-before-seen submission");
    for (i, outcome) in outcomes.iter().enumerate().skip(1) {
        assert_eq!(
            *outcome,
            Err(Error(ET_TX_REPLAY)),
            "REGRESSION: copy {i} of the same signed tx within one block must be refused as a replay"
        );
    }
    assert_eq!(
        r.state.contracts[&cid].outstanding,
        edet_state::State::to_minor(30.0),
        "REGRESSION: four identical copies in one block must discharge only 10, not 40"
    );
}

/// The state commitment: "a block
/// commits to transactions, but not to the resulting state":
/// `Block` carried no state commitment at all, so nothing stopped a divergent
/// or forged block from committing — the failure mode was silent, permanent
/// inconsistency between replicas. If the `app_hash` check is ever removed
/// from `commit_block`, this starts accepting a block that claims an
/// arbitrary, wrong app_hash.
#[test]
fn a_wrong_app_hash_is_refused_at_commit() {
    let mut r = Replica::new(genesis(2));
    let bad = Block { height: 1, time_secs: 60, app_hash: [0xAA; 32], txs: Vec::new() };

    let err = r
        .commit_block(&bad)
        .expect_err("REGRESSION: a block claiming the wrong app_hash must be refused, not silently applied");
    assert!(matches!(err, ReplicaError::AppHashMismatch { .. }), "unexpected error: {err:?}");
    assert_eq!(r.height, 0, "a refused block must not advance height");
}

// ------------------------------------------------ what a block may cost --

/// **A block may not carry more transactions than the proposer batches.**
///
/// Nothing checked the count. The reassembly buffer allowed 4 MiB per proposal
/// — tens of thousands of envelopes — and the bond gate runs a `seed_reach`
/// max-flow per signer BEFORE dispatch, so a Byzantine proposer inside the `f`
/// the design tolerates could hand every honest validator a block whose
/// application costs minutes, every turn it got.
#[test]
fn a_block_over_the_batch_ceiling_is_refused_at_commit() {
    let mut r = Replica::new(genesis(2));
    let over = edet_node::block::MAX_TXS_PER_BLOCK + 1;
    let txs: Vec<_> = (0..over)
        .map(|i| {
            sign_tx(DEV_CHAIN_ID, Tx::MarkExpired { contract: i as u64 }, counter_nonce(i as u64), 9_999, &[])
                .expect("a crank always encodes")
        })
        .collect();
    let block = Block { height: 1, time_secs: 60, app_hash: r.app_hash(), txs };

    let err = r.commit_block(&block).expect_err("a block over the ceiling must be refused");
    assert!(
        matches!(err, ReplicaError::Refused { why: edet_node::block::BlockRefusal::TooManyTxs(n), .. } if n == over),
        "unexpected error: {err:?}"
    );
    assert_eq!(r.height, 0, "a refused block must not advance height");
}

/// **A block may not carry an envelope that authorises nothing.**
///
/// The attack, and it needs no forgery: an `Accept` naming the victim as
/// debtor, signed only by the attacker. Every signature on it VERIFIES, so the
/// pre-vote screen's authentication half passes it; it then fails at dispatch
/// with `ET-MEM-NOT_SIGNER`, which `apply` refunds and forgets — deliberately,
/// because burning the id would let anybody who saw a pending request strip a
/// co-signature off it and kill the genuine transaction for free. Measured on
/// the state crate's own harness: 1,000 applications of one such envelope under
/// ONE id spent zero allowance, encumbered zero bonds and recorded zero ids,
/// while costing every node that applied it a max-flow per signer.
#[test]
fn a_block_carrying_an_envelope_nobody_authorised_is_refused() {
    let mut r = Replica::new(genesis(3));
    // Attacker is member 0; the victim it names as debtor is member 1, who
    // signs nothing.
    let attacker = sign_tx(
        DEV_CHAIN_ID,
        Tx::Accept {
            debtor: Party::Member(1),
            creditor: Party::Member(0),
            amount: 10_000.0,
            maturity_epochs: 30,
            arb: None,
        },
        counter_nonce(1),
        9_999,
        &[dev_seed(0)],
    )
    .expect("an Accept always encodes");
    assert!(attacker.verify(DEV_CHAIN_ID), "the envelope is properly SIGNED — that is what makes it dangerous");

    let block = Block { height: 1, time_secs: 60, app_hash: r.app_hash(), txs: vec![attacker] };
    let err = r
        .commit_block(&block)
        .expect_err("an envelope authorising nothing must not reach the ledger");
    assert!(
        matches!(err, ReplicaError::Refused { why: edet_node::block::BlockRefusal::Unauthorised(0), .. }),
        "unexpected error: {err:?}"
    );
    assert_eq!(r.height, 0);

    // And the same transaction, co-signed by the party it names, is ordinary
    // traffic — the rule refuses envelopes, not amounts.
    let honest = sign_tx(
        DEV_CHAIN_ID,
        Tx::Accept {
            debtor: Party::Member(1),
            creditor: Party::Member(0),
            amount: 10_000.0,
            maturity_epochs: 30,
            arb: None,
        },
        counter_nonce(2),
        9_999,
        &[dev_seed(0), dev_seed(1)],
    )
    .expect("an Accept always encodes");
    let ok = Block { height: 1, time_secs: 60, app_hash: r.app_hash(), txs: vec![honest] };
    r.commit_block(&ok).expect("a block whose envelopes are authorised commits");
    assert_eq!(r.height, 1);
}
