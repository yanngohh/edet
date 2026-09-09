//! INDEPENDENT VERIFICATION — written by the reviewer, not by the fix agents.
//! Same attacks as doc/adversarial-poc-reference.rs.txt, re-aimed at the new
//! API. Every assertion here says "the attack FAILS". If any of these panic,
//! the corresponding fix did not land.

use edet_node::block::{dev_seed, pubkey_of, sign_tx, Block};
use edet_node::replica::{Replica, ReplicaError};
use edet_state::tx::Tx;
use edet_state::types::Party;
use edet_state::State;

const CHAIN: &str = "edet-dev";

fn genesis(n: u8) -> State {
    let mut st = State::default();
    for i in 0..n {
        let id = st.add_underwriter(vec![pubkey_of(&dev_seed(i))], 25_000.0).expect("member");
        st.set_consensus_key(id, edet_node::block::pubkey_of(&edet_node::block::dev_consensus_seed(id as u8)))
            .expect("consensus key");
        st.set_genesis_validator(id, 1).expect("validator");
    }
    st
}

fn nonce(n: u8) -> [u8; 16] {
    [n; 16]
}

/// The epoch bomb must not hang. Unclamped it demands ~18.7 years
/// of CPU for one block; this must now return in milliseconds.
#[test]
fn the_epoch_bomb_does_not_hang() {
    let mut st = genesis(4);
    let t0 = std::time::Instant::now();
    st.begin_block(u64::MAX);
    let dt = t0.elapsed();
    println!("begin_block(u64::MAX) returned in {dt:?}, epoch = {}", st.epoch);
    assert!(dt.as_secs() < 5, "ATTACK STILL LIVE: begin_block(u64::MAX) took {dt:?}");
    assert_eq!(st.epoch, 10_000, "clamped to MAX_EPOCH_ADVANCE_PER_BLOCK");
}

/// A block that rewinds the clock must be refused by the commit path.
#[test]
fn a_rewinding_block_is_refused() {
    let mut r = Replica::new(genesis(2));
    let app0 = r.app_hash();
    r.commit_block(&Block { height: 1, time_secs: 100 * 86_400, app_hash: app0, txs: Vec::new() })
        .expect("forward commit");
    let before = r.state.epoch;

    let app1 = r.app_hash();
    let err = r
        .commit_block(&Block { height: 2, time_secs: 0, app_hash: app1, txs: Vec::new() })
        .expect_err("ATTACK STILL LIVE: a rewinding block committed");
    assert!(matches!(err, ReplicaError::NonMonotonicTime { .. }), "wrong rejection: {err}");
    assert_eq!(r.height, 1, "height must not advance");
    assert_eq!(r.state.epoch, before, "state must not advance");
}

/// An equal timestamp must still be ACCEPTED. The monotonicity rule is
/// deliberately non-strict: block time is whole seconds and a lively cluster
/// decides more than one block per second, so requiring a strictly greater
/// timestamp would stall it. If this fails, the fix broke liveness.
#[test]
fn an_equal_timestamp_is_still_accepted() {
    let mut r = Replica::new(genesis(2));
    let app0 = r.app_hash();
    r.commit_block(&Block { height: 1, time_secs: 86_400, app_hash: app0, txs: Vec::new() })
        .expect("first");
    let app1 = r.app_hash();
    r.commit_block(&Block { height: 2, time_secs: 86_400, app_hash: app1, txs: Vec::new() })
        .expect("LIVENESS REGRESSION: two blocks in the same second must both commit");
    assert_eq!(r.height, 2);
}

/// The headline attack. A creditor co-signs ONE settlement of 10; the
/// debtor replays those identical bytes. Without replay protection, all 40 discharge.
#[test]
fn the_cosigned_settlement_replay_is_refused() {
    let mut r = Replica::new(genesis(2));

    let accept = sign_tx(
        CHAIN,
        Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        },
        nonce(1),
        30,
        &[dev_seed(0), dev_seed(1)],
    )
    .expect("sign");
    let app0 = r.app_hash();
    r.commit_block(&Block { height: 1, time_secs: 86_400, app_hash: app0, txs: vec![accept] })
        .expect("commit")[0]
        .expect("the obligation is booked");
    let cid = *r.state.contracts.keys().next().expect("one contract");

    let settle = sign_tx(CHAIN, Tx::Settle { contract: cid, amount: 10.0 }, nonce(2), 30, &[dev_seed(0), dev_seed(1)])
        .expect("sign");

    let app1 = r.app_hash();
    r.commit_block(&Block { height: 2, time_secs: 86_400, app_hash: app1, txs: vec![settle.clone()] })
        .expect("commit")[0]
        .expect("the genuine settlement applies");
    assert_eq!(r.state.contracts[&cid].outstanding, edet_state::State::to_minor(30.0));

    // The replay. Identical bytes, three more times. Each block's app_hash is
    // read fresh from `r` right before it's built (never assumed constant):
    // a rejected replay's block still commits (only the tx inside fails), so
    // whether state changes underneath it or not, this stays correct.
    for h in 3..=5u64 {
        let app_h = r.app_hash();
        let outcome = r
            .commit_block(&Block { height: h, time_secs: 86_400, app_hash: app_h, txs: vec![settle.clone()] })
            .expect("block commits; the TX inside must not");
        assert!(outcome[0].is_err(), "ATTACK STILL LIVE: replay {h} of a co-signed settlement was applied");
    }

    println!("after 3 replay attempts, outstanding = {}", r.state.contracts[&cid].outstanding);
    assert_eq!(
        r.state.contracts[&cid].outstanding,
        edet_state::State::to_minor(30.0),
        "ATTACK STILL LIVE: debt was discharged by replay"
    );
    assert_eq!(
        r.state.members[&0].debt_out,
        edet_state::State::to_minor(30.0),
        "ATTACK STILL LIVE: debtor cleared more than was co-signed"
    );
}

/// The proposer variant, which needs no mempool cooperation at all: the
/// same signed tx four times in ONE block.
#[test]
fn the_same_tx_four_times_in_one_block_applies_once() {
    let mut r = Replica::new(genesis(2));
    let accept = sign_tx(
        CHAIN,
        Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        },
        nonce(3),
        30,
        &[dev_seed(0), dev_seed(1)],
    )
    .expect("sign");
    let app0 = r.app_hash();
    r.commit_block(&Block { height: 1, time_secs: 86_400, app_hash: app0, txs: vec![accept] })
        .expect("commit");
    let cid = *r.state.contracts.keys().next().expect("one contract");

    let settle = sign_tx(CHAIN, Tx::Settle { contract: cid, amount: 10.0 }, nonce(4), 30, &[dev_seed(0), dev_seed(1)])
        .expect("sign");
    let app1 = r.app_hash();
    let outcomes = r
        .commit_block(&Block { height: 2, time_secs: 86_400, app_hash: app1, txs: vec![settle; 4] })
        .expect("commit");

    let applied = outcomes.iter().filter(|o| o.is_ok()).count();
    println!("4 identical txs in one block -> {applied} applied");
    assert_eq!(applied, 1, "ATTACK STILL LIVE: {applied} of 4 duplicate transactions applied");
    assert_eq!(r.state.contracts[&cid].outstanding, edet_state::State::to_minor(30.0));
}

/// The load-bearing interaction: once an id is pruned from the replay
/// cache, the validity window is what protects it. If the id is forgotten AND
/// the window has passed, a replay must be refused as EXPIRED, never applied.
#[test]
fn replay_after_cache_pruning_is_refused_as_expired() {
    let mut r = Replica::new(genesis(2));
    let accept = sign_tx(
        CHAIN,
        Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        },
        nonce(5),
        30,
        &[dev_seed(0), dev_seed(1)],
    )
    .expect("sign");
    let app0 = r.app_hash();
    r.commit_block(&Block { height: 1, time_secs: 86_400, app_hash: app0, txs: vec![accept.clone()] })
        .expect("commit");
    let contracts_after_first = r.state.contracts.len();

    // Advance well past the transaction's not_after_epoch so its id is pruned.
    let app1 = r.app_hash();
    r.commit_block(&Block { height: 2, time_secs: 200 * 86_400, app_hash: app1, txs: Vec::new() })
        .expect("advance");
    assert!(
        !r.state.is_applied(&accept.id(CHAIN).unwrap(), accept.not_after_epoch),
        "precondition: the id should have been pruned from the cache"
    );

    let app2 = r.app_hash();
    let outcome = r
        .commit_block(&Block { height: 3, time_secs: 200 * 86_400, app_hash: app2, txs: vec![accept] })
        .expect("block commits");
    assert!(outcome[0].is_err(), "ATTACK STILL LIVE: a pruned-cache replay was applied");
    assert_eq!(
        r.state.contracts.len(),
        contracts_after_first,
        "ATTACK STILL LIVE: a duplicate obligation was booked after cache pruning"
    );
}

/// A signature made for one chain must not verify on another.
#[test]
fn a_signature_is_bound_to_its_chain() {
    let stx = sign_tx(
        "chain-a",
        Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        },
        nonce(6),
        30,
        &[dev_seed(0), dev_seed(1)],
    )
    .expect("sign");
    assert!(stx.verify("chain-a"), "must verify on its own chain");
    assert!(!stx.verify("chain-b"), "ATTACK STILL LIVE: cross-chain replay is possible");
}

/// The clamp is per `begin_block` CALL, but `apply` calls
/// `begin_block` once per transaction on top of `apply_block_to`'s own call.
/// The real bound is therefore (N_txs + 1) x MAX_EPOCH_ADVANCE_PER_BLOCK.
/// This test FAILS while the amplification is live and passes once it is fixed.
#[test]
fn a_multi_tx_block_must_not_amplify_the_epoch_clamp() {
    use edet_node::block::SignedTx;

    let n_txs = 2000usize;
    let txs: Vec<SignedTx> = (0..n_txs as u64)
        .map(|i| SignedTx {
            tx: Tx::MarkExpired { contract: i },
            nonce: [0u8; 16],
            not_after_epoch: 30,
            signers: vec![],
            signatures: vec![],
        })
        .collect();

    let mut r = Replica::new(genesis(4));
    let app0 = r.app_hash();
    let block = Block { height: 1, time_secs: u64::MAX, app_hash: app0, txs };
    assert!(block.verify_txs(CHAIN), "permissionless cranks authenticate trivially — the screen sees nothing wrong");

    let t0 = std::time::Instant::now();
    let _ = r.commit_block(&block);
    let dt = t0.elapsed();

    println!("{n_txs} txs + u64::MAX -> {dt:?}, epoch = {}", r.state.epoch);
    assert!(
        r.state.epoch <= 10_000,
        "AMPLIFICATION LIVE: epoch reached {} ({}x the 10_000 ceiling)",
        r.state.epoch,
        r.state.epoch / 10_000
    );
    assert!(dt.as_secs() < 5, "AMPLIFICATION LIVE: one block took {dt:?}");
}
