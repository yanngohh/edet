//! The one driver: a distinct id per transaction, the clock the caller sets,
//! the gate lockstep before every write and the audit after it.
//!
//! **Every transition is audited.** The whole §Verification audit runs after
//! each call, accepted or refused, so a property that holds in the scene it
//! was written for and breaks the ledger elsewhere fails here rather than in
//! production. The state suite's `Chain` is this driver with the fixtures it
//! grew; a population is this driver with strategies in front of it. Two
//! drivers would drift, and the one that drifted would be the one nothing
//! else in the tree exercises.

use edet_kernel::constants as k;
use edet_state::errors::*;
use edet_state::invariants::{audit, audit_with_cache, AuditCache};
use edet_state::root::{state_root, RootCache};
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

/// Seconds at the start of `epoch`.
pub fn epochs(n: u64) -> u64 {
    n * k::EPOCH_SECS
}

/// A transition that is always well-formed, always bonded at the ordinary
/// multiple, and always fails on its own merits. Lets a caller spend headroom
/// without dragging other state along, and doubles as the probe for "does a
/// failed transaction still pay?".
pub fn bonded_dud() -> Tx {
    Tx::Extend { contract: u64::MAX, new_maturity_epoch: 9_999 }
}

/// When the audit runs.
///
/// `EveryTransition` is the definition and the only mode a test may use.
/// `EveryTick` keeps the gate lockstep on every write and moves the post-write
/// audit to the end of the tick, which is what makes a long run affordable;
/// a violation it finds is named exactly by re-running the seed in
/// `EveryTransition`, because the run is deterministic. `Off` is for
/// measurements, and `Run::corpus` refuses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Audit {
    EveryTransition,
    EveryTick,
    Off,
}

/// What `Driver::apply` last built, so a caller can resubmit it verbatim or
/// read back the id it spent.
#[derive(Clone, Debug)]
pub struct Envelope {
    pub tx: Tx,
    pub id: [u8; 32],
    pub not_after: u64,
    pub signers: Vec<Key>,
}

/// Every outstanding hold, taken together, is a CONSERVED FLOW.
///
/// Invariant 1 asks `drawn_into(S) <= GrossCapa(S)`, and `GrossCapa` is a
/// MAXIMUM flow, so any feasible flow of that value settles it. The ledger
/// already stores one: every obligation's `Held`. Feasibility is three
/// conditions and the audit already checks two — arc usage inside the stake
/// (invariant 4) and supply usage inside the declaration (invariant 5). This
/// is the third, and it is the one that has to hold for the stored data to be
/// a witness at all.
///
/// **A splitter that does not divide by route breaks it.** A proportional
/// split of each arc gives every underwriter a share of arcs its own supply
/// never reaches; the shares still sum to the original arc for arc, so every
/// invariant stays green, and then curing one piece leaves flow on arcs with
/// nothing arriving at them — measured under such a splitter: 71,429 leaving
/// an account with an inflow of zero.
///
/// Read straight off `reserved` / `committed` / the book, so it costs one pass
/// and no query. Driven after every transition and every epoch crank, which is
/// why it is here and not in a probe of its own.
pub fn witness_conserved(st: &State) -> Result<(), String> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut inflow: BTreeMap<usize, u64> = BTreeMap::new();
    let mut outflow: BTreeMap<usize, u64> = BTreeMap::new();
    for (&(c, d), &a) in &st.reserved {
        *outflow.entry(c).or_default() += a;
        *inflow.entry(d).or_default() += a;
    }
    let mut sink: BTreeMap<usize, u64> = BTreeMap::new();
    for con in st.contracts.values() {
        if con.insured && matches!(con.status, ContractStatus::Active | ContractStatus::Expired) {
            *sink.entry(con.debtor as usize).or_default() += con.held.amount();
        }
    }
    let nodes: BTreeSet<usize> = inflow
        .keys()
        .chain(outflow.keys())
        .chain(sink.keys())
        .chain(st.committed.keys())
        .copied()
        .collect();
    for v in nodes {
        let src = st.committed.get(&v).copied().unwrap_or(0);
        let i = inflow.get(&v).copied().unwrap_or(0);
        let o = outflow.get(&v).copied().unwrap_or(0);
        let s = sink.get(&v).copied().unwrap_or(0);
        if src + i != o + s {
            return Err(format!(
                "node {v}: supply {src} + in {i} = {} against out {o} + sink {s} = {}",
                src + i,
                o + s
            ));
        }
    }
    Ok(())
}

/// A driver that does what a consensus replica does: assigns each transaction
/// a distinct id, applies it, and audits the result.
pub struct Driver {
    pub st: State,
    /// The commit path's cut cache, driven in lockstep with the cold audit.
    ///
    /// Every suite and every run therefore gates the one property the cache
    /// has to have — that it never changes a verdict — over every scene
    /// anybody has written, rather than over the handful a dedicated probe
    /// would think of. `queries()` is what the cost probes read.
    pub cache: AuditCache,
    /// The write gate's own memo, driven in lockstep with the definition for
    /// the same reason the audit cache is: `due_with_cache` answers a
    /// threshold from a lower bound where it passes and exactly where it
    /// fails, and what it must never do is change a verdict.
    pub gate: edet_state::bond::GateCache,
    /// The incremental state root, driven beside the definition for the same
    /// reason the other two caches are: `state_root` is a pure function of a
    /// `State` and `RootCache::refresh` rehashes only what a block wrote, and
    /// what the second must never do is answer differently. Held equal after
    /// every transition and every epoch crank in the tree.
    pub root: RootCache,
    next_id: u64,
    audit: Audit,
    /// Whether to drive the cached audit beside the cold one. On everywhere
    /// except the exhaustive amount grids; see [`Driver::cold_audit_only`].
    check_cache: bool,
    /// Whether a violation stops the process or is handed back. A harness
    /// wants the panic and its backtrace; a search wants the seed, the tick
    /// and the transition, which only the caller holds.
    halt: bool,
    violation: Option<String>,
    last: Option<Envelope>,
}

impl Driver {
    /// A driver over any genesis: a founded fixture, or a node's own.
    pub fn new(st: State) -> Self {
        Driver {
            st,
            cache: AuditCache::default(),
            gate: Default::default(),
            root: Default::default(),
            next_id: 0,
            audit: Audit::EveryTransition,
            check_cache: true,
            halt: true,
            violation: None,
            last: None,
        }
    }

    pub fn audit_mode(mut self, mode: Audit) -> Self {
        self.audit = mode;
        self
    }

    /// Drive only the cold audit, for a fixture that repeats one scene shape
    /// tens of thousands of times.
    ///
    /// The agreement between the two audits is a property of the cache, not of
    /// the amount being walked, so re-checking it once per cent from 0.02 to
    /// 1,000.00 buys nothing over checking it everywhere else in the tree —
    /// and it doubles the audit work in the slowest fixture there is. The COLD
    /// audit still runs after every transition here.
    pub fn cold_audit_only(mut self) -> Self {
        self.check_cache = false;
        self
    }

    /// Hand a violation back instead of panicking on it. What a search needs:
    /// the seed, the tick, the agent and the intent are the caller's, and a
    /// panic loses all four.
    pub fn report_violations(mut self) -> Self {
        self.halt = false;
        self
    }

    /// A copy of this driver to age alongside it, for a probe that has to
    /// separate the effect it is measuring from the decay every epoch boundary
    /// applies. **Measure against a counterfactual, not a before**: a window
    /// that also advances the clock gets credited with the clock's work.
    pub fn clone_for_control(&self) -> Self {
        Driver {
            st: self.st.clone(),
            cache: Default::default(),
            gate: Default::default(),
            root: Default::default(),
            next_id: self.next_id,
            audit: self.audit,
            check_cache: self.check_cache,
            halt: self.halt,
            violation: None,
            last: None,
        }
    }

    /// A fresh transaction id. An atomic-free counter per driver is enough:
    /// each run owns its own driver, and ids never have to be unique across
    /// binaries — only within the state they are recorded in.
    pub fn tx_id(&mut self) -> [u8; 32] {
        self.next_id += 1;
        let mut id = [0u8; 32];
        id[..8].copy_from_slice(&self.next_id.to_be_bytes());
        id
    }

    /// Apply under a fresh id and a window wide enough never to be the reason
    /// a call fails. The envelope rules have their own probes.
    pub fn apply(&mut self, tx: Tx, signers: &[Key]) -> Res<()> {
        let id = self.tx_id();
        let not_after = self.st.epoch + 5;
        self.apply_raw(tx, id, not_after, signers)
    }

    /// Apply with the envelope fields chosen by the caller — replay, expiry
    /// and window are exactly what these want to control.
    pub fn apply_raw(&mut self, tx: Tx, id: [u8; 32], not_after: u64, signers: &[Key]) -> Res<()> {
        let now = self.st.last_begin_secs;
        self.apply_at(tx, id, not_after, signers, now)
    }

    /// The same, at a stated wall clock — what replaying a committed block
    /// needs, since a block carries its own `time_secs` and the driver must
    /// not substitute its own.
    pub fn apply_at(&mut self, tx: Tx, id: [u8; 32], not_after: u64, signers: &[Key], now_secs: u64) -> Res<()> {
        // **The gate memo may not change a verdict**, and this is where that is
        // checked — before the write, on the state the gate will see, over
        // every scene anybody has written rather than over the handful a
        // dedicated probe would think of. `due` is the definition;
        // `due_with_cache` is what a validator runs.
        //
        // `begin_block` first, because `apply` runs it and both forms have to
        // be asked about the same state: an epoch boundary is one of the three
        // things that invalidate the memo, and asking across one would compare
        // two different states rather than two readings of one.
        self.st.begin_block(now_secs);
        if self.audit != Audit::Off {
            let cold = edet_state::bond::due(&self.st, &tx, signers);
            let warm = edet_state::bond::due_with_cache(&self.st, &tx, signers, &mut self.gate);
            assert_eq!(cold, warm, "the gate memo changed a verdict");
        }
        self.last = Some(Envelope { tx: tx.clone(), id, not_after, signers: signers.to_vec() });
        let out = edet_state::apply_with_cache(&mut self.st, tx, id, not_after, signers, now_secs, &mut self.gate);
        if self.audit == Audit::EveryTransition {
            self.audit_now("after a transition");
        }
        out
    }

    /// Open a block at `now_secs`: the epoch boundaries it crosses, then the
    /// audit. What replaying a committed block does.
    pub fn begin(&mut self, now_secs: u64) {
        self.st.begin_block(now_secs);
        self.audit_now("after the epoch crank");
    }

    /// Advance to `epoch`, running every epoch boundary along the way.
    pub fn goto(&mut self, epoch: u64) {
        self.begin(epochs(epoch));
    }

    pub fn ok(&mut self, tx: Tx, signers: &[Key]) {
        self.apply(tx, signers).expect("transition should have been accepted");
    }

    pub fn err(&mut self, tx: Tx, signers: &[Key], code: Code) {
        assert_eq!(self.apply(tx, signers), Err(Error(code)), "expected {code}");
    }

    /// The envelope the last `apply` built — its id, its window and the
    /// signers it went out with. A griefer's replay reads it back.
    pub fn last_envelope(&self) -> Option<&Envelope> {
        self.last.as_ref()
    }

    /// The witness, the cold audit and the cached one, held to the same
    /// verdict. Panics unless the driver was asked to report instead.
    pub fn audit_now(&mut self, when: &str) {
        if self.audit == Audit::Off {
            // The journals are still drained, and the cache still forgets
            // what it knew. A search that never refreshes would otherwise
            // accumulate dirty keys for the length of the run.
            self.root.discard(&mut self.st);
            return;
        }
        if let Err(e) = self.check(when) {
            if self.halt {
                panic!("{e}");
            }
            if self.violation.is_none() {
                self.violation = Some(e);
            }
        }
    }

    /// The first violation this driver saw, if it was asked to report them.
    pub fn violation(&self) -> Option<&str> {
        self.violation.as_deref()
    }

    /// Run the witness, the cold audit and the cached one, and hold them to
    /// the same verdict. The cached form skips work a monotone argument
    /// already proves; what it may never do is skip a violation, so the two
    /// are driven side by side over every scene rather than only where a probe
    /// remembers to look.
    fn check(&mut self, when: &str) -> Result<(), String> {
        if let Err(e) = witness_conserved(&self.st) {
            return Err(format!("the holds are not a conserved flow {when}: {e}"));
        }
        // **The cached root is the definition.** Both forms are computed on
        // every state this driver reaches, which is every scene anybody has
        // written rather than the handful a dedicated probe would think of.
        //
        // Under `cold_audit_only` the journals are drained and the cache
        // forgets, for the reason that flag exists at all: agreement is a
        // property of the cache rather than of the amount being walked, and
        // re-checking it once per cent buys nothing over checking it
        // everywhere else in the tree.
        if self.check_cache {
            let definition = state_root(&self.st);
            let cached = self.root.refresh(&mut self.st);
            if definition != cached {
                return Err(format!("the cached state root left the definition {when}: {definition:?} vs {cached:?}"));
            }
        } else {
            self.root.discard(&mut self.st);
        }
        let cold = audit(&self.st);
        if !self.check_cache {
            return match cold {
                Ok(()) => Ok(()),
                Err(v) => Err(format!("invariant broken {when}: {v:?}")),
            };
        }
        let warm = audit_with_cache(&self.st, &mut self.cache);
        match (&cold, &warm) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(v), Err(_)) => Err(format!("invariant broken {when}: {v:?}")),
            (Ok(()), Err(w)) => Err(format!("the cache invented a violation {when}: {w:?}")),
            (Err(v), Ok(())) => Err(format!("the cache HID a violation {when}: {v:?}")),
        }
    }
}
