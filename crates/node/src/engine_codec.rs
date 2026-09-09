//! The wire codec for `EdetContext` (`--features malachite`).
//!
//! `start_engine` (vendored `crates/app-channel/src/run.rs`) needs a
//! `WalCodec<Ctx> + Clone` and a type satisfying both `ConsensusCodec<Ctx>`
//! and `SyncCodec<Ctx>` — three traits that are each auto-implemented
//! (`crates/engine/src/{consensus,sync}.rs`, `crates/engine/src/wal/entry.rs`)
//! for any type implementing a fixed set of `malachitebft_codec::Codec<T>`
//! instances:
//!
//! - `ConsensusCodec<Ctx>`: `Codec<Ctx::ProposalPart>`,
//!   `Codec<SignedConsensusMsg<Ctx>>`, `Codec<LivenessMsg<Ctx>>`,
//!   `Codec<StreamMessage<Ctx::ProposalPart>>`.
//! - `SyncCodec<Ctx>`: `Codec<sync::Status<Ctx>>`, `Codec<sync::Request<Ctx>>`,
//!   `Codec<sync::Response<Ctx>>`.
//! - `WalCodec<Ctx>`: `Codec<SignedConsensusMsg<Ctx>>`,
//!   `Codec<ProposedValue<Ctx>>`.
//!
//! Modeled on the vendored `malachitebft_test::codec::json` (a serde_json
//! codec for `TestContext`, `~/.cargo/git/checkouts/malachite-*/code/crates/
//! test/src/codec/json/{mod,raw}.rs`) — `EdetCodec` below is the same
//! approach for `EdetContext`: every wire type gets a `Raw*` mirror struct
//! that swaps any field without a `serde` impl for one that has it, and a
//! pair of `From`/`TryFrom` conversions to the real type.
//!
//! Unlike `TestContext`'s `Vote`/`Proposal` (which round-trip through
//! `Protobuf::{to_sign_bytes,from_sign_bytes}` because that's all the toy
//! type exposes), `EdetVote`/`EdetProposal`/`EdetProposalPart`/`EdetValue`
//! already derive `Serialize`/`Deserialize` directly (`engine_context.rs`),
//! so a `Raw*` mirror can just embed them as a field with no extra encoding
//! step. The one thing every signed/certified payload is missing is a
//! `Serialize` impl for its signature: `EdetSignature` (`[u8; 64]`)
//! deliberately does not derive it (`engine_context.rs`'s own doc comment on
//! that type). Every `Raw*` type below carries a signature as a plain
//! `Vec<u8>` instead, via `EdetSigningScheme::{encode_signature,
//! decode_signature}` — the same encode/decode pair `SigningScheme` already
//! defines and `engine_context.rs` already unit-tests.
//! `Validity` (`core_types::proposal::Validity`) has the same problem for a
//! different reason (no `serde` derive at all, `cfg_attr`-gated only for
//! `borsh`) — `RawValidity` is a two-variant mirror enum for exactly that.
//!
//! Two supporting deps that are NOT already covered by re-exports through
//! `malachitebft-app-channel`/`malachitebft-app`
//! (`crates/app/src/types.rs`'s curated `codec`/`streaming`/`sync` modules):
//! `sync::ValueRequest`/`sync::ValueResponse` (the payload structs behind
//! `sync::Request`/`sync::Response`, not re-exported there) and `PeerId`'s
//! `serde` impl (gated behind that crate's own `serde` feature, which
//! nothing else in the dependency graph turns on outside dev-dependencies).
//! Both are pulled in as direct optional deps on the same pinned git commit
//! (see `Cargo.toml`), so Cargo unifies them with the copies `app-channel`
//! already pulls in transitively rather than mixing two incompatible copies.
//!
//! Tested here (in-process, no network — same posture as `engine_context.rs`
//! and `engine_malachite.rs`): encode→decode round-trips for a vote, a
//! proposal, a multi-chunk proposal-part stream (`Init`/`Chunk`/`Chunk`/
//! `Fin`), a commit certificate (nested inside a `sync::Response`), a sync
//! `Status`/`Request`, a `LivenessMsg` (all three variants), a
//! `ProposedValue` (the WAL's other codec requirement), and a `WalEntry`
//! built from this codec's `SignedConsensusMsg`/`ProposedValue` encodings.
//! NOT tested here: this codec traveling over a real libp2p transport
//! (that is `start_engine`'s own job, out of scope for a codec unit).

use std::fmt;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use malachitebft_app_channel::app::consensus::LivenessMsg;
use malachitebft_app_channel::app::streaming::{StreamContent, StreamId, StreamMessage};
use malachitebft_app_channel::app::types::codec::{Codec, ConsensusCodec, HasEncodedLen, SyncCodec, WalCodec};
use malachitebft_app_channel::app::types::sync::{RawDecidedValue, Request, Response, Status};
use malachitebft_app_channel::app::types::{ProposedValue, SignedConsensusMsg};
use malachitebft_core_types::{
    CommitCertificate, CommitSignature, NilOrVal, PolkaCertificate, PolkaSignature, Round, RoundCertificate,
    RoundCertificateType, RoundSignature, SignedMessage, SigningScheme, Validity, VoteType,
};
use malachitebft_peer::PeerId;
use malachitebft_sync::{ValueRequest, ValueResponse};

use crate::engine_context::{
    EdetAddress, EdetContext, EdetHeight, EdetProposal, EdetProposalPart, EdetSignature, EdetSigningScheme, EdetValue,
    EdetValueId, EdetVote,
};

// ---------------------------------------------------------------------------
// Codec + error type
// ---------------------------------------------------------------------------

/// The wire codec for `EdetContext`: JSON over the network and the WAL,
/// exactly like `malachitebft_test::codec::json::JsonCodec` for
/// `TestContext`. Zero-sized and `Copy` — `start_engine` needs `WalCodec:
/// Clone` and hands the `NetCodec` around by value too.
#[derive(Copy, Clone, Debug, Default)]
pub struct EdetCodec;

/// Either the JSON layer failed, or a signature's byte length didn't match
/// `EdetSigningScheme`'s fixed 64-byte encoding. Both are "this bytes blob is
/// not a valid encoding of `T`", which is exactly what `Codec::Error` means
/// here — this codec never fails for any OTHER reason (no I/O, no partial
/// reads: `Codec` hands us the whole message as one `Bytes`).
#[derive(Debug)]
pub enum EdetCodecError {
    Json(serde_json::Error),
    InvalidSignature(String),
}

impl fmt::Display for EdetCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EdetCodecError::Json(e) => write!(f, "JSON codec error: {e}"),
            EdetCodecError::InvalidSignature(msg) => write!(f, "invalid signature encoding: {msg}"),
        }
    }
}

impl std::error::Error for EdetCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EdetCodecError::Json(e) => Some(e),
            EdetCodecError::InvalidSignature(_) => None,
        }
    }
}

impl From<serde_json::Error> for EdetCodecError {
    fn from(e: serde_json::Error) -> Self {
        EdetCodecError::Json(e)
    }
}

fn encode_signature(sig: &EdetSignature) -> Vec<u8> {
    EdetSigningScheme::encode_signature(sig)
}

fn decode_signature(bytes: &[u8]) -> Result<EdetSignature, EdetCodecError> {
    EdetSigningScheme::decode_signature(bytes).map_err(|e| EdetCodecError::InvalidSignature(e.to_string()))
}

// ---------------------------------------------------------------------------
// Raw: signed messages (vote / proposal), generic over the signed payload
// ---------------------------------------------------------------------------

/// Mirror of `SignedMessage<EdetContext, M>`: the message as-is (already
/// `Serialize`/`Deserialize` for every `M` used below) plus the signature as
/// bytes instead of the non-`serde` `EdetSignature`.
#[derive(Serialize, Deserialize)]
struct RawSignedMessage<M> {
    message: M,
    signature: Vec<u8>,
}

fn to_raw_signed<M: Clone>(signed: &SignedMessage<EdetContext, M>) -> RawSignedMessage<M> {
    RawSignedMessage { message: signed.message.clone(), signature: encode_signature(&signed.signature) }
}

fn from_raw_signed<M>(raw: RawSignedMessage<M>) -> Result<SignedMessage<EdetContext, M>, EdetCodecError> {
    let signature = decode_signature(&raw.signature)?;
    Ok(SignedMessage::new(raw.message, signature))
}

// ---------------------------------------------------------------------------
// Raw: SignedConsensusMsg
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
enum RawSignedConsensusMsg {
    Vote(RawSignedMessage<EdetVote>),
    Proposal(RawSignedMessage<EdetProposal>),
}

impl From<&SignedConsensusMsg<EdetContext>> for RawSignedConsensusMsg {
    fn from(msg: &SignedConsensusMsg<EdetContext>) -> Self {
        match msg {
            SignedConsensusMsg::Vote(v) => RawSignedConsensusMsg::Vote(to_raw_signed(v)),
            SignedConsensusMsg::Proposal(p) => RawSignedConsensusMsg::Proposal(to_raw_signed(p)),
        }
    }
}

impl TryFrom<RawSignedConsensusMsg> for SignedConsensusMsg<EdetContext> {
    type Error = EdetCodecError;

    fn try_from(raw: RawSignedConsensusMsg) -> Result<Self, Self::Error> {
        Ok(match raw {
            RawSignedConsensusMsg::Vote(v) => SignedConsensusMsg::Vote(from_raw_signed(v)?),
            RawSignedConsensusMsg::Proposal(p) => SignedConsensusMsg::Proposal(from_raw_signed(p)?),
        })
    }
}

impl Codec<SignedConsensusMsg<EdetContext>> for EdetCodec {
    type Error = EdetCodecError;

    fn decode(&self, bytes: Bytes) -> Result<SignedConsensusMsg<EdetContext>, Self::Error> {
        let raw: RawSignedConsensusMsg = serde_json::from_slice(&bytes)?;
        raw.try_into()
    }

    fn encode(&self, msg: &SignedConsensusMsg<EdetContext>) -> Result<Bytes, Self::Error> {
        Ok(Bytes::from(serde_json::to_vec(&RawSignedConsensusMsg::from(msg))?))
    }
}

// ---------------------------------------------------------------------------
// ProposalPart: already fully serde — no Raw mirror needed
// ---------------------------------------------------------------------------

impl Codec<EdetProposalPart> for EdetCodec {
    type Error = EdetCodecError;

    fn decode(&self, bytes: Bytes) -> Result<EdetProposalPart, Self::Error> {
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn encode(&self, msg: &EdetProposalPart) -> Result<Bytes, Self::Error> {
        Ok(Bytes::from(serde_json::to_vec(msg)?))
    }
}

// ---------------------------------------------------------------------------
// Raw: StreamMessage<EdetProposalPart>
// ---------------------------------------------------------------------------

/// `StreamId` wraps a private-to-the-engine-crate `Bytes` field (only
/// `StreamId::new`/`to_bytes` are public) and derives no `serde` impl, so a
/// `#[serde(remote)]` shim (the same pattern the vendored `raw.rs` uses) is
/// the only way to serialize one without reaching into the engine crate.
#[derive(Serialize, Deserialize)]
#[serde(remote = "StreamId")]
struct RawStreamId(#[serde(getter = "StreamId::to_bytes")] Bytes);

impl From<RawStreamId> for StreamId {
    fn from(raw: RawStreamId) -> Self {
        StreamId::new(raw.0)
    }
}

#[derive(Serialize, Deserialize)]
enum RawStreamContent {
    Data(EdetProposalPart),
    Fin,
}

#[derive(Serialize, Deserialize)]
struct RawStreamMessage {
    #[serde(with = "RawStreamId")]
    stream_id: StreamId,
    sequence: u64,
    content: RawStreamContent,
}

impl From<&StreamMessage<EdetProposalPart>> for RawStreamMessage {
    fn from(msg: &StreamMessage<EdetProposalPart>) -> Self {
        Self {
            stream_id: msg.stream_id.clone(),
            sequence: msg.sequence,
            content: match &msg.content {
                StreamContent::Data(part) => RawStreamContent::Data(part.clone()),
                StreamContent::Fin => RawStreamContent::Fin,
            },
        }
    }
}

impl From<RawStreamMessage> for StreamMessage<EdetProposalPart> {
    fn from(raw: RawStreamMessage) -> Self {
        StreamMessage {
            stream_id: raw.stream_id,
            sequence: raw.sequence,
            content: match raw.content {
                RawStreamContent::Data(part) => StreamContent::Data(part),
                RawStreamContent::Fin => StreamContent::Fin,
            },
        }
    }
}

impl Codec<StreamMessage<EdetProposalPart>> for EdetCodec {
    type Error = EdetCodecError;

    fn decode(&self, bytes: Bytes) -> Result<StreamMessage<EdetProposalPart>, Self::Error> {
        let raw: RawStreamMessage = serde_json::from_slice(&bytes)?;
        Ok(raw.into())
    }

    fn encode(&self, msg: &StreamMessage<EdetProposalPart>) -> Result<Bytes, Self::Error> {
        Ok(Bytes::from(serde_json::to_vec(&RawStreamMessage::from(msg))?))
    }
}

// ---------------------------------------------------------------------------
// Raw: sync::Status
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct RawStatus {
    peer_id: PeerId,
    tip_height: EdetHeight,
    history_min_height: EdetHeight,
}

impl From<&Status<EdetContext>> for RawStatus {
    fn from(s: &Status<EdetContext>) -> Self {
        Self { peer_id: s.peer_id, tip_height: s.tip_height, history_min_height: s.history_min_height }
    }
}

impl From<RawStatus> for Status<EdetContext> {
    fn from(raw: RawStatus) -> Self {
        Status { peer_id: raw.peer_id, tip_height: raw.tip_height, history_min_height: raw.history_min_height }
    }
}

impl Codec<Status<EdetContext>> for EdetCodec {
    type Error = EdetCodecError;

    fn decode(&self, bytes: Bytes) -> Result<Status<EdetContext>, Self::Error> {
        let raw: RawStatus = serde_json::from_slice(&bytes)?;
        Ok(raw.into())
    }

    fn encode(&self, msg: &Status<EdetContext>) -> Result<Bytes, Self::Error> {
        Ok(Bytes::from(serde_json::to_vec(&RawStatus::from(msg))?))
    }
}

// ---------------------------------------------------------------------------
// Raw: sync::Request (ValueRequest)
// ---------------------------------------------------------------------------

/// A request for the decided values of an inclusive range of heights, as
/// upstream's sync asks for them in batches.
#[derive(Serialize, Deserialize)]
struct RawValueRequest {
    start: EdetHeight,
    end: EdetHeight,
}

#[derive(Serialize, Deserialize)]
enum RawRequest {
    ValueRequest(RawValueRequest),
}

impl From<&Request<EdetContext>> for RawRequest {
    fn from(r: &Request<EdetContext>) -> Self {
        match r {
            Request::ValueRequest(vr) => {
                RawRequest::ValueRequest(RawValueRequest { start: *vr.range.start(), end: *vr.range.end() })
            }
        }
    }
}

impl From<RawRequest> for Request<EdetContext> {
    fn from(raw: RawRequest) -> Self {
        match raw {
            RawRequest::ValueRequest(v) => Request::ValueRequest(ValueRequest { range: v.start..=v.end }),
        }
    }
}

impl Codec<Request<EdetContext>> for EdetCodec {
    type Error = EdetCodecError;

    fn decode(&self, bytes: Bytes) -> Result<Request<EdetContext>, Self::Error> {
        let raw: RawRequest = serde_json::from_slice(&bytes)?;
        Ok(raw.into())
    }

    fn encode(&self, msg: &Request<EdetContext>) -> Result<Bytes, Self::Error> {
        Ok(Bytes::from(serde_json::to_vec(&RawRequest::from(msg))?))
    }
}

// ---------------------------------------------------------------------------
// Raw: CommitCertificate (nested inside sync::Response's RawDecidedValue)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct RawCommitSignature {
    address: EdetAddress,
    signature: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct RawCommitCertificate {
    height: EdetHeight,
    round: Round,
    value_id: EdetValueId,
    commit_signatures: Vec<RawCommitSignature>,
}

fn commit_certificate_to_raw(c: &CommitCertificate<EdetContext>) -> RawCommitCertificate {
    RawCommitCertificate {
        height: c.height,
        round: c.round,
        value_id: c.value_id,
        commit_signatures: c
            .commit_signatures
            .iter()
            .map(|s| RawCommitSignature { address: s.address, signature: encode_signature(&s.signature) })
            .collect(),
    }
}

fn commit_certificate_from_raw(r: RawCommitCertificate) -> Result<CommitCertificate<EdetContext>, EdetCodecError> {
    let commit_signatures = r
        .commit_signatures
        .into_iter()
        .map(|s| Ok(CommitSignature { address: s.address, signature: decode_signature(&s.signature)? }))
        .collect::<Result<Vec<_>, EdetCodecError>>()?;

    Ok(CommitCertificate { height: r.height, round: r.round, value_id: r.value_id, commit_signatures })
}

// ---------------------------------------------------------------------------
// Raw: sync::Response (ValueResponse / RawDecidedValue)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct RawDecidedValueRepr {
    value_bytes: Bytes,
    certificate: RawCommitCertificate,
}

/// The decided values from `start_height` upward, in height order — as many
/// as the answering node holds of the range asked for, and none when it
/// holds nothing of it.
#[derive(Serialize, Deserialize)]
struct RawValueResponse {
    start_height: EdetHeight,
    values: Vec<RawDecidedValueRepr>,
}

#[derive(Serialize, Deserialize)]
enum RawResponse {
    ValueResponse(RawValueResponse),
}

impl From<&Response<EdetContext>> for RawResponse {
    fn from(r: &Response<EdetContext>) -> Self {
        match r {
            Response::ValueResponse(vr) => RawResponse::ValueResponse(RawValueResponse {
                start_height: vr.start_height,
                values: vr
                    .values
                    .iter()
                    .map(|v| RawDecidedValueRepr {
                        value_bytes: v.value_bytes.clone(),
                        certificate: commit_certificate_to_raw(&v.certificate),
                    })
                    .collect(),
            }),
        }
    }
}

impl TryFrom<RawResponse> for Response<EdetContext> {
    type Error = EdetCodecError;

    fn try_from(raw: RawResponse) -> Result<Self, Self::Error> {
        match raw {
            RawResponse::ValueResponse(vr) => {
                let values = vr
                    .values
                    .into_iter()
                    .map(|v| {
                        Ok::<_, EdetCodecError>(RawDecidedValue {
                            value_bytes: v.value_bytes,
                            certificate: commit_certificate_from_raw(v.certificate)?,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Response::ValueResponse(ValueResponse { start_height: vr.start_height, values }))
            }
        }
    }
}

impl Codec<Response<EdetContext>> for EdetCodec {
    type Error = EdetCodecError;

    fn decode(&self, bytes: Bytes) -> Result<Response<EdetContext>, Self::Error> {
        let raw: RawResponse = serde_json::from_slice(&bytes)?;
        raw.try_into()
    }

    fn encode(&self, msg: &Response<EdetContext>) -> Result<Bytes, Self::Error> {
        Ok(Bytes::from(serde_json::to_vec(&RawResponse::from(msg))?))
    }
}

/// The engine sizes a sync batch by the encoded length of its response; a
/// JSON codec has no way to know that short of encoding, and a sync batch is
/// a handful of blocks, so this encodes.
impl HasEncodedLen<Response<EdetContext>> for EdetCodec {
    fn encoded_len(&self, msg: &Response<EdetContext>) -> Result<usize, <Self as Codec<Response<EdetContext>>>::Error> {
        Ok(self.encode(msg)?.len())
    }
}

// ---------------------------------------------------------------------------
// Raw: LivenessMsg (Vote / PolkaCertificate / SkipRoundCertificate)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct RawPolkaSignature {
    address: EdetAddress,
    signature: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct RawPolkaCertificate {
    height: EdetHeight,
    round: Round,
    value_id: EdetValueId,
    polka_signatures: Vec<RawPolkaSignature>,
}

#[derive(Serialize, Deserialize)]
struct RawRoundSignature {
    vote_type: VoteType,
    value_id: NilOrVal<EdetValueId>,
    address: EdetAddress,
    signature: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct RawRoundCertificate {
    height: EdetHeight,
    round: Round,
    cert_type: RoundCertificateType,
    round_signatures: Vec<RawRoundSignature>,
}

#[derive(Serialize, Deserialize)]
enum RawLivenessMsg {
    Vote(RawSignedMessage<EdetVote>),
    PolkaCertificate(RawPolkaCertificate),
    SkipRoundCertificate(RawRoundCertificate),
}

impl From<&LivenessMsg<EdetContext>> for RawLivenessMsg {
    fn from(msg: &LivenessMsg<EdetContext>) -> Self {
        match msg {
            LivenessMsg::Vote(v) => RawLivenessMsg::Vote(to_raw_signed(v)),
            LivenessMsg::PolkaCertificate(p) => RawLivenessMsg::PolkaCertificate(RawPolkaCertificate {
                height: p.height,
                round: p.round,
                value_id: p.value_id,
                polka_signatures: p
                    .polka_signatures
                    .iter()
                    .map(|s| RawPolkaSignature { address: s.address, signature: encode_signature(&s.signature) })
                    .collect(),
            }),
            LivenessMsg::SkipRoundCertificate(c) => RawLivenessMsg::SkipRoundCertificate(RawRoundCertificate {
                height: c.height,
                round: c.round,
                cert_type: c.cert_type.clone(),
                round_signatures: c
                    .round_signatures
                    .iter()
                    .map(|s| RawRoundSignature {
                        vote_type: s.vote_type,
                        value_id: s.value_id,
                        address: s.address,
                        signature: encode_signature(&s.signature),
                    })
                    .collect(),
            }),
        }
    }
}

impl TryFrom<RawLivenessMsg> for LivenessMsg<EdetContext> {
    type Error = EdetCodecError;

    fn try_from(raw: RawLivenessMsg) -> Result<Self, Self::Error> {
        Ok(match raw {
            RawLivenessMsg::Vote(v) => LivenessMsg::Vote(from_raw_signed(v)?),
            RawLivenessMsg::PolkaCertificate(p) => LivenessMsg::PolkaCertificate(PolkaCertificate {
                height: p.height,
                round: p.round,
                value_id: p.value_id,
                polka_signatures: p
                    .polka_signatures
                    .into_iter()
                    .map(|s| Ok(PolkaSignature { address: s.address, signature: decode_signature(&s.signature)? }))
                    .collect::<Result<Vec<_>, EdetCodecError>>()?,
            }),
            RawLivenessMsg::SkipRoundCertificate(c) => LivenessMsg::SkipRoundCertificate(RoundCertificate {
                height: c.height,
                round: c.round,
                cert_type: c.cert_type,
                round_signatures: c
                    .round_signatures
                    .into_iter()
                    .map(|s| {
                        Ok(RoundSignature {
                            vote_type: s.vote_type,
                            value_id: s.value_id,
                            address: s.address,
                            signature: decode_signature(&s.signature)?,
                        })
                    })
                    .collect::<Result<Vec<_>, EdetCodecError>>()?,
            }),
        })
    }
}

impl Codec<LivenessMsg<EdetContext>> for EdetCodec {
    type Error = EdetCodecError;

    fn decode(&self, bytes: Bytes) -> Result<LivenessMsg<EdetContext>, Self::Error> {
        let raw: RawLivenessMsg = serde_json::from_slice(&bytes)?;
        raw.try_into()
    }

    fn encode(&self, msg: &LivenessMsg<EdetContext>) -> Result<Bytes, Self::Error> {
        Ok(Bytes::from(serde_json::to_vec(&RawLivenessMsg::from(msg))?))
    }
}

// ---------------------------------------------------------------------------
// Raw: ProposedValue (the WAL's second codec requirement)
// ---------------------------------------------------------------------------

/// `Validity` derives no `serde` impl at all (only `cfg_attr`-gated for
/// `borsh`), hence this two-variant mirror.
#[derive(Serialize, Deserialize)]
enum RawValidity {
    Valid,
    Invalid,
}

impl From<Validity> for RawValidity {
    fn from(v: Validity) -> Self {
        match v {
            Validity::Valid => RawValidity::Valid,
            Validity::Invalid => RawValidity::Invalid,
        }
    }
}

impl From<RawValidity> for Validity {
    fn from(v: RawValidity) -> Self {
        match v {
            RawValidity::Valid => Validity::Valid,
            RawValidity::Invalid => Validity::Invalid,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct RawProposedValue {
    height: EdetHeight,
    round: Round,
    valid_round: Round,
    proposer: EdetAddress,
    value: EdetValue,
    validity: RawValidity,
}

impl From<&ProposedValue<EdetContext>> for RawProposedValue {
    fn from(v: &ProposedValue<EdetContext>) -> Self {
        Self {
            height: v.height,
            round: v.round,
            valid_round: v.valid_round,
            proposer: v.proposer,
            value: v.value.clone(),
            validity: v.validity.into(),
        }
    }
}

impl From<RawProposedValue> for ProposedValue<EdetContext> {
    fn from(raw: RawProposedValue) -> Self {
        ProposedValue {
            height: raw.height,
            round: raw.round,
            valid_round: raw.valid_round,
            proposer: raw.proposer,
            value: raw.value,
            validity: raw.validity.into(),
        }
    }
}

impl Codec<ProposedValue<EdetContext>> for EdetCodec {
    type Error = EdetCodecError;

    fn decode(&self, bytes: Bytes) -> Result<ProposedValue<EdetContext>, Self::Error> {
        let raw: RawProposedValue = serde_json::from_slice(&bytes)?;
        Ok(raw.into())
    }

    fn encode(&self, msg: &ProposedValue<EdetContext>) -> Result<Bytes, Self::Error> {
        Ok(Bytes::from(serde_json::to_vec(&RawProposedValue::from(msg))?))
    }
}

// ---------------------------------------------------------------------------
// Trait-bound sanity: EdetCodec really does satisfy every trait start_engine
// asks for. These never run anything — a failure to compile IS the check.
// ---------------------------------------------------------------------------

const _: fn() = || {
    fn assert_consensus_codec<C: ConsensusCodec<EdetContext>>() {}
    fn assert_sync_codec<C: SyncCodec<EdetContext>>() {}
    fn assert_wal_codec<C: WalCodec<EdetContext> + Clone>() {}

    assert_consensus_codec::<EdetCodec>();
    assert_sync_codec::<EdetCodec>();
    assert_wal_codec::<EdetCodec>();
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use malachitebft_app_channel::app::consensus::WalEntry;
    use malachitebft_signing::SigningProvider;

    use crate::block::{dev_seed, Block};
    use crate::engine_context::EdetSigningProvider;

    fn sample_vote() -> EdetVote {
        EdetVote {
            vote_type: VoteType::Precommit,
            height: EdetHeight::new(3),
            round: Round::new(1),
            value_id: NilOrVal::Val(EdetValueId([5u8; 32])),
            address: EdetAddress(2),
            extension: None,
        }
    }

    fn sample_proposal() -> EdetProposal {
        let block = Block { height: 5, time_secs: 150, app_hash: [0u8; 32], txs: Vec::new() };
        let value = EdetValue::from_block(block).unwrap();
        EdetProposal {
            height: EdetHeight::new(5),
            round: Round::new(0),
            value,
            pol_round: Round::Nil,
            address: EdetAddress(3),
        }
    }

    fn signed_vote() -> SignedMessage<EdetContext, EdetVote> {
        let provider = EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(0)), "edet-dev".to_string());
        futures::executor::block_on(provider.sign_vote(sample_vote())).unwrap()
    }

    fn signed_proposal() -> SignedMessage<EdetContext, EdetProposal> {
        let provider = EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(1)), "edet-dev".to_string());
        futures::executor::block_on(provider.sign_proposal(sample_proposal())).unwrap()
    }

    // --- SignedConsensusMsg (vote + proposal) ------------------------------

    #[test]
    fn signed_consensus_vote_round_trips() {
        let codec = EdetCodec;
        let msg = SignedConsensusMsg::Vote(signed_vote());
        let bytes = codec.encode(&msg).unwrap();
        let back = codec.decode(bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn signed_consensus_proposal_round_trips() {
        let codec = EdetCodec;
        let msg = SignedConsensusMsg::Proposal(signed_proposal());
        let bytes = codec.encode(&msg).unwrap();
        let back = codec.decode(bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn tampered_signature_bytes_are_rejected_at_decode() {
        let codec = EdetCodec;
        let msg = SignedConsensusMsg::Vote(signed_vote());
        let bytes = codec.encode(&msg).unwrap();

        // Corrupt the JSON so the embedded signature array has the wrong
        // length (65 bytes instead of 64) — decode must fail closed, not
        // silently truncate/pad.
        let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        json["Vote"]["signature"].as_array_mut().unwrap().push(serde_json::json!(0));
        let corrupted = Bytes::from(serde_json::to_vec(&json).unwrap());

        let result: Result<SignedConsensusMsg<EdetContext>, _> = codec.decode(corrupted);
        assert!(matches!(result, Err(EdetCodecError::InvalidSignature(_))));
    }

    // --- Multi-chunk proposal-part stream ----------------------------------

    #[test]
    fn proposal_part_stream_round_trips() {
        let codec = EdetCodec;
        let stream_id = StreamId::new(Bytes::from_static(b"stream-42"));

        let parts = vec![
            StreamMessage::new(
                stream_id.clone(),
                0,
                StreamContent::Data(EdetProposalPart::Init {
                    height: EdetHeight::new(7),
                    round: Round::new(0),
                    pol_round: Round::Nil,
                    proposer: EdetAddress(0),
                }),
            ),
            StreamMessage::new(
                stream_id.clone(),
                1,
                StreamContent::Data(EdetProposalPart::Chunk { seq: 0, bytes: vec![1, 2, 3] }),
            ),
            StreamMessage::new(
                stream_id.clone(),
                2,
                StreamContent::Data(EdetProposalPart::Chunk { seq: 1, bytes: vec![4, 5, 6, 7] }),
            ),
            StreamMessage::new(
                stream_id.clone(),
                3,
                StreamContent::Data(EdetProposalPart::Fin { block_hash: [9u8; 32] }),
            ),
            StreamMessage::new(stream_id, 4, StreamContent::<EdetProposalPart>::Fin),
        ];

        for part in parts {
            let bytes = codec.encode(&part).unwrap();
            let back = codec.decode(bytes).unwrap();
            assert_eq!(part, back);
        }
    }

    // --- sync::Status / Request / Response (commit certificate nested) ----

    #[test]
    fn status_round_trips() {
        let codec = EdetCodec;
        // Identity multihash (code 0, length 4) — avoids needing the `rand`
        // feature just to mint a `PeerId` for a codec unit test.
        let peer_id = PeerId::from_bytes(&[0x00, 0x04, 1, 2, 3, 4]).unwrap();
        let status = Status { peer_id, tip_height: EdetHeight::new(10), history_min_height: EdetHeight::new(1) };
        let bytes = codec.encode(&status).unwrap();
        let back = codec.decode(bytes).unwrap();
        assert_eq!(status, back);
    }

    #[test]
    fn value_request_round_trips() {
        let codec = EdetCodec;
        let req = Request::ValueRequest(ValueRequest::new(EdetHeight::new(9)..=EdetHeight::new(12)));
        let bytes = codec.encode(&req).unwrap();
        let back = codec.decode(bytes).unwrap();
        assert_eq!(req, back);
    }

    fn sample_certificate() -> CommitCertificate<EdetContext> {
        let commits = (0..3u64)
            .map(|i| {
                let provider =
                    EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(i as u8)), "edet-dev".to_string());
                futures::executor::block_on(provider.sign_vote(EdetVote {
                    vote_type: VoteType::Precommit,
                    height: EdetHeight::new(4),
                    round: Round::new(0),
                    value_id: NilOrVal::Val(EdetValueId([3u8; 32])),
                    address: EdetAddress(i),
                    extension: None,
                }))
                .unwrap()
            })
            .collect();
        CommitCertificate::new(EdetHeight::new(4), Round::new(0), EdetValueId([3u8; 32]), commits)
    }

    #[test]
    fn commit_certificate_round_trips_via_sync_response() {
        let codec = EdetCodec;
        let certificate = sample_certificate();
        assert_eq!(certificate.commit_signatures.len(), 3, "sanity: every sample commit matches height/round/value_id");

        let response = Response::ValueResponse(ValueResponse::new(
            EdetHeight::new(4),
            vec![RawDecidedValue::new(Bytes::from_static(b"decided-block-bytes"), certificate)],
        ));

        let bytes = codec.encode(&response).unwrap();
        let back = codec.decode(bytes).unwrap();
        assert_eq!(response, back);
    }

    #[test]
    fn response_with_no_decided_value_round_trips() {
        let codec = EdetCodec;
        let response = Response::ValueResponse(ValueResponse::new(EdetHeight::new(4), vec![]));
        let bytes = codec.encode(&response).unwrap();
        let back = codec.decode(bytes).unwrap();
        assert_eq!(response, back);
    }

    // --- LivenessMsg: all three variants ------------------------------------

    #[test]
    fn liveness_vote_round_trips() {
        let codec = EdetCodec;
        let msg = LivenessMsg::Vote(signed_vote());
        let bytes = codec.encode(&msg).unwrap();
        let back = codec.decode(bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn liveness_polka_certificate_round_trips() {
        let codec = EdetCodec;
        let votes = (0..2u64)
            .map(|i| {
                let provider =
                    EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(i as u8)), "edet-dev".to_string());
                futures::executor::block_on(provider.sign_vote(EdetVote {
                    vote_type: VoteType::Prevote,
                    height: EdetHeight::new(2),
                    round: Round::new(1),
                    value_id: NilOrVal::Val(EdetValueId([6u8; 32])),
                    address: EdetAddress(i),
                    extension: None,
                }))
                .unwrap()
            })
            .collect();
        let cert = PolkaCertificate::new(EdetHeight::new(2), Round::new(1), EdetValueId([6u8; 32]), votes);
        let msg = LivenessMsg::PolkaCertificate(cert);

        let bytes = codec.encode(&msg).unwrap();
        let back = codec.decode(bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn liveness_skip_round_certificate_round_trips() {
        let codec = EdetCodec;
        let votes = (0..2u64)
            .map(|i| {
                let provider =
                    EdetSigningProvider::new(SigningKey::from_bytes(&dev_seed(i as u8)), "edet-dev".to_string());
                futures::executor::block_on(provider.sign_vote(EdetVote {
                    vote_type: VoteType::Precommit,
                    height: EdetHeight::new(2),
                    round: Round::new(1),
                    value_id: NilOrVal::Nil,
                    address: EdetAddress(i),
                    extension: None,
                }))
                .unwrap()
            })
            .collect();
        let cert =
            RoundCertificate::new_from_votes(EdetHeight::new(2), Round::new(1), RoundCertificateType::Skip, votes);
        let msg = LivenessMsg::<EdetContext>::SkipRoundCertificate(cert);

        let bytes = codec.encode(&msg).unwrap();
        let back = codec.decode(bytes).unwrap();
        assert_eq!(msg, back);
    }

    // --- ProposedValue (WAL's second Codec requirement) --------------------

    #[test]
    fn proposed_value_round_trips() {
        let codec = EdetCodec;
        let block = Block { height: 6, time_secs: 180, app_hash: [0u8; 32], txs: Vec::new() };
        let value = EdetValue::from_block(block).unwrap();
        let proposed = ProposedValue {
            height: EdetHeight::new(6),
            round: Round::new(0),
            valid_round: Round::Nil,
            proposer: EdetAddress(1),
            value,
            validity: Validity::Valid,
        };

        let bytes = codec.encode(&proposed).unwrap();
        let back = codec.decode(bytes).unwrap();
        assert_eq!(proposed, back);
    }

    // --- WAL entry: SignedConsensusMsg + ProposedValue through this codec --

    #[test]
    fn wal_entry_consensus_msg_round_trips_through_codec() {
        let codec = EdetCodec;
        let original = WalEntry::<EdetContext>::ConsensusMsg(SignedConsensusMsg::Vote(signed_vote()));

        // Mirrors what `malachitebft_engine::wal::entry::encode_entry`/
        // `decode_entry` do byte-for-byte (those two functions are private
        // to the engine crate — only the `WalCodec` trait they require is
        // public — so this reproduces their per-payload step directly
        // against our own codec instead of linking their private helper).
        let bytes = match original.as_consensus_msg() {
            Some(msg) => codec.encode(msg).unwrap(),
            None => unreachable!(),
        };
        let decoded: SignedConsensusMsg<EdetContext> = codec.decode(bytes).unwrap();
        let roundtripped = WalEntry::<EdetContext>::ConsensusMsg(decoded);

        assert_eq!(original.as_consensus_msg(), roundtripped.as_consensus_msg());
    }

    #[test]
    fn wal_entry_proposed_value_round_trips_through_codec() {
        let codec = EdetCodec;
        let block = Block { height: 8, time_secs: 240, app_hash: [0u8; 32], txs: Vec::new() };
        let value = EdetValue::from_block(block).unwrap();
        let proposed = ProposedValue {
            height: EdetHeight::new(8),
            round: Round::new(1),
            valid_round: Round::new(0),
            proposer: EdetAddress(4),
            value,
            validity: Validity::Invalid,
        };
        let original = WalEntry::<EdetContext>::ProposedValue(proposed);

        let bytes = match original.as_proposed_value() {
            Some(v) => codec.encode(v).unwrap(),
            None => unreachable!(),
        };
        let decoded: ProposedValue<EdetContext> = codec.decode(bytes).unwrap();
        let roundtripped = WalEntry::<EdetContext>::ProposedValue(decoded);

        assert_eq!(original.as_proposed_value(), roundtripped.as_proposed_value());
    }
}
