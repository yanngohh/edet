//! The embedded-app path: the Malachite engine drives a shared `serve::Node`,
//! so a LIVE client submission (`serve::submit` — exactly what the Tauri IPC
//! `submit_tx` command calls) is committed by real consensus and immediately
//! readable through the same node the UI's view layer reads. This is the
//! "wire the app on Malachite" proof at the library level (the Tauri shell
//! itself can't be built in this sandbox).
#![cfg(feature = "malachite")]

use std::time::Duration;

use edet_node::block::{dev_seed, sign_tx};
use edet_node::engine_node::{write_solo_home, EdetApp};
use edet_state::tx::Tx;
use edet_state::types::Party;

#[tokio::test]
async fn a_live_submission_is_committed_by_malachite_and_read_through_the_shared_node() {
    let dir = std::env::temp_dir().join(format!("edet-malachite-app-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // 1 sole validator (commits alone) + 1 extra member, so Accept(0 -> 1) has
    // two real member ids as parties.
    write_solo_home(&dir, 1).expect("write solo home");

    // Consensus only: this test submits in-process through the shared node,
    // exactly as the Tauri IPC command does. The HTTP client API a browser UI
    // needs is `malachite_http.rs`'s subject.
    let app = EdetApp::at(dir.clone(), None, None);
    let handle = app.start_embedded().await.expect("the Malachite node starts");

    // A fresh genesis starts at economic epoch 0, but this
    // node runs on real wall-clock time, so its very first committed
    // block(s) must close the gap between epoch 0 and "today" — capped at
    // `MAX_EPOCH_ADVANCE_PER_BLOCK` (10,000) per block, so a genesis dated
    // decades before "today" needs several blocks to catch up. A
    // `not_after_epoch` computed from wall-clock time BEFORE that catch-up
    // finishes can be miles ahead of `state.epoch` at the instant the tx
    // actually lands (`ET_TX_WINDOW_TOO_LONG`) — so this submits with a
    // window anchored to the REPLICA'S OWN current epoch, re-signed with a
    // fresh nonce and re-read each retry, exactly the "re-sign and retry"
    // behaviour `edet_state::apply`'s doc comment prescribes for a
    // legitimately-failed-then-retried transaction.
    // ONE live submission at a time, and that is load-bearing. A version that
    // sign and submit a fresh transaction on every 100 ms turn of the loop
    // and break on the first sign of debt, which was only ever right because
    // blocks came as fast as the CPU allowed: the first tx committed before
    // the second was minted. Once empty blocks were paced to one a second
    // (`engine_malachite::EMPTY_BLOCK_INTERVAL`) ten valid, distinctly-nonced
    // Accepts piled up and committed together — debt 400, and the test read
    // it as a failure to commit. It was asserting the absence of block time.
    //
    // The retry still exists, for the reason the comment above gives: a tx
    // whose window the epoch catch-up jumped past is legitimately dead and
    // must be re-signed. So resubmit when the outstanding one has EXPIRED,
    // never merely because it has not landed yet.
    let mut committed_debt: f64 = 0.0;
    let mut live_until_epoch: Option<u64> = None;
    for attempt in 0..150u64 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Some(m) = handle.node.lock().replica.state.members.get(&0) {
            if m.debt_out > 0 {
                committed_debt = edet_state::State::from_minor(m.debt_out);
                break;
            }
        }
        let current_epoch = handle.node.lock().replica.state.epoch;
        // Still in flight and still valid — wait for it rather than adding a
        // second obligation to the one already on its way.
        if live_until_epoch.is_some_and(|until| current_epoch <= until) {
            continue;
        }
        let not_after_epoch = current_epoch + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS;
        let tx = Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        };
        // A real signed, trial-sized Accept submitted LIVE (not seeded at
        // boot) — the same path the app's IPC `submit_tx` takes: verify the
        // signatures, push into the shared mempool the engine's GetValue
        // drains.
        let signed = sign_tx(
            edet_node::block::DEV_CHAIN_ID,
            tx,
            edet_node::block::counter_nonce(attempt),
            not_after_epoch,
            &[dev_seed(0), dev_seed(1)],
        )
        .expect("sign");
        let queued = edet_node::serve::submit(&handle.node, signed).await;
        assert!(queued, "a validly signed tx must pass the verified ingress and queue");
        live_until_epoch = Some(not_after_epoch);
    }

    let _ = handle.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        (committed_debt - 40.0).abs() < 1e-9,
        "the live-submitted Accept must be committed by Malachite and visible via the shared node (got debt {committed_debt})",
    );
}
