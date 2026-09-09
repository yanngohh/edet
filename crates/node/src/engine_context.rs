//! The edet-owned Malachite `Context` (`--features malachite`).
//!
//! Implements `malachitebft_core_types::Context` directly instead of
//! reusing `malachitebft_test::TestContext` — the scaffold `engine_malachite`
//! would compile against — because `TestContext`'s `Value`/`ProposalPart`
//! cannot carry an edet `Block` (the paper's §Implementation).
//! Types, Ed25519 signing,
//! proposer selection) and quorum equivalence (
//! `ValidatorSet::from_state`/`build`); `engine_malachite::run` is
//! now wired against these types — everything here is exercised by that
//! file's handlers, still only compile-checked and in-process-unit-tested
//! (`--features malachite`), never by a running networked node.
//!
//! Vendored trait shapes this was written against:
//! `~/.cargo/git/checkouts/malachite-*/code/crates/core-types/src/{context,
//! signing,certificate,proposal_part,vote,proposal,validator_set,height,
//! value}.rs` (the pinned commit).

use std::fmt;

use async_trait::async_trait;
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use malachitebft_core_types::{
    Context, NilOrVal, Proposal as ProposalTrait, ProposalPart as ProposalPartTrait, Round, SignedExtension,
    SignedMessage, SigningScheme, Validator as ValidatorTrait, ValidatorSet as ValidatorSetTrait, Value as ValueTrait,
    Vote as VoteTrait, VoteType, VotingPower,
};
use malachitebft_signing::{Error as SigningError, SigningProvider, VerificationResult};
use serde::{Deserialize, Serialize};

use edet_state::types::{Key, MemberId};
use edet_state::State;

use crate::block::{hex32, sha256, Block, CodecError};

// ---------------------------------------------------------------------------
// Height
// ---------------------------------------------------------------------------

/// Thin wrapper over `u64`, matching `Replica.height` (`replica.rs`) — the
/// height Malachite is about to decide is `replica.height + 1` .
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdetHeight(pub u64);

impl EdetHeight {
    pub const fn new(height: u64) -> Self {
        Self(height)
    }

    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for EdetHeight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl malachitebft_core_types::Height for EdetHeight {
    const ZERO: Self = Self(0);
    const INITIAL: Self = Self(1);

    fn increment_by(&self, n: u64) -> Self {
        Self(self.0 + n)
    }

    fn decrement_by(&self, n: u64) -> Option<Self> {
        self.0.checked_sub(n).map(Self)
    }

    fn as_u64(&self) -> u64 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Address
// ---------------------------------------------------------------------------

/// A validator's address IS its member id — never a hash of a signing key.
/// Members rotate keys (`Member.pending_rotation`), and a key-derived address
/// would silently orphan a validator's vote/proposal history across a
/// rotation: bind to a stable id, never to a mutable key hash. This is a
/// newtype over `MemberId` rather than a literal
/// `type Address = MemberId` only because Rust's orphan rules forbid
/// implementing a foreign trait (`Address`) for a foreign type alias (`u64`);
/// the bits carried are exactly the member id either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdetAddress(pub MemberId);

impl fmt::Display for EdetAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl malachitebft_core_types::Address for EdetAddress {}

// ---------------------------------------------------------------------------
// Value / ValueId
// ---------------------------------------------------------------------------

/// The full SHA-256 block hash — not `engine_malachite::devnet_value`'s
/// truncated 8-byte id, which that file's own doc comment already flags as
/// "not collision-safe for production" .
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdetValueId(pub [u8; 32]);

impl fmt::Display for EdetValueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex32(&self.0))
    }
}

/// The value Malachite reaches consensus on: the block plus its id. `id` is
/// carried by votes (small, `Copy`); `block` is what a proposer streams and a
/// replica ultimately commits (`Replica::commit_block`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EdetValue {
    pub id: EdetValueId,
    pub block: Block,
}

impl EdetValue {
    /// Hash `block` (`Block::hash`, already the full-SHA-256 primitive used
    /// elsewhere in this crate) and pair it with itself.
    pub fn from_block(block: Block) -> Result<Self, CodecError> {
        let id = EdetValueId(block.hash()?);
        Ok(Self { id, block })
    }
}

// `Block`/`SignedTx` carry no `PartialEq`/`Ord` (they hold `Vec<u8>` signature
// material nobody has ever needed to compare or order); `EdetValue`'s
// identity for consensus purposes is exactly `id`, which is already a
// collision-resistant hash of `block`, so ordering/equality by `id` alone is
// sound and is what lets `EdetValue` satisfy `Value`'s `Ord` bound cheaply.
impl PartialEq for EdetValue {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for EdetValue {}
impl PartialOrd for EdetValue {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for EdetValue {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}

impl ValueTrait for EdetValue {
    type Id = EdetValueId;

    fn id(&self) -> EdetValueId {
        self.id
    }
}

// ---------------------------------------------------------------------------
// ProposalPart
// ---------------------------------------------------------------------------

/// A bytes-carrying proposal part — what `malachitebft_test::ProposalPart`
/// cannot express (its `Data` variant factors a bare `u64`). `Chunk` is
/// a slice of `Block::encode()`; a proposer streams `Init`, then `Chunk`s,
/// then `Fin` (the reassembly hash), per `GetValue`'s handler-completion plan
/// (not implemented by this file, which stops at the type).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdetProposalPart {
    Init { height: EdetHeight, round: Round, pol_round: Round, proposer: EdetAddress },
    Chunk { seq: u32, bytes: Vec<u8> },
    Fin { block_hash: [u8; 32] },
}

impl ProposalPartTrait<EdetContext> for EdetProposalPart {
    fn is_first(&self) -> bool {
        matches!(self, Self::Init { .. })
    }

    fn is_last(&self) -> bool {
        matches!(self, Self::Fin { .. })
    }
}

// ---------------------------------------------------------------------------
// Vote
// ---------------------------------------------------------------------------

/// A prevote or precommit. `extension` is never populated in practice
/// (`Extension = ()`, no vote extensions in scope) and is excluded from
/// the signed/encoded payload (`#[serde(skip)]`) — `SignedMessage` itself
/// carries no `serde` impl at all (vendored `signed_message.rs`), so a field
/// of that type could not derive `Serialize` regardless.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EdetVote {
    pub vote_type: VoteType,
    pub height: EdetHeight,
    pub round: Round,
    pub value_id: NilOrVal<EdetValueId>,
    pub address: EdetAddress,
    #[serde(skip)]
    pub extension: Option<SignedExtension<EdetContext>>,
}

impl VoteTrait<EdetContext> for EdetVote {
    fn height(&self) -> EdetHeight {
        self.height
    }

    fn round(&self) -> Round {
        self.round
    }

    fn value(&self) -> &NilOrVal<EdetValueId> {
        &self.value_id
    }

    fn take_value(self) -> NilOrVal<EdetValueId> {
        self.value_id
    }

    fn vote_type(&self) -> VoteType {
        self.vote_type
    }

    fn validator_address(&self) -> &EdetAddress {
        &self.address
    }

    fn extension(&self) -> Option<&SignedExtension<EdetContext>> {
        self.extension.as_ref()
    }

    fn take_extension(&mut self) -> Option<SignedExtension<EdetContext>> {
        self.extension.take()
    }

    fn extend(self, extension: SignedExtension<EdetContext>) -> Self {
        Self { extension: Some(extension), ..self }
    }
}

// ---------------------------------------------------------------------------
// Proposal
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdetProposal {
    pub height: EdetHeight,
    pub round: Round,
    pub value: EdetValue,
    pub pol_round: Round,
    pub address: EdetAddress,
}

impl ProposalTrait<EdetContext> for EdetProposal {
    fn height(&self) -> EdetHeight {
        self.height
    }

    fn round(&self) -> Round {
        self.round
    }

    fn value(&self) -> &EdetValue {
        &self.value
    }

    fn take_value(self) -> EdetValue {
        self.value
    }

    fn pol_round(&self) -> Round {
        self.pol_round
    }

    fn validator_address(&self) -> &EdetAddress {
        &self.address
    }
}

// ---------------------------------------------------------------------------
// Validator / ValidatorSet
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdetValidator {
    pub address: EdetAddress,
    pub public_key: VerifyingKey,
    pub power: VotingPower,
}

impl ValidatorTrait<EdetContext> for EdetValidator {
    fn address(&self) -> &EdetAddress {
        &self.address
    }

    fn public_key(&self) -> &VerifyingKey {
        &self.public_key
    }

    fn voting_power(&self) -> VotingPower {
        self.power
    }
}

/// Built from `edet_state::State` — a power map (typically
/// `state.validators`, or a past height's from `Replica::validators_at`)
/// joined against signing keys. The key for each validator is resolved
/// from an explicit per-height key map first (`Replica::keys_at`), falling
/// back to `state.members[id].keys.first()` (the member's CURRENT key) only
/// when that map has no recorded rotation for this id — see `build`'s doc
/// comment for exactly why that fallback is sound. A member's `keys` is
/// replaced wholesale on `RotateFinalize` (`apply.rs::rotate_finalize`),
/// never appended to, so `keys.first()` is always "the" current key at
/// whatever moment it's read, never a stale one from before a since-applied
/// rotation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdetValidatorSet {
    /// Sorted per the `ValidatorSet` trait's documented contract
    /// (`validator_set.rs`): power descending, then address ascending. This
    /// is the order `EdetContext::select_proposer`'s round-robin indexes
    /// into, and it coincides with plain ascending `MemberId` order whenever
    /// every validator has equal power — true of every dev-genesis cluster
    /// today (`dev_genesis` seeds every founder at power 1) — which is what
    /// makes the round-robin identical to `NodeCore::proposer`'s in that case
    ///.
    validators: Vec<EdetValidator>,
}

impl EdetValidatorSet {
    /// # Panics
    /// If `validators` is empty — matches `malachitebft_test::ValidatorSet`'s
    /// documented precondition; an empty validator set cannot decide. Every
    /// path that can reach this from live state now goes through `build`,
    /// which refuses (`Err`, not a smaller `Self`) before ever calling this,
    /// so the panic is reachable only by a direct, non-empty-by-construction
    /// caller passing an empty `Vec` explicitly (there are none in this
    /// crate) — kept as a loud precondition rather than silently accepting
    /// `[]`, matching the upstream contract this type stands in for.
    pub fn new(mut validators: Vec<EdetValidator>) -> Self {
        assert!(!validators.is_empty(), "a validator set must not be empty");
        validators.sort_by(|a, b| b.power.cmp(&a.power).then_with(|| a.address.cmp(&b.address)));
        Self { validators }
    }

    /// No historical key journal to join against — resolves every validator
    /// from `state.members`' CURRENT key, i.e. exactly `build`'s behaviour
    /// when its `keys` argument has no entry for a given id. Correct only
    /// for "the set as of right now"; a caller reconstructing a PAST height
    /// (`engine_malachite`'s handlers) must go through `build` with
    /// `Replica::keys_at(height)` instead — this is what `from_state`
    /// itself does with an empty key map.
    pub fn from_state(state: &State) -> Result<Self, ValidatorSetError> {
        Self::build(state, &state.validators, &std::collections::BTreeMap::new())
    }

    /// Joins an explicit power map (typically `Replica::
    /// validators_at(height)`) against an explicit per-height key map
    /// (`Replica::keys_at(height)`) instead of always reading `state.members`'
    /// CURRENT key — this is what lets `engine_malachite`'s
    /// `ConsensusReady`/`GetValidatorSet`/`Decided` handlers build the set
    /// that was ACTUALLY live at a past height, keys included, rather than
    /// silently substituting whatever key that member holds today.
    ///
    /// For each id in `validators`, `keys` is consulted FIRST; only when
    /// `keys` has no entry for that id does this fall back to
    /// `state.members[id]`'s current key. That fallback is sound, not a
    /// shortcut: `keys` omitting an id means no block ever recorded that
    /// member rotating a key while they were a validator at or before this
    /// height (`Replica::keys_at`'s doc comment spells out why), so their
    /// current key already equals their key at every such height. Without the journal
    /// this always read `state.members` unconditionally — the one
    /// spec-acknowledged gap that let a certificate signed with an OLD key
    /// fail to verify once that key had since rotated (real the moment key
    /// rotation is exercised alongside historical certificate verification,
    /// e.g. during peer sync). Closed now that `keys` carries the actual
    /// per-height history.
    ///
    /// Used to `filter_map` away any entry whose member lookup or key
    /// parse failed, silently returning a SMALLER set — which silently
    /// LOWERS the quorum threshold (`ThresholdParams` is computed against
    /// whatever set this returns), a fail-open shape in a place that must
    /// fail closed. Now returns `Err` and constructs nothing instead: every
    /// caller either refuses to advance (`engine_malachite`'s handlers) or
    /// propagates the error, but none of them ever certifies against a
    /// quorum smaller than what governance actually configured.
    pub fn build(
        state: &State,
        validators: &std::collections::BTreeMap<MemberId, u64>,
        keys: &std::collections::BTreeMap<MemberId, Key>,
    ) -> Result<Self, ValidatorSetError> {
        if validators.is_empty() {
            return Err(ValidatorSetError::Empty);
        }
        let mut built = Vec::with_capacity(validators.len());
        let mut total: u64 = 0;
        for (&id, &power) in validators {
            // The same ceiling the ledger enforces on a governed power
            // change (`apply.rs`, `k::MAX_VALIDATOR_POWER`), applied again
            // here because a validator set does not only ever arrive from
            // this node's own ledger — `EdetApp::start` builds one straight
            // out of a genesis file this node did not author. Checking the
            // per-validator bound AND the running total is what makes
            // `total_voting_power`'s `u64` sum unreachable rather than merely
            // saturating: the total is the number every quorum threshold is a
            // fraction of.
            if power > edet_kernel::constants::MAX_VALIDATOR_POWER {
                return Err(ValidatorSetError::PowerTooLarge(id));
            }
            total = total.checked_add(power).ok_or(ValidatorSetError::TotalPowerOverflow)?;
            // The per-height key first; a member's CURRENT key only when
            // history has nothing recorded for this id (see this method's
            // doc comment for why that's sound rather than a guess).
            //
            // The key here is the CONSENSUS key: a validator signs votes with
            // a key that lives on its host, never with the member key that
            // signs its obligations. A validator
            // holding none is refused by the ledger in both directions
            // (`ET-VAL-004`), so this fails CLOSED rather than returning a
            // smaller set: a shorter set is a lower quorum.
            let key = match keys.get(&id) {
                Some(k) => *k,
                None => state
                    .members
                    .get(&id)
                    .ok_or(ValidatorSetError::MissingMember(id))?
                    .consensus_key
                    .ok_or(ValidatorSetError::MissingConsensusKey(id))?,
            };
            let public_key = VerifyingKey::from_bytes(&key).map_err(|_| ValidatorSetError::UnparseableKey(id))?;
            built.push(EdetValidator { address: EdetAddress(id), public_key, power });
        }
        // `built` is non-empty (same length as `validators`, checked above)
        // and every entry succeeded, so `new`'s precondition holds.
        Ok(Self::new(built))
    }
}

/// Why `EdetValidatorSet::build` could not construct a set. Three
/// distinguishable causes, each meaningfully different to a caller deciding
/// what to do next:
/// - `Empty`: the power map itself had no entries. Should be unreachable
///   once every validator-removal site in `apply.rs` enforces
///   `MIN_VALIDATORS`, but `build` does not trust that invariant blindly —
///   it fails closed rather than assuming.
/// - `MissingConsensusKey`: it resolves to a member who runs no validator.
/// - `MissingMember`: an address in the power map does not resolve to a
///   member in `state.members`, or that member has no signing key at all.
///   Both mean the same thing to a verifier — there is no key to check a
///   signature against — so they share a variant.
/// - `UnparseableKey`: the member has a key, but its bytes are not a valid
///   Ed25519 public key.
/// - `PowerTooLarge` / `TotalPowerOverflow`: a validator's voting power
///   is past `k::MAX_VALIDATOR_POWER`, or the set's total does not fit the
///   `u64` `total_voting_power` sums into. Distinguished because they point
///   at different mistakes — one entry written wrong, versus a set that is
///   individually legal but collectively out of range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidatorSetError {
    Empty,
    MissingMember(MemberId),
    /// The member exists and has registered no consensus key. Distinguished
    /// from `MissingMember` because it is an operator's configuration error
    /// rather than a corrupt set, and because the ledger refuses to create it
    /// (`ET-VAL-004`) — reaching it means a genesis file that never passed
    /// `genesis_state`, or a state this build did not produce.
    MissingConsensusKey(MemberId),
    UnparseableKey(MemberId),
    PowerTooLarge(MemberId),
    TotalPowerOverflow,
}

impl fmt::Display for ValidatorSetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidatorSetError::Empty => write!(f, "validator power map is empty"),
            ValidatorSetError::MissingMember(id) => {
                write!(f, "validator {id} has no known member record or signing key")
            }
            ValidatorSetError::MissingConsensusKey(id) => {
                write!(
                    f,
                    "validator {id} has registered no consensus key, so no certificate can be verified against it"
                )
            }
            ValidatorSetError::UnparseableKey(id) => write!(f, "validator {id}'s signing key does not parse"),
            ValidatorSetError::PowerTooLarge(id) => write!(
                f,
                "validator {id}'s voting power is above the maximum {} — consensus sums the whole set into one u64",
                edet_kernel::constants::MAX_VALIDATOR_POWER
            ),
            ValidatorSetError::TotalPowerOverflow => {
                write!(f, "the validator set's total voting power does not fit a u64")
            }
        }
    }
}

impl std::error::Error for ValidatorSetError {}

impl ValidatorSetTrait<EdetContext> for EdetValidatorSet {
    fn count(&self) -> usize {
        self.validators.len()
    }

    /// Saturating, never a plain `sum()`. `VotingPower` is a `u64` and a
    /// wrapped total is a SAFETY break, not a crash: every quorum threshold
    /// (`ThresholdParams`) is a fraction of this number, so a total that
    /// wrapped to a small value makes quorum satisfiable by a fraction of the
    /// real power — a validator set that overflows would certify blocks a
    /// minority signed. Saturating fails the other way (an unreachable
    /// threshold, so nothing decides), which is the direction a consensus
    /// sum must fail in.
    ///
    /// `build` refuses such a set outright (`ValidatorSetError::PowerTooLarge`
    /// / `TotalPowerOverflow`), so on every set this crate constructs from a
    /// ledger or a genesis file the saturation is unreachable — it is the
    /// backstop for a set handed to `new` directly, not the primary gate.
    /// That ordering matters because saturation alone is not safe either:
    /// `ThresholdParam::is_met` multiplies the weight by 3 and panics on
    /// overflow (vendored `threshold.rs`), so a saturated total halts the
    /// node rather than certifying anything — loud, but still a halt. Not
    /// building the set is the only outcome that is neither a false quorum
    /// nor a crash.
    fn total_voting_power(&self) -> VotingPower {
        self.validators.iter().fold(0u64, |acc, v| acc.saturating_add(v.power))
    }

    fn get_by_address(&self, address: &EdetAddress) -> Option<&EdetValidator> {
        self.validators.iter().find(|v| &v.address == address)
    }

    fn get_by_index(&self, index: usize) -> Option<&EdetValidator> {
        self.validators.get(index)
    }
}

// ---------------------------------------------------------------------------
// Signing
// ---------------------------------------------------------------------------

/// Raw 64-byte Ed25519 signature bytes. A newtype (rather than
/// `ed25519_dalek::Signature` directly) because `SigningScheme::Signature`
/// requires `Ord`, which `ed25519_dalek::Signature` does not implement;
/// `[u8; 64]` does (array trait impls are generic over length). No `serde`
/// derive: serde's array impl only covers `[T; 0..=32]` — `engine_malachite`'s
/// `StoredCertificate` carries this as a plain `Vec<u8>` instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EdetSignature(pub [u8; 64]);

#[derive(Debug)]
pub struct SignatureDecodeError(String);

impl fmt::Display for SignatureDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid signature: {}", self.0)
    }
}

/// Ed25519 over `ed25519-dalek`, the same crate (and, via `sign_bytes`'s
/// sha256(codec::encode(..)) convention, the same digest scheme) `SignedTx::verify`
/// already uses (`block.rs:80-91`) — a validator's consensus key and its
/// tx-signing key can be, but need not be, the same key material.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdetSigningScheme;

impl SigningScheme for EdetSigningScheme {
    type DecodingError = SignatureDecodeError;
    type Signature = EdetSignature;
    type PublicKey = VerifyingKey;
    type PrivateKey = SigningKey;

    fn decode_signature(bytes: &[u8]) -> Result<EdetSignature, SignatureDecodeError> {
        let arr: [u8; 64] = bytes
            .try_into()
            .map_err(|_| SignatureDecodeError(format!("expected 64 bytes, got {}", bytes.len())))?;
        Ok(EdetSignature(arr))
    }

    fn encode_signature(signature: &EdetSignature) -> Vec<u8> {
        signature.0.to_vec()
    }
}

/// Per-message-type domain tags, mirroring `crate::block::TX_DOMAIN`'s
/// role for transactions: a `sha256(domain || ...)` prefix removes any
/// question of a signed payload of one kind being reinterpreted as another.
/// `EdetVote`/`EdetProposal`/`EdetProposalPart` share no fields that would
/// make a cross-type collision easy to construct today, but tagging is a
/// two-line change that removes the question entirely rather than leaving it
/// for an auditor to (correctly) refuse to reason about — see this module's
/// top-level review note. `VOTE_EXTENSION_DOMAIN` covers `Extension = ()`,
/// never exercised by a real consensus flow (`ExtendVote`/
/// `VerifyVoteExtension` are no-ops in `engine_malachite.rs`) but still
/// tagged for the same reason and so every `SigningProvider` method goes
/// through the identical scheme.
const VOTE_DOMAIN: &[u8] = b"edet-vote-v1";
const PROPOSAL_DOMAIN: &[u8] = b"edet-proposal-v1";
const PROPOSAL_PART_DOMAIN: &[u8] = b"edet-proposal-part-v1";
const VOTE_EXTENSION_DOMAIN: &[u8] = b"edet-vote-ext-v1";

/// Digest a signable payload exactly like `crate::block::tx_digest`'s scheme:
/// SHA-256 over `domain || len(chain_id) as u64 LE || chain_id ||
/// codec::encode(value)`. The domain tag separates this message TYPE from
/// every other signable type in the system (including transactions —
/// `crate::block::TX_DOMAIN` is a distinct tag); the chain id stops a
/// signature produced for one edet network from verifying on another, the
/// same reason `tx_digest` binds to it. `expect` is safe here — every type
/// signed through this provider (`EdetVote`, `EdetProposal`,
/// `EdetProposalPart`) is a plain, finite data structure with a total
/// `Serialize` impl; the only way `edet_state::codec::encode` fails for such a type
/// is a writer I/O error, which cannot happen serializing into a `Vec`.
fn sign_bytes<T: Serialize>(domain: &[u8], chain_id: &str, value: &T) -> [u8; 32] {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(&(chain_id.len() as u64).to_le_bytes());
    bytes.extend_from_slice(chain_id.as_bytes());
    let payload = edet_state::codec::encode(value).expect("engine_context signable types always encode");
    bytes.extend_from_slice(&payload);
    sha256(&bytes)
}

/// A validator's consensus-message signing key, together with the chain id
/// every message it signs or verifies is bound to. Holds the private
/// key in-process; `SigningProvider`'s verify methods take the signer's
/// public key explicitly (from the `ValidatorSet`), so any node can verify
/// anyone else's messages without holding their key — but every node must
/// agree on `chain_id`, or two honest validators on the same genesis would
/// compute different digests for the identical message.
pub struct EdetSigningProvider {
    signing_key: SigningKey,
    chain_id: String,
}

impl EdetSigningProvider {
    pub fn new(signing_key: SigningKey, chain_id: String) -> Self {
        Self { signing_key, chain_id }
    }

    pub fn public_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }
}

/// Every method is `async` on the trait and synchronous in fact: Ed25519 over
/// a digest yields nothing, and the `Result` is always `Ok` — this provider
/// has no failure of its own to report, and a signature that does not verify
/// is `VerificationResult::Invalid`, never an error.
#[async_trait]
impl SigningProvider<EdetContext> for EdetSigningProvider {
    async fn sign_vote(&self, vote: EdetVote) -> Result<SignedMessage<EdetContext, EdetVote>, SigningError> {
        let digest = sign_bytes(VOTE_DOMAIN, &self.chain_id, &vote);
        let signature = EdetSignature(self.signing_key.sign(&digest).to_bytes());
        Ok(SignedMessage::new(vote, signature))
    }

    async fn verify_signed_vote(
        &self,
        vote: &EdetVote,
        signature: &EdetSignature,
        public_key: &VerifyingKey,
    ) -> Result<VerificationResult, SigningError> {
        Ok(VerificationResult::from_bool(verify_ed25519(
            sign_bytes(VOTE_DOMAIN, &self.chain_id, vote),
            signature,
            public_key,
        )))
    }

    async fn sign_proposal(
        &self,
        proposal: EdetProposal,
    ) -> Result<SignedMessage<EdetContext, EdetProposal>, SigningError> {
        let digest = sign_bytes(PROPOSAL_DOMAIN, &self.chain_id, &proposal);
        let signature = EdetSignature(self.signing_key.sign(&digest).to_bytes());
        Ok(SignedMessage::new(proposal, signature))
    }

    async fn verify_signed_proposal(
        &self,
        proposal: &EdetProposal,
        signature: &EdetSignature,
        public_key: &VerifyingKey,
    ) -> Result<VerificationResult, SigningError> {
        Ok(VerificationResult::from_bool(verify_ed25519(
            sign_bytes(PROPOSAL_DOMAIN, &self.chain_id, proposal),
            signature,
            public_key,
        )))
    }

    async fn sign_proposal_part(
        &self,
        proposal_part: EdetProposalPart,
    ) -> Result<SignedMessage<EdetContext, EdetProposalPart>, SigningError> {
        let digest = sign_bytes(PROPOSAL_PART_DOMAIN, &self.chain_id, &proposal_part);
        let signature = EdetSignature(self.signing_key.sign(&digest).to_bytes());
        Ok(SignedMessage::new(proposal_part, signature))
    }

    async fn verify_signed_proposal_part(
        &self,
        proposal_part: &EdetProposalPart,
        signature: &EdetSignature,
        public_key: &VerifyingKey,
    ) -> Result<VerificationResult, SigningError> {
        Ok(VerificationResult::from_bool(verify_ed25519(
            sign_bytes(PROPOSAL_PART_DOMAIN, &self.chain_id, proposal_part),
            signature,
            public_key,
        )))
    }

    async fn sign_vote_extension(&self, extension: ()) -> Result<SignedMessage<EdetContext, ()>, SigningError> {
        // `Extension = ()`: no vote extensions in scope. Signs a fixed
        // payload so the trait is satisfiable; never exercised by a real
        // consensus flow (`ExtendVote`/`VerifyVoteExtension` are no-ops in
        // `engine_malachite.rs`).
        let digest = sign_bytes(VOTE_EXTENSION_DOMAIN, &self.chain_id, &extension);
        let signature = EdetSignature(self.signing_key.sign(&digest).to_bytes());
        Ok(SignedMessage::new(extension, signature))
    }

    async fn verify_signed_vote_extension(
        &self,
        extension: &(),
        signature: &EdetSignature,
        public_key: &VerifyingKey,
    ) -> Result<VerificationResult, SigningError> {
        Ok(VerificationResult::from_bool(verify_ed25519(
            sign_bytes(VOTE_EXTENSION_DOMAIN, &self.chain_id, extension),
            signature,
            public_key,
        )))
    }
}

fn verify_ed25519(digest: [u8; 32], signature: &EdetSignature, public_key: &VerifyingKey) -> bool {
    let Ok(sig) = ed25519_dalek::Signature::from_slice(&signature.0) else { return false };
    public_key.verify(&digest, &sig).is_ok()
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct EdetContext;

impl Context for EdetContext {
    type Address = EdetAddress;
    type Height = EdetHeight;
    type ProposalPart = EdetProposalPart;
    type Proposal = EdetProposal;
    type Validator = EdetValidator;
    type ValidatorSet = EdetValidatorSet;
    type Value = EdetValue;
    type Vote = EdetVote;
    type Extension = ();
    type SigningScheme = EdetSigningScheme;

    /// Power-proportional, stateless round-robin. Walks the
    /// CometBFT-ordered list (`EdetValidatorSet::new` — power descending,
    /// then address ascending; every node builds and iterates that same
    /// order, so every node lands on the same proposer) and picks whichever
    /// validator's power-weighted "slot" contains `pos`. A validator with
    /// power 1000 owns 1000 of the `total_voting_power()` slots and so is
    /// selected roughly 1000x as often as one with power 1 — closing the gap
    /// the review flagged: quorum was already power-weighted
    /// (`ThresholdParams`/`verify_commit_certificate`), but proposal rights —
    /// and therefore censorship resistance and ordering power — were
    /// one-member-one-turn regardless of power.
    ///
    /// This is deliberately NOT CometBFT's incremental-priority algorithm
    /// (`accum += power` every round, highest-priority proposes, then
    /// `accum -= total`), which keeps per-validator priority state across
    /// heights and gives better short-run fairness (no validator can be
    /// unlucky for long). This scheme is stateless — a pure function of
    /// `(height, round, validator_set)` — which is what makes it trivial for
    /// every node to agree on without persisting or gossiping any selector
    /// state; long-run fairness is preserved by the power-weighted slot
    /// widths even though short-run luck is not smoothed.
    ///
    /// Reduces EXACTLY to `(height + round) % n` when every validator has
    /// power 1 (every dev-genesis cluster today): `total == n`, and walking
    /// unit-width slots is equivalent to plain modular indexing — this is
    /// asserted directly in `select_proposer_matches_dev_harness_rule_when_powers_are_uniform`.
    fn select_proposer<'a>(
        &self,
        validator_set: &'a EdetValidatorSet,
        height: EdetHeight,
        round: Round,
    ) -> &'a EdetValidator {
        assert!(validator_set.count() > 0, "select_proposer requires a non-empty validator set");
        assert!(round.is_defined(), "select_proposer is only ever called with a concrete round");

        let total = validator_set.total_voting_power();
        // Guarded rather than panicking: `total == 0` can only happen if
        // every validator in the set carries power 0, which `apply.rs`'s
        // `MIN_VALIDATORS` floor and `ValidatorPower`'s `power > 0` gate are
        // meant to make unreachable in practice — but this function has no
        // way to enforce that invariant itself, and a stateless selector
        // dividing by zero is a worse failure than falling back to the first
        // validator in canonical order.
        if total == 0 {
            return validator_set.get_by_index(0).expect("count() > 0 was just asserted above");
        }
        let mut pos = height.as_u64().wrapping_add(round.as_i64() as u64) % total;
        for index in 0..validator_set.count() {
            let v = validator_set.get_by_index(index).expect("index < count() is always in range");
            if pos < v.power {
                return v;
            }
            pos -= v.power;
        }
        // Unreachable: `pos < total` by construction (it's the result of
        // `% total`) and the loop above subtracts exactly `total` in total
        // across all validators, so some validator's slot always contains
        // `pos`. Kept as a defined fallback rather than a `panic!`/`unreachable!`
        // so a bug here degrades to "always propose the top validator"
        // instead of aborting the process.
        validator_set.get_by_index(0).expect("count() > 0 was just asserted above")
    }

    fn new_proposal(
        &self,
        height: EdetHeight,
        round: Round,
        value: EdetValue,
        pol_round: Round,
        address: EdetAddress,
    ) -> EdetProposal {
        EdetProposal { height, round, value, pol_round, address }
    }

    fn new_prevote(
        &self,
        height: EdetHeight,
        round: Round,
        value_id: NilOrVal<EdetValueId>,
        address: EdetAddress,
    ) -> EdetVote {
        EdetVote { vote_type: VoteType::Prevote, height, round, value_id, address, extension: None }
    }

    fn new_precommit(
        &self,
        height: EdetHeight,
        round: Round,
        value_id: NilOrVal<EdetValueId>,
        address: EdetAddress,
    ) -> EdetVote {
        EdetVote { vote_type: VoteType::Precommit, height, round, value_id, address, extension: None }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use malachitebft_core_types::ThresholdParam;

    use crate::block::{dev_seed, pubkey_of, Block, SignedTx};

    /// Same seeding `serve::dev_genesis` performs (that function lives behind
    /// the `serve` feature, which this test binary doesn't enable) — `n`
    /// founders, each a real BIP39-derived key, each a genesis validator at
    /// power 1.
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
        // The parent-state commitment is immaterial to these codec/context
        // round-trip fixtures — they assert on encoding, never on validity —
        // so a zero hash keeps them honest about what they cover.
        Block { height, time_secs: height * 30, app_hash: [0u8; 32], txs: Vec::<SignedTx>::new() }
    }

    // --- round-trip encode/decode -------------------------------------

    #[test]
    fn height_round_trips() {
        let h = EdetHeight::new(42);
        let bytes = edet_state::codec::encode(&h).unwrap();
        let back: EdetHeight = edet_state::codec::decode(&bytes).unwrap();
        assert_eq!(h, back);
        assert_eq!(back.as_u64(), 42);
    }

    #[test]
    fn address_round_trips() {
        let a = EdetAddress(7);
        let bytes = edet_state::codec::encode(&a).unwrap();
        let back: EdetAddress = edet_state::codec::decode(&bytes).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn value_round_trips() {
        let block = sample_block(1);
        let value = EdetValue::from_block(block.clone()).unwrap();
        // id is the full block hash, not a truncated devnet id .
        assert_eq!(value.id.0, block.hash().unwrap());

        let bytes = edet_state::codec::encode(&value).unwrap();
        let back: EdetValue = edet_state::codec::decode(&bytes).unwrap();
        assert_eq!(value, back);
        assert_eq!(back.block.height, 1);
    }

    #[test]
    fn proposal_part_round_trips() {
        let parts = vec![
            EdetProposalPart::Init {
                height: EdetHeight::new(1),
                round: Round::new(0),
                pol_round: Round::Nil,
                proposer: EdetAddress(0),
            },
            EdetProposalPart::Chunk { seq: 0, bytes: vec![1, 2, 3, 4] },
            EdetProposalPart::Fin { block_hash: [9u8; 32] },
        ];
        for part in parts {
            assert!(matches!(part, EdetProposalPart::Init { .. }) == part.is_first());
            assert!(matches!(part, EdetProposalPart::Fin { .. }) == part.is_last());
            let bytes = edet_state::codec::encode(&part).unwrap();
            let back: EdetProposalPart = edet_state::codec::decode(&bytes).unwrap();
            assert_eq!(part, back);
        }
    }

    #[test]
    fn vote_round_trips() {
        let vote = EdetVote {
            vote_type: VoteType::Precommit,
            height: EdetHeight::new(3),
            round: Round::new(1),
            value_id: NilOrVal::Val(EdetValueId([5u8; 32])),
            address: EdetAddress(2),
            extension: None,
        };
        let bytes = edet_state::codec::encode(&vote).unwrap();
        let back: EdetVote = edet_state::codec::decode(&bytes).unwrap();
        assert_eq!(vote, back);
    }

    #[test]
    fn proposal_round_trips() {
        let value = EdetValue::from_block(sample_block(5)).unwrap();
        let proposal = EdetProposal {
            height: EdetHeight::new(5),
            round: Round::new(0),
            value,
            pol_round: Round::Nil,
            address: EdetAddress(3),
        };
        let bytes = edet_state::codec::encode(&proposal).unwrap();
        let back: EdetProposal = edet_state::codec::decode(&bytes).unwrap();
        assert_eq!(proposal, back);
    }

    // --- sign+verify round-trip, tamper rejected ----------------------

    #[test]
    fn vote_sign_verify_round_trips_and_rejects_tampering() {
        let seed = dev_seed(0);
        let signing_key = SigningKey::from_bytes(&seed);
        let public_key = signing_key.verifying_key();
        let provider = EdetSigningProvider::new(signing_key, crate::block::DEV_CHAIN_ID.to_string());

        let vote = EdetVote {
            vote_type: VoteType::Prevote,
            height: EdetHeight::new(1),
            round: Round::new(0),
            value_id: NilOrVal::Val(EdetValueId([1u8; 32])),
            address: EdetAddress(0),
            extension: None,
        };

        let signed = futures::executor::block_on(provider.sign_vote(vote.clone())).unwrap();
        assert!(futures::executor::block_on(provider.verify_signed_vote(&vote, &signed.signature, &public_key))
            .unwrap()
            .is_valid());

        // Tampered signature byte.
        let mut bad_sig = signed.signature;
        bad_sig.0[0] ^= 0xFF;
        assert!(!futures::executor::block_on(provider.verify_signed_vote(&vote, &bad_sig, &public_key))
            .unwrap()
            .is_valid());

        // Tampered message (different value id) under the original signature.
        let mut tampered_vote = vote;
        tampered_vote.value_id = NilOrVal::Val(EdetValueId([2u8; 32]));
        assert!(!futures::executor::block_on(provider.verify_signed_vote(
            &tampered_vote,
            &signed.signature,
            &public_key
        ))
        .unwrap()
        .is_valid());

        // Wrong public key.
        let other_public = SigningKey::from_bytes(&dev_seed(1)).verifying_key();
        assert!(!futures::executor::block_on(provider.verify_signed_vote(
            &signed.message,
            &signed.signature,
            &other_public
        ))
        .unwrap()
        .is_valid());
    }

    #[test]
    fn proposal_sign_verify_round_trips_and_rejects_tampering() {
        let signing_key = SigningKey::from_bytes(&dev_seed(2));
        let public_key = signing_key.verifying_key();
        let provider = EdetSigningProvider::new(signing_key, crate::block::DEV_CHAIN_ID.to_string());

        let value = EdetValue::from_block(sample_block(1)).unwrap();
        let proposal = EdetProposal {
            height: EdetHeight::new(1),
            round: Round::new(0),
            value,
            pol_round: Round::Nil,
            address: EdetAddress(2),
        };

        let signed = futures::executor::block_on(provider.sign_proposal(proposal.clone())).unwrap();
        assert!(futures::executor::block_on(provider.verify_signed_proposal(
            &proposal,
            &signed.signature,
            &public_key
        ))
        .unwrap()
        .is_valid());

        let mut bad_sig = signed.signature;
        bad_sig.0[10] ^= 0x01;
        assert!(!futures::executor::block_on(provider.verify_signed_proposal(&proposal, &bad_sig, &public_key))
            .unwrap()
            .is_valid());

        let mut tampered = proposal;
        tampered.round = Round::new(9);
        assert!(!futures::executor::block_on(provider.verify_signed_proposal(
            &tampered,
            &signed.signature,
            &public_key
        ))
        .unwrap()
        .is_valid());
    }

    #[test]
    fn proposal_part_sign_verify_round_trips_and_rejects_tampering() {
        let signing_key = SigningKey::from_bytes(&dev_seed(3));
        let public_key = signing_key.verifying_key();
        let provider = EdetSigningProvider::new(signing_key, crate::block::DEV_CHAIN_ID.to_string());

        let part = EdetProposalPart::Chunk { seq: 0, bytes: vec![1, 2, 3] };
        let signed = futures::executor::block_on(provider.sign_proposal_part(part.clone())).unwrap();
        assert!(futures::executor::block_on(provider.verify_signed_proposal_part(
            &part,
            &signed.signature,
            &public_key
        ))
        .unwrap()
        .is_valid());

        let mut bad_sig = signed.signature;
        bad_sig.0[0] ^= 0xFF;
        assert!(!futures::executor::block_on(provider.verify_signed_proposal_part(&part, &bad_sig, &public_key))
            .unwrap()
            .is_valid());

        let tampered = EdetProposalPart::Chunk { seq: 0, bytes: vec![1, 2, 4] };
        assert!(!futures::executor::block_on(provider.verify_signed_proposal_part(
            &tampered,
            &signed.signature,
            &public_key
        ))
        .unwrap()
        .is_valid());
    }

    // --- domain separation -----------------------------------------

    /// A vote signed under chain id "a" must not verify under chain id "b" —
    /// the same key material, the same message, different networks.
    #[test]
    fn a_vote_signature_does_not_verify_under_a_different_chain_id() {
        let signing_key = SigningKey::from_bytes(&dev_seed(4));
        let public_key = signing_key.verifying_key();
        let provider_a = EdetSigningProvider::new(signing_key, "chain-a".to_string());
        let signing_key_b = SigningKey::from_bytes(&dev_seed(4));
        let provider_b = EdetSigningProvider::new(signing_key_b, "chain-b".to_string());

        let vote = EdetVote {
            vote_type: VoteType::Precommit,
            height: EdetHeight::new(1),
            round: Round::new(0),
            value_id: NilOrVal::Val(EdetValueId([1u8; 32])),
            address: EdetAddress(0),
            extension: None,
        };
        let signed = futures::executor::block_on(provider_a.sign_vote(vote.clone())).unwrap();
        assert!(
            futures::executor::block_on(provider_a.verify_signed_vote(&vote, &signed.signature, &public_key))
                .unwrap()
                .is_valid(),
            "verifies under its own chain"
        );
        assert!(
            !futures::executor::block_on(provider_b.verify_signed_vote(&vote, &signed.signature, &public_key))
                .unwrap()
                .is_valid(),
            "PROVEN: a signature from chain-a must not verify under chain-b, even with the identical key and message"
        );
    }

    /// A vote signature must not verify as a proposal signature (and vice
    /// versa) — the per-type domain tag, not merely the encoded layout,
    /// removes any cross-type reinterpretation question.
    #[test]
    fn a_vote_signature_does_not_verify_as_a_proposal_signature() {
        let signing_key = SigningKey::from_bytes(&dev_seed(5));
        let public_key = signing_key.verifying_key();
        let provider = EdetSigningProvider::new(signing_key, crate::block::DEV_CHAIN_ID.to_string());

        let vote = EdetVote {
            vote_type: VoteType::Prevote,
            height: EdetHeight::new(2),
            round: Round::new(0),
            value_id: NilOrVal::Val(EdetValueId([7u8; 32])),
            address: EdetAddress(1),
            extension: None,
        };
        let signed_vote = futures::executor::block_on(provider.sign_vote(vote)).unwrap();

        let value = EdetValue::from_block(sample_block(2)).unwrap();
        let proposal = EdetProposal {
            height: EdetHeight::new(2),
            round: Round::new(0),
            value,
            pol_round: Round::Nil,
            address: EdetAddress(1),
        };
        assert!(
            !futures::executor::block_on(provider.verify_signed_proposal(
                &proposal,
                &signed_vote.signature,
                &public_key
            ))
            .unwrap()
            .is_valid(),
            "PROVEN: a vote's signature must not verify as a signature over a proposal"
        );
    }

    // --- proposer selection is the documented round-robin --------------

    /// The rule `select_proposer` is specified to implement: plain
    /// round-robin over the validator set, advancing with both height and
    /// round so a failed round rotates to a different proposer.
    fn round_robin(height: u64, round: u64, n: u64) -> u64 {
        (height + round) % n
    }

    #[test]
    fn select_proposer_is_round_robin_when_powers_are_uniform() {
        let n = 5u8;
        let state = seeded_state(n);
        let vset = EdetValidatorSet::from_state(&state).expect("seeded genesis state always builds");
        assert_eq!(vset.count(), n as usize);

        let ctx = EdetContext;
        for height in 1..=20u64 {
            for round in 0..=4u32 {
                let expected = round_robin(height, round as u64, n as u64);
                let got = ctx.select_proposer(&vset, EdetHeight::new(height), Round::new(round));
                assert_eq!(
                    got.address.0, expected,
                    "height={height} round={round}: expected proposer {expected}, got {}",
                    got.address.0
                );
            }
        }
    }

    #[test]
    fn select_proposer_is_deterministic() {
        let state = seeded_state(4);
        let vset = EdetValidatorSet::from_state(&state).expect("seeded genesis state always builds");
        let ctx = EdetContext;
        let a = ctx.select_proposer(&vset, EdetHeight::new(10), Round::new(2));
        let b = ctx.select_proposer(&vset, EdetHeight::new(10), Round::new(2));
        assert_eq!(a.address, b.address);
    }

    // --- the paper's quorum IS Malachite's threshold -------------------

    /// The quorum the paper and this crate's own prose state throughout:
    /// strictly more than two thirds of total validator POWER.
    fn stated_quorum_power(total: u64) -> u64 {
        (2 * total) / 3 + 1
    }

    #[test]
    fn the_stated_quorum_matches_malachite_threshold_params() {
        // the equivalence note, as a test rather than prose. Everything edet
        // says about safety is stated in terms of ⌊2·total/3⌋+1, but nothing
        // in this crate computes that at runtime any more: certificates are
        // verified by Malachite's default `ThresholdParams`
        // (`ThresholdParam::TWO_F_PLUS_ONE`, used by
        // `SigningProviderExt::verify_commit_certificate`). If the two ever
        // disagreed, every claim edet makes about what a quorum means would
        // be about a threshold the engine does not enforce.
        let quorum = ThresholdParam::TWO_F_PLUS_ONE;
        for total in 1..=200u64 {
            assert_eq!(
                stated_quorum_power(total),
                quorum.min_expected(total),
                "min_expected diverges from the stated quorum at total={total}"
            );
            // And the two formulations of "is a quorum met" agree pointwise.
            let threshold = stated_quorum_power(total);
            for signed in 0..=total {
                assert_eq!(
                    signed >= threshold,
                    quorum.is_met(signed, total),
                    "quorum verdict diverges at signed={signed} total={total}"
                );
            }
        }
    }

    // --- from_state matches dev_genesis's 5 seeded validators ----------

    #[test]
    fn validator_set_from_state_matches_dev_genesis_seeding() {
        let state = seeded_state(5);
        let vset = EdetValidatorSet::from_state(&state).expect("seeded genesis state always builds");

        assert_eq!(vset.count(), 5);
        assert_eq!(vset.total_voting_power(), 5);

        for i in 0..5u64 {
            // The CONSENSUS key: a validator signs votes with a key that lives
            // on its host, never with the member key that signs its obligations.
            let expected_key = pubkey_of(&crate::block::dev_consensus_seed(i as u8));
            let v = vset
                .get_by_address(&EdetAddress(i))
                .unwrap_or_else(|| panic!("missing validator {i}"));
            assert_eq!(v.power, 1);
            assert_eq!(v.public_key.to_bytes(), expected_key);
        }
    }

    /// `build` must honor an explicit (e.g. historical) power map
    /// instead of always reading `state.validators` — this is exactly what
    /// lets `engine_malachite` reconstruct the validator set live at a past
    /// height (`Replica::validators_at`) rather than the present one.
    #[test]
    fn build_uses_the_given_power_map_not_current_state_validators() {
        let state = seeded_state(5);
        // A historical set: only validators 0 and 1 were live (e.g. before
        // validators 2-4 were ever registered).
        let historical: std::collections::BTreeMap<MemberId, u64> = [(0u64, 1u64), (1u64, 1u64)].into_iter().collect();

        let vset = EdetValidatorSet::build(&state, &historical, &std::collections::BTreeMap::new())
            .expect("both historical members are real and keyed");
        assert_eq!(vset.count(), 2, "must reflect the passed-in map, not state.validators' full 5");
        assert!(vset.get_by_address(&EdetAddress(2)).is_none(), "validator 2 was not live in the historical map");
        assert_eq!(
            vset.get_by_address(&EdetAddress(0)).unwrap().public_key.to_bytes(),
            pubkey_of(&crate::block::dev_consensus_seed(0))
        );
    }

    // --- the voting-power sum is a u64 and quorum is a fraction of it --

    /// The set the consensus half must never construct: two validators whose
    /// powers are individually representable but whose SUM wraps. A wrapped
    /// total is not a crash — it is a small number that every
    /// `ThresholdParams` fraction is then computed against, i.e. quorum met
    /// by a rounding error. `build` refuses it instead of returning a set.
    #[test]
    fn build_refuses_a_validator_set_whose_total_power_would_overflow_a_u64() {
        let state = seeded_state(2);
        let overflowing: std::collections::BTreeMap<MemberId, u64> =
            [(0u64, u64::MAX), (1u64, 1u64)].into_iter().collect();
        let err = EdetValidatorSet::build(&state, &overflowing, &std::collections::BTreeMap::new())
            .expect_err("a set whose total wraps must never be built");
        // Caught at the per-validator ceiling first, which is the tighter of
        // the two gates — either refusal is correct, neither may construct.
        assert!(
            matches!(err, ValidatorSetError::PowerTooLarge(0) | ValidatorSetError::TotalPowerOverflow),
            "unexpected error: {err}"
        );
    }

    /// The per-validator ceiling is the ledger's own (`k::MAX_VALIDATOR_POWER`),
    /// not a second number this file invented: a genesis file this node did
    /// not author is held to exactly what a governed `ValidatorPower` change
    /// would be held to.
    #[test]
    fn build_refuses_a_validator_powered_above_the_ledgers_own_ceiling() {
        let state = seeded_state(2);
        let too_big: std::collections::BTreeMap<MemberId, u64> =
            [(0u64, edet_kernel::constants::MAX_VALIDATOR_POWER + 1), (1u64, 1)]
                .into_iter()
                .collect();
        assert_eq!(
            EdetValidatorSet::build(&state, &too_big, &std::collections::BTreeMap::new()).expect_err("must refuse"),
            ValidatorSetError::PowerTooLarge(0)
        );

        // And exactly at the ceiling it builds — the bound is the ledger's,
        // applied identically, not one off it.
        let at_bound: std::collections::BTreeMap<MemberId, u64> =
            [(0u64, edet_kernel::constants::MAX_VALIDATOR_POWER), (1u64, 1)]
                .into_iter()
                .collect();
        let vset = EdetValidatorSet::build(&state, &at_bound, &std::collections::BTreeMap::new()).expect("builds");
        assert_eq!(vset.total_voting_power(), edet_kernel::constants::MAX_VALIDATOR_POWER + 1);
    }

    /// The backstop behind `build`: a set handed to `new` directly (no
    /// ledger, no genesis) still cannot produce a WRAPPED total. Wrapping
    /// here would report a total of 4 for a set holding `u64::MAX + 5` —
    /// below the power of its own second validator, so a single 5-power
    /// validator would satisfy every threshold computed against it.
    #[test]
    fn total_voting_power_saturates_rather_than_wrapping() {
        let key = ed25519_dalek::VerifyingKey::from_bytes(&pubkey_of(&dev_seed(0))).expect("dev key parses");
        let vset = EdetValidatorSet::new(vec![
            EdetValidator { address: EdetAddress(0), public_key: key, power: u64::MAX },
            EdetValidator { address: EdetAddress(1), public_key: key, power: 5 },
        ]);
        assert_eq!(vset.total_voting_power(), u64::MAX, "the sum must saturate, never wrap to 4");
    }

    #[test]
    fn signing_scheme_signature_decode_round_trips_and_rejects_bad_length() {
        let sig = EdetSignature([7u8; 64]);
        let encoded = EdetSigningScheme::encode_signature(&sig);
        let decoded = EdetSigningScheme::decode_signature(&encoded).unwrap();
        assert_eq!(sig, decoded);

        assert!(EdetSigningScheme::decode_signature(&[0u8; 10]).is_err());
    }
}
