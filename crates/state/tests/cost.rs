//! **What a BLOCK costs** — the half of §Implementation its own opening sentence denies.
//!
//! §Implementation scopes its table "per community, at acceptance time — not per block, not
//! per account per epoch", and `crates/kernel/tests/cost.rs` measures the
//! acceptance half. This is the other one, and it exists because §Verification already
//! contradicts that sentence in the same document: the six invariants run on
//! the commit path, invariant 1 is evaluated over sets, and §Verification says in as many
//! words that it "costs one query per underwriter".
//!
//! So a validator pays `1 + U + 2` FULL max-flow queries per committed block —
//! all insured debtors as one set, one set per underwriter carrying live
//! insured debt, and two more for invariant 6's determinism check — whatever
//! the block contained, including nothing. `Replica::commit_block_unchecked`
//! runs `invariants::audit` unconditionally after `apply_block_to`, and its
//! A note beside the call calling the invariants "cheap next to the epoch
//! machinery `begin_block` already ran" — a claim inside the code with no
//! measurement behind it, which is now the third probe below and was false by
//! 28x to 434x against a cost paid one block in seventeen thousand.
//!
//! `#[ignore]`d for the same reason as the kernel harness: a wall-clock
//! assertion in CI is a flake generator, and what a gate can honestly hold is
//! that this still compiles and still runs. Run it with `just cost`, on an
//! otherwise idle machine, and record the hardware beside any figure quoted
//! from it.

use std::time::Instant;

use edet_state::invariants::{audit, audit_with_cache, AuditCache};
use edet_state::state::State;
use edet_state::types::{Contract, ContractStatus, Key, MemberId};

/// The community `crates/kernel/tests/cost.rs` builds, as ledger state: `n`
/// accounts, `n / 100` founding underwriters, and the same deterministic
/// stake graph, so the two halves of the table are measured over one shape
/// rather than two.
///
/// The underwriters reach the first slice of ordinary members directly, which
/// is what lets `insure` place one obligation per underwriter: the shortest
/// augmenting path to member `u + i * deg` is `source -> underwriter i ->
/// member`, so a small reservation there draws through underwriter `i` and
/// puts that underwriter's own set on the audit's list.
fn community(n: usize, deg: usize, u: usize) -> State {
    let u = u.clamp(1, n - 1);
    let mut st = State::default();
    let key = |i: usize| -> Key {
        let mut k = [0u8; 32];
        k[0..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
        k
    };
    for i in 0..u {
        // 1,000,000 minor units, matching the kernel harness's supply arcs.
        st.add_underwriter(vec![key(i)], 10_000.0).expect("founding underwriter");
    }
    for i in u..n {
        st.new_account(vec![key(i)]);
    }
    for i in 0..u {
        for k in 0..deg {
            let d = u + (i * deg + k) % (n - u);
            st.edges.insert((i, d), 50_000);
        }
    }
    for c in u..n {
        for k in 0..deg {
            let d = u + ((c * 7919 + k * 104_729) % (n - u));
            if d != c {
                st.edges.insert((c, d), 10_000);
            }
        }
    }
    st
}

/// Book `per_uw` live insured obligations against each underwriter's directly
/// reached members, through the ordinary reservation path.
///
/// Returns how many were actually insured. Booking them by hand rather than
/// through `Tx::Accept` keeps this a measurement of the AUDIT: the transition
/// function has its own bond gate, epoch machinery and signature checks, none
/// of which is what a block pays per commit.
fn insure(st: &mut State, deg: usize, per_uw: usize, amount: f64) -> usize {
    let u = st.underwriters.len();
    let n = st.members.len();
    let mut booked = 0usize;
    for i in 0..u {
        for j in 0..per_uw.min(deg) {
            let debtor = (u + (i * deg + j) % (n - u)) as MemberId;
            // Somebody outside the underwriter set holds the claim; who it is
            // does not enter the audit, only that it is neither the debtor nor
            // an account that does not exist.
            let creditor = ((debtor as usize % (n - u - 1)) + u) as MemberId;
            let creditor = if creditor == debtor { debtor + 1 } else { creditor };
            let Some(held) = st.reserve_capacity(debtor, amount) else { continue };
            if held.supply.is_empty() {
                continue;
            }
            let id = st.next_contract;
            st.next_contract += 1;
            st.contracts.insert(
                id,
                Contract {
                    id,
                    debtor,
                    creditor,
                    outstanding: State::to_minor(amount),
                    original: State::to_minor(amount),
                    maturity_epoch: st.epoch + 30,
                    status: ContractStatus::Active,
                    created_epoch: st.epoch,
                    accepted_epoch: st.epoch,
                    insured: true,
                    held,
                    arb: None,
                    arb_attestations: Default::default(),
                    arb_awarded: false,
                },
            );
            let m = st.members.get_mut(&debtor).expect("debtor exists");
            m.debt_out += State::to_minor(amount);
            m.rep.d_in += amount;
            booked += 1;
        }
    }
    booked
}

/// How many sets invariant 1 will measure: all live insured debtors together,
/// plus one per underwriter carrying any of them.
fn sets_audited(st: &State) -> usize {
    let mut uw: std::collections::BTreeSet<usize> = Default::default();
    let mut any = false;
    for c in st.contracts.values() {
        if c.insured && matches!(c.status, ContractStatus::Active | ContractStatus::Expired) {
            any = true;
            for &(u, _) in &c.held.supply {
                uw.insert(u);
            }
        }
    }
    usize::from(any) + uw.len()
}

#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn one_commit_path_audit_at_four_community_sizes() {
    println!("\n  accounts     edges  underwriters   insured   sets   audit per block");
    for n in [1_000usize, 5_000, 10_000, 20_000] {
        let deg = 8;
        let mut st = community(n, deg, (n / 100).max(1));
        let booked = insure(&mut st, deg, 4, 100.0);
        audit(&st).expect("the measured state is a legal one");
        let sets = sets_audited(&st);

        let reps = 5;
        let t0 = Instant::now();
        for _ in 0..reps {
            audit(&st).expect("legal");
        }
        let per = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;
        println!("{n:10}  {:8}  {:12}  {booked:8}  {sets:5}   {per:9.1} ms", st.edges.len(), st.underwriters.len());
    }
    println!();
}

/// Is the block cost a property of community SIZE or of the underwriter ratio?
///
/// The table above fixes `U = n/100`, which is a choice and not a rule: the
/// underwriter set is open by design (§Standing), so `U` is whatever a community's
/// members have declared. Held at 20,000 accounts, the cost should be linear in
/// `U` — one full capacity query per underwriter carrying live insured debt,
/// plus one for the whole debtor set and two for invariant 6 — and this is
/// where that is checked rather than assumed.
#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn the_block_cost_is_linear_in_the_underwriter_count() {
    let (n, deg) = (20_000usize, 8);
    println!("\n  accounts  underwriters   sets   audit per block   per set");
    for u in [20usize, 50, 100, 200, 400] {
        let mut st = community(n, deg, u);
        let booked = insure(&mut st, deg, 4, 100.0);
        assert!(booked > 0, "the scene must carry live insured debt");
        audit(&st).expect("the measured state is a legal one");
        let sets = sets_audited(&st);

        let reps = 3;
        let t0 = Instant::now();
        for _ in 0..reps {
            audit(&st).expect("legal");
        }
        let per = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;
        println!("{n:10}  {:12}  {sets:5}   {per:9.1} ms   {:7.1} ms", st.underwriters.len(), per / sets as f64);
    }
    println!();
}

/// **"They are cheap next to the epoch machinery `begin_block` already ran."**
///
/// That is `Replica::commit_block_unchecked`'s own note beside the audit call,
/// and it is the sentence that puts the audit on the commit path
/// unconditionally — a claim inside the code with nothing gating it, of the
/// same shape as §Implementation's "not per block" one level in.
///
/// What it compares against does not run on almost any block. `EPOCH_SECS` is
/// a day and a block is a consensus round, so `begin_block` returns at its
/// first line when the timestamp repeats, and otherwise falls straight past
/// the `while` without entering it: no decay, no sweep, no release. The epoch
/// machinery is paid **once a day**; the audit is paid **every block**.
///
/// Three columns, on the same state: an ordinary block five seconds on, a
/// block that crosses an epoch boundary (decay over every edge, the crank
/// sweep, the bond release), and the audit.
#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn the_audit_is_not_cheap_next_to_the_epoch_machinery() {
    println!("\n  accounts   ordinary begin_block   epoch begin_block   audit   audit / epoch-block");
    for n in [1_000usize, 5_000, 10_000, 20_000] {
        let deg = 8;
        let mut st = community(n, deg, (n / 100).max(1));
        insure(&mut st, deg, 4, 100.0);
        // Inside epoch 0, exactly where the tables above measure. Starting
        // any later would advance the epoch first, and 400 days of decay
        // leaves a different (much smaller) graph to audit.
        let t_start = 100;
        st.begin_block(t_start);
        audit(&st).expect("the measured state is a legal one");

        // The ordinary block: a new timestamp inside the same epoch. Nothing
        // to clone away from — no epoch is crossed, so the call touches one
        // field and returns.
        let reps = 1_000;
        let t0 = Instant::now();
        for i in 0..reps {
            st.begin_block(t_start + 1 + i as u64);
        }
        let ordinary = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        // The once-a-day block: one epoch crossed, so decay walks every edge
        // and `sweep_cranks` walks the contract book. Cloned OUTSIDE the
        // timer — the copy is the harness's cost and not the block's.
        let reps = 5;
        let mut epoch_block = 0.0;
        for i in 0..reps {
            let mut fresh = st.clone();
            let t0 = Instant::now();
            fresh.begin_block(t_start + (i as u64 + 1) * edet_kernel::constants::EPOCH_SECS);
            epoch_block += t0.elapsed().as_secs_f64() * 1e3;
        }
        let epoch_block = epoch_block / reps as f64;

        let reps = 3;
        let t0 = Instant::now();
        for _ in 0..reps {
            audit(&st).expect("legal");
        }
        let audit_ms = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        println!(
            "{n:10}  {ordinary:17.4} ms  {epoch_block:14.2} ms  {audit_ms:8.1} ms  {:12.0}x",
            audit_ms / epoch_block
        );
    }
    println!();
}

/// **"It costs one flow query per underwriter, on a set that is small by
/// design."** — `capacity_invariants`, and §Verification in the same words.
///
/// The second half is true and it is not a statement about the cost. A cut is
/// a max-flow over the WHOLE edge map into the set, so what a set costs is a
/// function of `E` and not of `|S|`: the network is built from every edge
/// either way, and the level-graph search visits them all. A reader who takes
/// "small by design" as a reason the family is affordable has read a property
/// of the sets as a property of the price.
#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn a_sets_price_is_flat_in_its_size() {
    let (n, deg) = (20_000usize, 8);
    let u = (n / 100).max(1);
    let mut st = community(n, deg, u);
    insure(&mut st, deg, 4, 100.0);
    audit(&st).expect("the measured state is a legal one");

    println!("\n  set size   one cut query");
    for size in [1usize, 10, 100, 1_000, 10_000] {
        let set: Vec<MemberId> = (u..u + size).map(|i| i as MemberId).collect();
        let reps = 20;
        let t0 = Instant::now();
        for _ in 0..reps {
            std::hint::black_box(st.gross_capacity_of_set(&set));
        }
        let per = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;
        println!("{size:10}   {per:10.1} ms");
    }
    println!();
}

/// **What the memo actually buys, measured on the shape the table above is
/// measured on.**
///
/// The decision taken is to memoise the cut rather than to check
/// fewer sets: the invariant must HOLD over the whole family after every
/// block, but it does not follow that every set must be RECOMPUTED after every
/// block. A cut reads `(edges, supplies, id space)` and is monotone in all
/// three, so a verified cut stays a valid lower bound until one of them falls
/// — and what can lower one is a short structural list (decay at the epoch
/// boundary, a governed re-denomination, a `DeclareSupply` that reduces),
/// never ordinary settlement, which raises stakes by a peak rule.
///
/// Three rows, because they are the three shapes of block a validator commits:
/// the first one after a restart (cold, and it must be), a block that changed
/// nothing economic (the original complaint — "whatever the block
/// contained, including nothing"), and a block that drew new insured credit
/// through one underwriter, which is what a busy community does all day.
#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn what_an_epoch_boundary_costs() {
    // The measurement the cost table never had, and the one a repair has to
    // move. The block that pays for the whole family is the one that CROSSES AN
    // EPOCH: decay lowers every edge free to fall, so `nothing_decreased`
    // throws the memo away.
    //
    // **Both arms are measured on the same state, adjacent in time, and
    // neither needs the code flipped.** `audit` is the cold definition and
    // computes every set — which is exactly what an unmemoised commit path pays
    // here. `audit_with_cache` is what a validator runs now. A ratio taken
    // back to back survives a box that is not idle, which this one is not:
    // the load average is printed beside the figures, because a wall-clock
    // number without the machine it was taken on is not a measurement.
    let deg = 8;
    println!("\n  accounts       U    definition    commit path    ratio   cuts def / commit");
    for n in [1_000usize, 5_000, 10_000, 20_000] {
        let u = (n / 100).max(1);
        let mut st = community(n, deg, u);
        insure(&mut st, deg, 4, 100.0);
        let t_start = 100;
        st.begin_block(t_start);

        // Warm a cache on the pre-decay state, exactly as a running validator
        // would have one.
        let mut cache = AuditCache::default();
        audit_with_cache(&st, &mut cache).expect("the measured state is a legal one");

        // Cross one epoch. The clone is outside both timers.
        let mut fresh = st.clone();
        fresh.begin_block(t_start + edet_kernel::constants::EPOCH_SECS);

        let t0 = Instant::now();
        audit(&fresh).expect("legal");
        let definition = t0.elapsed().as_secs_f64() * 1e3;

        let before = cache.queries();
        let t0 = Instant::now();
        audit_with_cache(&fresh, &mut cache).expect("legal");
        let commit_path = t0.elapsed().as_secs_f64() * 1e3;
        let drew = cache.queries() - before;

        println!(
            "{n:10}  {u:6}  {definition:9.1} ms  {commit_path:9.1} ms  {:6.1}x   {:8} / {drew}",
            definition / commit_path,
            sets_audited(&fresh)
        );
    }
    println!("  (load average at the time of this run: {})", load_average());
    println!();
}

/// The one-minute load average, printed beside every figure above.
///
/// A wall-clock figure without its machine is not a measurement, and "the
/// machine" includes what else was running on it. This tree has already paid
/// once for a harness that raced itself.
fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn what_the_memo_buys() {
    let deg = 8;
    println!("\n  accounts       U        cold       unchanged   one new draw");
    for n in [1_000usize, 5_000, 10_000, 20_000] {
        let u = (n / 100).max(1);
        let mut st = community(n, deg, u);
        insure(&mut st, deg, 4, 100.0);
        audit(&st).expect("the measured state is a legal one");

        let mut cache = AuditCache::default();
        let t0 = Instant::now();
        audit_with_cache(&st, &mut cache).expect("legal");
        let cold = t0.elapsed().as_secs_f64() * 1e3;
        let cold_q = cache.queries();

        let reps = 20;
        let t0 = Instant::now();
        for _ in 0..reps {
            audit_with_cache(&st, &mut cache).expect("legal");
        }
        let unchanged = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;
        assert_eq!(cache.queries(), cold_q, "an unchanged state must cost no query at all");

        // One more insured obligation, drawing through one underwriter: the
        // `all` set and that underwriter's set change, and nothing else does.
        let before = cache.queries();
        insure(&mut st, deg, 1, 1.0);
        let t0 = Instant::now();
        audit_with_cache(&st, &mut cache).expect("legal");
        let one_draw = t0.elapsed().as_secs_f64() * 1e3;
        let drew = cache.queries() - before;

        println!(
            "{n:10}  {u:6}  {cold:8.1} ms  {unchanged:10.3} ms  {one_draw:8.1} ms   \
             ({cold_q} cuts cold, {drew} after one draw)"
        );
    }
    println!();
}

/// **A committed block pays for two whole-ledger passes, and only one of them
/// has ever been measured.**
///
/// The tables above are the invariant audit. Beside it, every block also
/// recomputes the state root — `Replica::app_hash`, the value each proposal
/// claims and each voter checks — and `root::state_root` re-encodes and
/// re-hashes EVERY member, contract, proposal and validator to produce it.
/// There is no incremental tree: a block that changed one row rebuilds the
/// whole commitment, so the cost is a function of the ledger's SIZE and not of
/// what the block contained. That is the same shape as the audit's own
/// complaint before the memo, and it has no memo.
///
/// So the two are measured on one state, in the units the paper's cost section
/// uses, and the third column is the one a community-size budget has to add
/// up: a validator pays both on every block.
#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn a_block_pays_for_the_root_as_well_as_the_audit() {
    println!("\n  accounts  contracts   state root   audit (cold)   audit (memo)   root + memo");
    for n in [1_000usize, 5_000, 10_000, 20_000] {
        let deg = 8;
        let mut st = community(n, deg, (n / 100).max(1));
        let booked = insure(&mut st, deg, 4, 100.0);
        assert!(booked > 0, "the scene must carry live insured debt");
        audit(&st).expect("the measured state is a legal one");

        let reps = 5;
        let t0 = Instant::now();
        for _ in 0..reps {
            edet_state::root::state_root(&st).expect("the measured state has a root");
        }
        let root = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        let t0 = Instant::now();
        for _ in 0..reps {
            audit(&st).expect("legal");
        }
        let cold = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        // What a validator actually pays for the audit on a block that changed
        // nothing economic: the memo answers with no cut at all.
        let mut cache = AuditCache::default();
        audit_with_cache(&st, &mut cache).expect("legal");
        let t0 = Instant::now();
        for _ in 0..reps {
            audit_with_cache(&st, &mut cache).expect("legal");
        }
        let memo = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        println!(
            "{n:10}  {:9}  {root:9.1} ms  {cold:10.1} ms  {memo:10.1} ms  {:8.1} ms",
            st.contracts.len(),
            root + memo
        );
    }
    println!();
}

/// **The top of the envelope**: what one bonded transaction and one block cost
/// at three community sizes, and what the write gate's memo takes off the
/// first.
///
/// The gate asks two questions and both read a full pristine max-flow cut
/// (`established`, `bond_headroom`), so an ordinary bonded transaction paid for
/// one or two of them on every write. `GateCache` holds a LOWER BOUND, which
/// settles a threshold whenever it passes — that is the whole reason the
/// billing rule is "the first signer who can pay" rather than "the signer with
/// the most", since two lower bounds cannot rank two signers.
///
/// No cold audit at 100,000: that is a thousand cuts and the row would measure
/// the afternoon rather than the ledger. `one_commit_path_audit_at_four_
/// community_sizes` is where the definition's own cost lives.
/// **The whole-ledger passes a block pays**, side by side on one state.
///
/// Three whole-ledger passes a committed block can pay: the state root, the
/// working copy a block is applied to, and the audit's own bookkeeping. The
/// first two are what this reports — the root as the pair
/// (`where_the_state_roots_time_goes` varies the shape; this one puts the
/// three next to each other), and `State::clone()`, which is what a working
/// copy costs and what a halt flag replaces.
///
/// The third, the audit's structural walks, is the term that remains. It is
/// the only one of the three still linear per block, and what it would take
/// to make it incremental is the journal these already use — which is why
/// this table is the one that frames that decision. Quote the ROW, never a
/// figure from it.
#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn what_a_block_pays_over_the_whole_ledger() {
    use edet_state::root::{state_root, RootCache};

    let reps = 5;
    let key = |i: usize| -> Key {
        let mut k = [0u8; 32];
        k[0..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
        k
    };
    println!("\n  {} — the whole-ledger passes a block pays", load_average());
    println!("  {:>10}  {:>14}  {:>14}  {:>14}", "rows", "root (defn)", "root (block)", "working copy");
    for n in [1_000usize, 10_000, 50_000, 100_000] {
        // The farm's shape — one member and one in-edge per seat — because
        // that is the state whose growth this is about.
        let mut st = State::default();
        st.add_underwriter(vec![key(0)], 10_000.0).expect("founding underwriter");
        for i in 1..n {
            st.new_account(vec![key(i)]);
            st.edges.insert((i - 1, i), 10_000);
        }

        state_root(&st).expect("root");
        let t0 = Instant::now();
        for _ in 0..reps {
            state_root(&st).expect("root");
        }
        let definition = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        let mut cache = RootCache::default();
        cache.refresh(&mut st).expect("the warm-up build");
        let t0 = Instant::now();
        for _ in 0..reps {
            st.members.get_mut(&1).expect("member 1").debt_out += 1;
            st.edges.touch(&(1, 2));
            cache.refresh(&mut st).expect("root");
        }
        let block = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        let _ = st.clone();
        let t0 = Instant::now();
        for _ in 0..reps {
            let copy = st.clone();
            std::hint::black_box(&copy);
        }
        let copy = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        println!("  {n:>10}  {definition:>11.1} ms  {block:>11.3} ms  {copy:>11.1} ms");
    }
    println!();
}

/// **Where the state root's time actually goes**, as the PAIR: the
/// definition's full build against what an ordinary block and an epoch
/// boundary cost through the cache.
///
/// `state_root` is a pure function of a `State` and rebuilds every leaf;
/// `RootCache::refresh` is what a validator runs, and rehashes the rows the
/// block wrote and the paths above them. The two agree on every state — the
/// swarm driver holds them to it after every transition — so what varies
/// here is only the work, and a figure without its pair says nothing.
///
/// The definition still has two terms with nothing in common but the word
/// "leaf": one small leaf per row across the four record sections and the
/// stake rows, and a scalar leaf plus a fixed-width replay window that grow
/// with neither. So this varies the member count at a fixed edge count, then
/// the edge count at a fixed member count.
///
/// The last block is the shape a wash farm produces — one member and one
/// in-edge per seat — because that is the state whose growth this is about,
/// and the cached column is what the change to that state's cost per block
/// actually is.
///
/// **The block column is the OUT-DEGREE of the accounts the block touched**,
/// not the size of the ledger: a stake row is one leaf per creditor, so
/// writing an edge re-encodes that creditor's whole row and nothing else.
/// The two shapes in the table say it plainly — a farm seat has one out-edge
/// and its block stays at hundredths of a millisecond from 1,000 rows to
/// 250,000, while the middle blocks fill every edge onto the first creditor
/// and its block rises with that one row. Both are bounded by the row the
/// writer owns rather than by the graph.
#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn where_the_state_roots_time_goes() {
    use edet_state::root::RootCache;

    let reps = 5;
    let time = |st: &State| -> f64 {
        edet_state::root::state_root(st).expect("a root");
        let t0 = Instant::now();
        for _ in 0..reps {
            edet_state::root::state_root(st).expect("a root");
        }
        t0.elapsed().as_secs_f64() * 1e3 / reps as f64
    };
    // What a validator pays: an ordinary block, which writes one member row
    // and one stake edge — the shape of an acceptance — and the boundary
    // block, where every salt changes and the tree is rebuilt whole.
    let cached = |st: &State| -> (f64, f64) {
        let mut st = st.clone();
        let mut cache = RootCache::default();
        cache.refresh(&mut st).expect("the warm-up build");
        let t0 = Instant::now();
        for _ in 0..reps {
            st.members.get_mut(&1).expect("member 1").debt_out += 1;
            st.edges.touch(&(1, 2));
            cache.refresh(&mut st).expect("a root");
        }
        let block = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;
        let t0 = Instant::now();
        for _ in 0..reps {
            st.epoch += 1;
            cache.refresh(&mut st).expect("a root");
        }
        let boundary = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;
        (block, boundary)
    };
    let row = |members: usize, edges: usize, st: &State| {
        let (block, boundary) = cached(st);
        println!("  {members:>10}  {edges:>10}  {:>9.1} ms  {:>9.3} ms  {:>9.1} ms", time(st), block, boundary);
    };
    let header = || {
        println!("  {:>10}  {:>10}  {:>12}  {:>12}  {:>12}", "members", "edges", "definition", "block", "boundary");
    };
    let key = |i: usize| -> Key {
        let mut k = [0u8; 32];
        k[0..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
        k
    };
    // A state with exactly `members` rows and exactly `edges` stake edges, and
    // nothing else — built directly, because what is being measured is the
    // commitment and not the transition function.
    let scene = |members: usize, edges: usize| -> State {
        let mut st = State::default();
        st.add_underwriter(vec![key(0)], 10_000.0).expect("founding underwriter");
        for i in 1..members {
            st.new_account(vec![key(i)]);
        }
        // Exactly `edges` DISTINCT pairs, enumerated rather than hashed into
        // place: a generator that collides silently caps the edge count at the
        // member count, and then the column varies nothing.
        let mut placed = 0usize;
        'fill: for c in 1..members {
            for d in 1..members {
                if c == d {
                    continue;
                }
                st.edges.insert((c, d), 10_000);
                placed += 1;
                if placed >= edges {
                    break 'fill;
                }
            }
        }
        st
    };

    println!("\n  {} — one block's state root, by what grew", load_average());

    println!("\n  members varying, edges fixed at 2,000 (the RECORD-leaf term)");
    header();
    for n in [1_000usize, 5_000, 10_000, 20_000, 40_000] {
        let st = scene(n, 2_000);
        row(n, st.edges.len(), &st);
    }

    println!("\n  edges varying, members fixed at 2,000 (the STAKE-row term)");
    header();
    for e in [1_000usize, 10_000, 50_000, 100_000, 200_000] {
        let st = scene(2_000, e);
        row(st.members.len(), st.edges.len(), &st);
    }

    println!("\n  members fixed at 100,000, edges varying — the two terms, separated");
    header();
    for e in [2_000usize, 50_000, 100_000, 200_000, 400_000] {
        let st = scene(100_000, e);
        row(st.members.len(), st.edges.len(), &st);
    }

    println!("\n  the shape a wash farm produces — one member and one in-edge per seat");
    header();
    for n in [1_000usize, 10_000, 50_000, 100_000, 250_000] {
        let mut st = State::default();
        st.add_underwriter(vec![key(0)], 10_000.0).expect("founding underwriter");
        for i in 1..n {
            st.new_account(vec![key(i)]);
            // Every seat is reached by exactly one edge from the row before
            // it, which is what a chain of washes leaves behind.
            st.edges.insert((i - 1, i), 10_000);
        }
        row(st.members.len(), st.edges.len(), &st);
    }
    println!();
}

#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn the_top_of_the_envelope() {
    use edet_state::bond::{admits, admits_with_cache, GateCache};
    use edet_state::tx::Tx;
    use edet_state::types::Party;

    println!(
        "\n  accounts  underwriters   root (full)  root (block)   gate (cold)   gate (memo)   reserve 100   resident"
    );
    for n in [20_000usize, 50_000, 100_000] {
        let u = n / 100;
        let mut st = community(n, 8, u);
        let booked = insure(&mut st, 8, 4, 100.0);
        assert!(booked > 0, "the scene must carry live insured debt");
        let key = |i: usize| -> Key {
            let mut k = [0u8; 32];
            k[0..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
            k
        };
        // An ordinary trade between two established members, signed by both.
        let tx = Tx::Accept {
            debtor: Party::Member(u as u64 + 1),
            creditor: Party::Member(0),
            amount: 100.0,
            maturity_epochs: 30,
            arb: None,
        };
        let signers = [key(0), key(u + 1)];

        let reps = 5;
        let t0 = Instant::now();
        for _ in 0..reps {
            edet_state::root::state_root(&st).expect("root");
        }
        let root = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        // What a validator actually pays for the commitment on an ordinary
        // block: the rows it wrote and the paths above them. The definition
        // above is the epoch boundary, and the pair is the figure to quote.
        let block = {
            let mut probe = st.clone();
            let mut cache = edet_state::root::RootCache::default();
            cache.refresh(&mut probe).expect("the warm-up build");
            let t0 = Instant::now();
            for _ in 0..reps {
                probe.members.get_mut(&1).expect("member 1").debt_out += 1;
                probe.edges.touch(&(1, 2));
                cache.refresh(&mut probe).expect("root");
            }
            t0.elapsed().as_secs_f64() * 1e3 / reps as f64
        };

        let _ = admits(&st, &tx, &signers);
        let t0 = Instant::now();
        for _ in 0..reps {
            let _ = admits(&st, &tx, &signers);
        }
        let cold = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        let mut gate = GateCache::default();
        let _ = admits_with_cache(&st, &tx, &signers, &mut gate);
        let warm = gate.queries();
        let t0 = Instant::now();
        for _ in 0..reps {
            let _ = admits_with_cache(&st, &tx, &signers, &mut gate);
        }
        let memo = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;
        assert_eq!(gate.queries(), warm, "a warm memo must compute no cut inside one epoch");

        // What the acceptance itself pays past the gate: one limited
        // reservation, which is the query `Tx::Accept` runs once.
        let t0 = Instant::now();
        let mut probe = st.clone();
        for _ in 0..reps {
            let _ = probe.reserve_capacity_minor(u as u64 + 1, 10_000);
        }
        let reserve = t0.elapsed().as_secs_f64() * 1e3 / reps as f64;

        println!(
            "{n:10}  {u:12}  {root:9.1} ms  {block:9.3} ms  {cold:10.1} ms  {memo:10.2} ms  {reserve:9.1} ms  {}",
            resident()
        );
    }
    println!();
}

/// **What a SEATING acceptance costs against a plain one.**
///
/// The same trade twice, and the only difference is whether the counterparty's
/// key already has a row: naming one that does is an ordinary acceptance,
/// naming one that does not seats it. So what the pair isolates is the seat and
/// nothing else — the gate's `can_seat_minor`, which is a limited cut on the
/// write layer, and the write's `reserve_seat`, which is that cut plus the
/// augmentation it keeps. About two reserves is the expectation.
///
/// Driven through the real transition function, because the claim is about
/// what an ACCEPTANCE costs and a probe that called the two kernel queries by
/// hand would be measuring a shape the ledger does not run.
///
/// **A ratio, in one sitting, and never a lone figure.** The same probe on the
/// same tree reads 81.7 ms for a full 20,000-account query on a busy box and
/// 25.2 ms on a quiet one, so the pair is what a claim may rest on. The
/// conditions are interleaved and the least of five rounds kept on each side,
/// because a box that drifts under one condition and not the other is the
/// loaded box the pair exists to avoid.
#[test]
#[ignore = "wall-clock measurement: run with `just cost`, not in CI"]
fn a_seating_acceptance_against_a_plain_one() {
    use edet_state::tx::Tx;
    use edet_state::types::Party;

    let deg = 8;
    let reps = 20u32;
    // A key nobody has a row for, in a namespace `community`'s own keys — the
    // big-endian index — cannot collide with.
    let stranger = |round: u32, i: u32| -> Key {
        let mut k = [0x7Eu8; 32];
        k[0..4].copy_from_slice(&round.to_be_bytes());
        k[4..8].copy_from_slice(&i.to_be_bytes());
        k
    };
    let sponsor_key = |u: usize| -> Key {
        let mut k = [0u8; 32];
        k[0..8].copy_from_slice(&(u as u64 + 1).to_be_bytes());
        k
    };

    println!("\n  accounts             U        plain      seating       ratio   load {}", load_average());
    for n in [1_000usize, 5_000, 10_000, 20_000] {
        let u = (n / 100).max(1);
        let mut base = community(n, deg, u);
        // Both conditions pay a real bond, so the pair is not measuring the
        // free allowance carrying one side and not the other — a seat is never
        // allowance-covered and an ordinary trade usually is.
        base.params.bond_free_allowance = 0;
        // A member the seed reaches directly, so the augmenting path is one hop
        // under both conditions and the difference is the seat.
        let sponsor = u as MemberId;
        let sk = sponsor_key(u);

        let accept = |counterparty: Party| Tx::Accept {
            debtor: counterparty,
            creditor: Party::Member(sponsor),
            amount: 1.0,
            maturity_epochs: 30,
            arb: None,
        };
        let mut id = 0u64;
        let mut next_id = || {
            id += 1;
            let mut b = [0u8; 32];
            b[0..8].copy_from_slice(&id.to_be_bytes());
            b
        };

        let (mut plain, mut seating) = (f64::MAX, f64::MAX);
        for round in 0..5u32 {
            // The plain side trades with counterparties that ALREADY have a
            // row, seated outside the timed region so the seat is not in it.
            let mut probe = base.clone();
            for i in 0..reps {
                let k = stranger(round, i);
                edet_state::apply(&mut probe, accept(Party::Key(k)), next_id(), 8, &[k, sk], 0)
                    .expect("the sponsor's reach carries the warm-up seats");
            }
            let t0 = Instant::now();
            for i in 0..reps {
                let k = stranger(round, i);
                edet_state::apply(&mut probe, accept(Party::Key(k)), next_id(), 8, &[k, sk], 0)
                    .expect("an ordinary acceptance against a member that exists");
            }
            plain = plain.min(t0.elapsed().as_secs_f64() * 1e3 / reps as f64);

            let mut probe = base.clone();
            let t0 = Instant::now();
            for i in 0..reps {
                let k = stranger(round + 100, i);
                edet_state::apply(&mut probe, accept(Party::Key(k)), next_id(), 8, &[k, sk], 0)
                    .expect("and the same acceptance, seating the counterparty");
            }
            seating = seating.min(t0.elapsed().as_secs_f64() * 1e3 / reps as f64);
        }
        println!("{n:10}  {u:12}  {plain:9.3} ms  {seating:9.3} ms  {:9.2}x", seating / plain.max(1e-9));
    }
    println!();
}

/// This process's resident set, read off `/proc/self/statm` — the one figure a
/// community-size budget needs that no timer gives.
fn resident() -> String {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else { return "n/a".into() };
    let pages: u64 = statm.split_whitespace().nth(1).and_then(|f| f.parse().ok()).unwrap_or(0);
    format!("{} MB", pages * 4096 / (1024 * 1024))
}

/// **A partial settlement of an insured obligation costs what a shrink costs,
/// not what a network build costs.** Both arms on one community in one
/// sitting, as a ratio: re-solving the hold read 0.7 ms against a microsecond
/// at 2,000 accounts and 2.3 ms at 10,000 — four thousand to one — for a free
/// transition bounded only by its amount.
#[test]
#[ignore]
fn a_partial_settlement_costs_a_shrink_and_not_a_build() {
    use edet_state::tx::Tx;
    use edet_state::types::Party;
    use edet_swarm::driver::{Audit, Driver};
    let key = |i: usize| -> Key {
        let mut k = [0u8; 32];
        k[0..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
        k
    };
    for (n, deg, u) in [(2_000usize, 8usize, 20usize), (10_000, 8, 100)] {
        let mut st = community(n, deg, u);
        let orphan_key = [0x64u8; 32];
        let orphan = st.new_account(vec![orphan_key]);
        let mut d = Driver::new(st).audit_mode(Audit::Off);
        let (debtor, creditor) = (u as u64, (u + 1) as u64);
        let insured = d.st.next_contract;
        d.ok(
            Tx::Accept {
                debtor: Party::Member(debtor),
                creditor: Party::Member(creditor),
                amount: 400.0,
                maturity_epochs: 30,
                arb: None,
            },
            &[key(debtor as usize), key(creditor as usize)],
        );
        assert!(d.st.contracts[&insured].insured);
        let uninsured = d.st.next_contract;
        d.ok(
            Tx::Accept {
                debtor: Party::Member(orphan),
                creditor: Party::Member(creditor),
                amount: 400.0,
                maturity_epochs: 30,
                arb: None,
            },
            &[key(creditor as usize), orphan_key],
        );
        let rounds = 100u32;
        // The smallest partial the floor admits, which is NOT a 128th of
        // 400.00. That is 3.125, and 3.125 is not on the ledger's grid:
        // `to_minor` snaps to the nearest minor unit only within `MINOR_SNAP`
        // and floors otherwise, so a half-cent floors away to 312, and
        // 312 * MAX_INSTALLMENTS < 40000 is below the installment floor — the
        // row refuses it as ET-CTR-004 rather than measuring anything. The
        // floor is a share of the ORIGINAL, so the admissible amount is
        // ceil(40000 / 128) = 313 minor units.
        let t0 = Instant::now();
        for _ in 0..rounds {
            d.ok(Tx::Settle { contract: uninsured, amount: 3.13 }, &[orphan_key, key(creditor as usize)]);
        }
        let plain = t0.elapsed();
        let t1 = Instant::now();
        for _ in 0..rounds {
            d.ok(Tx::Settle { contract: insured, amount: 3.13 }, &[key(debtor as usize), key(creditor as usize)]);
        }
        let flow = t1.elapsed();
        println!(
            "{n} accounts, {} edges: uninsured partial settle {:.4} ms, insured {:.4} ms, ratio {:.1}x",
            d.st.edges.len(),
            plain.as_secs_f64() * 1e3 / rounds as f64,
            flow.as_secs_f64() * 1e3 / rounds as f64,
            flow.as_secs_f64() / plain.as_secs_f64().max(1e-9)
        );
    }
}
