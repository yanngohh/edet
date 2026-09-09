//! Replication properties: determinism across replicas, durable recovery,
//! torn-tail tolerance, and order sensitivity of the state commitment.

use edet_kernel::constants::EPOCH_SECS;
use edet_node::block::{hex32, Block, SignedTx};
use edet_node::replica::Replica;
use edet_state::tx::Tx;
use edet_state::types::{Key, Party};
use edet_state::State;

fn key(n: u8) -> Key {
    [n; 32]
}

fn genesis(n: u8) -> State {
    let mut st = State::default();
    for i in 0..n {
        let id = st.add_underwriter(vec![key(i + 1)], 25_000.0).expect("unique genesis");
        // The last founder has carried and paid a debt to each of the others,
        // so everybody has standing behind them. Placed rather than declared:
        // a stake is what a settlement leaves behind, and `place_stake` caps
        // it by what the creditor may actually confer.
        st.place_stake((n - 1) as u64, id, 2500.0);
    }
    st
}

/// Like `genesis`, but every founder is also seeded as a genesis validator
/// (power 1) — what the history tests need, since `genesis` above never
/// registers anyone in `state.validators`.
fn genesis_with_validators(n: u8) -> State {
    let mut st = State::default();
    for i in 0..n {
        let id = st.add_underwriter(vec![key(i + 1)], 25_000.0).expect("unique genesis");
        st.set_consensus_key(id, edet_node::block::pubkey_of(&edet_node::block::dev_consensus_seed(id as u8)))
            .expect("consensus key");
        st.set_genesis_validator(id, 1).expect("genesis validator");
    }
    st
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A deterministic stream of blocks mixing valid and invalid transactions.
/// Every generated tx gets its own nonce (a running counter — this test
/// cares about determinism, not unpredictability) and a validity window
/// generous enough for the whole stream, so replay/expiry never becomes the
/// (unintended) reason a generated transaction is rejected.
fn block_stream(members: u8, blocks: u64, txs_per_block: u64, seed: u64) -> Vec<Block> {
    let mut rng = Lcg(seed);
    let mut out = Vec::new();
    let mut next_contract: u64 = 0;
    let mut nonce_counter: u64 = 0;
    // Every block in the stream must carry the app_hash the state
    // actually held before it. A scratch replica tracks that exactly the
    // way a live commit chain would — every caller of this helper builds its
    // OWN separate replica(s) from the same deterministic `genesis(members)`,
    // so the chain this produces is exactly what those replicas expect.
    let mut scratch = Replica::new(genesis(members));
    for h in 1..=blocks {
        let mut txs = Vec::new();
        let not_after_epoch = (h * EPOCH_SECS / 2) / EPOCH_SECS + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS;
        for _ in 0..txs_per_block {
            let a = rng.pick(members as u64);
            let b = rng.pick(members as u64);
            // A contract's own parties, so the envelope carries the signatures
            // the transition requires. A block whose envelopes authorise
            // nothing is refused before it is applied
            // (`block::deterministic_validity`), which is right — no honest
            // proposer builds one — and would make this generator produce a
            // stream no replica will take.
            let target = rng.pick(next_contract.max(1));
            let parties = scratch.state.contracts.get(&target).map(|c| (c.debtor, c.creditor));
            let (cd, cc) = parties.unwrap_or((a, b));
            let tx = match rng.pick(4) {
                0 => {
                    next_contract += 1;
                    Tx::Accept {
                        debtor: Party::Member(a),
                        creditor: Party::Member(b),
                        amount: 20.0 + rng.pick(300) as f64,
                        maturity_epochs: 30 + rng.pick(10),
                        arb: None,
                    }
                }
                1 => Tx::Settle { contract: target, amount: 1.0 + rng.pick(150) as f64 },
                2 => Tx::MarkExpired { contract: target },
                _ => Tx::Cure { contract: target, amount: 1.0 + rng.pick(100) as f64 },
            };
            let signers = match tx {
                Tx::Accept { .. } => vec![key(a as u8 + 1), key(b as u8 + 1)],
                _ => vec![key(cd as u8 + 1), key(cc as u8 + 1)],
            };
            let nonce = edet_node::block::counter_nonce(nonce_counter);
            nonce_counter += 1;
            txs.push(SignedTx { tx, nonce, not_after_epoch, signers, signatures: vec![] });
        }
        let block = Block { height: h, time_secs: h * EPOCH_SECS / 2, app_hash: scratch.app_hash(), txs };
        scratch
            .commit_block_unchecked(&block)
            .expect("scratch replica commits its own generated block");
        out.push(block);
    }
    out
}

#[test]
fn replicas_agree_at_every_height() {
    let blocks = block_stream(5, 40, 6, 0xE0E7);
    let mut replicas: Vec<Replica> = (0..3).map(|_| Replica::new(genesis(5))).collect();
    for block in &blocks {
        let mut hashes = Vec::new();
        for r in &mut replicas {
            r.commit_block_unchecked(block).expect("commit");
            hashes.push(r.app_hash());
        }
        assert_eq!(hashes[0], hashes[1], "replica divergence at height {}", block.height);
        assert_eq!(hashes[1], hashes[2], "replica divergence at height {}", block.height);
    }
    assert_eq!(replicas[0].height, 40);
}

#[test]
fn recovery_from_snapshot_and_wal() {
    let dir = std::env::temp_dir().join(format!("edet-test-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let blocks = block_stream(4, 12, 5, 0xBEEF);

    // A durable replica with a snapshot every 5 blocks…
    let mut durable = Replica::open(&dir, genesis(4), 5).expect("open");
    // …and an in-memory reference.
    let mut reference = Replica::new(genesis(4));
    for block in &blocks {
        durable.commit_block_unchecked(block).expect("commit durable");
        reference.commit_block_unchecked(block).expect("commit reference");
    }
    let want = reference.app_hash();
    assert_eq!(durable.app_hash(), want);

    // Crash: drop and reopen from disk (snapshot at 10 + WAL tail 11..12).
    drop(durable);
    let recovered = Replica::open(&dir, genesis(4), 5).expect("reopen");
    assert_eq!(recovered.height, 12);
    assert_eq!(recovered.app_hash(), want, "recovered state diverges");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn torn_wal_tail_is_tolerated() {
    let dir = std::env::temp_dir().join(format!("edet-test-torn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let blocks = block_stream(4, 6, 4, 0x7047);
    let mut durable = Replica::open(&dir, genesis(4), 0).expect("open");
    for block in &blocks {
        durable.commit_block_unchecked(block).expect("commit");
    }
    let want = durable.app_hash();
    drop(durable);
    // Simulate a crash mid-append: garbage half-frame at the tail.
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(dir.join("wal.bin")).expect("wal");
    f.write_all(&[0xFF, 0x00, 0x00, 0x00, 0xAA, 0xBB]).expect("tear");
    drop(f);
    let recovered = Replica::open(&dir, genesis(4), 0).expect("reopen");
    assert_eq!(recovered.height, 6);
    assert_eq!(recovered.app_hash(), want);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn commitment_is_order_sensitive() {
    // Accept-then-settle closes the contract; settle-then-accept leaves it
    // open (the settle hits a contract that does not exist yet). The two
    // orders must commit to different states.
    let signers = vec![key(1), key(2)];
    let accept = SignedTx {
        tx: Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        },
        nonce: edet_node::block::counter_nonce(0),
        not_after_epoch: 30,
        signers: signers.clone(),
        signatures: vec![],
    };
    let settle = SignedTx {
        tx: Tx::Settle { contract: 0, amount: 40.0 },
        nonce: edet_node::block::counter_nonce(1),
        not_after_epoch: 30,
        signers,
        signatures: vec![],
    };
    let mut ra = Replica::new(genesis(5));
    let mut rb = Replica::new(genesis(5));
    // Deterministic same genesis on both, so either replica's app_hash is
    // the value both blocks below must carry.
    let app0 = ra.app_hash();
    let forward = Block { height: 1, time_secs: EPOCH_SECS, app_hash: app0, txs: vec![accept.clone(), settle.clone()] };
    let swapped = Block { height: 1, time_secs: EPOCH_SECS, app_hash: app0, txs: vec![settle, accept] };
    ra.commit_block_unchecked(&forward).unwrap();
    rb.commit_block_unchecked(&swapped).unwrap();
    let (ha, hb) = (ra.app_hash(), rb.app_hash());
    assert_ne!(hex32(&ha), hex32(&hb), "tx order must be commitment-relevant");
}

/// Resuming from a snapshot must report the validator set live at the
/// snapshot height, not whatever genesis the caller happens to pass to
/// `Replica::open` on reopen — a stand-in for the compiled `dev_genesis` a
/// real node would otherwise fall back to.
#[test]
fn validator_set_survives_snapshot_resume() {
    let dir = std::env::temp_dir().join(format!("edet-test-vhist-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let mut durable = Replica::open(&dir, genesis_with_validators(3), 2).expect("open");
    // Member 1 exits at height 1 (validators go from {0,1,2} to {0,2}), then
    // a quiet block at height 2 lands exactly on the snapshot interval.
    // Chained from `durable`'s own app_hash — this test reopens below,
    // and WAL replay validates app_hash on reopen too.
    let app0 = durable.app_hash();
    // Leaving the community and leaving the UNDERWRITER role are two acts,
    // and `Exit` refuses the second (`ET-UWR-003`) — a member walking out
    // while credit stands on their supply would break the cut bound through a
    // transition that never mentions supply. So member 1 withdraws first.
    let leave = |seq: u64, k: [u8; 32], tx: Tx| SignedTx {
        tx,
        nonce: edet_node::block::counter_nonce(seq),
        not_after_epoch: 30,
        signers: vec![k],
        signatures: vec![],
    };
    let b1 = Block {
        height: 1,
        time_secs: EPOCH_SECS,
        app_hash: app0,
        txs: vec![
            leave(0, key(2), Tx::DeclareSupply { member: 1, supply: 0.0 }),
            leave(1, key(2), Tx::Exit { member: 1 }),
        ],
    };
    durable.commit_block_unchecked(&b1).expect("exit commits");
    let app1 = durable.app_hash();
    let b2 = Block { height: 2, time_secs: 2 * EPOCH_SECS, app_hash: app1, txs: vec![] };
    durable.commit_block_unchecked(&b2).expect("quiet block");
    assert_eq!(durable.height, 2);
    assert!(!durable.validators_at(2).unwrap().contains_key(&1));

    drop(durable);

    // Reopen with a deliberately different, wrong "genesis" (5 validators,
    // none exited) standing in for a compiled dev_genesis that must NOT be
    // what a resumed node reports once a snapshot exists.
    let wrong_genesis = genesis_with_validators(5);
    let recovered = Replica::open(&dir, wrong_genesis, 2).expect("reopen");
    assert_eq!(recovered.height, 2);
    let live = recovered.validators_at(2).expect("history reaches the resumed height");
    assert_eq!(live.len(), 2, "must reflect the persisted 2-validator set, not the 5-validator dev genesis");
    assert!(!live.contains_key(&1));
    assert!(live.contains_key(&0) && live.contains_key(&2));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A validator that drops out of `state.validators` at height H (here:
/// exits) must be absent from the set reported live at H+1 — a `Suspend`
/// committed at H must take effect no later than the next height.
#[test]
fn suspended_validator_absent_from_next_height() {
    let mut r = Replica::new(genesis_with_validators(4));
    assert_eq!(r.validators_at(0).unwrap().len(), 4);

    let app0 = r.app_hash();
    // Withdraw the supply before leaving: an underwriter may not simply exit.
    let signed = |seq: u64, k: [u8; 32], tx: Tx| SignedTx {
        tx,
        nonce: edet_node::block::counter_nonce(seq),
        not_after_epoch: 30,
        signers: vec![k],
        signatures: vec![],
    };
    let block = Block {
        height: 1,
        time_secs: EPOCH_SECS,
        app_hash: app0,
        txs: vec![
            signed(0, key(3), Tx::DeclareSupply { member: 2, supply: 0.0 }),
            signed(1, key(3), Tx::Exit { member: 2 }),
        ],
    };
    r.commit_block_unchecked(&block).expect("exit commits");

    let at_h = r.validators_at(1).expect("history reaches height 1");
    assert!(!at_h.contains_key(&2), "the exited validator must already be gone at H");

    // No block at height 2 has been committed yet; `validators_at` must
    // still answer for H+1 with the set that took effect at H.
    let at_h_plus_1 = r.validators_at(2).expect("history covers H+1 too");
    assert!(!at_h_plus_1.contains_key(&2), "exited validator must be absent from H+1");
    assert_eq!(at_h_plus_1.len(), 3);
}

#[test]
fn height_gaps_are_rejected() {
    let blocks = block_stream(4, 3, 2, 0x1111);
    let mut r = Replica::new(genesis(4));
    r.commit_block_unchecked(&blocks[0]).unwrap();
    let err = r.commit_block_unchecked(&blocks[2]);
    assert!(err.is_err(), "skipping a height must fail");
}
