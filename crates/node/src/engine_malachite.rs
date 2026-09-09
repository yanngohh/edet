//! Embedded consensus driver (EXPERIMENTAL — `--features malachite`).
//!
//! Wires the edet-owned `Context` (`engine_context.rs`) into every handler of
//! the engine's application channel (the paper's §Implementation). This
//! replaces the old scaffolding that imported `malachitebft_test::TestContext`
//! (a compile-checked contract map whose toy `Value`/`ProposalPart` could not
//! carry an edet `Block` at all, per the integration notes).
//!
//! What is genuinely done and unit-tested here (in-process, no network): the
//! chunk-streaming (`GetValue`) and reassembly (`ReceivedProposalPart`) round
//! trip (`stream_value`/`reassemble_proposal_part`); synced-value decoding
//! (`ProcessSyncedValue`); commit-certificate verification gating `Decided`
//! before it ever reaches `replica.commit_block` — the acceptance
//! criterion, that a certificate not signed by 2/3+ of a height's real
//! validator set can never commit, regardless of what block a node happens
//! to hold (`verify_decided`); and the certificate's own WAL-facing byte
//! codec (`encode_commit_certificate`/`decode_commit_certificate`,
//! `store.rs`'s new `append_certificate`/`find_certificate`).
//!
//! **`run` is what `main.rs` starts** (`engine_node.rs::start_engine`), so this
//! is the consensus a deployed node actually runs, over a real libp2p mesh,
//! with real signatures and the real wire codec. `tests/malachite_cluster.rs`
//! drives four validator PROCESSES over loopback and asserts they commit and
//! agree; `tests/malachite_byzantine.rs` puts a misbehaving proposer among
//! them. Neither is a substitute for a network with adversarial latency and
//! partitions, which is a pilot's job rather than a test suite's.
//!
//! One narrower limitation lives inside the handlers: `ReceivedProposalPart`'s
//! reassembly trusts in-order delivery within one stream — it never falsely
//! ACCEPTS a mis-reassembled value (the `Fin` hash check catches that), but a
//! reordered transport can cause a spurious `None` (see
//! `reassemble_proposal_part`'s doc comment). The buffer under it is
//! segregated by sending peer and bounded on every axis a peer controls
//! (`PartStreams`).
//!
//! Excluded from default builds (not a workspace default feature) but CI
//! does compile-check this file (`cargo build -p edet-node --features
//! malachite`) so a breaking change in the pinned engine's API is caught
//! even though nothing here runs as a live node.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use malachitebft_app_channel::app::engine::host::Next;
use malachitebft_app_channel::app::streaming::{StreamContent, StreamId, StreamMessage};
use malachitebft_app_channel::app::types::sync::RawDecidedValue;
use malachitebft_app_channel::app::types::{LocallyProposedValue, PeerId, ProposedValue};
use malachitebft_app_channel::{AppMsg, Channels, NetworkMsg};
use malachitebft_core_types::{CommitCertificate, CommitSignature, Round, ThresholdParams, Validity};
use malachitebft_signing::SigningProviderExt;

use crate::block::Block;
use crate::engine_context::{
    EdetAddress, EdetContext, EdetHeight, EdetProposalPart, EdetSignature, EdetSigningProvider, EdetValidatorSet,
    EdetValue, EdetValueId,
};
use crate::replica::Replica;

/// The proposer's batch size, and the block-validity ceiling, which are one
/// number (`block::MAX_TXS_PER_BLOCK`) because a proposer that could build a
/// block the rule refuses would be a liveness fault, and a rule looser than the
/// proposer would bound nothing.
const GET_VALUE_BATCH: usize = crate::block::MAX_TXS_PER_BLOCK;

/// The floor between two EMPTY blocks.
///
/// **An empty block produced twice within the same second cannot change
/// anything the ledger keeps.** It carries no transactions by definition, and
/// the only other thing a block advances — the epoch — moves at most once per
/// wall-clock second, because `begin_block` is idempotent within a second (the
/// is idempotent within a second). So the second one is pure cost, and unpaced it is a great deal of
/// cost: a validator with nobody to wait for commits as fast as the CPU
/// allows. Measured on a solo node: 1.87M heights and 500 MB of store in 26
/// minutes on a desktop, and on a phone the store passed 86 MB at 116% CPU —
/// a handset writing continuously to flash to record that nothing happened.
///
/// One second, not more: the epoch catch-up on a fresh chain needs a block per
/// jump (`MAX_EPOCH_ADVANCE_PER_BLOCK`), and those blocks are empty, so a
/// longer floor would slow the one thing empty blocks are good for. Nothing
/// paces a block that CARRIES transactions — latency there is the point.
const EMPTY_BLOCK_INTERVAL: Duration = Duration::from_secs(1);

/// How long to wait before proposing, given what there is to propose.
///
/// Pure so it can be tested without a clock: consensus hands `GetValue` a
/// `timeout` ("maximum time allowed for the application to respond"), and the
/// answer must stay well inside it — a proposer that misses its own slot
/// hands the round to the next one, which is a worse failure than a fast
/// empty block. Half is the margin.
fn propose_delay(has_txs: bool, since_last_block: Duration, timeout: Duration) -> Duration {
    if has_txs {
        return Duration::ZERO;
    }
    let remaining = EMPTY_BLOCK_INTERVAL.saturating_sub(since_last_block);
    remaining.min(timeout / 2)
}

/// Bound each streamed proposal part so a large block never violates the
/// app-channel's message-size assumptions (the integration notes' own instruction: "size the
/// constant from the mempool's own batch cap" — 64 KiB comfortably covers a
/// `GET_VALUE_BATCH`-sized block of typical edet transactions with headroom,
/// while keeping each wire message small).
const PROPOSAL_CHUNK_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// GetValue — build a block, chunk-stream it
// ---------------------------------------------------------------------------

/// A stable per-(height, round) stream identifier, mirroring the vendored
/// channel example's convention (`examples/channel/src/state.rs:365-370`).
fn stream_id_for(height: EdetHeight, round: Round) -> StreamId {
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&height.as_u64().to_be_bytes());
    bytes.extend_from_slice(&round.as_i64().to_be_bytes());
    StreamId::new(Bytes::from(bytes))
}

/// Break `block` into a bounded chunk stream: `Init`, `Chunk×N`, the
/// business-level `Fin{block_hash}`, then the transport-level
/// `StreamContent::Fin` envelope that closes the stream — mirrors the
/// vendored channel example's `State::stream_proposal`
/// (`examples/channel/src/state.rs:339-363`), adapted to edet's bytes-
/// carrying `Init`/`Chunk`/`Fin` shape instead of the toy context's
/// factored-`u64` parts. Shared by `GetValue` (a freshly built value) and
/// `RestreamProposal` (re-publishing one this node already holds in
/// `pending`).
fn stream_value(
    proposer: EdetAddress,
    height: EdetHeight,
    round: Round,
    pol_round: Round,
    block: &Block,
    block_id: EdetValueId,
) -> Vec<StreamMessage<EdetProposalPart>> {
    let stream_id = stream_id_for(height, round);
    let mut msgs = Vec::new();
    let mut seq: u64 = 0;

    msgs.push(StreamMessage::new(
        stream_id.clone(),
        seq,
        StreamContent::Data(EdetProposalPart::Init { height, round, pol_round, proposer }),
    ));
    seq += 1;

    let bytes = block
        .encode()
        .expect("Block always encodes: a finite in-memory struct serializing into a Vec cannot fail");
    for (chunk_seq, chunk) in bytes.chunks(PROPOSAL_CHUNK_BYTES).enumerate() {
        msgs.push(StreamMessage::new(
            stream_id.clone(),
            seq,
            StreamContent::Data(EdetProposalPart::Chunk { seq: chunk_seq as u32, bytes: chunk.to_vec() }),
        ));
        seq += 1;
    }

    msgs.push(StreamMessage::new(
        stream_id.clone(),
        seq,
        StreamContent::Data(EdetProposalPart::Fin { block_hash: block_id.0 }),
    ));
    seq += 1;

    msgs.push(StreamMessage::new(stream_id, seq, StreamContent::Fin));
    msgs
}

// ---------------------------------------------------------------------------
// ReceivedProposalPart — reassemble chunks by seq, verify the Fin hash
// ... within a buffer bounded on every axis a peer controls
// ---------------------------------------------------------------------------
//
// Every constant below bounds memory an UNTRUSTED peer can make this node
// hold. Before them, `assembling` was an unbounded map keyed on an
// attacker-chosen stream id and attacker-chosen chunk sequence, shrunk only
// by a transport `Fin` the attacker simply never sent: one Byzantine
// validator — inside the `f` budget, so within the model the rest of this
// file is written against — gossiping chunks under fresh stream ids drove
// every honest node out of memory. gossipsub dedups byte-identical messages
// only, so varying one byte per part defeats that layer entirely.
//
// The values are sized off what an HONEST proposer sends. A block is capped
// at `GET_VALUE_BATCH` transactions and chunked at `PROPOSAL_CHUNK_BYTES`, so
// a real proposal is a handful of chunks; the per-stream ceilings are an
// order of magnitude above that, and the global ceiling is what actually
// bounds the process.

/// In-flight streams from any one peer. An honest peer streams one proposal
/// per (height, round) and its entry is removed by the transport `Fin`, so
/// the only honest way to hold several at once is a lost or reordered `Fin`
/// — one per round, cleared by the next commit. Set well above that: a peer
/// at its allowance is refused rather than evicted (`accept`), so a peer
/// whose slots wedge wedges only ITSELF, but there is no reason to make an
/// honest peer with a lossy link pay for a bound this cheap to widen.
const MAX_STREAMS_PER_PEER: usize = 8;
/// In-flight streams across all peers — the backstop for a mesh larger than
/// the charter validator set, where the per-peer cap alone would scale with
/// however many peers can reach us.
const MAX_INFLIGHT_STREAMS: usize = 64;
/// Chunks retained for one stream. An honest `stream_value` emits
/// `ceil(block_bytes / PROPOSAL_CHUNK_BYTES)`.
const MAX_CHUNKS_PER_STREAM: usize = 64;
/// Bytes retained for one stream (`MAX_CHUNKS_PER_STREAM` full chunks, 4 MiB)
/// — checked independently of the chunk count, because a peer is free to send
/// chunks of any size it likes.
const MAX_STREAM_BYTES: usize = MAX_CHUNKS_PER_STREAM * PROPOSAL_CHUNK_BYTES;
/// Bytes retained across ALL streams: the number that actually bounds this
/// node's exposure, since the per-stream and per-peer caps multiply.
const MAX_BUFFERED_BYTES: usize = 16 * 1024 * 1024;

/// Reassembly state for one in-flight stream, held under a `(peer, stream_id)`
/// key in `PartStreams`. `chunks` is keyed by each `Chunk`'s OWN `seq` field
/// (not the transport `StreamMessage::sequence`), so concatenation
/// (`Assembly::chunks.into_values()`, `BTreeMap` iterates in ascending key
/// order) is correct however the underlying chunks arrived.
#[derive(Default)]
struct Assembly {
    height: Option<EdetHeight>,
    round: Option<Round>,
    pol_round: Option<Round>,
    proposer: Option<EdetAddress>,
    chunks: BTreeMap<u32, Vec<u8>>,
    block_hash: Option<[u8; 32]>,
    /// Transport sequence numbers already folded into this stream — the
    /// vendored `PartStreamsMap::seen_sequences`, restored. Without it a peer
    /// can re-send the same slot indefinitely with one byte changed, and each
    /// re-send is a fresh gossipsub message that reaches this handler.
    seen_sequences: BTreeSet<u64>,
    /// Running total of `chunks`' payload bytes, so the caps are checked
    /// against what is actually retained rather than the chunk COUNT alone.
    bytes: usize,
}

/// The bounded, peer-segregated reassembly buffer — this file's equivalent of
/// the vendored channel example's `PartStreamsMap`
/// (`test/app/src/streaming.rs`), whose two defining properties the original
/// port dropped: it keys on `(peer_id, stream_id)`, and it refuses a
/// transport sequence it has already seen.
///
/// Peer segregation is what makes the buffer's contents attributable at all.
/// `stream_id_for` is a pure function of `(height, round)`, so any mesh peer
/// can compute the exact stream id an honest proposer will use; with one
/// shared entry per stream id, injecting a colliding `Chunk` seq with garbage
/// meant the honest proposer's bytes were overwritten (last writer wins) and
/// the reassembly failed its `Fin` hash check on every honest node — the
/// round stalls, repeatably, for as long as the attacker keeps sending. That
/// is per-round censorship by any peer, not just a validator. Segregated by
/// peer, an injected chunk lands in the INJECTOR's own entry and the honest
/// stream reassembles untouched.
///
/// `from` is sound to key on: it is gossipsub's `message.source` under
/// `MessageAuthenticity::Signed` with `ValidationMode::Strict` (vendored
/// `network/src/behaviour.rs`), i.e. the authenticated originating peer, not
/// the forwarder — so a Byzantine peer cannot file its parts under someone
/// else's identity, and cannot multiply its own budget by claiming several.
///
/// Safety was never the issue and is unchanged: a node still needs the
/// proposer's signed `Proposal` for the value to be voted on, and the `Fin`
/// hash check still means mixed or tampered bytes produce `None` rather than
/// a wrong value. What this closes is liveness — memory, and the stall.
#[derive(Default)]
struct PartStreams {
    streams: BTreeMap<(PeerId, StreamId), Assembly>,
}

/// **A part the buffer refuses is said, never swallowed.** A refusal is the
/// difference between a round that decides and one that prevotes Nil at the
/// propose timeout, and a node that refuses silently stalls with a log that
/// reads healthy: every line in it is a vote, and the one thing it does not
/// say is why this node had no value to vote for. Two words per refusal —
/// which stream and which bound — is what turns "node 1 prevoted Nil for
/// three rounds" into a cause.
fn refused(key: &(PeerId, StreamId), sequence: u64, why: &str) {
    eprintln!("proposal part refused: peer {} stream {:?} sequence {sequence}: {why}", key.0, key.1);
}

impl PartStreams {
    fn len(&self) -> usize {
        self.streams.len()
    }

    fn buffered_bytes(&self) -> usize {
        self.streams.values().map(|a| a.bytes).sum()
    }

    /// The entry this part belongs to, or `None` when a bound refuses it:
    /// the caps (for a stream not already in flight) or the sequence dedup.
    ///
    /// A full buffer refuses NEW streams rather than evicting an existing one
    /// to make room. Eviction would be the wrong direction: a peer that can
    /// force an eviction can flush every honest stream on demand, which is
    /// the censorship this segregation exists to stop. Refusing means a
    /// flooding peer can at worst fill its OWN per-peer allowance.
    fn accept(&mut self, key: &(PeerId, StreamId), sequence: u64) -> Option<&mut Assembly> {
        if !self.streams.contains_key(key) {
            if self.len() >= MAX_INFLIGHT_STREAMS {
                refused(key, sequence, &format!("{} streams in flight", self.len()));
                return None;
            }
            let held = self.streams.keys().filter(|(peer, _)| *peer == key.0).count();
            if held >= MAX_STREAMS_PER_PEER {
                refused(key, sequence, &format!("{held} streams in flight from this peer"));
                return None;
            }
            self.streams.insert(key.clone(), Assembly::default());
        }
        let assembly = self.streams.get_mut(key)?;
        if !assembly.seen_sequences.insert(sequence) {
            refused(key, sequence, "transport sequence already folded in");
            return None;
        }
        Some(assembly)
    }

    fn remove(&mut self, key: &(PeerId, StreamId)) -> Option<Assembly> {
        self.streams.remove(key)
    }

    /// Drop every stream a commit made irrelevant. Called from `Decided`,
    /// which must not clear `pending` and leave this map untouched, or the
    /// only thing that ever shrank it was a transport `Fin`, i.e. the one
    /// message a hostile sender withholds and a lost/aborted honest round
    /// never produces.
    ///
    /// A stream survives only if it has NAMED a height above the one just
    /// decided. Anything at or below it is for a height already settled, and
    /// anything that has not named a height by the time a commit lands is
    /// unattributable — either garbage, or an honest stream whose `Init` was
    /// lost, and the cost of being wrong about the second is one restream
    /// (the `Fin` hash check turns a partial reassembly into `None`, never a
    /// wrong value).
    fn evict_decided(&mut self, committed: u64) {
        self.streams.retain(|_, a| a.height.is_some_and(|h| h.as_u64() > committed));
    }
}

/// Everything `run`'s `pending` map needs to hold onto per undecided
/// value — the `Block` itself (what `verify_decided`/`GetDecidedValue` care
/// about, keyed by value id regardless of round) PLUS the round metadata
/// (`AppMsg::StartedRound`'s `ProposedValue::{round,valid_round,proposer}`)
/// that only matters for replaying the value back into consensus. Without the persisted cache
/// `pending` held a bare `Block`; that was enough for the value-id lookups
/// but threw away exactly what `StartedRound` needs to answer honestly, so
/// this widens the map's value type instead of adding a second map to keep
/// in sync — one insert, one clear, one source of truth.
///
/// `round`/`valid_round`/`proposer` are set from whichever of the two ways a
/// block enters `pending`: this node's own `GetValue` (proposer =
/// `own_address`, `valid_round = Round::Nil` — a fresh proposal, no prior
/// polka) or a peer's stream/sync value (round/valid_round/proposer exactly
/// as the proposer signed them, threaded through unchanged).
#[derive(Clone)]
struct PendingValue {
    round: Round,
    valid_round: Round,
    proposer: EdetAddress,
    block: Block,
}

/// Fold one received `StreamMessage` into `streams`, completing and removing
/// that stream's entry (and recording the reassembled block into `pending`)
/// only on the transport-level `StreamContent::Fin` envelope that closes the
/// stream. Returns `Some` only when the reassembled bytes decode AND their
/// hash matches the `Fin{block_hash}` part the proposer signed — this hash
/// check is what makes the whole scheme safe even though it does NOT guard
/// against out-of-order delivery: an incomplete or wrongly-ordered
/// reassembly will either fail to decode or produce the wrong hash, never a
/// silently-accepted wrong value — it only ever produces a spurious `None`
/// (the caller must re-request/resend), never a false `Some`. Verifying that
/// property against a REAL reordering transport is exactly the job.
///
/// Segregated by the sending peer and bounded on every axis — see
/// `PartStreams`, which is where all of that lives. Everything a part can do
/// to this node's memory is decided by `PartStreams::accept` before a single
/// byte is retained; this function only ever folds a part the buffer has
/// already agreed to hold.
fn reassemble_proposal_part(
    streams: &mut PartStreams,
    pending: &mut BTreeMap<EdetValueId, PendingValue>,
    from: PeerId,
    part: StreamMessage<EdetProposalPart>,
    ctx: ScreenCtx<'_>,
) -> Option<ProposedValue<EdetContext>> {
    let StreamMessage { stream_id, sequence, content } = part;
    let key = (from, stream_id);
    match content {
        StreamContent::Data(EdetProposalPart::Init { height, round, pol_round, proposer }) => {
            let a = streams.accept(&key, sequence)?;
            a.height = Some(height);
            a.round = Some(round);
            a.pol_round = Some(pol_round);
            a.proposer = Some(proposer);
            None
        }
        StreamContent::Data(EdetProposalPart::Chunk { seq, bytes }) => {
            // Checked against the WHOLE buffer before the entry is even
            // reached, so one oversized chunk cannot be admitted by a stream
            // that happens to be under its own ceiling.
            if bytes.len() > MAX_STREAM_BYTES || streams.buffered_bytes() + bytes.len() > MAX_BUFFERED_BYTES {
                refused(&key, sequence, &format!("{} bytes would exceed the buffer", bytes.len()));
                return None;
            }
            let a = streams.accept(&key, sequence)?;
            if a.chunks.len() >= MAX_CHUNKS_PER_STREAM || a.bytes + bytes.len() > MAX_STREAM_BYTES {
                refused(&key, sequence, &format!("stream holds {} chunks, {} bytes", a.chunks.len(), a.bytes));
                return None;
            }
            // First writer wins per chunk `seq`, so a peer cannot rewrite a
            // slot it already filled: the retained bytes only ever grow by
            // what `a.bytes` was just checked against, and there is no
            // sequence of parts that makes the accounting drift from the
            // map. (Between peers the question does not arise — a peer only
            // ever writes into its own entry.)
            if a.chunks.contains_key(&seq) {
                return None;
            }
            a.bytes += bytes.len();
            a.chunks.insert(seq, bytes);
            None
        }
        StreamContent::Data(EdetProposalPart::Fin { block_hash }) => {
            streams.accept(&key, sequence)?.block_hash = Some(block_hash);
            None
        }
        StreamContent::Fin => {
            let Some(assembly) = streams.remove(&key) else {
                refused(&key, sequence, "closed a stream this node never opened");
                return None;
            };
            let (Some(height), Some(round), Some(pol_round), Some(proposer), Some(block_hash)) =
                (assembly.height, assembly.round, assembly.pol_round, assembly.proposer, assembly.block_hash)
            else {
                refused(
                    &key,
                    sequence,
                    &format!(
                        "closed with {} chunk(s), init {}, fin {}",
                        assembly.chunks.len(),
                        assembly.height.is_some(),
                        assembly.block_hash.is_some()
                    ),
                );
                return None;
            };
            let chunks = assembly.chunks.len();
            let bytes: Vec<u8> = assembly.chunks.into_values().flatten().collect();
            // Hash the RAW reassembled bytes, not `Block::hash()` on the
            // decoded result: `edet_state::codec::decode` tolerates trailing
            // garbage in its input (it does not require the whole buffer to
            // be consumed), so re-encoding a successfully-decoded block and
            // hashing THAT would silently absorb any tampering appended
            // after a valid encoding — the hash must cover exactly what was
            // transmitted, not a cleaned-up re-derivation of it.
            if crate::block::sha256(&bytes) != block_hash {
                // Reassembled bytes that do not match what the proposer signed
                // are never trusted — and a mismatch at height `h` with `n`
                // chunks is what a lost chunk looks like from here.
                refused(
                    &key,
                    sequence,
                    &format!("height {height} round {round}: {chunks} chunk(s) do not hash to the Fin"),
                );
                return None;
            }
            let Ok(block) = Block::decode(&bytes) else {
                refused(&key, sequence, &format!("height {height} round {round}: the bytes do not decode"));
                return None;
            };
            let value = EdetValue { id: EdetValueId(block_hash), block: block.clone() };
            // The Fin-hash check above only proves these bytes are the ones
            // the proposer streamed — that they are AUTHENTIC, not that they
            // are VALID. Whether the transactions inside carry real signatures
            // is a separate question, and it must be answered HERE, before
            // this node votes: see `screen`.
            let validity = screen(&block, ctx);
            if validity == Validity::Valid {
                pending.insert(value.id, PendingValue { round, valid_round: pol_round, proposer, block });
            }
            Some(ProposedValue { height, round, valid_round: pol_round, proposer, value, validity })
        }
    }
}

/// Everything `screen` consults to judge a block, grouped so both handlers
/// that reach it (`ReceivedProposalPart` and the sync path) take one argument
/// instead of four trailing scalars — three of which are bare `u64`/`[u8; 32]`
/// values a caller could silently transpose.
#[derive(Clone, Copy)]
struct ScreenCtx<'a> {
    chain_id: &'a str,
    /// The height this node has COMMITTED. Everything below that reads the
    /// parent state is meaningful only for `at + 1`; see `screen`.
    at: u64,
    /// `time_secs` of the parent block: the monotonicity floor.
    parent_time: u64,
    now_secs: u64,
    /// This node's own `app_hash` — the state it holds before this height.
    expected_app_hash: [u8; 32],
    /// The state-dependent half of block validity
    /// (`block::deterministic_validity`): the batch bound and the rule that
    /// every envelope is signed by somebody the transition requires.
    ///
    /// A callback rather than a `&State`, so the caller decides when to hold
    /// the node lock — this runs once per completed proposal, not once per
    /// read, and the guard is taken and dropped inside it rather than held
    /// across the reassembly's own hashing.
    deterministic: &'a dyn Fn(&Block) -> bool,
}

/// The pre-vote authentication screen for a block this node did NOT build,
/// and the reason `Replica::commit_block`'s identical check is a backstop
/// rather than the only gate.
///
/// Voting `Valid` on a block whose transactions are not properly signed does
/// NOT let it commit — `commit_block_with_certificate` refuses, so nothing
/// unsigned is ever booked, and the ledger does not fork. What it does is put
/// this node's SIGNATURE on a commit certificate for a value it will then
/// refuse. Measured on a 2-validator loopback cluster with one misbehaving
/// proposer (`tests/malachite_byzantine.rs`), that is not theoretical: over a
/// 25s window, 15 `Decided` events produced 10 committed heights, and TWO
/// heights were decided twice with DIFFERENT values — two conflicting,
/// fully-valid 2/3+ certificates for one height, both signed by honest
/// validators, neither matching what those validators actually committed.
///
/// That is the real defect, and it is a safety one. Everything downstream that
/// trusts a certificate instead of re-running validation — `verify_decided`,
/// the `GetDecidedValue`/`ProcessSyncedValue` sync path, any light client the certificate store's
/// certificate storage exists to serve — can be handed a valid quorum
/// certificate for a block that is not the committed block at that height. The
/// ledger holds today only because every node happens to apply an identical
/// commit-time rejection; the certificates say otherwise, and they are what
/// gets shipped to anyone not replaying the whole chain.
///
/// Screening before the vote makes validity part of what the network agrees
/// on: an honest validator votes nil, the round changes, and an honest
/// proposer's block is decided once. In the same measurement the screened run
/// committed 12 heights from exactly 12 `Decided` events, no height decided
/// twice — and, incidentally, out-progressed the unscreened one while under
/// the same attack.
///
/// A rejected block is also kept OUT of `pending`: consensus will not decide
/// a value this node called invalid, and if a Byzantine quorum decides it
/// anyway the missing entry makes `verify_decided` return `None` — the same
/// fail-closed `restart` the commit-path check would have produced, without
/// this node ever restreaming or retaining an unauthenticated block.
///
/// Also carries BOTH halves of the timestamp validity rule —
/// `crate::block::time_is_monotonic` against `parent_time` (this replica's
/// `last_time_secs`) and `crate::block::time_is_plausible` against
/// `now_secs` (this node's own clock). This is the one place in the whole
/// system both halves belong together: a vote is a local decision, so
/// consulting a local wall clock here can never fork the ledger the way it
/// would on the commit path (`Replica::commit_block` deliberately re-checks
/// only the monotonic half — see its doc comment). The plausibility check is
/// correct on the sync path too (`process_synced_value`): a genuinely old,
/// already-decided block carries a small timestamp and always passes an
/// upper bound — only a timestamp actually ahead of the checking node's own
/// clock can ever fail it.
///
/// Also checks `block.app_hash` against `expected_app_hash` (this
/// replica's own `Replica::app_hash()`, the state commitment it holds
/// immediately before this height). This is what converts a diverged
/// proposer — or a diverged voter, for that matter — into an immediate,
/// local `Invalid` instead of a signature on a certificate for a block whose
/// claimed starting state nobody here actually recognizes. Comparing against
/// LOCAL state makes this safe to also run before a vote, unlike the state
/// itself: `expected_app_hash` is exactly what `commit_block`'s identical
/// check compares against, so a block that passes here is guaranteed to also
/// pass there (bar this node's own state changing between the two calls,
/// which the single-threaded `run` loop's locking discipline rules out).
fn screen(block: &Block, ctx: ScreenCtx<'_>) -> Validity {
    // **What a certificate cannot make true**, and what this node can
    // therefore say about any block whatever its height: the transactions are
    // signed for THIS chain, and the block does not claim a time out of the
    // future. Both are pure functions of the block and this node's clock.
    if !(block.verify_txs(ctx.chain_id) && crate::block::time_is_plausible(block.time_secs, ctx.now_secs)) {
        return Validity::Invalid;
    }
    // **`Invalid` is a fail-stop, not a vote.** Upstream's `decide` asserts
    // that the value it is deciding was screened Valid
    // (`core-consensus/src/handle/decide.rs`), so a node that answers Invalid
    // for a block a quorum then decides does not vote against it — its
    // consensus actor panics and the process goes on running with no
    // consensus at all, reporting `SYNC REQUIRED` for ever. Measured on four
    // loopback validators: one sat at height 1 while its peers reached 15,
    // asking for 4, 5, 6 and using none of them, because it had answered
    // Invalid for height 2 a millisecond before it committed height 1.
    //
    // So the three checks below are asked ONLY where this node is in a
    // position to answer them: they are all about the block's PARENT, and a
    // proposal or a synced value routinely arrives before this node has
    // committed that parent. Sync pipelines by design, and a streamed
    // proposal for the next height overtakes the commit of the current one.
    // Where they cannot be asked, they are not skipped but DEFERRED: the app
    // hash, the timestamp and the transaction signatures are all re-checked
    // at commit against the state the block actually follows
    // (`Replica::commit_block_with_certificate`), and the certificate is
    // verified before that.
    if block.height != ctx.at + 1 {
        return Validity::Valid;
    }
    if crate::block::time_is_monotonic(block.time_secs, ctx.parent_time)
        && block.app_hash == ctx.expected_app_hash
        // The two rules that bound what a block can COST every honest
        // validator — a batch ceiling, and no envelope that authorises
        // nothing. Both are pure functions of the parent state, and this node
        // has just checked that the block claims the parent state it holds, so
        // every honest voter reaches the same verdict.
        && (ctx.deterministic)(block)
    {
        Validity::Valid
    } else {
        Validity::Invalid
    }
}

// ---------------------------------------------------------------------------
// ProcessSyncedValue — decode raw synced bytes into a ProposedValue
// ---------------------------------------------------------------------------

/// The sync path carries raw bytes directly (no chunking): decode, remember
/// the block by its value id (same `pending` map `GetValue`/
/// `ReceivedProposalPart` use), and construct the `ProposedValue` shape.
/// `None` on any decode failure — the engine's own contract for this
/// handler.
///
/// Screened exactly like a streamed proposal (`screen`): a block handed over
/// by a syncing peer is untrusted input too, and one carrying unsigned
/// transactions must not be adopted just because it arrived with a
/// certificate. Refusing it stalls THIS node's catch-up, which is the correct
/// fail-closed answer — a network that decided a block this node considers
/// unauthenticated is not one to follow silently.
fn process_synced_value(
    pending: &mut BTreeMap<EdetValueId, PendingValue>,
    height: EdetHeight,
    round: Round,
    proposer: EdetAddress,
    value_bytes: &[u8],
    ctx: ScreenCtx<'_>,
) -> Option<ProposedValue<EdetContext>> {
    let block = Block::decode(value_bytes).ok()?;
    let value = EdetValue::from_block(block).ok()?;
    let validity = screen(&value.block, ctx);
    if validity == Validity::Valid {
        pending.insert(value.id, PendingValue { round, valid_round: Round::Nil, proposer, block: value.block.clone() });
    }
    Some(ProposedValue { height, round, valid_round: Round::Nil, proposer, value, validity })
}

// ---------------------------------------------------------------------------
// Validator set live as of a given height
// ---------------------------------------------------------------------------

/// The validator set live AS OF `height` — `Replica::validators_at(height)`'s
/// historical power map, joined against `Replica::keys_at(height)`'s
/// historical key map via `EdetValidatorSet::build`, which falls back
/// to a member's current key only for an id that map has no rotation
/// recorded for (see `build`'s doc comment for why that fallback is sound).
/// `None` if `height` predates this replica's oldest retained history entry
/// (cannot happen for `replica.height` itself — the baseline always
/// covers it) OR if `build` refused (an empty power map, a member that
/// no longer resolves, or an unparseable key) — both collapse to the same
/// `None` because every caller below already treats "no set available" as
/// "stall this height, do not advance", which is the correct response to
/// either cause. `build` refusing is logged here rather than silently
/// swallowed, since — unlike the height-out-of-range case — it should not
/// normally happen and is worth a human noticing.
fn validator_set_at(replica: &Replica, height: u64) -> Option<EdetValidatorSet> {
    let vs = replica.validators_at(height)?;
    let keys = replica.keys_at(height);
    match EdetValidatorSet::build(&replica.state, vs, &keys) {
        Ok(set) => Some(set),
        Err(e) => {
            eprintln!("validator_set_at({height}): refusing a validator set that failed to build: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Decided — verify the certificate before ever committing
// ---------------------------------------------------------------------------

/// Verify `certificate` against the validator set live for the height it
/// decides, THEN (and only then) look up the matching block in `pending`.
/// `replica.height` is the set to verify against because `Decided` fires
/// BEFORE that height advances — i.e. `certificate.height` is always
/// `replica.height + 1`, and `validator_set_at(replica, replica.height)` is
/// exactly "the set live going into the height being decided".
///
/// Pulled out of the `Decided` handler so it is unit-testable in-process
/// without an actor system (exercising the real engine needs a real cluster):
/// this function alone IS the acceptance criterion — a
/// certificate not signed by 2/3+ of the validator set's voting power, or
/// "signed" by keys that are not the validator set's registered keys (a
/// caller past the cluster-token perimeter but holding no validator's private key),
/// must never yield `Some`, no matter what block happens to sit in
/// `pending`.
fn verify_decided(
    replica: &Replica,
    pending: &BTreeMap<EdetValueId, PendingValue>,
    signing: &EdetSigningProvider,
    certificate: &CommitCertificate<EdetContext>,
) -> Option<Block> {
    let validator_set = validator_set_at(replica, replica.height)?;
    // `async` on the trait and pure computation in fact: driven to completion
    // here rather than awaited, because the caller holds the node lock and
    // a yield under it would be the deadlock the lock discipline forbids.
    futures::executor::block_on(signing.verify_commit_certificate(
        &EdetContext,
        certificate,
        &validator_set,
        ThresholdParams::default(),
    ))
    .ok()?;
    pending.get(&certificate.value_id).map(|pv| pv.block.clone())
}

/// Instruct consensus to retry `height` — the response whenever `Decided`
/// cannot commit: an unverified certificate, or a verified certificate
/// for a value this node never observed/reassembled (crash recovery, or it
/// joined mid-round — nothing to commit locally; a real deployment's sync
/// mechanism, `ProcessSyncedValue`, is what closes that gap, not a stall
/// here). Never advances the height.
///
/// `None` only if BOTH the height-aware lookup and the direct
/// current-state fallback fail to build a set — meaning `build` refused
/// (empty power map, a validator that no longer resolves to a keyed member,
/// or a bad key) against `replica.state.validators` itself, the same source
/// `apply.rs`'s `MIN_VALIDATORS` floor is meant to keep always non-empty and
/// always resolvable. Should be unreachable; the caller logs and drops the
/// reply rather than fabricating a set or aborting the process.
fn restart(replica: &Replica, height: EdetHeight) -> Option<Next<EdetContext>> {
    let validator_set = match validator_set_at(replica, replica.height) {
        Some(vs) => vs,
        None => {
            match EdetValidatorSet::build(&replica.state, &replica.state.validators, &replica.keys_at(replica.height)) {
                Ok(vs) => vs,
                Err(e) => {
                    eprintln!(
                        "restart(height={}): no validator set could be built even from current state.validators \
                     (should be unreachable): {e}",
                        height.as_u64()
                    );
                    return None;
                }
            }
        }
    };
    Some(Next::Restart(height, validator_set))
}

// ---------------------------------------------------------------------------
// CommitCertificate <-> WAL bytes
// ---------------------------------------------------------------------------

/// A byte-portable mirror of `CommitCertificate<EdetContext>` for WAL
/// storage. `CommitCertificate<Ctx>` itself carries no `Serialize`/
/// `Deserialize` impl — only a handful of sibling types in the vendored
/// `certificate.rs` derive `serde` behind that crate's own feature, and
/// `CommitCertificate` is not one of them — and Rust's orphan rules forbid
/// implementing a foreign trait for a foreign generic type from this crate
/// regardless. So this is a small owned mirror edet can derive `Serialize`/
/// `Deserialize` on directly, matching `store.rs`'s existing manual-frame
/// convention rather than reaching for a third crate. Signatures are carried
/// as plain `Vec<u8>` rather than `EdetSignature`'s `[u8; 64]` directly:
/// serde's built-in array impl only covers `[T; 0..=32]`, and a 64-byte array
/// needs either a hand-rolled `Visitor` or this — the simpler option, since
/// `EdetSignature`'s `[u8; 64] <-> Vec<u8>` conversion is already trivial.
#[derive(Serialize, Deserialize)]
struct StoredCertificate {
    height: EdetHeight,
    round: Round,
    value_id: EdetValueId,
    signatures: Vec<(EdetAddress, Vec<u8>)>,
}

/// Encode a certificate for `Store::append_certificate`. `store.rs` itself
/// never sees a `CommitCertificate` — it only ever handles the opaque bytes
/// this produces, keeping the crate's default (non-`malachite`) build free
/// of any dependency on `malachitebft-core-types`.
fn encode_commit_certificate(cert: &CommitCertificate<EdetContext>) -> Vec<u8> {
    let stored = StoredCertificate {
        height: cert.height,
        round: cert.round,
        value_id: cert.value_id,
        signatures: cert
            .commit_signatures
            .iter()
            .map(|cs| (cs.address, cs.signature.0.to_vec()))
            .collect(),
    };
    edet_state::codec::encode(&stored)
        .expect("StoredCertificate is a plain, finite struct: encoding into a Vec cannot fail")
}

/// The inverse of `encode_commit_certificate`, for `GetDecidedValue`. Fails
/// (rather than panicking) on a malformed signature length — WAL bytes are
/// untrusted input from this function's point of view (corruption, a future
/// format change), never assumed well-formed.
fn decode_commit_certificate(bytes: &[u8]) -> Result<CommitCertificate<EdetContext>, edet_state::codec::CodecError> {
    let stored: StoredCertificate = edet_state::codec::decode(bytes)?;
    let mut commit_signatures = Vec::with_capacity(stored.signatures.len());
    for (address, sig_bytes) in stored.signatures {
        let arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| edet_state::codec::CodecError("stored signature is not 64 bytes".into()))?;
        commit_signatures.push(CommitSignature::new(address, EdetSignature(arr)));
    }
    Ok(CommitCertificate { height: stored.height, round: stored.round, value_id: stored.value_id, commit_signatures })
}

// ---------------------------------------------------------------------------
// Snapshot export and import — the way back for a node below every peer's
// history floor
// ---------------------------------------------------------------------------

/// What one node hands another when the WAL cannot bridge the gap.
///
/// A node down longer than the shortest peer's `prune_margin_blocks` is below
/// every peer's floor: nobody holds the blocks it needs and value sync has
/// nothing to serve. The way back is an operator carrying a state, and this is
/// the file — a snapshot, plus exactly what makes it checkable.
///
/// **What the node checks is that this is a CERTIFIED, invariant-satisfying
/// state of this chain.** Trust in the file's source stays the operator's: a
/// state names its own validator set, so a fabricated chain would carry a
/// fabricated set that signed it perfectly. What cannot be fabricated is
/// agreement with the genesis this node already holds, which is where the
/// import starts.
#[derive(Serialize, Deserialize)]
struct SnapshotBundle {
    chain_id: String,
    /// The height the snapshot state is AT.
    height: u64,
    /// The state, in the consensus encoding.
    state: Vec<u8>,
    /// The block at `height + 1`, whose `app_hash` is the commitment of the
    /// state above — which is how a certificate over a BLOCK certifies a
    /// STATE.
    next_block: Block,
    /// The commit certificate for that block.
    next_certificate: Vec<u8>,
}

/// Write this node's latest snapshot, with the block and certificate that
/// certify it, to `out`.
pub fn export_snapshot(home_dir: &std::path::Path, out: &std::path::Path) -> eyre::Result<u64> {
    let mut store = crate::store::Store::open(home_dir.join("edet-store"))?;
    let (height, state) = store
        .read_snapshot()?
        .ok_or_else(|| eyre::eyre!("this home holds no snapshot yet — a node writes one every snapshot interval"))?;
    // The NEXT block is what ties a certificate to this state: block H+1
    // claims as its `app_hash` the commitment of the state after H, which is
    // exactly what the snapshot holds.
    let next_block = store.find_block(height + 1)?.ok_or_else(|| {
        eyre::eyre!(
            "the snapshot is at height {height} and this node has not committed {} yet — a snapshot is certified by \
             the block above it, so wait one block",
            height + 1
        )
    })?;
    let next_certificate = store
        .find_certificate(height + 1)?
        .ok_or_else(|| eyre::eyre!("no commit certificate for height {}", height + 1))?;
    let app_hash = crate::block::state_hash(&state)?;
    if next_block.app_hash != app_hash {
        eyre::bail!(
            "this node's own store disagrees with itself: block {} claims app hash {} and the snapshot at {height} \
             commits to {}",
            height + 1,
            crate::block::hex32(&next_block.app_hash),
            crate::block::hex32(&app_hash)
        );
    }
    let bundle = SnapshotBundle {
        chain_id: state.chain_id.clone(),
        height,
        state: edet_state::codec::encode(&state).map_err(|e| eyre::eyre!("encoding the snapshot state: {e}"))?,
        next_block,
        next_certificate,
    };
    let bytes = edet_state::codec::encode(&bundle).map_err(|e| eyre::eyre!("encoding the bundle: {e}"))?;
    std::fs::write(out, bytes)?;
    Ok(height)
}

/// Install `from` as this home's snapshot, after checking everything a node
/// can check about it.
///
/// Five refusals, and the first is the one that makes the rest mean anything:
/// the state has to be a state of THIS chain, by the genesis this home already
/// holds. Then it must satisfy the invariants, its commitment must be what the
/// certified block above it claims, that block's id must be what the
/// certificate decides, and the certificate must verify against the validator
/// set. A home already at or past that height is refused outright — importing
/// into it could only ever rewind it.
pub fn import_snapshot(home_dir: &std::path::Path, from: &std::path::Path) -> eyre::Result<u64> {
    let app = crate::engine_node::EdetApp::at(home_dir.to_path_buf(), None, None);
    let genesis = {
        use malachitebft_app_channel::app::node::Node;
        app.load_genesis()?
    };
    let expected = crate::engine_node::genesis_state(&genesis)?;

    let bundle: SnapshotBundle = edet_state::codec::decode(&std::fs::read(from)?)
        .map_err(|e| eyre::eyre!("{} is not a snapshot bundle: {e}", from.display()))?;
    if bundle.chain_id != expected.chain_id {
        eyre::bail!(
            "that snapshot is for chain \"{}\" and this home is chain \"{}\"",
            bundle.chain_id,
            expected.chain_id
        );
    }
    let state: edet_state::State =
        edet_state::codec::decode(&bundle.state).map_err(|e| eyre::eyre!("the snapshot state does not decode: {e}"))?;
    if state.chain_id != expected.chain_id {
        eyre::bail!("the snapshot state names chain \"{}\"", state.chain_id);
    }
    if state.root_salt != expected.root_salt {
        eyre::bail!(
            "the snapshot state carries root salt {} and this home's genesis says {} — every validator must hold \
             the same value or they compute different roots",
            crate::block::hex32(&state.root_salt),
            crate::block::hex32(&expected.root_salt)
        );
    }
    edet_state::invariants::audit(&state)
        .map_err(|v| eyre::eyre!("the snapshot state violates invariant {}: it is not a legal state", v.0))?;

    let app_hash = crate::block::state_hash(&state)?;
    if bundle.next_block.app_hash != app_hash {
        eyre::bail!(
            "the certified block claims app hash {} and this state commits to {} — the file's state and its \
             certificate are not about each other",
            crate::block::hex32(&bundle.next_block.app_hash),
            crate::block::hex32(&app_hash)
        );
    }
    if bundle.next_block.height != bundle.height + 1 {
        eyre::bail!(
            "the certified block is at height {} and the snapshot at {}",
            bundle.next_block.height,
            bundle.height
        );
    }

    let certificate = decode_commit_certificate(&bundle.next_certificate)
        .map_err(|e| eyre::eyre!("the commit certificate does not decode: {e}"))?;
    let value = EdetValue::from_block(bundle.next_block.clone())
        .map_err(|e| eyre::eyre!("the certified block does not encode: {e}"))?;
    if certificate.value_id != value.id || certificate.height.as_u64() != bundle.next_block.height {
        eyre::bail!("the certificate decides a different value than the block in the file");
    }

    // **The set that signed is the set the imported state names**, which is
    // the one live at the height the certificate decides. Where that differs
    // from the genesis set, this check is a statement about the file's own
    // internal consistency and not about the chain — so say so, rather than
    // let the word "verified" carry more than it holds.
    if state.validators != expected.validators {
        eprintln!(
            "note: this chain's validator set has changed since genesis, so the certificate below verifies against \
             the set the imported state itself names. Vouch for where this file came from."
        );
    }
    let keys = validator_key_snapshot_of(&state);
    let validator_set = EdetValidatorSet::build(&state, &state.validators, &keys)
        .map_err(|e| eyre::eyre!("the imported state names a validator set that will not build: {e}"))?;
    // A verifier needs no key of its own: `verify_commit_certificate` checks
    // signatures against the validator set's public keys, and the provider's
    // own key signs nothing here.
    futures::executor::block_on(
        EdetSigningProvider::new(ed25519_dalek::SigningKey::from_bytes(&[0u8; 32]), state.chain_id.clone())
            .verify_commit_certificate(&EdetContext, &certificate, &validator_set, ThresholdParams::default()),
    )
    .map_err(|e| eyre::eyre!("the commit certificate does not verify against this chain's validators: {e:?}"))?;

    let mut store = crate::store::Store::open(home_dir.join("edet-store"))?;
    if let Some((have, _)) = store.read_snapshot()? {
        if have >= bundle.height {
            eyre::bail!(
                "this home already holds a snapshot at height {have}; importing {} would rewind it",
                bundle.height
            );
        }
    }
    if let Some(top) = store.max_block_height() {
        if top >= bundle.height {
            eyre::bail!("this home has already committed height {top}; importing {} would rewind it", bundle.height);
        }
    }
    store.write_snapshot(bundle.height, &state)?;
    Ok(bundle.height)
}

/// The validator key map an imported state implies, in the shape
/// `EdetValidatorSet::build` takes.
fn validator_key_snapshot_of(
    state: &edet_state::State,
) -> BTreeMap<edet_state::types::MemberId, edet_state::types::Key> {
    state
        .validators
        .keys()
        .filter_map(|id| state.members.get(id).and_then(|m| m.consensus_key).map(|k| (*id, k)))
        .collect()
}

// ---------------------------------------------------------------------------
// External observability for a real running node (loopback cluster
// harness + its automated test — this crate has no other window into a
// node once its `Replica` is moved into this loop's ownership below).
// Deliberately NOT part of the consensus-critical path: nothing here is
// consulted by any `AppMsg` handler, only written after a commit already
// landed, so a failure to write it can never affect agreement itself.
// ---------------------------------------------------------------------------

/// What one node currently knows about its own committed state — the same
/// two facts `serve/views.rs`'s `/network` endpoint exposes for the dev-mode
/// HTTP cluster, mirrored here for the embedded engine (which has no HTTP
/// server of its own).
#[derive(Serialize)]
struct EngineStatus {
    height: u64,
    state_hash: String,
}

/// Best-effort, atomic (write-temp-then-rename) status write after a commit.
/// Never propagates an error — this is an observability side channel, not
/// something the consensus loop should ever fail over.
fn write_status(path: &std::path::Path, height: u64, state_hash: [u8; 32]) {
    let status = EngineStatus { height, state_hash: state_hash.iter().map(|b| format!("{b:02x}")).collect() };
    let Ok(json) = serde_json::to_vec_pretty(&status) else { return };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Snapshot `pending` to `undecided.bin` under `node.cfg.data_dir` — a
/// no-op for an in-memory replica (`data_dir: None`, e.g. every unit test in
/// this module), matching `Replica`'s own durability gating. Reads
/// `node.cfg.data_dir` directly, never `node.lock()`: `cfg` is a plain
/// field, not behind the core `Mutex`, which is exactly what lets this be
/// called from inside the `Decided` handler while that handler still holds
/// the lock (`core.record_outcomes` et al) without deadlocking.
///
/// A write failure (full disk, permissions) is logged and swallowed, never
/// propagated: this cache is a liveness optimization only (`store::
/// write_undecided`'s doc comment), and a consensus loop that stalled or
/// crashed over a failed BACKUP write would be strictly worse than one that
/// just tries again next time `pending` changes.
fn persist_pending(node: &crate::serve::Node, height: u64, pending: &BTreeMap<EdetValueId, PendingValue>) {
    let Some(dir) = &node.cfg.data_dir else { return };
    let blocks: Vec<Block> = pending.values().map(|pv| pv.block.clone()).collect();
    if let Err(e) = crate::store::write_undecided(dir, height, &blocks) {
        eprintln!("persist_pending(height={height}): failed to write the undecided-proposal cache: {e}");
    }
}

// ---------------------------------------------------------------------------
// The application side of the engine channel
// ---------------------------------------------------------------------------

/// One loop per validator. `time_source` supplies the proposer timestamp for
/// new blocks (consensus time; replicas apply whatever the decided block
/// carries). `own_address` is this validator's own `MemberId`-derived
/// address (used as the `proposer` field when this node builds a value in
/// `GetValue`). `signing` verifies commit certificates in `Decided`
/// — verification only ever checks a signature against the validator set's
/// OWN recorded public keys, never `signing`'s private key, so any
/// `EdetSigningProvider` instance (this validator's own consensus key, in a
/// real deployment reused from the same key handed to
/// `start_engine`/`spawn_consensus_actor`) works.
///
/// Never invoked from `main.rs` (see this file's module doc) — reachable
/// only by its own unit tests below.
pub async fn run(
    node: std::sync::Arc<crate::serve::Node>,
    mut channels: Channels<EdetContext>,
    mut time_source: impl FnMut() -> u64,
    own_address: EdetAddress,
    signing: EdetSigningProvider,
    status_path: Option<std::path::PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    // The engine drives the SAME shared `NodeCore` (replica + mempool) that
    // the IPC/read/submit layer locks — so the wallet reads Malachite-committed
    // state and client submissions (`serve::submit` → `core.mempool`) get
    // proposed by this node. Each arm locks briefly and drops the guard before
    // any `.await` (never holds the node lock across a network send).
    // Blocks proposed or received this height, by full block-hash value id.
    let mut pending: BTreeMap<EdetValueId, PendingValue> = BTreeMap::new();
    // In-flight proposal-part reassembly, by (peer, stream id).
    let mut assembling = PartStreams::default();
    // When this node last proposed, for `EMPTY_BLOCK_INTERVAL`. Starts in the
    // past so the first proposal — the one that begins the epoch catch-up on
    // a fresh chain — is never delayed.
    let mut last_block_at = Instant::now() - EMPTY_BLOCK_INTERVAL;

    // `pending` is otherwise in-memory only and lost on restart — a
    // restarted node could not restream or commit a value it had already
    // voted for, and fell back to sync every time. `node.cfg.data_dir` is
    // the SAME directory `Replica`'s own `Store` opens (`engine_node.rs`),
    // so this reads whatever the previous process last wrote via
    // `write_undecided`. Only the height metadata is kept here — the
    // round/proposer/valid_round each block needs for `StartedRound`'s
    // reply are NOT recoverable from a bare `Block` (they were never part of
    // it), so a value reloaded this way is tagged `Round::Nil`/`own_address`
    // rather than its true origin. That is honestly imprecise, not unsound:
    // `verify_decided`/`GetDecidedValue` key `pending` by value id alone and
    // never look at this metadata, so a reloaded value is exactly as usable
    // for committing a certificate this node already has as one still held
    // from before the restart. Only `StartedRound`'s replay-into-consensus
    // path reads the metadata, and there a wrong round is never worse than
    // today's baseline (an empty reply) — the value simply will not match
    // the round consensus is currently on and sits unused until a real
    // proposal for it arrives some other way, same as it would if this
    // loaded nothing at all.
    if let Some(dir) = &node.cfg.data_dir {
        if let Some((height, blocks)) = crate::store::read_undecided(dir) {
            for block in blocks {
                if block.height != height {
                    continue; // defensive: a torn/foreign write should never mix heights
                }
                if let Ok(value) = EdetValue::from_block(block.clone()) {
                    pending.insert(
                        value.id,
                        PendingValue { round: Round::Nil, valid_round: Round::Nil, proposer: own_address, block },
                    );
                }
            }
        }
    }

    while let Some(msg) = channels.consensus.recv().await {
        match msg {
            AppMsg::ConsensusReady { reply } => {
                let core = node.lock();
                let start = EdetHeight::new(core.replica.height + 1);
                let height = core.replica.height;
                // Was `.expect(...)` — a replica always retains at least
                // the genesis validator-set baseline, so this should
                // never fail, but "should never" is not "cannot": `build`
                // now fails closed on a state it cannot resolve rather than
                // shrinking the set, and this handler must not turn that
                // into a process abort. Dropping `reply` without sending
                // leaves consensus unable to start — a stall, loudly logged,
                // instead of a crash.
                match validator_set_at(&core.replica, height) {
                    Some(vset) => {
                        drop(core);
                        let _ = reply.send((start, vset));
                    }
                    None => {
                        drop(core);
                        eprintln!(
                            "ConsensusReady: no validator set could be built for height {height} \
                             (should be unreachable) — refusing to start consensus"
                        );
                    }
                }
            }

            AppMsg::StartedRound { height, reply_value, .. } => {
                // Reply with every value this node already holds in
                // `pending` for THIS height — a fresh in-process round
                // change (the common case: a timeout advanced the round,
                // nothing crashed) or a value reloaded from `undecided.bin`
                // at startup (§ above `run`'s WAL-restore block). Filtered
                // by height, not round: `AppMsg::StartedRound`'s own
                // contract (`msgs.rs`, vendored) says only "reply with the
                // values you have, or an empty vector" — it does not require
                // an exact round match, and `on_proposed_value` (core-
                // consensus) itself queues a value whose round does not yet
                // match rather than discarding it, so handing back every
                // held value for the height is the more complete answer, not
                // an incorrect one.
                let values: Vec<ProposedValue<EdetContext>> = pending
                    .iter()
                    .filter(|(_, pv)| pv.block.height == height.as_u64())
                    .map(|(id, pv)| ProposedValue {
                        height,
                        round: pv.round,
                        valid_round: pv.valid_round,
                        proposer: pv.proposer,
                        value: EdetValue { id: *id, block: pv.block.clone() },
                        // Only ever `Valid` reaches `pending` in the first
                        // place (`screen`/`reassemble_proposal_part`,
                        // `process_synced_value`, `GetValue`'s own build) —
                        // an `Invalid` block is never inserted, so there is
                        // nothing else this could honestly report.
                        validity: Validity::Valid,
                    })
                    .collect();
                let _ = reply_value.send(values);
            }

            AppMsg::GetValue { height, round, reply, timeout, .. } => {
                // Lock only to snapshot the mempool, the last committed
                // timestamp, and the current app_hash; drop before streaming.
                let (txs, last_time_secs, app_hash) = {
                    let mut core = node.lock();
                    // Bond-filtered: a
                    // transaction the gate would now refuse costs block space
                    // and buys nothing, since `apply` re-checks on commit.
                    // Advisory and node-local — `screen` deliberately does not
                    // consult it, so no vote ever turns on a headroom reading.
                    let txs = core.proposable_batch(GET_VALUE_BATCH);
                    (txs, core.replica.last_time_secs, core.replica.app_hash())
                };
                // Pace an EMPTY block, and only an empty one — see
                // `EMPTY_BLOCK_INTERVAL`. The wait sits here, before the
                // block is built, so its timestamp is the one it is actually
                // proposed at. It also stalls this loop, which is why it is
                // bounded well inside the timeout consensus gave us: the
                // messages that queue behind it are the next height's, and
                // every Malachite timeout is longer than this wait.
                let wait = propose_delay(!txs.is_empty(), last_block_at.elapsed(), timeout);
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                last_block_at = Instant::now();
                // A proposer must never build a block it would itself
                // refuse in `screen` — clamp to `last_time_secs` so a node
                // whose own clock went backwards (a restart, a bad RTC)
                // proposes a valid (if slightly stale) timestamp instead of
                // one every honest validator, including this one on replay,
                // votes Invalid on.
                let time_secs = time_source().max(last_time_secs);
                // App_hash is THIS node's own `Replica::app_hash()` — the
                // state it holds right now, before this height — never a
                // guess or a hash of the block this call is about to build.
                let block = Block { height: height.as_u64(), time_secs, app_hash, txs };
                let value = EdetValue::from_block(block.clone())
                    .expect("Block always encodes: a finite in-memory struct serializing into a Vec cannot fail");
                pending.insert(
                    value.id,
                    PendingValue { round, valid_round: Round::Nil, proposer: own_address, block: block.clone() },
                );
                persist_pending(&node, height.as_u64(), &pending);

                let _ = reply.send(LocallyProposedValue::new(height, round, value.clone()));

                for msg in stream_value(own_address, height, round, Round::Nil, &block, value.id) {
                    channels.network.send(NetworkMsg::PublishProposalPart(msg)).await?;
                }
            }

            AppMsg::ExtendVote { reply, .. } => {
                // No vote extensions in scope (`Extension = ()`).
                let _ = reply.send(None);
            }

            AppMsg::VerifyVoteExtension { reply, .. } => {
                let _ = reply.send(Ok(()));
            }

            AppMsg::RestreamProposal { height, round, valid_round, address, value_id } => {
                if let Some(block) = pending.get(&value_id).map(|pv| pv.block.clone()) {
                    for msg in stream_value(address, height, round, valid_round, &block, value_id) {
                        channels.network.send(NetworkMsg::PublishProposalPart(msg)).await?;
                    }
                }
                // Else: we no longer hold this value. `pending` is now
                // restored from `undecided.bin` at startup, so this is
                // narrow — it is still reachable for a
                // value from a height/round this node never actually saw
                // (joined mid-round; nothing was ever in `pending` to lose)
                // or one committed and cleared since. Documented limitation,
                // not a silent bug.
            }

            AppMsg::GetHistoryMinHeight { reply } => {
                // Read from the store rather than answered `1`: the WAL is
                // pruned below the last snapshot (`Store::prune_below`), so a
                // node that has been running long enough no longer holds its
                // early heights. Claiming to would turn every sync request for
                // one into a silent failure the peer cannot diagnose.
                let min = node.lock().replica.history_min_height();
                let _ = reply.send(EdetHeight::new(min));
            }

            AppMsg::ReceivedProposalPart { from, part, reply } => {
                // Parent_time is this replica's own last-committed
                // timestamp (never a peer's claim); now_secs is this node's
                // own clock. `chain_id` is this replica's own state, the
                // same chain a signature must be bound to; `expected_app_hash`
                // is this replica's own `Replica::app_hash()`. All local
                // reads — no `.await` follows before they're consumed, so
                // nothing here holds the node lock across one.
                let (parent_time, chain_id, expected_app_hash, at) = {
                    let core = node.lock();
                    (
                        core.replica.last_time_secs,
                        core.replica.state.chain_id.clone(),
                        core.replica.app_hash(),
                        core.replica.height,
                    )
                };
                let now_secs = time_source();
                let result = reassemble_proposal_part(
                    &mut assembling,
                    &mut pending,
                    from,
                    part,
                    ScreenCtx {
                        chain_id: &chain_id,
                        at,
                        parent_time,
                        now_secs,
                        expected_app_hash,
                        deterministic: &|b: &Block| {
                            crate::block::deterministic_validity(b, &node.lock().replica.state).is_ok()
                        },
                    },
                );
                // Only a `Valid` result ever changed `pending`
                // (`reassemble_proposal_part` only inserts on that branch) —
                // skip the write otherwise, since it would just re-persist
                // whatever was already on disk.
                if let Some(pv) = &result {
                    if pv.validity == Validity::Valid {
                        persist_pending(&node, pv.height.as_u64(), &pending);
                    }
                }
                let _ = reply.send(result);
            }

            AppMsg::Decided { certificate, reply, .. } => {
                // `next` is `Option`: the happy path and the `restart`
                // fallback both now go through fallible validator-set
                // construction. `None` means neither could build a set even
                // from this replica's OWN just-committed state — should be
                // unreachable (`MIN_VALIDATORS` is enforced on every removal
                // path in `apply.rs`), but this handler must fail closed
                // (drop the reply, stall this height, loudly log) rather
                // than abort the process on an `.expect()`.
                let next = {
                    let mut core = node.lock();
                    match verify_decided(&core.replica, &pending, &signing, &certificate) {
                        Some(block) => {
                            let cert_bytes = encode_commit_certificate(&certificate);
                            // The one commit path (`NodeCore::commit_decided`):
                            // it applies the block against its certificate and
                            // runs every post-commit duty with it — the tx
                            // outcomes for `/tx/outcome/:hash`, draining the
                            // mempool, the commit counter, and the
                            // session sweep that cuts off a token whose key was
                            // rotated out or whose member was suspended by this
                            // very block. Those duties belong on the one
                            // consensus's `try_commit` and this handler ran only
                            // the first two.
                            match core.commit_decided(&block, &cert_bytes) {
                                Ok(()) => {
                                    pending.clear();
                                    // `pending` was not the only map this
                                    // commit invalidates. Reassembly buffers
                                    // for the height just decided are dead,
                                    // and before this the ONLY thing that ever
                                    // removed one was a transport `Fin` — the
                                    // one message a hostile sender withholds
                                    // and a lost round never produces, so the
                                    // map only ever grew.
                                    assembling.evict_decided(core.replica.height);
                                    // The persisted cache must not
                                    // outlive what it caches — clear it the
                                    // same commit that clears `pending`
                                    // in-memory, or a later restart would
                                    // reload an already-decided height's
                                    // blocks as if they were still undecided
                                    // (harmless for `verify_decided`/sync,
                                    // which key by value id and re-check
                                    // everything anyway, but pointless
                                    // clutter `StartedRound` would then also
                                    // hand back for a height already behind
                                    // this replica).
                                    persist_pending(&node, core.replica.height, &pending);
                                    if let Some(path) = &status_path {
                                        // The cached root, not a fresh one:
                                        // a status file written per block
                                        // used to be a second full build of
                                        // the whole ledger every block.
                                        write_status(path, core.replica.height, core.replica.app_hash());
                                    }
                                    match validator_set_at(&core.replica, core.replica.height) {
                                        Some(next_vs) => {
                                            Some(Next::Start(EdetHeight::new(core.replica.height + 1), next_vs))
                                        }
                                        None => {
                                            eprintln!(
                                                "Decided: committed height {} but its own validator set failed \
                                                 to build (should be unreachable) — refusing to \
                                                 advance consensus past it",
                                                core.replica.height
                                            );
                                            None
                                        }
                                    }
                                }
                                // **A violated invariant ends the loop
                                // rather than re-deciding the height** (the

                                //
                                // `restart` asks consensus to decide this
                                // height again, which is right for a
                                // certificate this node could not verify or a
                                // block it has not seen: those can succeed on
                                // the next attempt. A block that left the
                                // ledger in a state the invariants say cannot
                                // exist cannot — the block is deterministic
                                // and so is the audit, so every retry reaches
                                // the same verdict, and this node would sit in
                                // a livelock re-deciding a height it will
                                // never accept.
                                //
                                // Stopping is also what makes the fail-stop
                                // MEAN something: every honest node runs the
                                // same code on the same block, so they all
                                // stop at the same height. Returning here
                                // exits `run`, which brings the process down
                                // with the reason named, rather than leaving a
                                // validator that looks alive and commits
                                // nothing.
                                Err(crate::replica::ReplicaError::InvariantViolated { height, detail }) => {
                                    eprintln!(
                                        "HALT at height {height}: the ledger is left in a state the \
                                         invariants say cannot exist — {detail}. Nothing was written: the \
                                         WAL and the snapshot are as they were before this block, so a \
                                         restart comes back at height {}. Until this process exits every \
                                         read answers 503 with this reason. Every honest node stops here.",
                                        core.replica.height
                                    );
                                    return Err(format!("invariant violated at height {height}: {detail}").into());
                                }
                                Err(_) => restart(&core.replica, certificate.height),
                            }
                        }
                        None => restart(&core.replica, certificate.height),
                    }
                };
                if let Some(next) = next {
                    let _ = reply.send(next);
                }
            }

            AppMsg::GetDecidedValue { height, reply } => {
                let decided = node.lock().replica.decided_value_at(height.as_u64()).ok().flatten();
                let result = decided.and_then(|(block, cert_bytes)| {
                    let value_bytes = block.encode().ok()?;
                    let certificate = decode_commit_certificate(&cert_bytes).ok()?;
                    Some(RawDecidedValue { value_bytes: Bytes::from(value_bytes), certificate })
                });
                let _ = reply.send(result);
            }

            AppMsg::ProcessSyncedValue { height, round, proposer, value_bytes, reply } => {
                let (parent_time, chain_id, expected_app_hash, at) = {
                    let core = node.lock();
                    (
                        core.replica.last_time_secs,
                        core.replica.state.chain_id.clone(),
                        core.replica.app_hash(),
                        core.replica.height,
                    )
                };
                let now_secs = time_source();
                let result = process_synced_value(
                    &mut pending,
                    height,
                    round,
                    proposer,
                    &value_bytes,
                    ScreenCtx {
                        chain_id: &chain_id,
                        at,
                        parent_time,
                        now_secs,
                        expected_app_hash,
                        deterministic: &|b: &Block| {
                            crate::block::deterministic_validity(b, &node.lock().replica.state).is_ok()
                        },
                    },
                );
                // Same reasoning as `ReceivedProposalPart` — only a
                // `Valid` result touched `pending`.
                if let Some(pv) = &result {
                    if pv.validity == Validity::Valid {
                        persist_pending(&node, pv.height.as_u64(), &pending);
                    }
                }
                let _ = reply.send(result);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use malachitebft_core_types::{Context, NilOrVal, ValidatorSet as _};
    use malachitebft_signing::SigningProvider;

    use crate::block::{dev_seed, pubkey_of, SignedTx};
    use edet_state::types::Party;
    use edet_state::State;

    fn seeded_state(n: u8) -> State {
        let mut st = State::default();
        for i in 0..n {
            let key = pubkey_of(&dev_seed(i));
            let id = st.add_underwriter(vec![key], 25_000.0).expect("genesis member");
            st.set_consensus_key(id, crate::block::pubkey_of(&crate::block::dev_consensus_seed(id as u8)))
                .expect("consensus key");
            st.set_genesis_validator(id, 1).expect("genesis validator");
        }
        st
    }

    fn sample_block(height: u64) -> Block {
        Block { height, time_secs: height * 30, app_hash: TEST_APP_HASH, txs: Vec::new() }
    }

    /// A generously large "now" for tests that exercise reassembly/decoding
    /// and do not care about the plausibility bound — comfortably ahead of
    /// every sample block's small `time_secs` (`sample_block`, `forged_block`,
    /// `unsigned_block` all use `height * 30`).
    const TEST_NOW: u64 = 1_000_000;

    /// A fixed app_hash for tests that are about signatures or timing, not
    /// the state-commitment rule itself — every helper block below carries this value, and every
    /// `screen`/`reassemble_proposal_part`/`process_synced_value` call in
    /// this module is given the SAME value as `expected_app_hash`, so those
    /// tests exercise exactly the property their name says and nothing about
    /// it leaks in as an incidental pass/fail. That property is proven
    /// separately (`screen_rejects_a_mismatched_app_hash`).
    const TEST_APP_HASH: [u8; 32] = [0x42; 32];

    /// The state-dependent half of the screen, granted, so a test named for a
    /// buffering or timestamp property fails for that property alone. The two
    /// rules it stands in for have their own probes below.
    fn permissive(_: &Block) -> bool {
        true
    }

    /// The screening context every reassembly test below hands in:
    /// permissive on timing, matching on chain id and app_hash, so a test
    /// named for a buffering property fails for that property alone.
    fn test_ctx() -> ScreenCtx<'static> {
        ScreenCtx {
            chain_id: crate::block::DEV_CHAIN_ID,
            // Genesis: a block at height 1 gets the whole screen, which is
            // what the probes asserting a verdict on one need. The multi-chunk
            // probe streams a block at 3 and is about chunking, not about the
            // parent state.
            at: 0,
            parent_time: 0,
            now_secs: TEST_NOW,
            expected_app_hash: TEST_APP_HASH,
            deterministic: &permissive,
        }
    }

    /// Distinct peer identities for the reassembly tests. An identity
    /// multihash (`0x00`, length 4) over four bytes — the same shape
    /// `engine_codec`'s tests mint, rather than pulling a libp2p keypair in
    /// just to have two peers that differ.
    fn test_peer(n: u8) -> PeerId {
        PeerId::from_bytes(&[0x00, 0x04, n, 2, 3, 4]).expect("a 4-byte identity multihash is a valid peer id")
    }

    fn sign_precommit(
        seed: [u8; 32],
        height: EdetHeight,
        round: Round,
        value_id: EdetValueId,
        address: EdetAddress,
    ) -> EdetSignature {
        let vote = EdetContext.new_precommit(height, round, NilOrVal::Val(value_id), address);
        let signing = EdetSigningProvider::new(SigningKey::from_bytes(&seed), crate::block::DEV_CHAIN_ID.to_string());
        futures::executor::block_on(signing.sign_vote(vote)).unwrap().signature
    }

    fn certificate_from(
        height: EdetHeight,
        round: Round,
        value_id: EdetValueId,
        signers: &[(EdetAddress, EdetSignature)],
    ) -> CommitCertificate<EdetContext> {
        CommitCertificate {
            height,
            round,
            value_id,
            commit_signatures: signers.iter().map(|&(a, s)| CommitSignature::new(a, s)).collect(),
        }
    }

    // --- GetValue's chunk stream reassembles back to the same block -

    #[test]
    fn stream_value_and_reassemble_round_trip_a_multi_chunk_block() {
        // A block whose encoding is comfortably larger than one chunk, so
        // the stream carries more than one `Chunk` part (the acceptance
        // criterion's ">1 chunk-worth" case).
        // Genuinely signed: `reassemble_proposal_part` now screens what it
        // reassembles (`screen`), so a stream of placeholder signatures would
        // come back `Invalid` and prove nothing about chunking.
        let big_tx_count = 50;
        let seeds: Vec<[u8; 32]> = (0..4).map(dev_seed).collect();
        let mut block = sample_block(3);
        for i in 0..big_tx_count {
            block.txs.push(
                crate::block::sign_tx(
                    crate::block::DEV_CHAIN_ID,
                    edet_state::tx::Tx::MarkExpired { contract: i },
                    crate::block::counter_nonce(i),
                    30,
                    &seeds,
                )
                .expect("sign"),
            );
        }
        let encoded_len = block.encode().unwrap().len();
        assert!(encoded_len > PROPOSAL_CHUNK_BYTES / 4, "test block should be a meaningful multi-chunk size");

        let value = EdetValue::from_block(block.clone()).unwrap();
        let proposer = EdetAddress(2);
        let msgs = stream_value(proposer, EdetHeight::new(3), Round::new(0), Round::Nil, &block, value.id);

        // Init, at least one Chunk, business Fin, transport Fin.
        assert!(msgs.len() >= 4);

        let mut assembling = PartStreams::default();
        let mut pending = BTreeMap::new();
        let mut result = None;
        for msg in msgs {
            let r = reassemble_proposal_part(&mut assembling, &mut pending, test_peer(1), msg, test_ctx());
            if r.is_some() {
                result = r;
            }
        }

        let proposed = result.expect("a complete, correctly-ordered stream must reassemble");
        assert_eq!(proposed.height, EdetHeight::new(3));
        assert_eq!(proposed.proposer, proposer);
        assert_eq!(proposed.value.id, value.id);
        assert_eq!(proposed.value.block.txs.len(), block.txs.len());
        assert_eq!(proposed.validity, Validity::Valid);
        assert_eq!(pending.get(&value.id).map(|pv| pv.block.height), Some(3));
        assert_eq!(assembling.len(), 0, "a completed stream must not linger in the reassembly map");
    }

    /// What paces an empty block, and what must never be paced.
    ///
    /// The defect: nothing did. A validator with nobody to wait for committed
    /// as fast as the CPU allowed — 1.87M heights and 500 MB of store in 26
    /// minutes solo, and on a phone a store past 86 MB at 116% CPU, every
    /// block of it recording that nothing had happened. A block with no
    /// transactions cannot change anything the ledger keeps within the same
    /// second: the epoch is the only other thing a block advances, and
    /// `begin_block` is idempotent within a wall-clock second.
    ///
    /// Pure, so this asserts the RULE rather than racing a clock — a timing
    /// test for this would be the loaded box it exists to avoid.
    #[test]
    fn empty_blocks_are_paced_and_full_ones_never_are() {
        let timeout = Duration::from_secs(3);

        // A block that carries transactions goes out now, however recently
        // the last one did. Latency for real work is the point.
        assert_eq!(propose_delay(true, Duration::ZERO, timeout), Duration::ZERO);
        assert_eq!(propose_delay(true, Duration::from_millis(1), timeout), Duration::ZERO);

        // An empty block waits out the remainder of the interval.
        assert_eq!(propose_delay(false, Duration::ZERO, timeout), EMPTY_BLOCK_INTERVAL);
        assert_eq!(propose_delay(false, Duration::from_millis(600), timeout), Duration::from_millis(400));

        // ...and once the interval has passed, not at all — so the epoch
        // catch-up on a fresh chain (whose blocks are all empty) still gets
        // one block per second, which is all it can use.
        assert_eq!(propose_delay(false, EMPTY_BLOCK_INTERVAL, timeout), Duration::ZERO);
        assert_eq!(propose_delay(false, Duration::from_secs(30), timeout), Duration::ZERO);
    }

    /// The wait must never cost this node its own proposal slot.
    ///
    /// `GetValue` carries the maximum time consensus will wait for a value;
    /// a proposer that overruns it hands the round to the next proposer,
    /// which is a worse outcome than the fast empty block being avoided.
    #[test]
    fn the_wait_stays_well_inside_the_timeout_consensus_allows() {
        for timeout_ms in [50u64, 200, 1000, 3000] {
            let timeout = Duration::from_millis(timeout_ms);
            let waited = propose_delay(false, Duration::ZERO, timeout);
            assert!(
                waited * 2 <= timeout,
                "waiting {waited:?} of a {timeout:?} budget leaves no margin to build and stream the block"
            );
        }
    }

    #[test]
    fn reassemble_rejects_a_tampered_chunk_via_the_fin_hash_check() {
        let block = sample_block(1);
        let value = EdetValue::from_block(block.clone()).unwrap();
        let mut msgs = stream_value(EdetAddress(0), EdetHeight::new(1), Round::new(0), Round::Nil, &block, value.id);

        // Tamper with a Chunk's bytes after streaming — the Fin hash the
        // proposer actually signed no longer matches what gets reassembled.
        for msg in &mut msgs {
            if let StreamContent::Data(EdetProposalPart::Chunk { bytes, .. }) = &mut msg.content {
                bytes.push(0xFF);
            }
        }

        let mut assembling = PartStreams::default();
        let mut pending = BTreeMap::new();
        let mut result = None;
        for msg in msgs {
            let r = reassemble_proposal_part(&mut assembling, &mut pending, test_peer(1), msg, test_ctx());
            if r.is_some() {
                result = r;
            }
        }
        assert!(result.is_none(), "a tampered chunk must fail the Fin hash check, not silently reassemble");
    }

    #[test]
    fn reassemble_returns_none_for_an_incomplete_stream() {
        let block = sample_block(1);
        let value = EdetValue::from_block(block.clone()).unwrap();
        let msgs = stream_value(EdetAddress(0), EdetHeight::new(1), Round::new(0), Round::Nil, &block, value.id);

        let mut assembling = PartStreams::default();
        let mut pending = BTreeMap::new();
        // Drop the transport Fin envelope (the last message) — the stream
        // never "closes".
        let without_fin = msgs.len() - 1;
        for msg in msgs.into_iter().take(without_fin) {
            assert!(reassemble_proposal_part(&mut assembling, &mut pending, test_peer(1), msg, test_ctx()).is_none());
        }
        assert_eq!(assembling.len(), 1, "an unfinished stream stays buffered, not silently discarded");
    }

    // --- the reassembly buffer is bounded and peer-segregated ----------
    //
    // The attack these close: `assembling` was keyed on an attacker-chosen
    // stream id and attacker-chosen chunk seq, never capped, never evicted,
    // and shrunk only by a transport `Fin` the attacker withholds. One
    // Byzantine validator gossiping chunks under fresh stream ids drove every
    // honest node out of memory — gossipsub dedups byte-identical messages
    // only, so one changed byte per part passes straight through. Unit-tested
    // against the buffer directly rather than by standing up a malicious
    // libp2p node: every bound is a property of this function, and a real
    // peer only decides which parts arrive.

    /// One `Chunk` part, as a hostile sender emits them: any stream id, any
    /// transport sequence, any chunk seq, `bytes` of whatever size.
    fn chunk_part(stream: u64, sequence: u64, seq: u32, len: usize) -> StreamMessage<EdetProposalPart> {
        StreamMessage::new(
            StreamId::new(Bytes::from(stream.to_be_bytes().to_vec())),
            sequence,
            StreamContent::Data(EdetProposalPart::Chunk { seq, bytes: vec![0xAB; len] }),
        )
    }

    /// A flood of chunks under ever-fresh stream ids, never finished, must
    /// not grow the buffer without limit. One peer is held to its own
    /// allowance and nothing else.
    #[test]
    fn one_peer_flooding_fresh_stream_ids_is_capped_at_its_own_allowance() {
        let mut assembling = PartStreams::default();
        let mut pending = BTreeMap::new();
        for stream in 0..10_000u64 {
            reassemble_proposal_part(
                &mut assembling,
                &mut pending,
                test_peer(7),
                chunk_part(stream, 0, 0, 1024),
                test_ctx(),
            );
        }
        assert_eq!(
            assembling.len(),
            MAX_STREAMS_PER_PEER,
            "10 000 unfinished streams from one peer must occupy exactly its per-peer allowance"
        );
        assert!(assembling.buffered_bytes() <= MAX_STREAMS_PER_PEER * MAX_STREAM_BYTES);
    }

    /// And across many peers, the global cap is what bounds the process —
    /// the per-peer cap alone would scale with however many peers can reach
    /// this node.
    #[test]
    fn a_flood_from_many_peers_is_capped_globally_in_streams_and_in_bytes() {
        let mut assembling = PartStreams::default();
        let mut pending = BTreeMap::new();
        for peer in 0..250u8 {
            for stream in 0..8u64 {
                reassemble_proposal_part(
                    &mut assembling,
                    &mut pending,
                    test_peer(peer),
                    chunk_part(stream, 0, 0, PROPOSAL_CHUNK_BYTES),
                    test_ctx(),
                );
            }
        }
        assert!(assembling.len() <= MAX_INFLIGHT_STREAMS, "in-flight streams: {}", assembling.len());
        assert!(
            assembling.buffered_bytes() <= MAX_BUFFERED_BYTES,
            "buffered bytes: {} > {MAX_BUFFERED_BYTES}",
            assembling.buffered_bytes()
        );
    }

    /// Within one stream, fresh chunk `seq`s are bounded too — the other
    /// unbounded axis, since a single stream id with 2^32 distinct sequence
    /// numbers was just as effective as fresh stream ids.
    #[test]
    fn fresh_chunk_sequences_within_one_stream_are_bounded() {
        let mut assembling = PartStreams::default();
        let mut pending = BTreeMap::new();
        for seq in 0..5_000u32 {
            reassemble_proposal_part(
                &mut assembling,
                &mut pending,
                test_peer(3),
                chunk_part(0, seq as u64, seq, PROPOSAL_CHUNK_BYTES),
                test_ctx(),
            );
        }
        assert_eq!(assembling.len(), 1);
        assert!(
            assembling.buffered_bytes() <= MAX_STREAM_BYTES,
            "one stream retained {} bytes",
            assembling.buffered_bytes()
        );
    }

    /// The vendored `PartStreamsMap`'s other restored property: a transport
    /// sequence already folded in is dropped. Without it, one slot can be
    /// re-sent forever with a byte changed each time — every re-send a
    /// distinct gossipsub message that reaches this handler.
    #[test]
    fn a_repeated_transport_sequence_is_dropped() {
        let mut assembling = PartStreams::default();
        let mut pending = BTreeMap::new();
        for round in 0..100u32 {
            // Same stream, same transport sequence, different chunk slot each
            // time: only the first may ever be folded in.
            reassemble_proposal_part(
                &mut assembling,
                &mut pending,
                test_peer(4),
                chunk_part(0, 9, round, 4096),
                test_ctx(),
            );
        }
        assert_eq!(assembling.buffered_bytes(), 4096, "only the first part at a given sequence may be retained");
    }

    /// The censorship half. `stream_id_for` is a pure function of
    /// `(height, round)`, so any peer can compute the stream id an honest
    /// proposer will use. Sharing one entry per stream id, an injected chunk
    /// overwrote the proposer's bytes (last writer wins) and every honest
    /// node failed the `Fin` hash check — the round stalls, for as long as
    /// the attacker keeps sending. Segregated by peer, the injection lands in
    /// the attacker's own entry.
    #[test]
    fn a_peer_injecting_into_the_proposers_stream_id_cannot_break_the_honest_reassembly() {
        let block = sample_block(1);
        let value = EdetValue::from_block(block.clone()).unwrap();
        let msgs = stream_value(EdetAddress(0), EdetHeight::new(1), Round::new(0), Round::Nil, &block, value.id);
        let stream_id = msgs[0].stream_id.clone();

        let mut assembling = PartStreams::default();
        let mut pending = BTreeMap::new();
        let mut result = None;
        for msg in msgs {
            // Before each honest part, the attacker writes garbage into every
            // chunk slot of the SAME stream id, from its own peer identity.
            for seq in 0..4u32 {
                reassemble_proposal_part(
                    &mut assembling,
                    &mut pending,
                    test_peer(99),
                    StreamMessage::new(
                        stream_id.clone(),
                        seq as u64,
                        StreamContent::Data(EdetProposalPart::Chunk { seq, bytes: vec![0xFF; 64] }),
                    ),
                    test_ctx(),
                );
            }
            if let Some(r) = reassemble_proposal_part(&mut assembling, &mut pending, test_peer(1), msg, test_ctx()) {
                result = Some(r);
            }
        }

        let proposed = result.expect("the honest stream must still reassemble under a colliding-stream-id injection");
        assert_eq!(proposed.value.id, value.id);
        assert_eq!(proposed.validity, Validity::Valid);
        assert_eq!(pending.len(), 1, "and the honest value is retained for the Decided lookup");
    }

    /// A commit is what makes a reassembly buffer dead. Before this, only a
    /// transport `Fin` ever removed an entry, so a stream for a height that
    /// has since been decided — or one that never named a height at all —
    /// sat there for the process's life.
    #[test]
    fn a_commit_evicts_the_streams_it_made_irrelevant() {
        let mut assembling = PartStreams::default();
        let mut pending = BTreeMap::new();

        // One stream for the height about to be decided, one for the height
        // after it, and one that never sent an Init.
        for (peer, height) in [(1u8, 5u64), (2, 6)] {
            reassemble_proposal_part(
                &mut assembling,
                &mut pending,
                test_peer(peer),
                StreamMessage::new(
                    StreamId::new(Bytes::from(vec![peer])),
                    0,
                    StreamContent::Data(EdetProposalPart::Init {
                        height: EdetHeight::new(height),
                        round: Round::new(0),
                        pol_round: Round::Nil,
                        proposer: EdetAddress(0),
                    }),
                ),
                test_ctx(),
            );
        }
        reassemble_proposal_part(&mut assembling, &mut pending, test_peer(3), chunk_part(77, 0, 0, 512), test_ctx());
        assert_eq!(assembling.len(), 3);

        assembling.evict_decided(5);
        assert_eq!(
            assembling.len(),
            1,
            "only the stream naming a height ABOVE the one just decided may survive a commit"
        );
        assert_eq!(assembling.buffered_bytes(), 0, "and the unattributable chunk buffer is released");
    }

    // --- ProcessSyncedValue ---------------------------------------------

    #[test]
    fn process_synced_value_decodes_a_valid_block_and_rejects_garbage() {
        let block = sample_block(9);
        let bytes = block.encode().unwrap();
        let mut pending = BTreeMap::new();

        let proposed = process_synced_value(
            &mut pending,
            EdetHeight::new(9),
            Round::new(0),
            EdetAddress(1),
            &bytes,
            ScreenCtx {
                chain_id: crate::block::DEV_CHAIN_ID,
                at: 8,
                parent_time: 0,
                now_secs: TEST_NOW,
                expected_app_hash: TEST_APP_HASH,
                deterministic: &permissive,
            },
        )
        .expect("a validly encoded block must decode");
        assert_eq!(proposed.height, EdetHeight::new(9));
        assert_eq!(proposed.value.block.height, 9);
        assert_eq!(pending.len(), 1);

        assert!(
            process_synced_value(
                &mut pending,
                EdetHeight::new(9),
                Round::new(0),
                EdetAddress(1),
                b"not a block",
                ScreenCtx {
                    chain_id: crate::block::DEV_CHAIN_ID,
                    at: 8,
                    parent_time: 0,
                    now_secs: TEST_NOW,
                    expected_app_hash: TEST_APP_HASH,
                    deterministic: &permissive,
                },
            )
            .is_none(),
            "garbage bytes must not decode into a value"
        );
    }

    /// **Sync pipelines, and a value about a height this node has not reached
    /// must not be judged against the parent state it does hold.**
    ///
    /// A node at height 1 asks for 2 and 3 in one breath; the answer to 3
    /// arrives while its state is still at 1. Screened against the current
    /// `app_hash` that block is Invalid — and sync does not re-request a
    /// height it has already answered, so the node stops there for good.
    /// Measured on four loopback validators: one sat at `height.tip=2
    /// height.sync=3` while its peers reached 21, asking for 4, 5, 6, 7 and
    /// using none of them.
    ///
    /// Mutation that bites: screen every synced value with the full `screen`.
    /// This block then reads Invalid and the pipelined catch-up dies.
    #[test]
    fn a_synced_value_for_a_height_past_the_next_one_is_not_judged_on_this_state() {
        let bytes = sample_block(9).encode().unwrap();
        let ctx = |at: u64| ScreenCtx {
            chain_id: crate::block::DEV_CHAIN_ID,
            at,
            parent_time: 0,
            now_secs: TEST_NOW,
            // The parent this node holds is NOT block 9's parent, so this is
            // the wrong hash to judge block 9 by — which is exactly the
            // situation, not a contrivance.
            expected_app_hash: [0x99; 32],
            deterministic: &|_: &Block| false,
        };

        let mut pending = BTreeMap::new();
        let ahead =
            process_synced_value(&mut pending, EdetHeight::new(9), Round::new(0), EdetAddress(1), &bytes, ctx(5))
                .expect("it decodes");
        assert_eq!(
            ahead.validity,
            Validity::Valid,
            "a value four heights ahead is screened on what a certificate cannot make true, not on a parent this \
             node does not have"
        );

        // And at the NEXT height the full screen applies, because there the
        // parent is by construction the one this node holds.
        let mut pending = BTreeMap::new();
        let next =
            process_synced_value(&mut pending, EdetHeight::new(9), Round::new(0), EdetAddress(1), &bytes, ctx(8))
                .expect("it decodes");
        assert_eq!(next.validity, Validity::Invalid, "the app hash is checkable here, and it does not match");
    }

    /// What the relaxed screen does NOT relax: a certificate does not make an
    /// unsigned transaction authentic, however far ahead the block is.
    #[test]
    fn a_synced_value_ahead_still_has_its_transactions_verified() {
        let bytes = forged_block(9).encode().unwrap();
        let mut pending = BTreeMap::new();
        let proposed = process_synced_value(
            &mut pending,
            EdetHeight::new(9),
            Round::new(0),
            EdetAddress(1),
            &bytes,
            ScreenCtx {
                chain_id: crate::block::DEV_CHAIN_ID,
                at: 8,
                parent_time: 0,
                now_secs: TEST_NOW,
                expected_app_hash: TEST_APP_HASH,
                deterministic: &permissive,
            },
        )
        .expect("it decodes");
        assert_eq!(proposed.validity, Validity::Invalid);
        assert!(pending.is_empty());
    }

    // --- the pre-vote authentication screen (`screen`) ----------------------
    //
    // The gap these close: BOTH proposal paths can hand consensus
    // `Validity::Valid` unconditionally, so a proposer could get honest
    // validators to vote for a block carrying forged transactions. The commit
    // path would then refuse it on every node and the height would never
    // advance — a network-wide halt any validator could trigger on its
    // proposer turn. Rejecting before the vote is what keeps that a lost
    // round instead.

    /// A block carrying one transaction whose signers are claimed but whose
    /// signatures are absent — the forgery `SignedTx::verify` catches.
    fn forged_block(height: u64) -> Block {
        Block {
            height,
            time_secs: height * 30,
            app_hash: TEST_APP_HASH,
            txs: vec![crate::block::SignedTx {
                tx: edet_state::tx::Tx::Accept {
                    debtor: Party::Member(0),
                    creditor: Party::Member(1),
                    amount: 40.0,
                    maturity_epochs: 30,
                    arb: None,
                },
                nonce: crate::block::counter_nonce(height),
                not_after_epoch: 30,
                signers: vec![pubkey_of(&dev_seed(0)), pubkey_of(&dev_seed(1))],
                signatures: vec![], // claimed, never signed
            }],
        }
    }

    /// A block whose transaction names NO signer at all: only the
    /// permissionless cranks may do that, and `Accept` is not one.
    fn unsigned_block(height: u64) -> Block {
        Block {
            height,
            time_secs: height * 30,
            app_hash: TEST_APP_HASH,
            txs: vec![crate::block::SignedTx {
                tx: edet_state::tx::Tx::Accept {
                    debtor: Party::Member(0),
                    creditor: Party::Member(1),
                    amount: 40.0,
                    maturity_epochs: 30,
                    arb: None,
                },
                nonce: crate::block::counter_nonce(height),
                not_after_epoch: 30,
                signers: vec![],
                signatures: vec![],
            }],
        }
    }

    /// Drive a whole proposal stream through reassembly, as `run` does.
    /// `(parent_time, now_secs)` fixed at `(0, TEST_NOW)` — comfortably
    /// permissive on the timestamp checks, since these tests are about
    /// signature authentication, not timing.
    fn stream_through(
        block: &Block,
        pending: &mut BTreeMap<EdetValueId, PendingValue>,
    ) -> Option<ProposedValue<EdetContext>> {
        let value = EdetValue::from_block(block.clone()).unwrap();
        let msgs =
            stream_value(EdetAddress(0), EdetHeight::new(block.height), Round::new(0), Round::Nil, block, value.id);
        let mut assembling = PartStreams::default();
        let mut result = None;
        for msg in msgs {
            if let Some(r) = reassemble_proposal_part(&mut assembling, pending, test_peer(1), msg, test_ctx()) {
                result = Some(r);
            }
        }
        result
    }

    #[test]
    fn a_proposal_carrying_a_forged_transaction_is_voted_invalid_not_valid() {
        let block = forged_block(1);
        let mut pending = BTreeMap::new();
        let proposed = stream_through(&block, &mut pending)
            .expect("the stream still reassembles — it is authentic bytes from the proposer, just not valid");
        assert_eq!(
            proposed.validity,
            Validity::Invalid,
            "an honest validator must never vote for a block whose transactions are not signed",
        );
        assert!(
            pending.is_empty(),
            "a rejected block must not be retained: nothing to restream, and `verify_decided` must miss it",
        );
    }

    #[test]
    fn a_proposal_with_no_signers_on_a_non_crank_transaction_is_voted_invalid() {
        let mut pending = BTreeMap::new();
        let proposed = stream_through(&unsigned_block(1), &mut pending).expect("reassembles");
        assert_eq!(
            proposed.validity,
            Validity::Invalid,
            "the empty-signer rule must hold on the consensus path, not just at ingress"
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn a_properly_signed_proposal_is_still_voted_valid_and_retained() {
        let signed = crate::block::sign_tx(
            crate::block::DEV_CHAIN_ID,
            edet_state::tx::Tx::Accept {
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
        .expect("sign");
        let block = Block { height: 1, time_secs: 30, app_hash: TEST_APP_HASH, txs: vec![signed] };
        let mut pending = BTreeMap::new();
        let proposed = stream_through(&block, &mut pending).expect("reassembles");
        assert_eq!(proposed.validity, Validity::Valid);
        assert_eq!(pending.len(), 1, "a valid block is retained for the Decided lookup");
    }

    #[test]
    fn a_synced_block_carrying_a_forged_transaction_is_rejected_too() {
        let bytes = forged_block(9).encode().unwrap();
        let mut pending = BTreeMap::new();
        let proposed = process_synced_value(
            &mut pending,
            EdetHeight::new(9),
            Round::new(0),
            EdetAddress(1),
            &bytes,
            ScreenCtx {
                chain_id: crate::block::DEV_CHAIN_ID,
                at: 8,
                parent_time: 0,
                now_secs: TEST_NOW,
                expected_app_hash: TEST_APP_HASH,
                deterministic: &permissive,
            },
        )
        .expect("it decodes — decoding is not the question");
        assert_eq!(
            proposed.validity,
            Validity::Invalid,
            "arriving with a certificate does not make an unsigned transaction authentic",
        );
        assert!(pending.is_empty());
    }

    // --- the pre-vote screen's timestamp checks -------------------------

    /// The bound is against THIS node's clock, not a fixed constant: the same
    /// block is Invalid when `now_secs` is far behind it and Valid once
    /// `now_secs` has caught up.
    #[test]
    fn screen_rejects_a_block_far_in_the_future_and_accepts_it_once_the_clock_catches_up() {
        let block = Block { height: 1, time_secs: 10_000, app_hash: TEST_APP_HASH, txs: Vec::new() };
        assert_eq!(
            screen(
                &block,
                ScreenCtx {
                    chain_id: crate::block::DEV_CHAIN_ID,
                    at: 0,
                    parent_time: 0,
                    now_secs: 100,
                    expected_app_hash: TEST_APP_HASH,
                    deterministic: &permissive
                }
            ),
            Validity::Invalid,
            "a block far ahead of this node's clock must not be voted Valid"
        );
        assert_eq!(
            screen(
                &block,
                ScreenCtx {
                    chain_id: crate::block::DEV_CHAIN_ID,
                    at: 0,
                    parent_time: 0,
                    now_secs: 10_000,
                    expected_app_hash: TEST_APP_HASH,
                    deterministic: &permissive
                }
            ),
            Validity::Valid,
            "the identical block must become Valid once now_secs reaches it"
        );
    }

    /// A block just within the skew bound is Valid; one second past it is
    /// Invalid — the bound is exact, not approximate.
    #[test]
    fn screen_is_exact_at_the_future_skew_boundary() {
        let now = 1_000;
        let at_bound = Block {
            height: 1,
            time_secs: now + crate::block::MAX_FUTURE_SKEW_SECS,
            app_hash: TEST_APP_HASH,
            txs: Vec::new(),
        };
        assert_eq!(
            screen(
                &at_bound,
                ScreenCtx {
                    chain_id: crate::block::DEV_CHAIN_ID,
                    at: 0,
                    parent_time: 0,
                    now_secs: now,
                    expected_app_hash: TEST_APP_HASH,
                    deterministic: &permissive
                }
            ),
            Validity::Valid
        );
        let past_bound = Block {
            height: 1,
            time_secs: now + crate::block::MAX_FUTURE_SKEW_SECS + 1,
            app_hash: TEST_APP_HASH,
            txs: Vec::new(),
        };
        assert_eq!(
            screen(
                &past_bound,
                ScreenCtx {
                    chain_id: crate::block::DEV_CHAIN_ID,
                    at: 0,
                    parent_time: 0,
                    now_secs: now,
                    expected_app_hash: TEST_APP_HASH,
                    deterministic: &permissive
                }
            ),
            Validity::Invalid
        );
    }

    /// A block that would rewind the clock relative to `parent_time` is
    /// Invalid, however plausible it is against this node's own clock.
    #[test]
    fn screen_rejects_a_block_that_rewinds_before_parent_time() {
        let block = Block { height: 1, time_secs: 50, app_hash: TEST_APP_HASH, txs: Vec::new() };
        assert_eq!(
            screen(
                &block,
                ScreenCtx {
                    chain_id: crate::block::DEV_CHAIN_ID,
                    at: 0,
                    parent_time: 100,
                    now_secs: TEST_NOW,
                    expected_app_hash: TEST_APP_HASH,
                    deterministic: &permissive
                }
            ),
            Validity::Invalid
        );
        assert_eq!(
            screen(
                &block,
                ScreenCtx {
                    chain_id: crate::block::DEV_CHAIN_ID,
                    at: 0,
                    parent_time: 50,
                    now_secs: TEST_NOW,
                    expected_app_hash: TEST_APP_HASH,
                    deterministic: &permissive
                }
            ),
            Validity::Valid,
            "an equal timestamp is not a rewind"
        );
    }

    /// The core property at the screen layer — a block whose `app_hash`
    /// does not match what this validator expects is voted Invalid, which is
    /// what stops an honest validator from ever certifying a block a diverged
    /// (or lying) proposer built on top of a different state than this node's
    /// own.
    #[test]
    fn screen_rejects_a_mismatched_app_hash() {
        let block = Block { height: 1, time_secs: 30, app_hash: [0x11; 32], txs: Vec::new() };
        assert_eq!(
            screen(
                &block,
                ScreenCtx {
                    chain_id: crate::block::DEV_CHAIN_ID,
                    at: 0,
                    parent_time: 0,
                    now_secs: TEST_NOW,
                    expected_app_hash: [0x22; 32],
                    deterministic: &permissive
                }
            ),
            Validity::Invalid,
            "a block whose app_hash doesn't match what this validator expects must not be voted Valid"
        );
        assert_eq!(
            screen(
                &block,
                ScreenCtx {
                    chain_id: crate::block::DEV_CHAIN_ID,
                    at: 0,
                    parent_time: 0,
                    now_secs: TEST_NOW,
                    expected_app_hash: [0x11; 32],
                    deterministic: &permissive
                }
            ),
            Validity::Valid,
            "the identical block is Valid once its app_hash matches"
        );
    }

    /// The backstop is still a backstop: even if a Byzantine quorum decided a
    /// forged block AND this node somehow held it, the commit path refuses.
    #[test]
    fn the_commit_path_still_refuses_a_forged_block_independently_of_the_screen() {
        let mut replica = Replica::new(seeded_state(2));
        assert!(
            replica.commit_block(&forged_block(1)).is_err(),
            "commit_block must reject a forged block regardless of what any pre-vote screen did",
        );
        assert!(
            replica.commit_block(&unsigned_block(1)).is_err(),
            "commit_block must reject an unsigned non-crank transaction too",
        );
        assert_eq!(replica.height, 0, "nothing may be applied from a rejected block");
    }

    // --- certificate WAL codec round trip -------------------------------

    #[test]
    fn commit_certificate_encode_decode_round_trips() {
        let value_id = EdetValueId([3u8; 32]);
        let cert = certificate_from(
            EdetHeight::new(4),
            Round::new(1),
            value_id,
            &[(EdetAddress(0), EdetSignature([9u8; 64])), (EdetAddress(1), EdetSignature([8u8; 64]))],
        );
        let bytes = encode_commit_certificate(&cert);
        let back = decode_commit_certificate(&bytes).expect("decode");
        assert_eq!(back.height, cert.height);
        assert_eq!(back.round, cert.round);
        assert_eq!(back.value_id, cert.value_id);
        assert_eq!(back.commit_signatures.len(), 2);
        assert_eq!(back.commit_signatures[0].address, EdetAddress(0));
        assert_eq!(back.commit_signatures[0].signature, EdetSignature([9u8; 64]));
    }

    // --- verify_decided is the certificate-authentication gate ------

    #[test]
    fn verify_decided_commits_a_properly_signed_quorum_certificate() {
        let state = seeded_state(4); // total power 4, quorum = 2*4/3+1 = 3
        let replica = Replica::new(state);
        let block = sample_block(1);
        let value = EdetValue::from_block(block.clone()).unwrap();

        let mut pending = BTreeMap::new();
        pending.insert(
            value.id,
            PendingValue {
                round: Round::new(0),
                valid_round: Round::Nil,
                proposer: EdetAddress(0),
                block: block.clone(),
            },
        );

        let signers: Vec<(EdetAddress, EdetSignature)> = (0..3u8)
            .map(|i| {
                (
                    EdetAddress(i as u64),
                    sign_precommit(
                        crate::block::dev_consensus_seed(i),
                        EdetHeight::new(1),
                        Round::new(0),
                        value.id,
                        EdetAddress(i as u64),
                    ),
                )
            })
            .collect();
        let cert = certificate_from(EdetHeight::new(1), Round::new(0), value.id, &signers);

        let signing =
            EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(0)), crate::block::DEV_CHAIN_ID.to_string());
        let committed = verify_decided(&replica, &pending, &signing, &cert);
        assert!(committed.is_some(), "3-of-4 real validator signatures must meet quorum and commit");
        assert_eq!(committed.unwrap().height, 1);
    }

    #[test]
    fn verify_decided_rejects_a_below_quorum_certificate() {
        let state = seeded_state(4);
        let replica = Replica::new(state);
        let block = sample_block(1);
        let value = EdetValue::from_block(block.clone()).unwrap();
        let mut pending = BTreeMap::new();
        pending.insert(
            value.id,
            PendingValue { round: Round::new(0), valid_round: Round::Nil, proposer: EdetAddress(0), block },
        );

        // Only 2 of 4 signed — below the quorum of 3.
        let signers: Vec<(EdetAddress, EdetSignature)> = (0..2u8)
            .map(|i| {
                (
                    EdetAddress(i as u64),
                    sign_precommit(
                        crate::block::dev_consensus_seed(i),
                        EdetHeight::new(1),
                        Round::new(0),
                        value.id,
                        EdetAddress(i as u64),
                    ),
                )
            })
            .collect();
        let cert = certificate_from(EdetHeight::new(1), Round::new(0), value.id, &signers);

        let signing =
            EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(0)), crate::block::DEV_CHAIN_ID.to_string());
        assert!(
            verify_decided(&replica, &pending, &signing, &cert).is_none(),
            "2-of-4 power must not meet a 3-of-4 quorum"
        );
    }

    /// A caller past the perimeter but holding NO validator's
    /// private key cannot produce a `Decided`-accepted block — even if they
    /// know every validator's address and claim to sign for 3 of 4 of them,
    /// each signature must verify against that validator's REGISTERED public
    /// key, which the attacker's own (unrelated) key cannot produce.
    #[test]
    fn verify_decided_rejects_signatures_from_keys_outside_the_validator_set() {
        let state = seeded_state(4);
        let replica = Replica::new(state);
        let block = sample_block(1);
        let value = EdetValue::from_block(block.clone()).unwrap();
        let mut pending = BTreeMap::new();
        pending.insert(
            value.id,
            PendingValue { round: Round::new(0), valid_round: Round::Nil, proposer: EdetAddress(0), block },
        );

        // The attacker's own key material — held by nobody in the
        // validator set — "signing" as if it were validators 0, 1, and 2.
        let attacker_seed = [200u8; 32];
        let signers: Vec<(EdetAddress, EdetSignature)> = (0..3u8)
            .map(|i| {
                (
                    EdetAddress(i as u64),
                    sign_precommit(attacker_seed, EdetHeight::new(1), Round::new(0), value.id, EdetAddress(i as u64)),
                )
            })
            .collect();
        let cert = certificate_from(EdetHeight::new(1), Round::new(0), value.id, &signers);

        let signing =
            EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(0)), crate::block::DEV_CHAIN_ID.to_string());
        assert!(
            verify_decided(&replica, &pending, &signing, &cert).is_none(),
            "signatures from a key outside the validator set must never verify, however many are presented"
        );
    }

    #[test]
    fn verify_decided_rejects_an_unknown_validator_address() {
        let state = seeded_state(4);
        let replica = Replica::new(state);
        let block = sample_block(1);
        let value = EdetValue::from_block(block.clone()).unwrap();
        let mut pending = BTreeMap::new();
        pending.insert(
            value.id,
            PendingValue { round: Round::new(0), valid_round: Round::Nil, proposer: EdetAddress(0), block },
        );

        // Address 9 is not in the (0..4) validator set at all.
        let sig = sign_precommit(
            crate::block::dev_consensus_seed(9),
            EdetHeight::new(1),
            Round::new(0),
            value.id,
            EdetAddress(9),
        );
        let cert = certificate_from(EdetHeight::new(1), Round::new(0), value.id, &[(EdetAddress(9), sig)]);

        let signing =
            EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(0)), crate::block::DEV_CHAIN_ID.to_string());
        assert!(verify_decided(&replica, &pending, &signing, &cert).is_none());
    }

    #[test]
    fn verify_decided_returns_none_for_a_value_never_observed_locally() {
        let state = seeded_state(4);
        let replica = Replica::new(state);
        let value_id = EdetValueId([42u8; 32]);
        let pending = BTreeMap::new(); // never populated — this node never saw the value

        let signers: Vec<(EdetAddress, EdetSignature)> = (0..3u8)
            .map(|i| {
                (
                    EdetAddress(i as u64),
                    sign_precommit(
                        crate::block::dev_consensus_seed(i),
                        EdetHeight::new(1),
                        Round::new(0),
                        value_id,
                        EdetAddress(i as u64),
                    ),
                )
            })
            .collect();
        let cert = certificate_from(EdetHeight::new(1), Round::new(0), value_id, &signers);

        let signing =
            EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(0)), crate::block::DEV_CHAIN_ID.to_string());
        assert!(
            verify_decided(&replica, &pending, &signing, &cert).is_none(),
            "a fully valid certificate for a value never seen locally must still yield None (nothing to commit)"
        );
    }

    /// A validator suspended at height H cannot help decide H+1.
    ///
    /// The half that the ledger already covers is its
    /// (`replication.rs::suspended_validator_absent_from_next_height`: the
    /// per-height history stops listing them). This is the half that
    /// matters to an attacker — that the certificate path actually consults
    /// that history, so a member the community has just removed cannot keep
    /// signing blocks with the key they still hold.
    ///
    /// Isolated by holding everything else fixed: ONE certificate, one set of
    /// real signatures, one value, checked against two replicas that differ
    /// only in what they committed at height 1 — a no-op block, or the
    /// governance transactions that suspend validator 3. It is accepted by
    /// the first and refused by the second, so the refusal cannot be blamed
    /// on a malformed certificate, a bad signature, or an address that was
    /// never a validator (`verify_decided_rejects_an_unknown_validator_address`
    /// covers that separately — here the address was a full validator one
    /// block ago, and its signature is genuine).
    ///
    /// Removed by `Suspend`, through a real proposal and real assents, since
    /// that is the transition; `Exit` and `ValidatorPower { power: 0 }`
    /// reach `state.validators.remove` the same way.
    #[test]
    fn verify_decided_rejects_a_certificate_signed_by_a_since_suspended_validator() {
        use edet_state::types::ProposalKind;

        /// Unsigned envelope — `commit_block_unchecked` is the trusted local
        /// path (no `verify_txs`), while `apply`'s own `require_signed` still
        /// checks the named key belongs to the acting member.
        fn tx(n: u64, tx: edet_state::Tx, signer: u8) -> SignedTx {
            SignedTx {
                tx,
                nonce: crate::block::counter_nonce(n),
                not_after_epoch: 30,
                signers: vec![pubkey_of(&dev_seed(signer))],
                signatures: vec![],
            }
        }

        // 4 validators at power 1 each: quorum 3 while all four are in, and
        // still 3 once one leaves (⌊2·3/3⌋+1) — so the certificate below is
        // exactly at quorum before the suspension and one short after it.
        //
        // `sigma_est = 0` because `propose` refuses an author whose earned
        // standing has not cleared the cold-start floor, and a fresh genesis
        // founder has none. That gate decides WHO MAY PROPOSE, which is
        // `apply`'s business and separately tested; wiring up settled history
        // for four founders would add a page of fixture between this test and
        // the one line it is about.
        let mut suspended = Replica::new(seeded_state(4));
        let mut untouched = Replica::new(seeded_state(4));

        let govern = Block {
            height: 1,
            time_secs: 30,
            app_hash: suspended.app_hash(),
            txs: vec![
                tx(0, edet_state::Tx::Propose { author: 0, kind: ProposalKind::Suspend { member: 3 } }, 0),
                // THREE of four founders, because member 3 is a validator and
                // suspending one is a change to who orders the ledger — the
                // bar of two thirds, against a half for
                // everything else. A fourth assent would be refused as an
                // unknown proposal, because by then it has already enacted.
                tx(1, edet_state::Tx::Assent { member: 0, proposal: 0 }, 0),
                tx(2, edet_state::Tx::Assent { member: 1, proposal: 0 }, 1),
                tx(3, edet_state::Tx::Assent { member: 2, proposal: 0 }, 2),
            ],
        };
        let outcomes = suspended.commit_block_unchecked(&govern).expect("the suspension block commits");
        assert!(outcomes.iter().all(|o| o.is_ok()), "every governance transaction must apply: {outcomes:?}");
        untouched
            .commit_block_unchecked(&Block {
                height: 1,
                time_secs: 30,
                app_hash: untouched.app_hash(),
                txs: Vec::new(),
            })
            .expect("the empty block commits");

        assert!(
            !suspended.state.validators.contains_key(&3),
            "fixture is only meaningful if the suspension actually enacted"
        );
        assert!(
            !suspended.validators_at(1).expect("history reaches H").contains_key(&3),
            "and if the per-height history at H already reflects it"
        );
        assert!(untouched.state.validators.contains_key(&3), "the control replica must still hold all four");

        // One certificate for height 2, signed for real by validators 0, 1
        // and 3 — 3 of the 4 that existed a block ago.
        let block = sample_block(2);
        let value = EdetValue::from_block(block.clone()).unwrap();
        let mut pending = BTreeMap::new();
        pending.insert(
            value.id,
            PendingValue { round: Round::new(0), valid_round: Round::Nil, proposer: EdetAddress(0), block },
        );
        let signers: Vec<(EdetAddress, EdetSignature)> = [0u8, 1, 3]
            .iter()
            .map(|&i| {
                (
                    EdetAddress(i as u64),
                    sign_precommit(
                        crate::block::dev_consensus_seed(i),
                        EdetHeight::new(2),
                        Round::new(0),
                        value.id,
                        EdetAddress(i as u64),
                    ),
                )
            })
            .collect();
        let cert = certificate_from(EdetHeight::new(2), Round::new(0), value.id, &signers);
        let signing =
            EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(0)), crate::block::DEV_CHAIN_ID.to_string());

        assert!(
            verify_decided(&untouched, &pending, &signing, &cert).is_some(),
            "control: while 3 is still a validator, these same three signatures are a quorum"
        );
        assert!(
            verify_decided(&suspended, &pending, &signing, &cert).is_none(),
            "a validator suspended at H must not count toward the quorum deciding H+1"
        );
    }

    // --- validator_set_at wires Replica::validators_at into a real
    // EdetValidatorSet --------------------------------------------------------

    /// `validator_set_at` is a thin join of `Replica::validators_at`'s
    /// per-height power map (already landed and tested in `replica.rs`)
    /// against `EdetValidatorSet::build`. This asserts that wiring is
    /// correct at the genesis baseline height every replica starts with —
    /// The multi-height history mechanism is exercised elsewhere.
    #[test]
    fn validator_set_at_matches_from_state_at_the_genesis_baseline() {
        let state = seeded_state(3);
        let replica = Replica::new(state.clone());

        let via_helper = validator_set_at(&replica, 0).expect("genesis baseline entry always present");
        let direct = EdetValidatorSet::from_state(&state).expect("seeded genesis state always builds");
        assert_eq!(via_helper.count(), direct.count());
        assert_eq!(via_helper.total_voting_power(), direct.total_voting_power());
        for i in 0..3u64 {
            assert_eq!(
                via_helper.get_by_address(&EdetAddress(i)).map(|v| v.public_key),
                direct.get_by_address(&EdetAddress(i)).map(|v| v.public_key),
            );
        }

        // A height below the only retained entry: `validators_at`/`validator_set_at`
        // still answer with the baseline (never `None` for `replica.height` or below,
        // per `Replica::validators_at`'s own doc comment) since the baseline
        // covers height 0 unconditionally.
        assert!(validator_set_at(&replica, 0).is_some());
    }

    #[test]
    fn restart_falls_back_to_a_validator_set_even_when_asked_about_a_height_with_no_history_entry() {
        let state = seeded_state(2);
        let replica = Replica::new(state);
        // `restart` always uses `replica.height` internally (never the
        // requested-but-failed height) precisely so it never needs to
        // fabricate a set for a height it has no history for.
        let next = restart(&replica, EdetHeight::new(1)).expect("2 seeded validators always build a set");
        match next {
            Next::Restart(height, vset) => {
                assert_eq!(height, EdetHeight::new(1), "must retry the SAME height, not advance");
                assert_eq!(vset.count(), 2);
            }
            Next::Start(..) => panic!("restart must never produce Next::Start"),
        }
    }
}
