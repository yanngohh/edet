//! A replica: the deterministic state machine driven by decided blocks.
//! Same genesis + same block stream = same state hash, on every device.

use std::collections::BTreeMap;

use edet_state::types::{Key, MemberId};
use edet_state::State;

use crate::block::{Block, CodecError};
use crate::store::{Store, StoreError, ValidatorKeysAt, ValidatorSetAt};

#[derive(Debug)]
pub enum ReplicaError {
    Store(StoreError),
    Codec(CodecError),
    HeightGap {
        expected: u64,
        got: u64,
    },
    /// A decided block carried a transaction whose signature did not verify.
    /// The authenticated commit path (`commit_block`) refuses it; only the
    /// explicit `commit_block_unchecked` escape hatch bypasses the check.
    UnverifiedTx {
        height: u64,
    },
    /// A block broke a rule every node can check without applying anything:
    /// too many transactions, or an envelope no required signer signed
    /// (`block::deterministic_validity`).
    ///
    /// Checked HERE as well as in the pre-vote screen, because a certificate
    /// can arrive by sync rather than by vote — the screen is what keeps a
    /// Byzantine proposal from being certified, and this is what keeps one
    /// that was certified anyway from being applied.
    Refused {
        height: u64,
        why: crate::block::BlockRefusal,
    },
    /// Applying a block left the ledger in a state `edet_state::invariants`
    /// says cannot exist — conservation broken, a contract with one member on
    /// both sides, a key indexed to the wrong member.
    ///
    /// Fail-stop, and deliberately so. The invariants encode the paper's
    /// theorems (the paper's §Verification), and a violated one means the
    /// transition function did something no reading of the rules allows. Every
    /// honest node runs the same deterministic code on the same block, so all
    /// of them stop at the same height rather than diverging — the same shape
    /// as `AppHashMismatch`. Committing corrupted state and carrying on is the
    /// one outcome from which there is no recovery: the root would certify it
    /// and it would become the chain's history.
    InvariantViolated {
        height: u64,
        detail: String,
    },
    /// This replica already stopped at `height`, and commits nothing further.
    /// See `Replica::halted`.
    Halted {
        height: u64,
        detail: String,
    },
    /// A block's `time_secs` was less than the last committed block's —
    /// it would rewind the ledger's clock. Checked at every point a block
    /// enters this replica from outside its own construction: `commit_block`,
    /// `commit_block_with_certificate`, and WAL replay in `open`. See
    /// `commit_block`'s doc comment for why this is monotonicity ONLY, never
    /// the plausibility/skew half of the rule — that half needs a local
    /// clock, and a local clock has no business on this deterministic path.
    NonMonotonicTime {
        expected_at_least: u64,
        got: u64,
    },
    /// The snapshot found in the data directory describes a different
    /// chain than the genesis this replica was opened with. `chain_id` and
    /// `root_salt` are genesis-fixed — no transition in `apply.rs` ever
    /// writes either — so a disagreement is never a legitimate evolution of
    /// state: it is a data dir belonging to another chain, or a genesis file
    /// that was corrected while a node that had already run kept the old
    /// values. Both must stop the node rather than resolve silently in
    /// favour of whichever copy happens to be on disk.
    GenesisMismatch {
        field: &'static str,
        genesis: String,
        snapshot: String,
    },
    /// A block's `app_hash` did not match the state this replica
    /// actually holds before applying it. Checked at every point a block
    /// enters this replica from outside its own construction: `commit_block`,
    /// `commit_block_with_certificate`, and WAL replay in `open` — the same
    /// three chokepoints `NonMonotonicTime` guards, and for the same reason:
    /// this is a pure function of locally-derived state, so it belongs on the
    /// deterministic commit path, not only in the local pre-vote screen. A
    /// node that hits this has DIVERGED from the rest of the network (or its
    /// own prior WAL is corrupt) — refusing is the loud halt that replaces
    /// the silent, permanent inconsistency it exists to prevent.
    AppHashMismatch {
        expected: [u8; 32],
        got: [u8; 32],
    },
}

impl From<StoreError> for ReplicaError {
    fn from(e: StoreError) -> Self {
        ReplicaError::Store(e)
    }
}
impl From<CodecError> for ReplicaError {
    fn from(e: CodecError) -> Self {
        ReplicaError::Codec(e)
    }
}
impl std::fmt::Display for ReplicaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplicaError::Store(e) => write!(f, "store: {e}"),
            ReplicaError::Codec(e) => write!(f, "codec: {e}"),
            ReplicaError::HeightGap { expected, got } => {
                write!(f, "height gap: expected {expected}, got {got}")
            }
            ReplicaError::UnverifiedTx { height } => {
                write!(f, "unverified transaction in decided block at height {height}")
            }
            ReplicaError::Refused { height, why } => {
                write!(f, "block {height} refused before it was applied: {why}")
            }
            ReplicaError::InvariantViolated { height, detail } => {
                write!(f, "block {height} left the ledger in an impossible state: {detail}")
            }
            ReplicaError::Halted { height, detail } => {
                write!(f, "this node stopped at height {height}: {detail}")
            }
            ReplicaError::NonMonotonicTime { expected_at_least, got } => {
                write!(f, "block time {got} is before the last committed block's {expected_at_least}")
            }
            ReplicaError::GenesisMismatch { field, genesis, snapshot } => {
                write!(
                    f,
                    "the store in this data directory belongs to a different chain: its {field} is {snapshot}, \
                     the genesis this node was started with says {genesis}. Point this node at its own data \
                     directory, or remove the store to resync from genesis — never both at once"
                )
            }
            ReplicaError::AppHashMismatch { expected, got } => {
                write!(
                    f,
                    "block app_hash {} does not match this replica's {} — the replica has diverged or the block is corrupt",
                    crate::block::hex32(got),
                    crate::block::hex32(expected)
                )
            }
        }
    }
}
impl std::error::Error for ReplicaError {}

/// Per-transaction outcome inside a decided block. Rejections are ordinary
/// and deterministic — every replica records the same ones.
pub type TxOutcome = Result<(), edet_state::Error>;

/// **One day of blocks**, at the one-a-second pace an empty block is held to.
///
/// The figure is a duration rather than a size: what it buys is how long a
/// validator may be down and still rejoin from a peer without an operator
/// carrying a snapshot. A day of frames is the outage a reboot, a move or a
/// weekend produces, and 86,400 empty frames is a few tens of megabytes —
/// against a WAL that grows with the chain's AGE if it is never pruned, about
/// 31.5 million frames a year, every one of them read and decoded at every
/// start.
pub const DEFAULT_PRUNE_MARGIN_BLOCKS: u64 = 86_400;

pub struct Replica {
    pub state: State,
    pub height: u64,
    store: Option<Store>,
    /// Write a snapshot every this many blocks (0 = never).
    pub snapshot_interval: u64,
    /// **How far back the WAL is kept**, in blocks, and therefore how long a
    /// validator may be down and still rejoin from a peer.
    ///
    /// A node that has pruned below a peer's height cannot serve it, and the
    /// peer cannot catch up from anybody who has pruned the same range: below
    /// every peer's floor the only way back is an operator carrying a
    /// snapshot (`export-snapshot` / `import-snapshot`). So this is an
    /// operational choice — disk against how long an outage may last — and
    /// not a constant.
    ///
    /// See `DEFAULT_PRUNE_MARGIN_BLOCKS` for the default and what it costs.
    pub prune_margin_blocks: u64,
    /// Timestamp carried by the last committed block (0 before any commit).
    /// The wall-clock proposer reads this to stay monotonic across a
    /// restart, since the durable state itself doesn't retain it.
    pub last_time_secs: u64,
    /// The state commitment (`state_hash`) of `state` RIGHT NOW — i.e.
    /// exactly the value the NEXT block must claim as its own `app_hash`.
    /// Cached rather than recomputed on demand: `state_hash` is O(state
    /// size), and every consensus round touches this at least twice
    /// (propose, screen) plus once more at commit — recomputing it that
    /// often would make an O(1) field lookup into an O(state size) one on
    /// every single step of the hot path. Updated in exactly one place,
    /// `commit_block_unchecked`, in the same uninterruptible run of
    /// assignments as `height`/`last_time_secs` and before any fallible
    /// durable write: a store error between the two would leave this
    /// cache describing a state the replica no longer held. Initialized in
    /// `new`/`open` to cover height 0 / the resumed snapshot height before
    /// any block has been applied.
    app_hash: [u8; 32],
    /// **The tree `app_hash` is the root of**, held across commits so an
    /// ordinary block rehashes the rows it wrote and the paths above them
    /// rather than the whole ledger (`edet_state::root::RootCache`).
    ///
    /// A third sibling of the two caches below, under the same rule: the pure
    /// `state_root` is the definition, this is what a validator runs, and the
    /// swarm driver holds them equal after every transition in the tree. It
    /// differs from those two in one way — it decides a CONSENSUS value
    /// rather than how much work to do — which is why the equality is driven
    /// everywhere rather than checked by a probe.
    ///
    /// Not persisted: `open` builds it once from the snapshot, which costs
    /// one boundary block.
    root_cache: edet_state::root::RootCache,
    /// Set once, by the audit, and never cleared: the height whose block left
    /// the ledger in a state the invariants say cannot exist, and what they
    /// said.
    ///
    /// **The fail-stop, in memory.** A block applies in place, so a refused
    /// one leaves the mutated ledger behind — and the engine's answer is to
    /// return out of `run` and bring the process down, which takes a moment
    /// during which the read surface is still up. This is what it answers
    /// with (`serve::http`, 503), and what refuses every later commit. It is
    /// not persisted and does not need to be: the WAL append happens after
    /// the audit, so a restart comes back at the last height that passed, and
    /// `open` audits the snapshot and every block it replays.
    halted: Option<Halt>,
    /// The audit's cut cache — a sibling of `app_hash` above and cached for
    /// the same reason, that a per-block `O(state size)` computation on the
    /// hot path should be paid once and only when its inputs moved.
    ///
    /// It differs in one way that matters: `app_hash` is consensus-visible and
    /// this is not. It decides only whether a dominated max-flow is run, never
    /// what the audit concludes, so two replicas with different cache warmth
    /// commit and halt identically. That is what lets it be history-dependent
    /// at all; see `edet_state::invariants::AuditCache`.
    audit_cache: edet_state::invariants::AuditCache,
    /// **The write gate's `seed_reach` memo**, carried across commits exactly
    /// as `audit_cache` is and for the same reason: both questions the gate
    /// asks read a full pristine max-flow cut, and an ordinary bonded
    /// transaction asked for one or two of them on every write. A lower bound
    /// settles a threshold whenever it passes, and a bound that fails is
    /// recomputed exactly, so the memo never changes a verdict
    /// (`edet_state::bond::GateCache`).
    pub gate_cache: edet_state::bond::GateCache,
    /// (the paper's §Implementation):
    /// the validator set as of every height it actually changed, ascending
    /// by height — never one entry per block. `validators_at` answers "what
    /// was live at height N" from this instead of re-deriving `State` by
    /// full replay, which is what `ConsensusReady`/`GetValidatorSet`/
    /// `Decided` will eventually need to verify a certificate against the
    /// set that was live at its height.
    validator_history: Vec<ValidatorSetAt>,
    /// The validator KEY set as of every height a committed block
    /// actually rotated one — mirrors `validator_history` exactly (same
    /// append-only, change-only retention), but tracks signing keys instead
    /// of voting power. `validators_at` alone was never enough to verify a
    /// HISTORICAL certificate: it tells you WHO was a validator and with how
    /// much power, but resolving "with which key" from `state.members`
    /// (the CURRENT member table) is wrong the moment that validator has
    /// rotated since — exactly the gap this journal closes. See `keys_at`
    /// and `commit_block_unchecked`'s doc comment for the exact retention
    /// rule.
    validator_key_history: Vec<ValidatorKeysAt>,
}

/// Where a replica stopped, and what the audit said when it did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Halt {
    pub height: u64,
    pub reason: String,
}

/// The cache's root, with its error carried the way `block::state_hash`
/// carries the definition's.
fn cached_root(cache: &mut edet_state::root::RootCache, state: &mut State) -> Result<[u8; 32], ReplicaError> {
    cache.refresh(state).map_err(|e| ReplicaError::Codec(CodecError::Encode(e.0)))
}

impl Replica {
    pub fn new(genesis: State) -> Replica {
        let validator_history = vec![(0, genesis.validators.clone())];
        let validator_key_history = vec![(0, validator_key_snapshot(&genesis, &genesis.validators))];
        // The genesis case: a chain's first block (height 1) must claim the
        // hash of the GENESIS state itself, which is exactly what falls out of
        // initializing the cache here rather than leaving it zeroed. A zero
        // here would make height 1 claim a state no node holds, so the first
        // block of every chain would fail its own verification.
        //
        // That list has not existed for two branches — a comment is the one
        // part of the tree no gate reads.)
        let mut genesis = genesis;
        let mut root_cache = edet_state::root::RootCache::default();
        let app_hash = root_cache
            .refresh(&mut genesis)
            .expect("State always encodes: a finite in-memory struct serializing into a Vec cannot fail");
        Replica {
            state: genesis,
            height: 0,
            store: None,
            snapshot_interval: 0,
            prune_margin_blocks: DEFAULT_PRUNE_MARGIN_BLOCKS,
            last_time_secs: 0,
            // Cold, and correct cold: the first commit does the full audit.
            audit_cache: Default::default(),
            gate_cache: Default::default(),
            root_cache,
            halted: None,
            validator_history,
            validator_key_history,
            app_hash,
        }
    }

    /// Open a durable replica: recover from snapshot + WAL if present,
    /// otherwise start from the provided genesis.
    pub fn open(
        dir: impl AsRef<std::path::Path>,
        genesis: State,
        snapshot_interval: u64,
    ) -> Result<Replica, ReplicaError> {
        let mut store = Store::open(dir)?;
        // A snapshot REPLACES the supplied genesis — which is what
        // resuming means, and is correct for everything the ledger evolves.
        // It is not correct for the two fields nothing evolves: `chain_id`
        // and `root_salt` are written once at genesis and never touched by
        // any transition, so the resumed state and the genesis file must
        // agree about them or one of them is not describing this node's
        // chain. Silently keeping the snapshot's copy is what made a
        // corrected genesis a no-op on every node that had already run — the
        // documented remediation for a chain booted on the wrong chain id or
        // the published dev salt did nothing at all — and what let a
        // reused or misplaced data dir put a validator on a chain its own
        // genesis file says it is not on.
        let (mut state, mut height) = match store.read_snapshot()? {
            Some((h, s)) => {
                if s.chain_id != genesis.chain_id {
                    return Err(ReplicaError::GenesisMismatch {
                        field: "chain_id",
                        genesis: genesis.chain_id.clone(),
                        snapshot: s.chain_id.clone(),
                    });
                }
                if s.root_salt != genesis.root_salt {
                    return Err(ReplicaError::GenesisMismatch {
                        field: "root_salt",
                        genesis: crate::block::hex32(&genesis.root_salt),
                        snapshot: crate::block::hex32(&s.root_salt),
                    });
                }
                (s, h)
            }
            None => (genesis, 0),
        };
        // Baseline the validator-set history at exactly the point
        // `state` starts from. Whether that's the snapshot's own
        // `state.validators` or the caller-supplied genesis, it is already
        // — by construction, since it's the same field the resumed `state`
        // carries — the correct live set at `height`; no replay needed to
        // establish it, which is the "not full state re-derivation" half of
        // the spec's instruction.
        let mut validator_history = vec![(height, state.validators.clone())];
        // Merge in change entries the running node persisted since that
        // point (`append_validator_set`, called from `commit_block_unchecked`)
        // — the compact journal half of the same instruction. A data dir
        // from simply has none (`read_validator_history` reads a
        // missing file as empty), so an old dir opens exactly as before,
        // just with a single baseline entry instead of finer-grained
        // history — additive, not a breaking format change.
        for (h, validators) in store.read_validator_history()? {
            if h > height {
                validator_history.push((h, validators));
            }
        }
        // Same baseline-then-merge shape as `validator_history` just
        // above, over the key journal instead of the power journal. A data
        // dir written has no `keys.bin` (`read_validator_key_history`
        // reads that as empty), so this degrades to exactly the baseline
        // entry alone — every lookup then falls through `keys_at`'s "absent"
        // case straight to each member's current key, i.e. this crate's
        // the behaviour a tree with no key journal has, unchanged, on an old dir.
        let mut validator_key_history = vec![(height, validator_key_snapshot(&state, &state.validators))];
        for (h, keys) in store.read_validator_key_history()? {
            if h > height {
                validator_key_history.push((h, keys));
            }
        }
        let mut last_time_secs = 0;
        // The app_hash a block at `height+1` must claim is
        // the commitment of `state` exactly as it stands right now, before
        // that block is applied — tracked as a running value through the
        // whole replay, updated only after a block is actually applied,
        // mirroring `commit_block_unchecked`'s own live update. This makes
        // WAL replay enforce the identical invariant a network commit does:
        // a poisoned store (a block that claims the wrong app_hash and
        // somehow made it onto disk — a bug, a bypassed screen, a bit flip)
        // is caught HERE, at open, instead of being silently re-absorbed as
        // legitimate history on every restart the way a bomb block was
        // before that check existed.
        // **The store this node is resuming from is audited before it is
        // trusted**, starting with the snapshot
        // it starts from. A snapshot is written by this node's own commit
        // path, so a healthy one always passes; what this refuses is a store
        // written by a build with a defect the audit catches, or damaged
        // in place — which is exactly the input a fail-stop exists for, and
        // exactly what "every honest node stops at the same height" stops
        // meaning if a restart absorbs it.
        //
        // One cold audit per process start, before any block is replayed.
        if let Err(v) = edet_state::invariants::audit(&state) {
            return Err(ReplicaError::InvariantViolated { height, detail: v.0 });
        }
        // The replay's own cache, warmed block by block exactly as the commit
        // path warms it — so replaying a long WAL costs what committing it
        // cost, rather than a full cold audit per block. The warm cache is
        // then handed to the replica this builds, which is strictly better
        // than a cold one.
        let mut audit_cache = edet_state::invariants::AuditCache::default();
        let mut gate_cache = edet_state::bond::GateCache::default();
        // Built once, here, and then patched per replayed block: a long WAL
        // replay would otherwise pay a full root for every height in it.
        let mut root_cache = edet_state::root::RootCache::default();
        let mut app_hash = cached_root(&mut root_cache, &mut state)?;
        for block in store.read_wal()? {
            if block.height <= height {
                continue;
            }
            if block.height != height + 1 {
                return Err(ReplicaError::HeightGap { expected: height + 1, got: block.height });
            }
            // A WAL is trusted-local input, not network input — but a
            // corrupt or poisoned store (the exact scenario this whole
            // defence exists for: a bomb block that made it past a bug or a
            // bypassed screen, got written, and would otherwise re-brick this
            // node on every restart) must be reported, not silently replayed.
            // `apply_block_to` runs `begin_block` regardless of what we do
            // here, so structurally this can't hang (Layer 1 always clamps),
            // but a rewinding block replayed straight through would silently
            // corrupt `last_time_secs` and, via it, every future block's
            // epoch derivation — refusing here is what keeps a poisoned WAL
            // from being absorbed as if it were legitimate history.
            if !crate::block::time_is_monotonic(block.time_secs, last_time_secs) {
                return Err(ReplicaError::NonMonotonicTime { expected_at_least: last_time_secs, got: block.time_secs });
            }
            if block.app_hash != app_hash {
                return Err(ReplicaError::AppHashMismatch { expected: app_hash, got: block.app_hash });
            }
            // **A replayed block is audited, exactly like a committed one**

            //
            // It was not, and that is what made the fail-stop one restart
            // deep: a block the commit-path audit rejected was already in this
            // WAL (the append came first), so replay took it back in with no
            // audit at all and the node carried on from the state the audit
            // had refused. Both halves are closed — the append now happens
            // after the audit, and this is the second lock, because a WAL
            // written by an older build or damaged in place is exactly the
            // input a fail-stop is for.
            //
            // Refusing here is a REFUSAL TO RESUME rather than a repair. The
            // store's own bit-rot tolerance drops a frame only when it can
            // prove nothing needs it (`quarantine_tail`); this frame is
            // needed by definition — it is the tail of the history this node
            // is trying to continue — so the honest answer is to stop with the
            // height named, and leave the store intact for whoever looks.
            if let Err(why) = crate::block::deterministic_validity(&block, &state) {
                return Err(ReplicaError::Refused { height: block.height, why });
            }
            apply_block_to(&mut state, &block, &mut gate_cache);
            if let Err(v) = edet_state::invariants::audit_with_cache(&state, &mut audit_cache) {
                return Err(ReplicaError::InvariantViolated { height: block.height, detail: v.0 });
            }
            height = block.height;
            last_time_secs = block.time_secs;
            app_hash = cached_root(&mut root_cache, &mut state)?;
        }
        Ok(Replica {
            state,
            height,
            store: Some(store),
            snapshot_interval,
            prune_margin_blocks: DEFAULT_PRUNE_MARGIN_BLOCKS,
            last_time_secs,
            // Warmed by the replay above, which audited every block it took
            // in. A replica that replayed nothing hands over a cold cache,
            // which is the case this field was documented for: it simply does
            // more work and reaches the same verdict.
            audit_cache,
            gate_cache,
            root_cache,
            halted: None,
            validator_history,
            validator_key_history,
            app_hash,
        })
    }

    /// The state commitment as of RIGHT NOW — before whatever block is
    /// currently being proposed/screened/committed is applied. This is the
    /// value the NEXT block must carry as its own `app_hash` (see that
    /// field's doc comment for why "before", never "after itself", is the
    /// only claim that can be checked before a vote). O(1): reads the cache,
    /// never recomputes `state_hash`.
    pub fn app_hash(&self) -> [u8; 32] {
        self.app_hash
    }

    /// The earliest height this replica can still serve to a syncing peer.
    ///
    /// One, until the WAL is pruned; after that, the lowest height still in
    /// it. A node that has dropped blocks must SAY so — answering `1` when the
    /// log starts at 90,000 makes every sync request below that a silent
    /// failure, and the peer cannot tell "this node has pruned" from "this
    /// node is broken".
    pub fn history_min_height(&self) -> u64 {
        self.store.as_ref().and_then(|s| s.min_block_height()).unwrap_or(1)
    }

    /// The validator set live as of `height` — the value `state.validators`
    /// held immediately after the block at `height` committed (height 0 =
    /// the genesis set, before any block). A `Suspend`/`ValidatorPower`/
    /// `Exit` committed at height H is reflected starting at `validators_at(H)`
    /// and stays live at every later height until the next change, which is
    /// exactly what lets a Suspend committed at H take effect for H+1
    /// with no special-casing at the query site. Returns `None` only if
    /// `height` predates the oldest entry this replica has ever retained
    /// (older than its baseline at open/genesis).
    pub fn validators_at(&self, height: u64) -> Option<&BTreeMap<MemberId, u64>> {
        self.validator_history.iter().rev().find(|(h, _)| *h <= height).map(|(_, v)| v)
    }

    /// The validator KEY map live as of `height` — the companion to
    /// `validators_at`. Unlike `validators_at`, returns an OWNED, possibly
    /// EMPTY map rather than `Option<&_>`: "no journal entry at or before
    /// `height`" is the common, expected case (most validators never rotate)
    /// and callers (`EdetValidatorSet::build`) need to treat "id absent from
    /// this map" as "fall back to the member's current key" regardless of
    /// whether that's because the whole map is empty or just missing that
    /// one id — folding both into the same lookup keeps that fallback a
    /// single rule instead of two.
    ///
    /// Soundness of the fallback: an id absent from `keys_at(height)` means
    /// no block ever recorded a key rotation for a validator holding that id
    /// at a height `<= height` (`commit_block_unchecked`'s trigger rule) —
    /// which means either this member has never rotated a key at all, or
    /// every rotation they've ever done happened while they were NOT a
    /// validator. Either way, their key at every height they WERE validating
    /// through `height` equals their key right now, so resolving from
    /// `state.members` at query time is exactly correct, not a guess.
    pub fn keys_at(&self, height: u64) -> BTreeMap<MemberId, Key> {
        self.validator_key_history
            .iter()
            .rev()
            .find(|(h, _)| *h <= height)
            .map(|(_, k)| k.clone())
            .unwrap_or_default()
    }

    /// Apply a decided block, refusing any whose transactions are not
    /// properly signed (`Block::verify_txs`). This is the authentication
    /// chokepoint of the commit path: `engine_malachite::screen` already
    /// screens peer proposals before voting, but enforcing verification here
    /// too makes it an invariant every commit inherits, whatever decided the
    /// block — so the `signers`-trusted state-application path can never book
    /// an obligation nobody signed. Blocks a validator did not receive over
    /// the network (trusted local WAL replay, tests) go through
    /// `commit_block_unchecked` / `open` instead.
    ///
    /// Also refuses a block whose `time_secs` is before the last
    /// committed block's — MONOTONICITY ONLY, deliberately not the
    /// plausibility/skew half of the timestamp rule (`crate::block::
    /// time_is_plausible`). That half needs a local wall clock, and this is
    /// the wrong place for one: `commit_block` must be a pure function of the
    /// block stream, because every honest node has to reach the SAME
    /// accept/reject decision on the SAME certified block. A local clock read
    /// here would let two nodes with different clocks disagree about whether
    /// an already-certified block commits — forking the ledger, not
    /// protecting it. The skew bound belongs only where a vote is a local
    /// decision: the pre-vote screen (`engine_malachite::screen`), which
    /// every honest validator runs BEFORE a
    /// block can ever reach quorum, is what keeps an implausible timestamp
    /// from getting this far in the first place. This is the deterministic
    /// backstop for the rewind direction alone.
    pub fn commit_block(&mut self, block: &Block) -> Result<Vec<TxOutcome>, ReplicaError> {
        if !block.verify_txs(&self.state.chain_id) {
            return Err(ReplicaError::UnverifiedTx { height: block.height });
        }
        if !crate::block::time_is_monotonic(block.time_secs, self.last_time_secs) {
            return Err(ReplicaError::NonMonotonicTime {
                expected_at_least: self.last_time_secs,
                got: block.time_secs,
            });
        }
        // Unlike the plausibility check, this compares the block
        // against LOCALLY DERIVED state, and every correct node derives the
        // same state from the same block stream — so this is a pure,
        // deterministic function of the block stream, exactly like the
        // monotonicity check above, and belongs on the commit path for the
        // same reason. A node that fails this has diverged (or is being fed
        // a corrupt/forged block); halting here is the point of the fail-stop, not a
        // bug to work around.
        if block.app_hash != self.app_hash {
            return Err(ReplicaError::AppHashMismatch { expected: self.app_hash, got: block.app_hash });
        }
        self.commit_block_unchecked(block)
    }

    /// Apply a decided block WITHOUT verifying its transaction signatures:
    /// advances time, applies every transaction (rejections recorded, never
    /// fatal), persists, snapshots on interval.
    ///
    /// The escape hatch for callers that operate BELOW the signature layer on
    /// locally-constructed, trusted blocks: the replication/persistence tests
    /// and the demo. NEVER call this for a block received from the network —
    /// that path must go through `commit_block`, which authenticates it. (WAL
    /// replay in `open` is likewise trusted and applies its blocks directly.)
    pub fn commit_block_unchecked(&mut self, block: &Block) -> Result<Vec<TxOutcome>, ReplicaError> {
        // A halted replica commits nothing. The in-memory ledger is the one
        // the audit refused, so applying anything on top of it would be
        // building on the state the fail-stop exists to stop at.
        if let Some(halt) = &self.halted {
            return Err(ReplicaError::Halted { height: halt.height, detail: halt.reason.clone() });
        }
        if block.height != self.height + 1 {
            return Err(ReplicaError::HeightGap { expected: self.height + 1, got: block.height });
        }
        // The rules a node can check without applying anything — the batch
        // ceiling and "every envelope authorises something". Both before the
        // WAL, because a block this refuses must leave no trace at all.
        if let Err(why) = crate::block::deterministic_validity(block, &self.state) {
            return Err(ReplicaError::Refused { height: block.height, why });
        }
        let validators_before = self.state.validators.clone();
        // Snapshot keys BEFORE applying the block, over the PRE-block
        // validator set — this is what a rotation comparison needs to be
        // against: the key each currently-validating member held going into
        // this block.
        let keys_before = validator_key_snapshot(&self.state, &validators_before);
        // **Applied IN PLACE, and the fail-stop is a halt rather than a
        // rollback.**
        //
        // The depth of the fail-stop is the WAL append below — which happens
        // after the audit, so a refused block leaves no trace on disk and no
        // restart can resurrect it — together with the audit `open` runs on
        // the snapshot and on every block it replays. Neither of those needs
        // a working copy. What a copy would protect is the in-memory state
        // between the audit's refusal and the process exiting, and `halted`
        // is what that costs instead: this replica commits nothing further
        // and every read answers with the height it stopped at and why.
        //
        // The copy was a whole-ledger pass per block for that window —
        // measured at 17.5 ms at 10,000 rows and 73.7 ms at 100,000, the same
        // order as the root and the audit beside it.
        let outcomes = apply_block_to(&mut self.state, block, &mut self.gate_cache);
        // The invariants run on the path that actually commits, not only in
        // tests and the development driver: they are the only thing standing
        // between a defect in the transition function and a corrupted ledger
        // being certified by quorum and becoming history.
        //
        // **They are not cheap.** "Cheap next to the epoch machinery
        // `begin_block` already ran" is the sentence that would put them here
        // unconditionally, and it is wrong: `begin_block` returns at its first
        // line on a repeated timestamp
        // and otherwise falls straight past its `while` without entering it, so
        // the epoch machinery is paid once per `EPOCH_SECS` — a day, against a
        // consensus round of a few seconds. The audit is paid every block, and
        // invariant 1 is evaluated over sets: `1 + U + 2` full max-flow queries
        // whatever the block contained, including nothing.
        //
        // Measured on an idle box (`just cost`, `crates/state/tests/cost.rs`),
        // at 1,000 / 5,000 / 10,000 / 20,000 accounts with `U = n/100`: the
        // ordinary block's `begin_block` is below the timer's resolution, the
        // epoch-crossing one costs 0.30 / 2.46 / 5.16 / 11.78 ms, and this
        // audit costs 8.4 / 271 / 1031 / 5107 ms — 28x to 434x the thing it is
        // said to be cheap beside, on the one block in ~17,000 that pays that
        // at all. At the top row a commit costs more than the whole default
        // consensus round this engine runs. The paper's §Implementation carries
        // the decision; this note carries what it costs.
        //
        // **Decided: memoise the cut rather than check fewer sets.** The
        // invariant must HOLD over the whole family after every block; it does
        // not follow that every set must be RECOMPUTED after every block. A
        // cut reads `(edges, supplies, id space)` and nothing else, and is
        // monotone in all three, so a cut this audit already verified stays a
        // valid lower bound until one of them falls — and `drawn(S) <= that`
        // closes the clause with no query. What may fall is a short structural
        // list rather than a hope about traffic: decay at the epoch boundary,
        // a governed re-denomination, and a `DeclareSupply` that reduces.
        // Settlement raises stakes and can only ever raise them.
        //
        // The three shapes NOT taken, because each changes what the ledger
        // means rather than what it computes: capping `U` closes §Standing's open
        // underwriter role; charging a standing fee for a live underwriter
        // seat is rent (`prop:no-rent`); and taking the audit off the commit
        // path changes what a commit guarantees. This one never changes a
        // verdict — only whether dominated work is performed — which is also
        // why the cache may be history-dependent without touching determinism:
        // that is a claim about the DECISION, not about the work, so this
        // replica resuming from a snapshot starts cold, does more of it, and
        // halts on exactly the states a warm replica halts on.
        if let Err(v) = edet_state::invariants::audit_with_cache(&self.state, &mut self.audit_cache) {
            // Nothing has been written: the WAL does not contain this block,
            // `height` has not advanced and `app_hash` still names the state
            // the previous block left. What HAS changed is the in-memory
            // ledger, so this replica stops — it commits nothing further and
            // answers every read with the height and the reason, rather than
            // serving a state the invariants say cannot exist for as long as
            // it takes the process to come down.
            self.halted = Some(Halt { height: block.height, reason: v.0.clone() });
            return Err(ReplicaError::InvariantViolated { height: block.height, detail: v.0 });
        }
        // The WAL append is here rather than at the top, and that ordering is
        // the whole of the fail-stop's depth: a block that fails the audit
        // must not be on disk to be replayed by the next restart.
        if let Some(store) = &mut self.store {
            store.append_block(block)?;
        }
        self.height = block.height;
        self.last_time_secs = block.time_secs;
        // The cached state commitment, recomputed HERE — with the
        // rest of the state-derived cache, immediately after `state` itself
        // changed and before anything that can fail. Every line below this
        // one is a durable write that can fail on a full disk, a revoked
        // permission or a transient I/O error, and none of them may sit
        // between the applied state and its cached commitment: a `?` out of
        // any of them left `height` advanced with the PREVIOUS height's
        // `app_hash` still cached, so this node then rejected every peer
        // proposal (the screen compares against this field) and proposed a hash no
        // peer would accept — an indefinite silent halt that only a restart
        // cleared. Computed exactly once per commit either way, so this
        // ordering costs nothing.
        self.app_hash = cached_root(&mut self.root_cache, &mut self.state)?;
        // Retain a new history entry only when this block actually
        // changed the validator set (`Suspend`/`ValidatorPower`/`Exit`,
        // `apply.rs`) — not on every block, which would make the journal
        // grow as fast as the WAL for no benefit.
        if self.state.validators != validators_before {
            self.validator_history.push((self.height, self.state.validators.clone()));
            if let Some(store) = &mut self.store {
                store.append_validator_set(self.height, &self.state.validators)?;
            }
        }
        // Retain a new KEY history entry only when this block actually
        // ROTATED the key of a member who was a validator both before and
        // after it (`RotateFinalize` on a `state.validators` member) — never
        // once per block, and deliberately NOT on a bare membership change
        // (a validator joining or leaving with no key rotation involved):
        // membership churn alone needs no new entry because `keys_at`'s
        // fallback already resolves an unseen id correctly (see its doc
        // comment). Detected by comparing, for every id that was a validator
        // BOTH before and after this block, its pre-block key
        // (`keys_before`) against its post-block key — restricting to that
        // intersection (rather than comparing the two maps wholesale) is
        // exactly what keeps a plain join/leave from registering as a
        // "rotation": a joining validator has no `keys_before` entry to
        // differ from, and a leaving one has no post-block entry to compare.
        // On an actual rotation, the value stored is the FULL post-block key
        // snapshot over `self.state.validators` (not just the one id that
        // changed) — so any later `keys_at(height)` lookup at or after this
        // height is a complete, self-sufficient map, never needing to merge
        // across multiple journal entries.
        let keys_after = validator_key_snapshot(&self.state, &self.state.validators);
        let rotated = keys_before
            .iter()
            .any(|(id, old_key)| keys_after.get(id).is_some_and(|new_key| new_key != old_key));
        if rotated {
            self.validator_key_history.push((self.height, keys_after.clone()));
            if let Some(store) = &mut self.store {
                store.append_validator_keys(self.height, &keys_after)?;
            }
        }
        if self.snapshot_interval > 0 && self.height.is_multiple_of(self.snapshot_interval) {
            if let Some(store) = &mut self.store {
                store.write_snapshot(self.height, &self.state)?;
                // **Prune the log the snapshot just made redundant.** Nothing
                // replays a height below the last snapshot, and a WAL that is
                // never pruned grows with the chain's AGE rather than with its
                // content: an empty block is paced at one a second, which is
                // about 31.5 million frames a year on an idle chain, every one
                // of them read and decoded at every start.
                //
                // **The margin is how long a validator may be down**, not a
                // tidiness setting: a node below every peer's floor cannot
                // catch up from anybody, and the only way back is an operator
                // carrying a snapshot. A node that has pruned says so through
                // `history_min_height` rather than answering a sync request
                // for a block it no longer holds.
                //
                // Never less than two snapshot intervals, whatever the margin
                // says: what a syncing peer needs is a snapshot AND every
                // block above it, so cutting inside the last interval would
                // leave a hole nothing can serve.
                let margin = self.prune_margin_blocks.max(2 * self.snapshot_interval);
                let keep_from = self.height.saturating_sub(margin).max(1);
                store.prune_below(keep_from)?;
            }
        }
        Ok(outcomes)
    }

    /// Where this replica stopped, if it did. `None` is a replica still
    /// committing.
    pub fn halted(&self) -> Option<&Halt> {
        self.halted.as_ref()
    }

    /// Prove member `id`'s record against `app_hash`, off the cache the block
    /// that produced it left behind. Equal to `root::prove_member` on every
    /// state, which the swarm driver's pair check underwrites.
    pub fn prove_member(&self, id: u64) -> Result<Option<edet_state::root::InclusionProof>, ReplicaError> {
        self.root_cache
            .prove_member(&self.state, id)
            .map_err(|e| ReplicaError::Codec(CodecError::Encode(e.0)))
    }

    /// Prove contract `id`'s record — the other common case, and the one an
    /// arbitrator asks for.
    pub fn prove_contract(&self, id: u64) -> Result<Option<edet_state::root::InclusionProof>, ReplicaError> {
        self.root_cache
            .prove_contract(&self.state, id)
            .map_err(|e| ReplicaError::Codec(CodecError::Encode(e.0)))
    }

    /// (the paper's §Implementation):
    /// commit a decided block exactly like `commit_block` (transaction
    /// signatures are still checked — the same authenticated chokepoint),
    /// AND persist its commit certificate alongside it, so a later
    /// `decided_value_at` can serve `(block, certificate)` to a syncing peer.
    /// `certificate_bytes` is an opaque, caller-encoded blob — `Replica`
    /// stays malachite-agnostic exactly like `Store::append_certificate`
    /// (this crate's default build never depends on
    /// `malachitebft-core-types`); encoding a real
    /// `CommitCertificate<EdetContext>` is `engine_malachite`'s job.
    ///
    /// The certificate is written BEFORE the block, deliberately the
    /// reverse of the naive order (commit, then certify). `Store` already
    /// documents that these two writes are not atomic with each other — a
    /// crash between them is always possible without a combined WAL frame
    /// format, which is out of scope here. What this ordering controls is
    /// WHICH of the two partial states that crash can leave behind:
    ///
    /// - certificate-first (this code): worst case is an ORPHAN
    ///   certificate — one recorded for a height whose block never made it
    ///   to the WAL. Harmless: `read_wal`/`find_block` have no idea it
    ///   exists, and `decided_value_at` below looks up the block FIRST and
    ///   returns `None` immediately on a miss, so an orphan certificate is
    ///   simply never reached, let alone served.
    /// - block-first (the old order): worst case is a committed, APPLIED
    ///   block with no certificate — state has already moved, but nothing
    ///   can ever prove to a syncing peer that this height was legitimately
    ///   decided. That is the strictly worse half to land on, since it
    ///   corrupts the one thing (state) this whole store exists to protect,
    ///   where the certificate-first failure only ever loses a piece of
    ///   provenance for a decision that itself never happened.
    ///
    /// So this reorder doesn't buy atomicity — nothing here does — it picks
    /// the crash outcome that costs nothing when it happens.
    ///
    /// Refuses a rewinding block exactly like `commit_block`, and for the
    /// same reason (see that doc comment) — monotonicity only, checked before
    /// either write, so a non-monotonic block leaves neither an orphan
    /// certificate nor an applied block behind.
    pub fn commit_block_with_certificate(
        &mut self,
        block: &Block,
        certificate_bytes: &[u8],
    ) -> Result<Vec<TxOutcome>, ReplicaError> {
        if !block.verify_txs(&self.state.chain_id) {
            return Err(ReplicaError::UnverifiedTx { height: block.height });
        }
        if !crate::block::time_is_monotonic(block.time_secs, self.last_time_secs) {
            return Err(ReplicaError::NonMonotonicTime {
                expected_at_least: self.last_time_secs,
                got: block.time_secs,
            });
        }
        // Refuses a mismatched app_hash exactly like `commit_block`, and
        // for the same reason — before either write, so a rejected block
        // leaves neither an orphan certificate nor an applied block behind.
        if block.app_hash != self.app_hash {
            return Err(ReplicaError::AppHashMismatch { expected: self.app_hash, got: block.app_hash });
        }
        if let Some(store) = &mut self.store {
            store.append_certificate(block.height, certificate_bytes)?;
        }
        self.commit_block_unchecked(block)
    }

    /// Serve a previously decided `(Block, certificate bytes)` pair for
    /// `height` from the durable store — the light-client/sync path
    /// (`GetDecidedValue`), driven by peer sync requests, i.e. by untrusted
    /// remote input. Returns `None` for an in-memory replica (no `data_dir`,
    /// `store` is `None`), a height this replica never committed a
    /// certificate for (heights committed via `commit_block`/
    /// `commit_block_unchecked` instead of this method — every non-Malachite
    /// consensus path, or any height written before certificates were stored), or an ORPHAN
    /// certificate left by a crash inside `commit_block_with_certificate`
    /// (see its doc comment) — the block lookup happens FIRST and short-
    /// circuits to `None` on a miss, so a certificate with no matching block
    /// is never reached.
    ///
    /// `Store::find_block`/`find_certificate` are now an index lookup
    /// plus one seek-and-read each, not a re-parse of the whole store — a
    /// peer that calls this in a loop would otherwise impose O(chain length) file
    /// I/O and decoding per call on the target for near-zero cost of its
    /// own; that amplification is what made this worth fixing.
    pub fn decided_value_at(&mut self, height: u64) -> Result<Option<(Block, Vec<u8>)>, ReplicaError> {
        let Some(store) = &mut self.store else { return Ok(None) };
        let Some(block) = store.find_block(height)? else { return Ok(None) };
        let Some(certificate) = store.find_certificate(height)? else { return Ok(None) };
        Ok(Some((block, certificate)))
    }
}

/// Snapshot each CURRENT validator's signing key, keyed by member id —
/// the exact shape `validator_key_history`/`ValidatorKeysAt` entries carry.
/// "Signing key" means `member.keys.first()`, the same key
/// `EdetValidatorSet::build` has always resolved a validator's identity
/// from — a member's `keys` vector can hold more than one key (multi-device
/// rotation overlap, `apply.rs`), but only the first has ever been the
/// consensus-signing key. A validator id with no member record, or a member
/// record with an empty `keys` vector, is simply omitted, not an error: this
/// is a snapshot of what's there, not a validity check — `EdetValidatorSet::
/// build`'s `MissingMember`/`UnparseableKey` refusal is what surfaces a
/// truly broken key at the point it actually matters (verification).
/// The CONSENSUS key of every current validator — what a certificate signed at
/// this height was signed with.
///
/// Reading `m.keys.first()` here would resolve a validator by its MEMBER key,
/// which is the separation this field exists to keep. A validator with no
/// consensus key is impossible by invariant, so the `filter_map` drops nothing
/// a reachable state contains.
fn validator_key_snapshot(state: &State, validators: &BTreeMap<MemberId, u64>) -> BTreeMap<MemberId, Key> {
    validators
        .keys()
        .filter_map(|&id| state.members.get(&id).and_then(|m| m.consensus_key).map(|k| (id, k)))
        .collect()
}

/// Computes each transaction's envelope id (`SignedTx::id`, bound to
/// THIS state's `chain_id`) and passes it — together with the envelope's own
/// claimed `not_after_epoch` — into `apply`, which is what actually enforces
/// replay/expiry/window. A `SignedTx` whose id fails to compute is recorded
/// as a failed outcome (`ET_TX_UNDIGESTABLE`) rather than silently dropped:
/// every transaction in a block must produce exactly one outcome, in order,
/// or `Replica::record_outcomes`' zip against `block.txs` would silently
/// misalign every outcome after the skipped one.
fn apply_block_to(state: &mut State, block: &Block, gate: &mut edet_state::bond::GateCache) -> Vec<TxOutcome> {
    state.begin_block(block.time_secs);
    let chain_id = state.chain_id.clone();
    block
        .txs
        .iter()
        .map(|stx| match stx.id(&chain_id) {
            Ok(tx_id) => edet_state::apply_with_cache(
                state,
                stx.tx.clone(),
                tx_id,
                stx.not_after_epoch,
                &stx.signers,
                block.time_secs,
                gate,
            ),
            Err(_) => Err(edet_state::errors::Error(edet_state::errors::ET_TX_UNDIGESTABLE)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{dev_seed, pubkey_of, sign_tx, SignedTx};
    use edet_state::tx::Tx;
    use edet_state::types::Party;

    /// Two-member genesis whose keys are real Ed25519 public keys, so a
    /// transaction can be genuinely signed (or genuinely forged).
    fn genesis() -> State {
        let mut st = State::default();
        for i in 0..2u8 {
            st.add_underwriter(vec![pubkey_of(&dev_seed(i))], 25_000.0).expect("unique");
        }
        st
    }

    /// The residual the adversarial review flagged: `commit_block`/`apply`
    /// trusted the `signers` field, so a block carrying real member keys as
    /// `signers` but NO `signatures` booked an obligation nobody signed.
    /// `commit_block` must now refuse it; only the explicit unchecked hatch
    /// (simulations / trusted replay) still applies it.
    #[test]
    fn commit_block_authenticates_but_unchecked_does_not() {
        // Trial-sized (< 0.05·V_base) so the economics permit it — the point
        // is authentication, not capacity.
        let tx = Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 40.0,
            maturity_epochs: 30,
            arb: None,
        };
        // Deterministic across the three separate `genesis()` calls below —
        // same construction steps every time — so one captured value is
        // valid app_hash for all of them.
        let app0 = Replica::new(genesis()).app_hash();
        let forged = Block {
            height: 1,
            time_secs: 86_400,
            app_hash: app0,
            txs: vec![SignedTx {
                tx: tx.clone(),
                nonce: crate::block::counter_nonce(0),
                not_after_epoch: 30,
                signers: vec![pubkey_of(&dev_seed(0)), pubkey_of(&dev_seed(1))],
                signatures: vec![], // forged: keys claimed, nothing signed
            }],
        };

        // Checked path (the network/consensus commit path) refuses it.
        let mut r = Replica::new(genesis());
        assert!(matches!(r.commit_block(&forged), Err(ReplicaError::UnverifiedTx { height: 1 })));
        assert_eq!(r.height, 0, "a forged block must not advance height");
        assert_eq!(r.state.members[&0].debt_out, 0, "a forged tx must book no debt");

        // The unchecked escape hatch DOES apply it — documenting the boundary
        // the simulations/tests deliberately operate below.
        let mut r_unchecked = Replica::new(genesis());
        r_unchecked.commit_block_unchecked(&forged).expect("unchecked applies");
        assert_eq!(r_unchecked.state.members[&0].debt_out, edet_state::State::to_minor(40.0));

        // A properly signed block passes the checked path.
        let signed = Block {
            height: 1,
            time_secs: 86_400,
            app_hash: app0,
            txs: vec![sign_tx(
                crate::block::DEV_CHAIN_ID,
                tx,
                crate::block::counter_nonce(1),
                30,
                &[dev_seed(0), dev_seed(1)],
            )
            .expect("sign")],
        };
        let mut r_signed = Replica::new(genesis());
        r_signed.commit_block(&signed).expect("a signed block commits");
        assert_eq!(r_signed.state.members[&0].debt_out, edet_state::State::to_minor(40.0));
    }

    /// The core property — a block whose `app_hash` does not match what
    /// this replica actually holds is refused, not silently applied. Proven
    /// against `commit_block`'s checked path; `screen`'s equivalent refusal
    /// (`engine_malachite.rs`) is what stops this from ever reaching a vote.
    #[test]
    fn commit_block_rejects_a_wrong_app_hash() {
        let mut r = Replica::new(genesis());
        let bad = Block { height: 1, time_secs: 60, app_hash: [0xAA; 32], txs: Vec::new() };
        let err = r.commit_block(&bad).expect_err("a wrong app_hash must be refused");
        assert!(matches!(err, ReplicaError::AppHashMismatch { .. }), "unexpected error: {err:?}");
        assert_eq!(r.height, 0, "a refused block must not advance height");
    }

    /// A scratch data dir, no `tempfile` dependency in this crate (see
    /// `store::tests::scratch_dir`, duplicated here since it's `store`-private).
    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("edet-replica-test-{tag}-{unique}-{:?}", std::thread::current().id()))
    }

    /// `commit_block_with_certificate` persists the block AND the
    /// caller-supplied certificate bytes; `decided_value_at` serves exactly
    /// that pair back, for a durable (data-dir-backed) replica. An in-memory
    /// replica (`Replica::new`, no store) has nowhere to serve from and must
    /// answer `None`, not panic or fabricate an answer.
    #[test]
    fn commit_with_certificate_round_trips_through_decided_value_at() {
        let dir = scratch_dir("commit-cert");
        let mut r = Replica::open(&dir, genesis(), 0).expect("open");
        let app0 = r.app_hash();

        let signed = Block {
            height: 1,
            time_secs: 60,
            app_hash: app0,
            txs: vec![sign_tx(
                crate::block::DEV_CHAIN_ID,
                Tx::MarkExpired { contract: 1 },
                crate::block::counter_nonce(0),
                30,
                &[],
            )
            .expect("a permissionless crank signs trivially")],
        };
        r.commit_block_with_certificate(&signed, b"fake-cert-bytes")
            .expect("commit with certificate");

        let (block, cert) = r
            .decided_value_at(1)
            .expect("query")
            .expect("height 1 was committed with a certificate");
        assert_eq!(block.height, 1);
        assert_eq!(cert, b"fake-cert-bytes");

        assert!(r.decided_value_at(2).expect("query").is_none(), "a height never committed must read back None");

        let mut in_memory = Replica::new(genesis());
        in_memory.commit_block_unchecked(&signed).expect("in-memory commit");
        assert!(
            in_memory.decided_value_at(1).expect("query").is_none(),
            "an in-memory replica (no store) has nowhere to serve a decided value from"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The residual the reordered write path deliberately accepts — an
    /// ORPHAN certificate (one recorded for a height whose block never made
    /// it into the WAL, the partial state a crash between the two writes in
    /// `commit_block_with_certificate` can now leave behind) must be
    /// tolerated by `decided_value_at`, not treated as corruption. Written
    /// directly via `Store` rather than by inducing an actual crash, since
    /// that's the observable postcondition regardless of how it arose.
    #[test]
    fn orphan_certificate_with_no_matching_block_is_tolerated_not_an_error() {
        let dir = scratch_dir("orphan-cert");
        {
            let mut store = crate::store::Store::open(&dir).expect("open store directly");
            store
                .append_certificate(1, b"orphan-cert-no-block")
                .expect("append orphan certificate");
        }

        let mut r = Replica::open(&dir, genesis(), 0).expect("open replica over the same dir");
        assert!(
            r.decided_value_at(1)
                .expect("query must not error on an orphan certificate")
                .is_none(),
            "PROVEN: a certificate with no matching block is served as None, not surfaced as corruption"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // --- a snapshot may not put this node on another chain --------------

    /// The remediation path. Correcting a chain that booted on
    /// the wrong chain id — or on the published dev salt — means editing the
    /// genesis file; before this, every node that had written even one
    /// snapshot ignored the edit entirely and carried on with the old value,
    /// so the fix appeared to land and changed nothing. It also means a
    /// reused or misplaced data dir can no longer put a validator on a chain
    /// its own genesis file says it is not on.
    #[test]
    fn open_refuses_a_snapshot_from_a_different_chain() {
        let dir = scratch_dir("chain-mismatch");
        {
            let mut r = Replica::open(&dir, genesis(), 1).expect("open");
            let app0 = r.app_hash();
            r.commit_block(&Block { height: 1, time_secs: 86_400, app_hash: app0, txs: Vec::new() })
                .expect("one committed block writes a snapshot at interval 1");
        }

        let mut corrected = genesis();
        corrected.chain_id = "edet-pilot".to_string();
        let err = match Replica::open(&dir, corrected, 1) {
            Err(e) => e,
            Ok(_) => panic!("a snapshot from another chain must not be resumed under this genesis"),
        };
        assert!(matches!(&err, ReplicaError::GenesisMismatch { field: "chain_id", .. }), "unexpected error: {err:?}");

        // The salt is the half the remediation actually turns on: same chain
        // id, corrected salt, and the old snapshot must not silently win.
        let mut resalted = genesis();
        resalted.root_salt = [0xAB; 32];
        let err = match Replica::open(&dir, resalted, 1) {
            Err(e) => e,
            Ok(_) => panic!("a snapshot carrying a different root_salt must not be resumed"),
        };
        assert!(matches!(&err, ReplicaError::GenesisMismatch { field: "root_salt", .. }), "unexpected error: {err:?}");

        // Control: the unmodified genesis still resumes, so the refusal above
        // is about the mismatch and not about opening a store at all.
        let r = Replica::open(&dir, genesis(), 1).expect("the matching genesis still resumes");
        assert_eq!(r.height, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    // --- the fail-stop --------------------------------------------------

    /// **A block the audit refuses halts this replica, and the halt is what
    /// the copy used to be.**
    ///
    /// A block applies in place, so a refused one leaves the ledger it
    /// refused behind. Three things have to hold together for that to be a
    /// fail-stop rather than a corruption: nothing further commits, every
    /// read says where it stopped and why, and the durable side is untouched
    /// so a restart comes back at the last height that passed — which is the
    /// WAL append sitting after the audit, not the copy.
    ///
    /// Mutation that bites: drop the `halted` assignment. The next block
    /// commits, on top of a ledger the invariants refused.
    #[test]
    fn a_refused_block_halts_the_replica() {
        let dir = std::env::temp_dir().join(format!("edet-test-halt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        {
            let mut r = Replica::open(&dir, genesis(), 1).expect("open");
            let app0 = r.app_hash();
            r.commit_block(&Block { height: 1, time_secs: 86_400, app_hash: app0, txs: Vec::new() })
                .expect("an ordinary block commits");
            assert!(r.halted().is_none(), "a replica that has committed cleanly has not stopped");

            // The conservation clause, broken by hand: the cached debt no
            // longer agrees with the contract book.
            r.state.members.get_mut(&0).expect("member 0").debt_out = 99_900;
            let app1 = r.app_hash();
            let err = r
                .commit_block(&Block { height: 2, time_secs: 2 * 86_400, app_hash: app1, txs: Vec::new() })
                .expect_err("a block that leaves the ledger impossible must be refused");
            assert!(matches!(err, ReplicaError::InvariantViolated { height: 2, .. }), "unexpected error: {err:?}");

            let halt = r.halted().expect("the replica must have stopped").clone();
            assert_eq!(halt.height, 2, "it stopped at the height whose block was refused");
            assert!(!halt.reason.is_empty(), "and it says why");
            assert_eq!(r.height, 1, "a refused block does not advance the height");

            // Nothing commits on top of it, and the refusal names the halt
            // rather than the new block.
            let app = r.app_hash();
            let err = r
                .commit_block(&Block { height: 2, time_secs: 3 * 86_400, app_hash: app, txs: Vec::new() })
                .expect_err("a halted replica commits nothing");
            assert!(matches!(err, ReplicaError::Halted { height: 2, .. }), "unexpected error: {err:?}");
        }

        // The durable side never saw any of it: a restart comes back at the
        // last height the audit passed, on the state it passed.
        {
            let r = Replica::open(&dir, genesis(), 1).expect("reopen");
            assert_eq!(r.height, 1, "the WAL holds only the block that passed");
            assert!(r.halted().is_none(), "and the reopened replica is not halted");
            assert_eq!(
                r.state.members.get(&0).expect("member 0").debt_out,
                0,
                "the doctoring was in memory and did not survive the restart"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- the cached app_hash never outlives the state it describes -----

    /// A commit's durable writes can fail — full disk, revoked permission, a
    /// transient I/O error. When one does, the in-memory replica must still
    /// be self-consistent: `app_hash()` is what the screen compares every peer
    /// proposal against AND what this node stamps on its own proposals, so a
    /// cached value describing the previous height's state means this node
    /// rejects everything the network sends and proposes something nobody
    /// accepts — a silent halt lasting until a restart.
    ///
    /// The failure is induced by putting a DIRECTORY where `write_snapshot`
    /// needs to create `snapshot.tmp`: `File::create` then fails for any
    /// user, root included, so the test does not depend on how the suite is
    /// run.
    #[test]
    fn a_failed_store_write_leaves_the_cached_app_hash_describing_the_state_actually_held() {
        let dir = scratch_dir("stale-app-hash");
        let mut r = Replica::open(&dir, genesis(), 1).expect("open with a snapshot every block");
        std::fs::create_dir(dir.join("snapshot.tmp")).expect("block the snapshot write");

        let app0 = r.app_hash();
        let block = Block {
            height: 1,
            time_secs: 86_400,
            app_hash: app0,
            txs: vec![sign_tx(
                crate::block::DEV_CHAIN_ID,
                Tx::Accept {
                    debtor: Party::Member(0),
                    creditor: Party::Member(1),
                    amount: 40.0,
                    maturity_epochs: 30,
                    arb: None,
                },
                crate::block::counter_nonce(0),
                30,
                &[dev_seed(0), dev_seed(1)],
            )
            .expect("sign")],
        };
        let err = match r.commit_block(&block) {
            Err(e) => e,
            Ok(_) => panic!("the snapshot write must fail with a directory in the way"),
        };
        assert!(matches!(err, ReplicaError::Store(_)), "unexpected error: {err:?}");

        assert_eq!(
            r.state.members[&0].debt_out,
            edet_state::State::to_minor(40.0),
            "the block did apply — the state genuinely moved"
        );
        assert_eq!(
            r.app_hash(),
            r.app_hash(),
            "the cached commitment must describe the state this replica now holds, not the one it left behind"
        );
        assert_ne!(r.app_hash(), app0, "and it must not still be the pre-block value");
        std::fs::remove_dir_all(&dir).ok();
    }

    // --- timestamp monotonicity at the commit chokepoint ----------------

    /// The regression this whole fix targets: a block whose `time_secs` is
    /// below the last committed block's must be refused, not silently
    /// committed with the clock rewound.
    #[test]
    fn commit_block_rejects_a_block_that_would_rewind_the_clock() {
        let mut r = Replica::new(genesis());
        let app0 = r.app_hash();
        let fwd = Block { height: 1, time_secs: 100 * 86_400, app_hash: app0, txs: Vec::new() };
        r.commit_block(&fwd).expect("forward commit");
        assert_eq!(r.last_time_secs, 100 * 86_400);
        assert_eq!(r.height, 1);

        let app1 = r.app_hash();
        let back = Block { height: 2, time_secs: 0, app_hash: app1, txs: Vec::new() };
        let err = r.commit_block(&back).expect_err("a rewinding block must be refused");
        assert!(
            matches!(err, ReplicaError::NonMonotonicTime { expected_at_least, got: 0 } if expected_at_least == 100 * 86_400),
            "unexpected error: {err:?}"
        );
        assert_eq!(r.height, 1, "a refused block must not advance height");
        assert_eq!(r.last_time_secs, 100 * 86_400, "a refused block must not rewind last_time_secs");
    }

    /// Non-decreasing, not strictly increasing: two blocks landing in the
    /// same wall-clock second must both commit.
    #[test]
    fn commit_block_accepts_an_equal_timestamp() {
        let mut r = Replica::new(genesis());
        let app0 = r.app_hash();
        r.commit_block(&Block { height: 1, time_secs: 500, app_hash: app0, txs: Vec::new() })
            .expect("first commit");
        let app1 = r.app_hash();
        r.commit_block(&Block { height: 2, time_secs: 500, app_hash: app1, txs: Vec::new() })
            .expect("a second block at the SAME timestamp must commit, not be treated as a rewind");
        assert_eq!(r.height, 2);
        assert_eq!(r.last_time_secs, 500);
    }

    /// `commit_block_with_certificate` enforces the identical rule as
    /// `commit_block` (both are network-facing commit chokepoints).
    #[test]
    fn commit_block_with_certificate_rejects_a_rewinding_block_too() {
        let dir = scratch_dir("cert-rewind");
        let mut r = Replica::open(&dir, genesis(), 0).expect("open");
        let app0 = r.app_hash();
        r.commit_block_with_certificate(
            &Block { height: 1, time_secs: 1_000, app_hash: app0, txs: Vec::new() },
            b"cert-1",
        )
        .expect("first commit");
        let app1 = r.app_hash();
        let err = r
            .commit_block_with_certificate(
                &Block { height: 2, time_secs: 999, app_hash: app1, txs: Vec::new() },
                b"cert-2",
            )
            .expect_err("a rewinding block must be refused even with a certificate");
        assert!(matches!(err, ReplicaError::NonMonotonicTime { expected_at_least: 1_000, got: 999 }));
        assert_eq!(r.height, 1);
        assert!(
            r.decided_value_at(2).expect("query").is_none(),
            "a refused block must leave no certificate behind for a height that never committed"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A normal, monotonically-increasing multi-block sequence is unaffected
    /// by the new checks — no regression on the ordinary path.
    #[test]
    fn a_normal_multi_block_sequence_still_commits_unchanged() {
        let mut r = Replica::new(genesis());
        for h in 1..=5u64 {
            // Each block's app_hash is exactly what the replica held
            // before it — the property `Block::app_hash`'s doc comment
            // describes, exercised here across a real multi-block sequence.
            let app_hash = r.app_hash();
            r.commit_block(&Block { height: h, time_secs: h * 30, app_hash, txs: Vec::new() })
                .unwrap_or_else(|e| panic!("block {h} must commit: {e}"));
        }
        assert_eq!(r.height, 5);
        assert_eq!(r.last_time_secs, 150);
    }

    /// WAL replay must apply the same monotonicity rule a live commit
    /// does — a store somehow containing a rewinding block (a poisoned WAL,
    /// however it got there) is corrupt and must be reported, not silently
    /// re-absorbed as legitimate history on every restart.
    #[test]
    fn open_rejects_a_wal_containing_a_rewinding_block() {
        let dir = scratch_dir("wal-rewind");
        {
            // Write two blocks directly via `commit_block_unchecked` — the
            // trusted-local escape hatch — bypassing the very check `open`'s
            // WAL replay must independently enforce, so this genuinely
            // exercises replay's own gate rather than reusing `commit_block`'s.
            let mut r = Replica::open(&dir, genesis(), 0).expect("open");
            let app0 = r.app_hash();
            r.commit_block_unchecked(&Block { height: 1, time_secs: 500, app_hash: app0, txs: Vec::new() })
                .expect("first block");
            let app1 = r.app_hash();
            r.commit_block_unchecked(&Block { height: 2, time_secs: 100, app_hash: app1, txs: Vec::new() })
                .expect("second (rewinding) block written to the WAL despite the rewind");
        }

        // `Replica` carries no `Debug` impl (its `Store` field doesn't), so
        // `.expect_err` isn't available here — match it out by hand instead.
        let err = match Replica::open(&dir, genesis(), 0) {
            Err(e) => e,
            Ok(_) => panic!("replaying a rewinding WAL must fail, not hang or silently apply"),
        };
        assert!(
            matches!(err, ReplicaError::NonMonotonicTime { expected_at_least: 500, got: 100 }),
            "unexpected error: {err:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
