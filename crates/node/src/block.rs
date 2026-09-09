//! Blocks: the unit of consensus. A block is an ordered batch of signed
//! transactions with the proposer's timestamp; applying the same block
//! stream to the same genesis yields bit-identical state on every replica.

use edet_state::types::Key;
use edet_state::{State, Tx};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedTx {
    pub tx: Tx,
    /// Client-chosen: makes an otherwise byte-identical transaction distinct
    /// from any other one, including a prior submission of the exact same
    /// `tx`. This is what the replay defence keys on — `tx` alone carries no
    /// uniqueness at all, so two submissions of the same content are
    /// indistinguishable without it. Picked by the signing client (a wallet
    /// generates it fresh per submission); never derived from `tx`, or two
    /// genuinely distinct intents to do the same thing would collide.
    pub nonce: [u8; 16],
    /// The last epoch this transaction may still be applied in (inclusive).
    /// Bounds how long the replay cache
    /// (`edet_state::State::applied_by_expiry`) has to remember this `nonce` — see `edet_kernel::constants::
    /// MAX_TX_LIFETIME_EPOCHS` for the ceiling `apply` enforces on this
    /// field, and `edet_state::apply`'s doc comment for why a rejected
    /// transaction is recorded here too, not just a successful one.
    pub not_after_epoch: u64,
    /// Keys whose signatures the driver verified over `tx`. By the time a
    /// block is applied they have been checked, and replay carries the
    /// verified claim.
    pub signers: Vec<Key>,
    /// Ed25519 signatures over `id(chain_id)`, aligned with `signers`.
    /// Checked at the ingress (`serve::submit`); absent (empty) inside
    /// blocks replayed from the store, where `signers` is the verified
    /// claim. `serde(default)` keeps old stored blocks decodable.
    #[serde(default)]
    pub signatures: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub height: u64,
    /// Consensus time for the block (drives epochs).
    pub time_secs: u64,
    /// The state commitment (`state_hash`) of the state this replica held
    /// immediately BEFORE this block was applied — i.e. the hash left behind
    /// by the PARENT block, never a claim about what this block itself
    /// produces.
    ///
    /// "Before", not "after this block", is the only direction that can
    /// work, and it's worth spelling out why: a validator asked to vote on a
    /// proposal for height N does not yet have the state height N would
    /// produce — computing that requires actually applying the very
    /// transactions the vote is about, so a block can never verifiably
    /// commit to its own result before it exists (that would need a second
    /// round just to reveal the claim, the shape real two-phase designs use
    /// instead). What every honest validator DOES already hold,
    /// deterministically, the instant a proposal for height N arrives, is
    /// the state left behind by height N-1. So that's the only claim
    /// `screen` can check before a vote, and the only claim `commit_block`
    /// can check against nothing but its own locally-derived state — this is
    /// exactly the CometBFT `AppHash` convention, for the same reason.
    ///
    /// Two replicas that ever disagree about the value this field should
    /// hold have already diverged — silently, before this field existed
    /// (the finding). Checking it turns that into an immediate, loud vote
    /// refusal (`engine_malachite::screen`) and a commit-path halt
    /// (`Replica::commit_block`), instead of two
    /// nodes quietly signing certificates for each other's blocks while
    /// serving different balances to different users.
    pub app_hash: [u8; 32],
    pub txs: Vec<SignedTx>,
}

/// A proposer may be ahead of our own clock by this much before we refuse to
/// vote for its block. Bounds the OTHER direction from
/// `time_is_monotonic`: monotonicity alone stops a rewind, but nothing stops
/// a proposer racing arbitrarily far AHEAD — and `State::begin_block` derives
/// the epoch to advance to directly from `time_secs`, so an unbounded future
/// timestamp is an unbounded amount of epoch-closing work (see
/// `edet_kernel::constants::MAX_EPOCH_ADVANCE_PER_BLOCK`, the structural
/// backstop for when this rule is bypassed). Ten minutes comfortably covers
/// clock drift between honest, NTP-synced validators while remaining useless
/// as a lever for meaningfully accelerating epoch advance.
pub const MAX_FUTURE_SKEW_SECS: u64 = 600;

/// Deterministic half of the timestamp rule: time must never rewind.
///
/// Uses `>=`, not `>`: block time is whole seconds and a lively cluster can
/// decide more than one block in the same second, so strict inequality would
/// stall it the moment two blocks land in the same wall-clock second.
/// Non-decreasing is all that's needed to stop a rewind — the epoch
/// derivation in `State::begin_block` only ever cares about the floor
/// division, which a same-second block leaves unchanged. Safe to run on the
/// commit path: it is a pure function of the block stream, consulting
/// nothing local.
pub fn time_is_monotonic(block_time: u64, parent_time: u64) -> bool {
    block_time >= parent_time
}

/// Local half of the timestamp rule: the proposer must not be implausibly far
/// in OUR future. NOT safe on the commit path — see `Replica::commit_block`'s
/// doc comment for why consulting a local wall clock there would let two
/// nodes with different clocks reach different conclusions about the same
/// block and fork the ledger. Belongs only in a vote (a local decision):
/// the pre-vote screen (`engine_malachite::screen`).
///
/// Correct on the sync path too: a genuinely old (already-decided,
/// historical) block carries a small timestamp, so it always passes an upper
/// bound — only a timestamp actually ahead of the checking node's clock can
/// ever fail this.
pub fn time_is_plausible(block_time: u64, now_secs: u64) -> bool {
    block_time <= now_secs.saturating_add(MAX_FUTURE_SKEW_SECS)
}

#[derive(Debug)]
pub enum CodecError {
    Encode(String),
    Decode(String),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecError::Encode(e) => write!(f, "encode: {e}"),
            CodecError::Decode(e) => write!(f, "decode: {e}"),
        }
    }
}
impl std::error::Error for CodecError {}

/// Domain tag prefixed onto every transaction's signing payload. A
/// per-type tag removes any question of a signed payload of one kind being
/// reinterpreted as another — `EdetVote`/`EdetProposal`/`EdetProposalPart`
/// (`engine_context::sign_bytes`) each carry a DIFFERENT tag from this one
/// and from each other, so a signature over a transaction can never be
/// replayed as a signature over a consensus message, or vice versa, however
/// their consensus encodings happen to line up.
pub const TX_DOMAIN: &[u8] = b"edet-tx-v1";

/// The default dev/test chain id — mirrors `State::default()`'s own value
/// (`crates/state/src/state.rs`) so a dev harness that has not loaded a real
/// genesis signs against the same chain a fresh `State::default()` actually
/// carries.
pub const DEV_CHAIN_ID: &str = "edet-dev";

/// The most transactions a block may carry, as a rule every node applies to
/// every block.
///
/// The proposer batches at this figure, and the batch alone bounds nothing: the
/// reassembly buffer allows 4 MiB per proposal — tens of thousands of
/// transactions. A Byzantine proposer, inside the `f` the design tolerates,
/// hands every honest validator a block whose application costs minutes: the bond gate runs a `seed_reach` max-flow per signer before
/// dispatch, so the cost is paid per envelope whatever each envelope then does.
///
/// Equal to the proposer's own batch, so an honest node can never build a block
/// this refuses. Raising it is a decision about what one round may cost; the
/// two numbers move together or the second is dead.
pub const MAX_TXS_PER_BLOCK: usize = 512;

/// Why a block is refused by the rules every node can check deterministically,
/// before applying a single transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockRefusal {
    /// More transactions than `MAX_TXS_PER_BLOCK`.
    TooManyTxs(usize),
    /// An envelope at this index carries no signature the transition requires,
    /// so it is CERTAIN to fail at dispatch — and to fail refunded, unbilled
    /// and re-appliable (`edet_state::authorises`).
    Unauthorised(usize),
}

impl std::fmt::Display for BlockRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockRefusal::TooManyTxs(n) => {
                write!(f, "block carries {n} transactions, above the {MAX_TXS_PER_BLOCK} a block may hold")
            }
            BlockRefusal::Unauthorised(i) => {
                write!(f, "transaction {i} is signed by nobody the transition requires, so it can only fail")
            }
        }
    }
}

/// **The block-validity rules that are a pure function of committed state**,
/// in one place because they are applied in three (the pre-vote screen, the
/// sync path, and the commit path) and three copies of a validity rule is a
/// fork waiting for a disagreement.
///
/// Both rules exist to keep work a block can force onto every honest validator
/// bounded by something somebody paid for. The count rule bounds the batch; the
/// authorisation rule removes the envelopes that pay NOTHING — an envelope
/// refused with `ET-MEM-NOT_SIGNER` has its bond refunded and its id forgotten,
/// which is right (otherwise stripping a co-signature would be a free way to
/// kill somebody else's transaction for ever) and leaves a transaction that
/// costs its sender nothing and every validator a max-flow per signer.
///
/// Safe as a validity rule because every voter runs it against the same parent
/// state: the screen already refuses a block whose `app_hash` does not match
/// the state this node holds.
pub fn deterministic_validity(block: &Block, state: &State) -> Result<(), BlockRefusal> {
    if block.txs.len() > MAX_TXS_PER_BLOCK {
        return Err(BlockRefusal::TooManyTxs(block.txs.len()));
    }
    for (i, stx) in block.txs.iter().enumerate() {
        if !edet_state::authorises(state, &stx.tx, &stx.signers) {
            return Err(BlockRefusal::Unauthorised(i));
        }
    }
    Ok(())
}

/// Deterministic nonce for callers that only need "distinct from the last
/// one" — test fixtures, demos, the CLI's `seed-tx` helper — never for a
/// nonce whose UNPREDICTABILITY matters (a real wallet must draw its nonce
/// from a CSPRNG, not this). The input's little-endian bytes, zero-padded to
/// 16; exists purely so callers that need uniqueness, not secrecy, have
/// something other than randomness to reach for (the test-determinism rule:
/// no `#[cfg(test)]` code in this codebase may depend on an RNG for
/// correctness).
pub fn counter_nonce(n: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&n.to_le_bytes());
    out
}

/// The signing payload for a transaction envelope: SHA-256 over
/// `TX_DOMAIN || len(chain_id) as u64 LE || chain_id || codec::encode(tx, nonce,
/// not_after_epoch)`. The chain id binds a signature to ONE network
/// — the dev/testnet genesis seeds every founder from published entropy
/// (`dev_seed`), so without this a transaction signed against a testnet
/// would be byte-identical to (and valid on) any other edet chain sharing
/// that founder's identity. The length prefix on `chain_id` is not optional:
/// without it, `chain_id = "ab"` followed by one payload can collide with
/// `chain_id = "a"` followed by a different payload that happens to start
/// with `"b"` — the consensus encoding is not self-delimiting against a
/// raw byte concatenation, so the length has to be carried explicitly.
/// **The wallet computes this itself** (`ui/src/lib/txdigest.ts`), and the two
/// implementations are cross-pinned by generated vectors (`just
/// tx-digest-check`, from `examples/tx_digest_fixture.rs`).
///
/// A wallet that fetched it from its node over `/tx/digest` would be signing
/// on the node's word: the app embeds no node, it reads one the member CHOOSES,
/// and a node reached through the client's `custom` entry can answer with the
/// digest of a different envelope and collect a valid signature over it. The
/// route exists for the e2e harnesses in `ui/scripts/`, which hold no member's
/// seed; no wallet may sign what it returns.
///
/// Verification (`SignedTx::verify`) recomputes it from the submitted
/// envelope, so a signature over any other bytes is refused at submit.
pub fn tx_digest(chain_id: &str, tx: &Tx, nonce: &[u8; 16], not_after_epoch: u64) -> Result<[u8; 32], CodecError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(TX_DOMAIN);
    bytes.extend_from_slice(&(chain_id.len() as u64).to_le_bytes());
    bytes.extend_from_slice(chain_id.as_bytes());
    let payload =
        edet_state::codec::encode(&(tx, nonce, not_after_epoch)).map_err(|e| CodecError::Encode(e.to_string()))?;
    bytes.extend_from_slice(&payload);
    Ok(sha256(&bytes))
}

/// True for the transactions deliberately designed to be permissionless
/// cranks: anyone may submit them with no signer at all, and `SignedTx::verify`
/// passes an empty signer/signature pair trivially for exactly this reason.
/// Every other transaction kind MUST name and prove at least one signer — a
/// non-crank transaction with an empty signer set carries no accountability
/// whatsoever. Enforced by `SignedTx::is_authenticated` (and so by
/// `Block::verify_txs`) as well as at the ingress (`serve::driver::submit`);
/// this itself stays a pure classification.
pub fn is_permissionless(tx: &Tx) -> bool {
    // One list, owned by the ledger: the transitions that may arrive unsigned
    // are exactly the ones whose refusal records nothing (`apply` forgets a
    // refused crank's id). Two lists drifted once — `RotateFinalize` was
    // admitted here without a signer and kept its replay id there on every
    // refusal, a keyless durable write anyone could repeat.
    edet_state::bond::is_permissionless(tx)
}

impl SignedTx {
    /// Content hash, for mempool dedup and gossip. Distinct from `id`: this
    /// covers the WHOLE wire envelope (including `signers`/`signatures`), so
    /// it changes if the signature set changes — exactly what mempool dedup
    /// wants (don't re-gossip bytes we've already seen). `id` covers only
    /// what the signer actually signed over (`tx`, `nonce`, `not_after_epoch`
    /// under the chain id) and is what the replay cache keys on — the same
    /// signed intent submitted twice must produce the same `id` regardless of
    /// how its signature bytes happen to be encoded (Ed25519 signing is not
    /// generally deterministic byte-for-byte across implementations), and
    /// must NOT change if the same intent is later assembled with signatures
    /// collected in a different order.
    pub fn hash(&self) -> Result<[u8; 32], CodecError> {
        let bytes = edet_state::codec::encode(self).map_err(|e| CodecError::Encode(e.to_string()))?;
        Ok(sha256(&bytes))
    }

    /// The envelope id: the exact digest this transaction's signatures cover
    /// (`tx_digest`). This IS the `tx_id` passed to `edet_state::apply` — the
    /// replay cache is keyed on it, not on `hash()` (see that method's doc
    /// comment for why the two must differ).
    pub fn id(&self, chain_id: &str) -> Result<[u8; 32], CodecError> {
        tx_digest(chain_id, &self.tx, &self.nonce, self.not_after_epoch)
    }

    /// The full authentication rule for a transaction arriving from anywhere
    /// untrusted — a client submission OR a peer validator's proposed block:
    /// every claimed signer really signed it (`verify`), AND the signer set is
    /// non-empty unless this is a permissionless crank.
    ///
    /// `verify` alone is deliberately weaker: it passes an empty signer list
    /// trivially, because there is nothing to check. That makes the empty-signer rule
    /// live only at the ingress (`serve::driver::submit`) while the consensus
    /// gate (`Block::verify_txs`) was blind to it — one rule in two places
    /// with two different strengths. This is the single rule both use.
    pub fn is_authenticated(&self, chain_id: &str) -> bool {
        if self.signers.is_empty() && !is_permissionless(&self.tx) {
            return false;
        }
        self.verify(chain_id)
    }

    /// Verify every claimed signer actually signed this transaction
    /// envelope's digest (`id`, bound to `chain_id`): `signatures[i]`
    /// must be a valid Ed25519 signature by `signers[i]`. An empty signer
    /// list verifies trivially — see `is_authenticated`, which is what
    /// untrusted input must be held to.
    pub fn verify(&self, chain_id: &str) -> bool {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        if self.signers.len() != self.signatures.len() {
            return false;
        }
        let Ok(digest) = self.id(chain_id) else { return false };
        self.signers.iter().zip(&self.signatures).all(|(key, sig)| {
            let Ok(vk) = VerifyingKey::from_bytes(key) else { return false };
            let Ok(sig) = Signature::from_slice(sig) else { return false };
            vk.verify(&digest, &sig).is_ok()
        })
    }
}

/// Dev-cluster founder entropy: `[i+1; 16]`, the published harness
/// convention behind each founder's BIP39 recovery phrase.
pub fn dev_entropy(i: u8) -> [u8; 16] {
    [i.wrapping_add(1); 16]
}

/// Founder `i`'s 12-word recovery phrase. Printed by the dev launchers so
/// a first-run user restores a founder exactly like any real identity —
/// there is no in-app impersonation path.
pub fn dev_phrase(i: u8) -> String {
    bip39::Mnemonic::from_entropy(&dev_entropy(i))
        .expect("16-byte entropy is valid")
        .to_string()
}

/// Founder `i`'s Ed25519 seed: the first 32 bytes of the BIP39 seed of
/// their dev phrase (empty passphrase) — the same derivation the client's
/// restore-from-phrase flow performs.
pub fn dev_seed(i: u8) -> [u8; 32] {
    let mnemonic = bip39::Mnemonic::from_entropy(&dev_entropy(i)).expect("16-byte entropy is valid");
    let seed = mnemonic.to_seed("");
    let mut out = [0u8; 32];
    out.copy_from_slice(&seed[..32]);
    out
}

/// Dev validator `i`'s CONSENSUS seed — the key its engine signs votes with,
/// and never the key its member row signs obligations with.
///
/// Not a BIP39 phrase, deliberately: a consensus key is never restored into a
/// wallet, and giving it one would invite exactly the mistake the separation
/// exists to prevent. Derived from its own domain string so it can never
/// collide with a `dev_seed`, and published on the same terms — `is_dev_key`
/// refuses both on any chain that is not the dev chain.
pub fn dev_consensus_seed(i: u8) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"edet-dev-consensus-v1");
    h.update([i]);
    h.finalize().into()
}

/// Public key for a 32-byte Ed25519 seed.
pub fn pubkey_of(seed: &[u8; 32]) -> Key {
    use ed25519_dalek::SigningKey;
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

/// Sign a transaction with the given seeds (test/dev helper): produces a
/// full envelope (`nonce`, `not_after_epoch`, `signers`, `signatures`) that
/// `verify` accepts under `chain_id`.
pub fn sign_tx(
    chain_id: &str,
    tx: Tx,
    nonce: [u8; 16],
    not_after_epoch: u64,
    seeds: &[[u8; 32]],
) -> Result<SignedTx, CodecError> {
    use ed25519_dalek::{Signer, SigningKey};
    let digest = tx_digest(chain_id, &tx, &nonce, not_after_epoch)?;
    let mut signers = Vec::with_capacity(seeds.len());
    let mut signatures = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let sk = SigningKey::from_bytes(seed);
        signers.push(sk.verifying_key().to_bytes());
        signatures.push(sk.sign(&digest).to_bytes().to_vec());
    }
    Ok(SignedTx { tx, nonce, not_after_epoch, signers, signatures })
}

impl Block {
    /// True iff every transaction in the block is authenticated to the same
    /// standard the ingress applies (`SignedTx::is_authenticated`: real
    /// signatures from every claimed signer, and no empty signer set outside
    /// the permissionless cranks).
    ///
    /// A validator must check this on any block it did not build itself —
    /// blocks it builds come from `mempool`, already ingress-verified — BEFORE
    /// VOTING FOR IT, not merely before committing it. Consensus agrees on
    /// ORDER, not on the authenticity of what's inside, so a validator that
    /// votes for a block its own commit path will refuse ends up signing a
    /// quorum certificate for a value that never becomes the ledger — see
    /// `engine_malachite::screen` for what that costs. The engine screens at
    /// exactly that point (`engine_malachite::reassemble_proposal_part`), and
    /// the commit path (`Replica::commit_block`) re-checks as the backstop.
    pub fn verify_txs(&self, chain_id: &str) -> bool {
        self.txs.iter().all(|stx| stx.is_authenticated(chain_id))
    }

    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        edet_state::codec::encode(self).map_err(|e| CodecError::Encode(e.to_string()))
    }

    pub fn decode(bytes: &[u8]) -> Result<Block, CodecError> {
        edet_state::codec::decode(bytes).map_err(|e| CodecError::Decode(e.to_string()))
    }

    pub fn hash(&self) -> Result<[u8; 32], CodecError> {
        Ok(sha256(&self.encode()?))
    }
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

/// The state commitment: the Merkle state root
/// (`edet_state::root::state_root`, spec in
/// the paper's §Implementation). Every replica at the same height
/// must produce the same value, and it is what a signed commit certifies.
///
/// It is not `sha256(codec::encode(state))`. That answers the agreement
/// question and nothing else. The root answers the same question at the same
/// cost per block, and additionally supports proving ONE record to somebody
/// holding neither the ledger nor any secret — which is what makes it worth
/// anchoring publicly, and why it is the same 32 bytes in the same
/// place rather than a second commitment alongside: two roots, one certified
/// by the validators and one published, would be two things that can
/// disagree.
pub fn state_hash(state: &edet_state::State) -> Result<[u8; 32], CodecError> {
    edet_state::root::state_root(state).map_err(|e| CodecError::Encode(e.0))
}

pub fn hex32(h: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in h {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod permissionless_tests {
    use super::*;
    use edet_state::types::{Party, ProposalKind};

    /// **One list.** The transitions the ingress admits without a signer and
    /// the transitions whose refusal the ledger forgets are the same function,
    /// so the two cannot drift apart — `RotateFinalize` was on one list and
    /// not the other, and a keyless refusal was a durable write.
    #[test]
    fn the_ingress_and_the_ledger_agree_on_which_transitions_arrive_unsigned() {
        let every = vec![
            Tx::RegisterGuardians { member: 0, guardians: vec![], threshold: 1, veto_window_epochs: 1 },
            Tx::RotateRequest { member: 0, new_keys: vec![] },
            Tx::RotateVeto { member: 0 },
            Tx::RotateFinalize { member: 0 },
            Tx::SetConsensusKey { member: 0, key: None },
            Tx::DeclareSupply { member: 0, supply: 1.0 },
            Tx::Exit { member: 0 },
            Tx::ListBeneficiaries { supporter: 0, entries: vec![] },
            Tx::ApproveSupporter { beneficiary: 0, supporter: 1, approved: true },
            Tx::Sale { seller: Party::Member(0), buyer: Party::Member(1), amount: 1.0, maturity_epochs: 1 },
            Tx::Accept {
                debtor: Party::Member(0),
                creditor: Party::Member(1),
                amount: 1.0,
                maturity_epochs: 1,
                arb: None,
            },
            Tx::Transfer { contract: 0, new_debtor: 1 },
            Tx::Settle { contract: 0, amount: 1.0 },
            Tx::Extend { contract: 0, new_maturity_epoch: 1 },
            Tx::MarkExpired { contract: 0 },
            Tx::Cure { contract: 0, amount: 1.0 },
            Tx::ArbAttest { contract: 0, arbiter: 0, amount: 1.0 },
            Tx::Propose { author: 0, kind: ProposalKind::Suspend { member: 1 } },
            Tx::Assent { member: 0, proposal: 0 },
            Tx::ForfeitBonds { member: 0 },
        ];
        let mut cranks = 0;
        for tx in every {
            assert_eq!(is_permissionless(&tx), edet_state::bond::is_permissionless(&tx), "{tx:?}");
            cranks += usize::from(is_permissionless(&tx));
        }
        assert_eq!(cranks, 3, "MarkExpired, ForfeitBonds and RotateFinalize, and nothing else");
    }
}
