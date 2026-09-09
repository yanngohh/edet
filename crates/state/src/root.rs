//! The state root: a Merkle commitment to the whole ledger, from which a
//! single record can be proved to somebody who holds neither the ledger nor
//! any secret.
//!
//! Spec: the paper's §Implementation, deciding the paper
//! §Implementation D6.
//!
//! It is not `sha256(codec::encode(State))`. That hash answers one question —
//! "do we hold the same ledger" between nodes running the same binary — and
//! is unfit as a public commitment three ways: it commits to a serialization
//! rather than to the ledger (so an unrelated field added to `State` moves the
//! hash of an unchanged ledger), verifying it needs the entire state handed
//! over including everything §Standing of the reads spec exists to keep
//! private, and it can prove nothing about any single record.
//!
//! What is here is the same 32 bytes in the same place — `Replica::app_hash`,
//! every block, every vote — so the root a validator set certifies IS the
//! root that gets anchored. Two commitments, one certified and one published,
//! would be two things that can disagree.
//!
//! **The tree is incremental, and the leaf salt is what makes it so.** A
//! salt binds the EPOCH (`salt_marker`), so within an epoch an unchanged
//! record keeps its leaf hash and an ordinary block rehashes only the rows it
//! wrote and their paths ([`RootCache`]); at the boundary every salt changes
//! and the whole tree is rebuilt, on the block that already rewrites every
//! member row and decays every stake. A section's leaf COUNT is bound once,
//! at the section root, rather than inside every leaf, so seating a row does
//! not rehash its section. What that discloses is one bit per epoch about the
//! records adjacent to one's own in id order, to the subject of a proof and
//! to nobody else — [`leaf_salt`] states it exactly.
//!
//! [`state_root`] is the DEFINITION: a pure function of a `State`, consulting
//! no cache. [`RootCache::refresh`] is what a validator runs, and the state
//! harness holds the two equal after every transition in the tree, exactly as
//! `invariants::audit` holds `audit_with_cache`.

use std::collections::{BTreeMap, BTreeSet};

use edet_kernel::constants as k;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::journal::Dirty;
use crate::params::Params;
use crate::state::State;
use crate::types::{Contract, ContractId, Member, MemberId, Proposal, ProposalId};

/// Salt derivation. Bumping this invalidates every previously published
/// root, which is why it is versioned rather than implicit.
const SALT_DOMAIN: &[u8] = b"edet-leaf-salt-v4";
/// Leaf preimage. See the paper's §Implementation: the record
/// encoding inside a leaf is still [`crate::codec`]'s, so this tag is what makes a
/// future encoding change a stateable migration instead of roots that
/// quietly stop matching.
const LEAF_DOMAIN: &[u8] = b"edet-leaf-v3";
/// The section root's preimage, which is where a section's leaf COUNT is
/// bound. Its own domain, because it is a third kind of node in the same
/// tree and the RFC 6962 argument that separates a leaf from an inner node
/// separates this from both.
const SECTION_DOMAIN: &[u8] = b"edet-section-v1";
/// An empty section still needs a hash, and it must not be a value any real
/// node could take — hence a domain string rather than zeroes.
const EMPTY_DOMAIN: &[u8] = b"edet-empty-section-v1";

/// RFC 6962 leaf/inner separation, extended to the section root. Without it
/// an inner node's preimage can be presented as a leaf, which is a known
/// inclusion-proof forgery.
const LEAF_PREFIX: u8 = 0x00;
const INNER_PREFIX: u8 = 0x01;
const SECTION_PREFIX: u8 = 0x02;

/// A record's [`crate::codec`] encoding failed, or the state carries a replay
/// bucket outside the window [`Section::Replay`] is defined over. The first is
/// only reachable for a value that cannot be serialized at all (`f64::NAN`
/// keys and the like are not a case this state machine can produce); the
/// second is made unreachable by `apply` (`ET_TX_EXPIRED`,
/// `ET_TX_WINDOW_TOO_LONG`) and `State::begin_block`'s pruning. Both are
/// propagated rather than papered over: a root that silently skipped a record
/// would be a root that commits to less than the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootError(pub String);

impl std::fmt::Display for RootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "state root: {}", self.0)
    }
}

impl std::error::Error for RootError {}

/// The seven trees the top root is built from. The order of `ALL` is part of
/// the format: it fixes each section's position in the top tree, so it may
/// not be reordered without bumping `LEAF_DOMAIN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Section {
    Members,
    Contracts,
    Proposals,
    Validators,
    /// The stake graph, one leaf per account: that creditor's out-edges with
    /// what outstanding credit reserves out of each. A row of its own rather
    /// than a slice of one blob, so a settlement rehashes one creditor
    /// instead of every stake ever written.
    Stakes,
    /// The replay cache, one leaf per expiry epoch across the whole live
    /// window. A recorded id rehashes the epoch it expires in and nothing
    /// else.
    Replay,
    /// What is left of `State`: scalars, the governed parameters, and the
    /// three `O(U)` maps. One leaf, re-encoded on every block because
    /// `last_begin_secs` moves on every block that did work. See `LedgerLeaf`.
    Ledger,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::Members,
        Section::Contracts,
        Section::Proposals,
        Section::Validators,
        Section::Stakes,
        Section::Replay,
        Section::Ledger,
    ];

    /// The byte that names this section inside every preimage. Assigned
    /// explicitly rather than derived from declaration order, so reordering
    /// the enum cannot silently renumber the sections — and fixed-width, so
    /// no section's identity can bleed into the bytes after it. A
    /// variable-length name would make the preimage's safety rest on no
    /// name being a prefix of another, which is a property of today's
    /// vocabulary rather than of the construction.
    ///
    /// Three tags differ from their position, deliberately: a verifier that
    /// derives one from the other passes every members-section fixture and
    /// fails only here, which is what `proof_fixture` exists to catch.
    pub fn tag(self) -> u8 {
        match self {
            Section::Members => 0,
            Section::Contracts => 1,
            Section::Proposals => 2,
            Section::Validators => 3,
            Section::Stakes => 5,
            Section::Replay => 6,
            Section::Ledger => 4,
        }
    }

    /// This section's leaf position in the top tree — its index in `ALL`,
    /// never its tag.
    pub fn position(self) -> usize {
        Section::ALL.iter().position(|&x| x == self).expect("every Section is in ALL")
    }
}

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn encode<T: Serialize>(what: &str, value: &T) -> Result<Vec<u8>, RootError> {
    crate::codec::encode(value).map_err(|e| RootError(format!("encoding {what}: {e}")))
}

/// The length prefix that frames every variable-length preimage component.
/// Framing is what makes the preimages injective: with each of key and value
/// preceded by its length, one byte string parses as exactly one
/// (key, value) pair, so no bytes can slide across the boundary between
/// them. Big-endian, matching `id_key`.
fn be_len(bytes: &[u8]) -> [u8; 8] {
    (bytes.len() as u64).to_be_bytes()
}

/// The block marker every leaf salt binds: the EPOCH.
///
/// One function, because it is the knob. Binding `last_begin_secs` instead
/// would re-salt every leaf on every block that did work, which is a full
/// rebuild per block and exactly what the tree costs without a cache; binding
/// nothing at all would leave an unchanged record recognisable for ever. The
/// epoch is the boundary that already rewrites every member row and decays
/// every stake, so the rebuild it forces costs nothing that block is not
/// already paying, and it bounds the disclosure window to one epoch.
pub fn salt_marker(state: &State) -> u64 {
    state.epoch
}

/// The salt for one leaf, derived from the chain's own `root_salt`.
///
/// Derived rather than used directly so that proving a leaf can disclose
/// THAT leaf's salt — which a verifier needs, since it recomputes the leaf —
/// without disclosing `root_salt`, which would let them brute-force every
/// sibling hash on the path and read the neighbouring records
/// (the paper's §Implementation).
///
/// The derivation binds the record's VALUE and the epoch marker
/// (`salt_marker`), not just its key, so a disclosed salt is the unblinding
/// key for exactly one epoch's value of one record. A salt derived from the
/// key alone would hand whoever once saw a proof for a leaf the means to
/// brute-force that leaf's value inside every later proof carrying it as a
/// sibling — record values live in small spaces, and the key's one permanent
/// salt would already be in their hands. With the value bound in, a changed
/// value has a salt no outsider can derive.
///
/// **What binding the epoch, rather than the block, discloses.** Within one
/// epoch an unchanged record keeps its leaf hash, so a member holding two of
/// their own proofs from that epoch learns, from the sibling hashes, whether
/// the record paired with theirs in id order changed between the two heights,
/// and at each level up whether a block of two, four, eight neighbours did.
/// Across epochs nothing links. A record's proof is served only to its
/// subject or to a validator, who holds the ledger anyway. What this does not
/// bound is resolution inside the window — a member may ask for a proof every
/// block — and a wider window would bound nothing an active prober cannot
/// already learn inside one. What it buys is the tree: an ordinary block
/// hashes the rows it wrote and their paths instead of the whole ledger.
///
/// `root_salt` remains the single per-chain secret, and its boundary is the
/// one the salt exists for: secret from anyone who was never given the ledger.
/// Whoever holds (or once held) the ledger holds `root_salt` and can derive
/// any salt directly — no derivation can defend against them, and none
/// tries.
///
/// The value enters as its DIGEST rather than in full, which binds exactly the
/// same thing under collision resistance — two values with one digest is a
/// SHA-256 collision — and hashes a large value once instead of twice.
///
/// The key is still framed by its length, for the reason `leaf_hash` frames
/// both: the digest is fixed-width and cannot slide, but a key that was not
/// framed could run into it.
pub fn leaf_salt(root_salt: &[u8; 32], marker: u64, section: Section, key: &[u8], value_digest: &[u8; 32]) -> [u8; 32] {
    sha(&[SALT_DOMAIN, root_salt, &marker.to_be_bytes(), &[section.tag()], &be_len(key), key, value_digest])
}

/// The digest `leaf_salt` binds a value by. One place, so a caller cannot
/// derive a salt over a different hash of the same bytes.
pub fn value_digest(value: &[u8]) -> [u8; 32] {
    sha(&[value])
}

/// A leaf's hash, over an injective preimage: every fixed-width component
/// (the RFC 6962 leaf tag, the domain, the salt, the section tag and `index`)
/// sits at a fixed offset, and each variable-length component is preceded by
/// its length. One byte string is one (section, position, key, value) — the
/// key/value boundary cannot slide, and no section can be spelled as the
/// prefix of another.
///
/// `index` is the leaf's position among its section's leaves, committed
/// inside the leaf itself. The section's COUNT is committed once, at the
/// section root (`section_hash`), rather than in every leaf: a leaf that
/// carried the count would be rehashed by every seat in its section, which
/// is the whole cost this tree exists to avoid. Between them they close the
/// promotion alias — the last leaf of a 3-leaf tree folds through the same
/// sibling sequence as leaf 1 of a 2-leaf tree — twice over, at the index
/// here and at the count above.
///
/// The index is also the ground non-membership proofs stand on (spec
/// §Standing): adjacency is a claim about positions, meaningful only if
/// positions are bound. It stays in the leaf rather than being left to the
/// fold because every section is append-only inside an epoch, so no existing
/// index moves between two full builds — see `Sections`.
fn leaf_hash(salt: &[u8; 32], section: Section, index: u64, key: &[u8], value: &[u8]) -> [u8; 32] {
    sha(&[
        &[LEAF_PREFIX],
        LEAF_DOMAIN,
        salt,
        &[section.tag()],
        &index.to_be_bytes(),
        &be_len(key),
        key,
        &be_len(value),
        value,
    ])
}

fn inner_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    sha(&[&[INNER_PREFIX], left, right])
}

/// A section's root: its tag, its leaf COUNT, and the root of its leaves.
///
/// The count is bound here and nowhere else. That is what lets a leaf survive
/// its section growing — the whole of the incremental tree — while still
/// refusing a path replayed against a tree of another size, because the count
/// a verifier folds with is the one it must then hash into this preimage.
fn section_hash(section: Section, count: u64, leaves_root: &[u8; 32]) -> [u8; 32] {
    sha(&[&[SECTION_PREFIX], SECTION_DOMAIN, &[section.tag()], &count.to_be_bytes(), leaves_root])
}

fn empty_section() -> [u8; 32] {
    sha(&[EMPTY_DOMAIN])
}

/// Fold a level of nodes into the next one up, recording the sibling
/// consumed by whichever node is currently on the proof path (`track`, an
/// index into this level, or `None` when no path is being built). Which
/// side that sibling is on is not recorded: it is implied by the tracked
/// position's parity, which is exactly what `fold_path` re-derives on the
/// verifying side — a proof carries hashes, never structure.
///
/// Odd levels PROMOTE the last node rather than duplicating it. Duplicating
/// is Bitcoin's construction and makes two distinct trees share a root
/// (CVE-2012-2459); promotion has no such collision and costs nothing.
fn fold_level(level: &[[u8; 32]], track: Option<usize>) -> (Vec<[u8; 32]>, Option<usize>, Option<[u8; 32]>) {
    let mut up = Vec::with_capacity(level.len().div_ceil(2));
    let mut sibling = None;
    let mut next_track = None;
    for (i, pair) in level.chunks(2).enumerate() {
        match pair {
            [l, r] => {
                up.push(inner_hash(l, r));
                if let Some(t) = track {
                    if t == 2 * i {
                        sibling = Some(*r);
                        next_track = Some(i);
                    } else if t == 2 * i + 1 {
                        sibling = Some(*l);
                        next_track = Some(i);
                    }
                }
            }
            // Promoted: it rises a level with no sibling, so no entry.
            [only] => {
                up.push(*only);
                if track == Some(2 * i) {
                    next_track = Some(i);
                }
            }
            _ => unreachable!("chunks(2) yields 1 or 2 elements"),
        }
    }
    (up, next_track, sibling)
}

/// The root of `leaves`, plus the sibling hashes from `index` if asked for.
fn merkle(leaves: &[[u8; 32]], index: Option<usize>) -> ([u8; 32], Vec<[u8; 32]>) {
    if leaves.is_empty() {
        return (empty_section(), Vec::new());
    }
    let mut level = leaves.to_vec();
    let mut track = index;
    let mut path = Vec::new();
    while level.len() > 1 {
        let (up, next_track, sibling) = fold_level(&level, track);
        if let Some(s) = sibling {
            path.push(s);
        }
        level = up;
        track = next_track;
    }
    (level[0], path)
}

/// Fold one node up a promotion tree of `count` leaves from position
/// `index`, consuming exactly the siblings the tree's shape dictates.
/// `None` when the claimed shape and the supplied siblings disagree: an
/// out-of-range index, a missing sibling, or one left over.
///
/// This is the verifying half of `fold_level`, and it is where the RFC 6962
/// discipline extends from tags to structure: which side each sibling is on
/// and how many there are is DERIVED from `(index, count)`, never read from
/// the proof. A proof that could declare its own structure could relocate a
/// leaf, resize its tree, or move hashes across the boundary between the
/// two folds `verify` performs.
fn fold_path(leaf: [u8; 32], index: u64, count: u64, siblings: &[[u8; 32]]) -> Option<[u8; 32]> {
    if index >= count {
        return None;
    }
    let mut acc = leaf;
    let mut t = index;
    let mut m = count;
    let mut steps = siblings.iter();
    while m > 1 {
        if t.is_multiple_of(2) {
            if t + 1 < m {
                acc = inner_hash(&acc, steps.next()?);
            }
            // else: the last node of an odd level, promoted with no sibling.
        } else {
            acc = inner_hash(steps.next()?, &acc);
        }
        t /= 2;
        m = m.div_ceil(2);
    }
    if steps.next().is_some() {
        return None;
    }
    Some(acc)
}

/// A member id / contract id / proposal id / expiry epoch as leaf-key bytes.
/// Big-endian so the byte order matches the numeric order the `BTreeMap`
/// iterates in — which is what makes the leaves sorted by key, so a leaf's
/// `index` is its key's rank, which is what a non-membership proof would rest
/// on (§Standing).
fn id_key(id: u64) -> [u8; 8] {
    id.to_be_bytes()
}

/// One creditor's row of the stake graph: `(debtor, stake, credit reserved,
/// seat reserved)`, debtor ascending, in minor units.
///
/// The one definition, called by the full build and by a proof alike: two
/// constructions of one leaf's value are two things that can disagree. Ids
/// are written as `u64` explicitly and never as `usize`, so the encoding does
/// not depend on the width of the machine that computed it.
///
/// Both reservation layers ride the same row because both are keyed by the
/// arc: an arc's credit hold and its seat hold are two facts about one edge,
/// and a section of their own would rehash a second tree for the same writes.
fn stake_row(
    edges: &edet_kernel::flow::Edges,
    reserved: &edet_kernel::flow::Reservations,
    seat_reserved: &edet_kernel::flow::Reservations,
    creditor: u64,
) -> Vec<(u64, u64, u64, u64)> {
    let c = creditor as usize;
    let mut row: BTreeMap<u64, (u64, u64, u64)> = BTreeMap::new();
    for (&(_, d), &w) in edges.range((c, 0)..=(c, usize::MAX)) {
        row.entry(d as u64).or_default().0 = w;
    }
    for (&(_, d), &r) in reserved.range((c, 0)..=(c, usize::MAX)) {
        row.entry(d as u64).or_default().1 = r;
    }
    for (&(_, d), &r) in seat_reserved.range((c, 0)..=(c, usize::MAX)) {
        row.entry(d as u64).or_default().2 = r;
    }
    row.into_iter().map(|(d, (w, r, s))| (d, w, r, s)).collect()
}

/// The accounts the `Stakes` section holds a leaf for: every member, plus any
/// creditor the graph names that is not one.
///
/// Invariant 3 refuses an edge naming an unknown account, so on any audited
/// ledger this is exactly the member set and the section appends precisely
/// when `Members` does. The union is there because the root must commit to
/// what the state HOLDS rather than to what an invariant says it should: a
/// root that skipped an edge because its creditor was missing from the member
/// table would commit to less than the ledger, silently, on exactly the state
/// the audit exists to catch.
fn stake_keys(state: &State) -> BTreeSet<u64> {
    let mut keys: BTreeSet<u64> = state.members.keys().copied().collect();
    keys.extend(state.edges.keys().map(|&(c, _)| c as u64));
    keys.extend(state.reserved.keys().map(|&(c, _)| c as u64));
    // A seat arc outlives the stake it was taken on — decay is floored at the
    // credit reservation and never at this one — so the seat map can name an
    // arc that `edges` no longer holds, and the root must commit to it.
    keys.extend(state.seat_reserved.keys().map(|&(c, _)| c as u64));
    keys
}

/// The replay window the `Replay` section is defined over: one leaf per epoch
/// in `[epoch, epoch + MAX_TX_LIFETIME_EPOCHS]`, whether or not the state
/// holds a bucket there.
///
/// Defined over the WINDOW and not over the map's keys, so an absent bucket
/// and an empty one are the same leaf: the root commits to the ledger, not to
/// a serialisation of it. A bucket outside the window is a `RootError` rather
/// than a leaf nobody could prove — `apply` refuses one at both ends
/// (`ET_TX_EXPIRED`, `ET_TX_WINDOW_TOO_LONG`) and `begin_block` prunes below
/// it, so the state machine cannot produce one.
fn replay_window(epoch: u64) -> std::ops::RangeInclusive<u64> {
    epoch..=epoch.saturating_add(k::MAX_TX_LIFETIME_EPOCHS)
}

/// Refuse a state whose replay cache holds a bucket the `Replay` section has
/// no position for.
///
/// Asked by the definition and by the cache alike, and it has to be: a patch
/// that quietly ignored an out-of-window bucket would answer where the
/// definition errors, which is the one thing the two may never do. Two
/// lookups rather than a scan — the map is ordered, so its ends bound it.
fn check_replay_window(state: &State) -> Result<(), RootError> {
    let window = replay_window(state.epoch);
    for (&e, _) in [state.applied_by_expiry.first_key_value(), state.applied_by_expiry.last_key_value()]
        .into_iter()
        .flatten()
    {
        if !window.contains(&e) {
            return Err(RootError(format!(
                "replay bucket {e} is outside the window [{}, {}] the section is defined over",
                window.start(),
                window.end()
            )));
        }
    }
    Ok(())
}

/// Everything in `State` that is not one of the six keyed sections.
///
/// A struct of references rather than an ad-hoc byte concatenation so the
/// field list is legible and `serde` does the framing. Every field is a
/// scalar or `O(U)`, which is what lets this be re-encoded on every block:
/// `last_begin_secs` moves on every block that did work, so this leaf is
/// never clean.
///
/// Two absences are deliberate. `root_salt` is committed to transitively —
/// every leaf's salt derives from it, so a node holding a different one
/// computes a different root and fails agreement at once — and including it
/// as a *value* would mean an inclusion proof for this leaf disclosed the
/// secret that keeps every other leaf unguessable. `key_index` is a function
/// of `members` and nothing else, and the audit round-trips it in both
/// directions on every block, so a divergent index is a halted node rather
/// than a silent one; committing it would put an `O(N)` map of public keys
/// back in this leaf for no claim the ledger does not already make.
#[derive(Serialize)]
struct LedgerLeaf<'a> {
    params: &'a Params,
    epoch: u64,
    next_member: MemberId,
    next_contract: ContractId,
    next_proposal: ProposalId,
    /// What this epoch's ceremonies have already admitted (§Governance). Not in
    /// the capacity path, but replicated and hashed like everything else: it is
    /// what the amendment rate bound subtracts, so two validators disagreeing
    /// about it would disagree about which amendments are valid.
    seed_amended_this_epoch: u64,
    chain_id: &'a str,
    last_begin_secs: u64,
    /// Who has declared a supply at all, and how much. The source arcs of the
    /// capacity path — a root that committed to the stake graph but not to
    /// these would let a validator move the community's insured credit
    /// without moving the hash.
    underwriters: &'a BTreeMap<MemberId, u64>,
    /// What is drawn through each supply arc. `O(U)`, so it stays here rather
    /// than becoming rows of its own.
    committed: &'a edet_kernel::flow::Committed,
    /// What the live seats hold on each supply arc. `O(U)` for the same
    /// reason, and hashed for the same one: it is what bounds how many rows a
    /// community can ever seat, so a validator moving it without moving the
    /// hash would be moving the ledger's account table.
    seat_committed: &'a edet_kernel::flow::Committed,
    forfeit_reserve: &'a BTreeMap<MemberId, u64>,
}

/// One section's leaves: the key bytes kept alongside each leaf hash, so
/// `prove` can find a key's position without rebuilding the section.
type Leaves = Vec<(Vec<u8>, [u8; 32])>;

/// Every section's leaves, in key order, indexed by the section's POSITION.
struct Sections {
    leaves: [Leaves; Section::ALL.len()],
    /// The ledger leaf's own encoded value, carried out of the one place it
    /// is built. `prove` needs it and must not construct a second
    /// `LedgerLeaf`: two constructions of the same struct are two things
    /// that can disagree the day a field is added to `State`, and the one
    /// inside `sections` is the one the exhaustive destructure guards.
    /// Record leaves are not carried the same way — re-encoding ONE record
    /// on demand is cheap, while holding every record's bytes would be paid
    /// on every block by `state_root`, which never looks at them.
    ledger_value: Vec<u8>,
}

impl Sections {
    fn get(&self, section: Section) -> &[(Vec<u8>, [u8; 32])] {
        &self.leaves[section.position()]
    }
}

/// Below this many leaves a section is hashed on the calling thread.
///
/// Dispatching a parallel job costs more than the hashes it would
/// distribute, and the definition is computed after every transition in the
/// state harness and the swarm, where most sections are a few hundred rows
/// at most and `Replay` is always thirty-one. **The bytes are identical
/// either way** — both collects are indexed, so both put every leaf back
/// where it started — which is what
/// `the_root_is_the_same_serially_and_in_parallel` holds them to, at a size
/// that crosses this line.
///
/// Measured as a RATIO, both branches in one sitting on one box. Where the
/// pool pays, at a fixed 2,000 edges: 1.3 ms against 1.7 ms at 1,000
/// members, 4.1 against 7.2 at 5,000, 60.0 against 135.5 at 100,000. Where
/// it does not: the swarm corpus, whose sections are hundreds of rows and
/// whose driver builds the definition after every transition, runs 284 s
/// against 124 s. So the line sits above every community-sized section and
/// below the sizes the pool was measured for.
const PARALLEL_LEAVES: usize = 1_024;

/// One section's leaves.
///
/// **Hashed in parallel, collected in ORDER.** A leaf commits its own index
/// and the tree folds the leaves in position, so the order is the format:
/// `par_iter().enumerate()` over an indexed parallel iterator collecting into
/// a `Result<Vec<_>, _>` puts every leaf back where it started, whatever
/// thread computed it and however many threads there were. A collect that
/// reordered — by completion, by hash, by anything — would be a different
/// root on a machine with a different core count, which is a consensus fault
/// rather than a performance one.
/// `the_root_is_the_same_serially_and_in_parallel` is the probe.
fn record_leaves<V: Serialize + Sync>(
    root_salt: &[u8; 32],
    marker: u64,
    section: Section,
    entries: impl Iterator<Item = (u64, V)>,
    what: &str,
) -> Result<Leaves, RootError> {
    let entries: Vec<(u64, V)> = entries.collect();
    let one = |index: usize, id: u64, value: &V| -> Result<(Vec<u8>, [u8; 32]), RootError> {
        let key = id_key(id).to_vec();
        let bytes = encode(what, value)?;
        let salt = leaf_salt(root_salt, marker, section, &key, &value_digest(&bytes));
        let hash = leaf_hash(&salt, section, index as u64, &key, &bytes);
        Ok((key, hash))
    };
    if entries.len() < PARALLEL_LEAVES {
        entries.iter().enumerate().map(|(i, (id, value))| one(i, *id, value)).collect()
    } else {
        entries
            .par_iter()
            .enumerate()
            .map(|(i, (id, value))| one(i, *id, value))
            .collect()
    }
}

/// The scalar leaf — everything in `State` that is not one of the six keyed
/// sections — as its encoded value and its leaf hash.
///
/// **One construction of `LedgerLeaf`, and the completeness gate for all
/// seven sections.** `State` is destructured EXHAUSTIVELY here — no `..` rest
/// pattern — so adding a field to it fails to compile until the field is
/// placed: in this leaf, or in a keyed section with a binding here saying
/// which. That is the completeness mechanism (§Standing of the spec): a root
/// that quietly stopped covering part of the state would let a validator
/// alter it undetected, and since `app_hash` is this root, would also break
/// agreement silently rather than loudly.
///
/// Built in one place because both the full build and an incremental refresh
/// need it: two constructions of the same struct are two things that can
/// disagree the day a field is added.
fn ledger_leaf(state: &State) -> Result<(Vec<u8>, [u8; 32]), RootError> {
    let State {
        // The six keyed sections, each with leaves of its own.
        members: _,
        contracts: _,
        proposals: _,
        validators: _,
        edges: _,
        reserved: _,
        seat_reserved: _,
        applied_by_expiry: _,
        // A function of `members`, round-tripped in both directions by the
        // audit on every block, so a divergent index is a halted node rather
        // than a silent one. Committing it would put an `O(N)` map of public
        // keys in this leaf for no claim the ledger does not already make.
        key_index: _,
        // Committed transitively: every leaf's salt derives from it, so a
        // node holding a different one computes a different root and fails
        // agreement at once. As a leaf VALUE it would be disclosed by an
        // inclusion proof for this leaf, unblinding every other leaf.
        root_salt: _,
        // And the scalar leaf itself.
        params,
        epoch,
        next_member,
        next_contract,
        next_proposal,
        seed_amended_this_epoch,
        chain_id,
        last_begin_secs,
        underwriters,
        committed,
        seat_committed,
        forfeit_reserve,
    } = state;

    let leaf = LedgerLeaf {
        params,
        epoch: *epoch,
        next_member: *next_member,
        next_contract: *next_contract,
        next_proposal: *next_proposal,
        seed_amended_this_epoch: *seed_amended_this_epoch,
        chain_id,
        last_begin_secs: *last_begin_secs,
        underwriters,
        committed,
        seat_committed,
        forfeit_reserve,
    };
    let bytes = encode("ledger", &leaf)?;
    let key: &[u8] = &[];
    let salt = leaf_salt(&state.root_salt, salt_marker(state), Section::Ledger, key, &value_digest(&bytes));
    let hash = leaf_hash(&salt, Section::Ledger, 0, key, &bytes);
    Ok((bytes, hash))
}

/// One section's leaves, in key order.
///
/// The single definition of what each section holds. The full build calls it
/// seven times; [`RootCache`] calls it for a section it has to rebuild and
/// for the two it rebuilds on every block, so a cache can never disagree with
/// the definition about a section's contents — only about how much of it was
/// recomputed.
fn section_leaves(state: &State, section: Section) -> Result<Leaves, RootError> {
    let salt = &state.root_salt;
    let marker = salt_marker(state);
    match section {
        Section::Members => {
            record_leaves::<&Member>(salt, marker, section, state.members.iter().map(|(k, v)| (*k, v)), "member")
        }
        Section::Contracts => {
            record_leaves::<&Contract>(salt, marker, section, state.contracts.iter().map(|(k, v)| (*k, v)), "contract")
        }
        Section::Proposals => {
            record_leaves::<&Proposal>(salt, marker, section, state.proposals.iter().map(|(k, v)| (*k, v)), "proposal")
        }
        Section::Validators => {
            record_leaves::<u64>(salt, marker, section, state.validators.iter().map(|(k, v)| (*k, *v)), "validator")
        }
        Section::Stakes => record_leaves::<Vec<(u64, u64, u64, u64)>>(
            salt,
            marker,
            section,
            stake_keys(state)
                .into_iter()
                .map(|c| (c, stake_row(&state.edges, &state.reserved, &state.seat_reserved, c))),
            "stake row",
        ),
        Section::Replay => {
            check_replay_window(state)?;
            let no_ids: BTreeSet<[u8; 32]> = BTreeSet::new();
            record_leaves::<&BTreeSet<[u8; 32]>>(
                salt,
                marker,
                section,
                replay_window(state.epoch).map(|e| (e, state.applied_by_expiry.get(&e).unwrap_or(&no_ids))),
                "replay bucket",
            )
        }
        Section::Ledger => Ok(vec![(Vec::new(), ledger_leaf(state)?.1)]),
    }
}

/// Build every section's leaves.
fn sections(state: &State) -> Result<Sections, RootError> {
    let mut leaves: [Leaves; Section::ALL.len()] = Default::default();
    for section in Section::ALL {
        leaves[section.position()] = section_leaves(state, section)?;
    }
    Ok(Sections { leaves, ledger_value: ledger_leaf(state)?.0 })
}

fn hashes(entries: &[(Vec<u8>, [u8; 32])]) -> Vec<[u8; 32]> {
    entries.iter().map(|(_, h)| *h).collect()
}

/// The seven section roots, in top-tree position order.
fn section_roots(built: &Sections) -> Vec<[u8; 32]> {
    Section::ALL
        .iter()
        .map(|&sec| {
            let entries = built.get(sec);
            section_hash(sec, entries.len() as u64, &merkle(&hashes(entries), None).0)
        })
        .collect()
}

/// The state root: `Replica::app_hash`, every block's claimed starting state,
/// and the value a pilot would anchor.
///
/// **The definition.** Pure, cache-free, and what every fixture, the proof
/// pair and the swarm driver compare against; [`RootCache::refresh`] is what
/// a validator actually runs, and the two are held equal after every
/// transition in the tree.
pub fn state_root(state: &State) -> Result<[u8; 32], RootError> {
    let built = sections(state)?;
    Ok(merkle(&section_roots(&built), None).0)
}

/// One record, provable against a root by someone holding neither the ledger
/// nor `root_salt`.
///
/// Carries its own leaf salt, which is the whole point: `verify` recomputes
/// the leaf and needs no secret, while `root_salt` — from which this salt was
/// derived one-way — never leaves the nodes that hold the ledger. What the
/// salt unblinds is this leaf in this epoch and nothing else: not the
/// siblings on the path, not this key's value in any other epoch
/// (`leaf_salt`).
///
/// The proof supplies hashes and a claimed position; it carries no
/// structure. Which side each sibling is on, and how many siblings each of
/// the two paths must hold, is derived by `verify` from `index`,
/// `leaf_count` and `section` — and the position is committed inside the leaf
/// hash while the count is committed at the section root, so both are part of
/// what the root signed rather than claims the proof gets to make.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InclusionProof {
    pub section: Section,
    /// The leaf's position among its section's leaves — its key's rank,
    /// since leaves sit in key order.
    pub index: u64,
    /// How many leaves the section held.
    pub leaf_count: u64,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub leaf_salt: [u8; 32],
    /// Sibling hashes, leaf up to its section's leaves root.
    pub path: Vec<[u8; 32]>,
    /// Sibling hashes, that section's root up to the state root.
    pub section_path: Vec<[u8; 32]>,
}

/// Prove that `key` holds `value` in `section` of this state.
///
/// `None` when the key is not in that section — this proves presence only.
/// Absence is provable against the same format (leaves are in key order,
/// every leaf commits to its index and every section commits to its leaf
/// count, so two adjacent leaves bracket a missing key) but is not
/// implemented; see the spec's §Standing.
pub fn prove(state: &State, section: Section, key: &[u8]) -> Result<Option<InclusionProof>, RootError> {
    let built = sections(state)?;
    let entries = built.get(section);
    let Some(index) = entries.iter().position(|(k, _)| k == key) else {
        return Ok(None);
    };
    let value = leaf_value(state, &built, section, key)?
        .ok_or_else(|| RootError("a leaf present in the section had no value".into()))?;

    let (_, path) = merkle(&hashes(entries), Some(index));
    let (_, section_path) = merkle(&section_roots(&built), Some(section.position()));

    Ok(Some(InclusionProof {
        section,
        index: index as u64,
        leaf_count: entries.len() as u64,
        leaf_salt: leaf_salt(&state.root_salt, salt_marker(state), section, key, &value_digest(&value)),
        key: key.to_vec(),
        value,
        path,
        section_path,
    }))
}

/// The `codec` bytes a leaf commits to, for the one key of a KEYED section.
///
/// `None` is the section not holding that key, which is what tells a patch a
/// leaf was removed — so `Stakes` answers `None` for an account the graph
/// does not name, matching `stake_keys`, and `Replay` for an epoch outside
/// the window. The `Ledger` leaf is not here: its value is built once with
/// the rest of the state (`ledger_leaf`) and carried, because two
/// constructions of one struct are two things that can disagree.
fn record_value(state: &State, section: Section, key: &[u8]) -> Result<Option<Vec<u8>>, RootError> {
    let id = match <[u8; 8]>::try_from(key) {
        Ok(bytes) => Some(u64::from_be_bytes(bytes)),
        Err(_) => None,
    };
    match (section, id) {
        (Section::Members, Some(id)) => state.members.get(&id).map(|m| encode("member", m)).transpose(),
        (Section::Contracts, Some(id)) => state.contracts.get(&id).map(|c| encode("contract", c)).transpose(),
        (Section::Proposals, Some(id)) => state.proposals.get(&id).map(|p| encode("proposal", p)).transpose(),
        (Section::Validators, Some(id)) => state.validators.get(&id).map(|p| encode("validator", p)).transpose(),
        (Section::Stakes, Some(id)) if holds_a_stake_row(state, id) => {
            encode("stake row", &stake_row(&state.edges, &state.reserved, &state.seat_reserved, id)).map(Some)
        }
        (Section::Replay, Some(epoch)) if replay_window(state.epoch).contains(&epoch) => {
            let no_ids: BTreeSet<[u8; 32]> = BTreeSet::new();
            encode("replay bucket", state.applied_by_expiry.get(&epoch).unwrap_or(&no_ids)).map(Some)
        }
        _ => Ok(None),
    }
}

/// Whether the `Stakes` section holds a leaf for this account — the
/// membership test of `stake_keys`, answered for one id without building the
/// set. Two range probes rather than a walk.
fn holds_a_stake_row(state: &State, id: u64) -> bool {
    let c = id as usize;
    state.members.contains_key(&id)
        || state.edges.range((c, 0)..=(c, usize::MAX)).next().is_some()
        || state.reserved.range((c, 0)..=(c, usize::MAX)).next().is_some()
        || state.seat_reserved.range((c, 0)..=(c, usize::MAX)).next().is_some()
}

/// The `codec` bytes a leaf commits to, for any section.
fn leaf_value(state: &State, built: &Sections, section: Section, key: &[u8]) -> Result<Option<Vec<u8>>, RootError> {
    match section {
        Section::Ledger if key.is_empty() => Ok(Some(built.ledger_value.clone())),
        Section::Ledger => Ok(None),
        keyed => record_value(state, keyed, key),
    }
}

/// Prove member `id`'s record. The common case: a member showing what the
/// ledger said about them at a height whose root was published.
pub fn prove_member(state: &State, id: MemberId) -> Result<Option<InclusionProof>, RootError> {
    prove(state, Section::Members, &id_key(id))
}

/// Prove contract `id`'s record — the other common case, and the one an
/// arbitrator asks for.
pub fn prove_contract(state: &State, id: ContractId) -> Result<Option<InclusionProof>, RootError> {
    prove(state, Section::Contracts, &id_key(id))
}

/// Check a proof against a root.
///
/// Needs nothing else: no ledger, no node, no `root_salt`. A member can hand
/// this and a published root to somebody who has never run edet.
///
/// Two folds and one section hash, each against a shape the proof does not
/// get to choose: the leaf's fold is derived from its committed `index` and
/// the `leaf_count` that must then hash into the section root, and the top
/// fold from the section's fixed position in the seven-section tree. Each
/// fold consumes exactly the siblings its shape dictates, so a hash cannot be
/// moved across the boundary between them, and a path cannot be replayed at
/// another position or against a tree of another size.
pub fn verify(root: &[u8; 32], proof: &InclusionProof) -> bool {
    let leaf = leaf_hash(&proof.leaf_salt, proof.section, proof.index, &proof.key, &proof.value);
    let Some(leaves_root) = fold_path(leaf, proof.index, proof.leaf_count, &proof.path) else {
        return false;
    };
    let section_root = section_hash(proof.section, proof.leaf_count, &leaves_root);
    let Some(top) =
        fold_path(section_root, proof.section.position() as u64, Section::ALL.len() as u64, &proof.section_path)
    else {
        return false;
    };
    &top == root
}

// ------------------------------------------------------ the incremental root ----

/// One section's tree, held level by level so a patch can fold a changed leaf
/// to the section root without touching its neighbours.
///
/// `levels[0]` is the leaf hashes in key order; each level above is the fold
/// of the one below, and the last holds a single node. `leaves` keeps the key
/// bytes beside each hash, so a patch can find a key's position and a proof
/// can be read off without rebuilding anything.
#[derive(Clone, Debug, Default)]
struct SectionTree {
    leaves: Leaves,
    levels: Vec<Vec<[u8; 32]>>,
    root: [u8; 32],
}

/// The root of a section's leaves — the top of `levels`, or the empty
/// section's own domain-separated hash.
fn leaves_root(levels: &[Vec<[u8; 32]>]) -> [u8; 32] {
    match levels.last() {
        Some(top) if !top.is_empty() => top[0],
        _ => empty_section(),
    }
}

impl SectionTree {
    fn built(section: Section, leaves: Leaves) -> SectionTree {
        let mut levels = vec![leaves.iter().map(|(_, h)| *h).collect::<Vec<[u8; 32]>>()];
        while levels.last().expect("levels always holds the leaf level").len() > 1 {
            let (up, _, _) = fold_level(levels.last().expect("levels always holds the leaf level"), None);
            levels.push(up);
        }
        let root = section_hash(section, leaves.len() as u64, &leaves_root(&levels));
        SectionTree { leaves, levels, root }
    }

    /// Fold `dirty` leaf positions up to the section root, recomputing only
    /// the nodes above them.
    ///
    /// A node one level up changes exactly when one of its children did, or
    /// when it gained a second child — and a leaf that gained a sibling was
    /// itself appended, so it is in `dirty` and its parent falls out of the
    /// same rule. The level is still resized where it grew, and everything
    /// from the previously-promoted node's parent to the new end is marked,
    /// which is the same set arrived at from the other side.
    ///
    /// `O(k log N)` inner hashes for `k` dirty leaves.
    fn refold(&mut self, section: Section, dirty: BTreeSet<usize>) {
        let mut dirty = dirty;
        let mut level = 0usize;
        loop {
            let n = self.levels[level].len();
            if n <= 1 {
                break;
            }
            let want = n.div_ceil(2);
            if self.levels.len() == level + 1 {
                self.levels.push(Vec::new());
            }
            let mut next: BTreeSet<usize> = dirty.iter().map(|i| i / 2).collect();
            let above = self.levels[level + 1].len();
            if above != want {
                self.levels[level + 1].resize(want, [0u8; 32]);
                next.extend(above.saturating_sub(1)..want);
            }
            let (below, at) = self.levels.split_at_mut(level + 1);
            let src = &below[level];
            let dst = &mut at[0];
            for &j in &next {
                dst[j] = if 2 * j + 1 < n { inner_hash(&src[2 * j], &src[2 * j + 1]) } else { src[2 * j] };
            }
            dirty = next;
            level += 1;
        }
        self.levels.truncate(level + 1);
        self.root = section_hash(section, self.leaves.len() as u64, &leaves_root(&self.levels));
    }

    /// The sibling hashes from `index` to the section's leaves root, by the
    /// rule `fold_level` folds with: a node at an even position takes the one
    /// to its right unless it is the last of an odd level, in which case it is
    /// promoted and takes none.
    fn path(&self, index: usize) -> Vec<[u8; 32]> {
        let mut path = Vec::new();
        let mut t = index;
        for level in &self.levels {
            let n = level.len();
            if n <= 1 {
                break;
            }
            if t.is_multiple_of(2) {
                if t + 1 < n {
                    path.push(level[t + 1]);
                }
            } else {
                path.push(level[t - 1]);
            }
            t /= 2;
        }
        path
    }
}

/// What a section's journals say a refresh has to do to it.
enum Patch {
    Clean,
    Keys(BTreeSet<Vec<u8>>),
    All,
}

/// **The state root a validator computes.** The same 32 bytes as
/// [`state_root`], reached by rehashing the rows a block wrote and the paths
/// above them instead of the whole ledger.
///
/// The pure [`state_root`] stays the DEFINITION, and the two are held equal
/// after every transition in the tree — by the state harness, by the swarm
/// driver, and by `the_cached_root_is_the_definition`. This is the same
/// discipline `invariants::audit` and `audit_with_cache` are under, and for
/// the same reason: a cache that could disagree with its definition is a
/// consensus fault rather than a performance one.
///
/// What makes it sound is not a rule the call sites keep. It is
/// [`crate::journal::Journal`]: every mutable path to a committed map either
/// records its key or marks the whole map, so the worst a missed attribution
/// can cost is a rebuild. What makes it CHEAP is that every whole-map
/// mutation in the tree sits at the epoch boundary or behind a governance
/// transition, and every per-block path is a point operation.
///
/// Two sections are rebuilt on every refresh and counted: `Validators`, which
/// is a handful of leaves, and the scalar `Ledger` leaf, which is never clean
/// because `last_begin_secs` moves on every block that did work.
///
/// Not persisted. `Replica::open` builds it once from the snapshot, which is
/// the price of one boundary block.
#[derive(Clone, Debug, Default)]
pub struct RootCache {
    /// `salt_marker` at the last build. `None` is cold.
    marker: Option<u64>,
    sections: [SectionTree; Section::ALL.len()],
    root: [u8; 32],
    /// The scalar leaf's bytes, so `prove` can serve it without a second
    /// `LedgerLeaf`.
    ledger_value: Vec<u8>,
    leaves_hashed: u64,
    full_builds: u64,
    structural_rebuilds: u64,
}

impl RootCache {
    /// Leaf hashes computed since this cache was created. The point of the
    /// cache is a cost, and a cost claim needs a figure a probe can read.
    pub fn leaves_hashed(&self) -> u64 {
        self.leaves_hashed
    }

    /// Whole-tree builds: one per epoch boundary, plus one to warm a cold
    /// cache.
    pub fn full_builds(&self) -> u64 {
        self.full_builds
    }

    /// Sections rebuilt because a patch found a leaf removed or inserted in
    /// the middle. **No section moves inside an epoch** — every removal is at
    /// the boundary, and every append is at the end — so this stays at zero
    /// between two boundaries, and a counter is how that claim is watched
    /// rather than asserted.
    pub fn structural_rebuilds(&self) -> u64 {
        self.structural_rebuilds
    }

    /// Bring the cache up to `state` and return the root.
    ///
    /// Drains every journal, so calling this is what makes the next call
    /// incremental — and what a caller that does NOT want the root must still
    /// do, through [`RootCache::discard`], or a state nobody refreshes
    /// accumulates keys it will never spend.
    pub fn refresh(&mut self, state: &mut State) -> Result<[u8; 32], RootError> {
        let patches = self.take_patches(state);
        if self.marker != Some(salt_marker(state)) {
            return self.full_build(state);
        }
        // The window is checked here as well as inside the full build: a
        // patch that ignored a stray bucket would answer where the definition
        // errors.
        check_replay_window(state)?;
        for (section, patch) in Section::ALL.into_iter().zip(patches) {
            // Rebuilt on every refresh: a handful of validator leaves, and a
            // scalar leaf that is never clean.
            if matches!(section, Section::Validators | Section::Ledger) {
                self.rebuild(state, section)?;
                continue;
            }
            match patch {
                Patch::Clean => {}
                Patch::All => self.rebuild(state, section)?,
                Patch::Keys(keys) => {
                    if !self.patch(state, section, &keys)? {
                        self.structural_rebuilds += 1;
                        self.rebuild(state, section)?;
                    }
                }
            }
        }
        self.ledger_value = ledger_leaf(state)?.0;
        self.root = merkle(&self.section_roots(), None).0;
        Ok(self.root)
    }

    /// Drain the journals and forget everything, for a caller that is not
    /// asking for a root. A state whose journals are never drained
    /// accumulates keys; a cache that let them be dropped unseen would serve
    /// a stale root, so the two happen together.
    pub fn discard(&mut self, state: &mut State) {
        let _ = self.take_patches(state);
        self.marker = None;
    }

    /// Prove one record, off the cache. Must equal [`prove`] on every state,
    /// which `a_proof_from_the_cache_is_the_definitions_proof` holds it to.
    pub fn prove(&self, state: &State, section: Section, key: &[u8]) -> Result<Option<InclusionProof>, RootError> {
        if self.marker.is_none() {
            return Err(RootError("the root cache is cold: refresh it before proving from it".into()));
        }
        let tree = &self.sections[section.position()];
        let Ok(index) = tree.leaves.binary_search_by(|(k, _)| k.as_slice().cmp(key)) else {
            return Ok(None);
        };
        let value = match section {
            Section::Ledger => self.ledger_value.clone(),
            keyed => record_value(state, keyed, key)?
                .ok_or_else(|| RootError("a leaf present in the section had no value".into()))?,
        };
        let (_, section_path) = merkle(&self.section_roots(), Some(section.position()));
        Ok(Some(InclusionProof {
            section,
            index: index as u64,
            leaf_count: tree.leaves.len() as u64,
            leaf_salt: leaf_salt(&state.root_salt, salt_marker(state), section, key, &value_digest(&value)),
            key: key.to_vec(),
            value,
            path: tree.path(index),
            section_path,
        }))
    }

    pub fn prove_member(&self, state: &State, id: MemberId) -> Result<Option<InclusionProof>, RootError> {
        self.prove(state, Section::Members, &id_key(id))
    }

    pub fn prove_contract(&self, state: &State, id: ContractId) -> Result<Option<InclusionProof>, RootError> {
        self.prove(state, Section::Contracts, &id_key(id))
    }

    fn section_roots(&self) -> Vec<[u8; 32]> {
        Section::ALL.iter().map(|s| self.sections[s.position()].root).collect()
    }

    /// Every journal's dirty set, drained once, mapped onto the sections that
    /// read it.
    ///
    /// `Stakes` is the one section fed by four journals: an edge, a credit
    /// reservation or a seat reservation is dirty at its CREDITOR's row, and a
    /// new member is a new empty row, so a member key is a stake key too.
    fn take_patches(&self, state: &mut State) -> [Patch; Section::ALL.len()] {
        let members = state.members.take_dirty();
        let contracts = state.contracts.take_dirty();
        let proposals = state.proposals.take_dirty();
        let edges = state.edges.take_dirty();
        let reserved = state.reserved.take_dirty();
        let seat_reserved = state.seat_reserved.take_dirty();
        let replay = state.applied_by_expiry.take_dirty();

        let ids = |d: &Dirty<u64>| -> Patch {
            match d {
                Dirty::Clean => Patch::Clean,
                Dirty::All => Patch::All,
                Dirty::Keys(keys) => Patch::Keys(keys.iter().map(|id| id_key(*id).to_vec()).collect()),
            }
        };

        let stakes = {
            let graph = [&edges, &reserved, &seat_reserved];
            if matches!(members, Dirty::All) || graph.iter().any(|d| matches!(d, Dirty::All)) {
                Patch::All
            } else {
                let mut keys: BTreeSet<Vec<u8>> = BTreeSet::new();
                if let Dirty::Keys(m) = &members {
                    keys.extend(m.iter().map(|id| id_key(*id).to_vec()));
                }
                for d in graph {
                    if let Dirty::Keys(e) = d {
                        keys.extend(e.iter().map(|&(c, _)| id_key(c as u64).to_vec()));
                    }
                }
                if keys.is_empty() {
                    Patch::Clean
                } else {
                    Patch::Keys(keys)
                }
            }
        };

        let mut out = [const { Patch::Clean }; Section::ALL.len()];
        out[Section::Members.position()] = ids(&members);
        out[Section::Contracts.position()] = ids(&contracts);
        out[Section::Proposals.position()] = ids(&proposals);
        out[Section::Stakes.position()] = stakes;
        out[Section::Replay.position()] = ids(&replay);
        // `Validators` and `Ledger` are not journaled: both are rebuilt on
        // every refresh, and a patch for them would be a second answer to a
        // question already settled.
        out
    }

    /// The whole tree, through the definition's own construction. One per
    /// epoch boundary — where the salt marker moves and every leaf's hash
    /// changes anyway — plus one to warm a cold cache.
    fn full_build(&mut self, state: &State) -> Result<[u8; 32], RootError> {
        let built = sections(state)?;
        let mut hashed = 0u64;
        for (position, leaves) in built.leaves.into_iter().enumerate() {
            hashed += leaves.len() as u64;
            self.sections[position] = SectionTree::built(Section::ALL[position], leaves);
        }
        self.ledger_value = built.ledger_value;
        self.leaves_hashed += hashed;
        self.full_builds += 1;
        self.marker = Some(salt_marker(state));
        self.root = merkle(&self.section_roots(), None).0;
        Ok(self.root)
    }

    fn rebuild(&mut self, state: &State, section: Section) -> Result<(), RootError> {
        let leaves = section_leaves(state, section)?;
        self.leaves_hashed += leaves.len() as u64;
        self.sections[section.position()] = SectionTree::built(section, leaves);
        Ok(())
    }

    /// Recompute the named leaves in place. `false` when the section moved
    /// under them — a removal, or an insert that is not at the end — which
    /// §Implementation says cannot happen inside an epoch, and which the
    /// caller answers with a rebuild and a counter rather than a wrong root.
    fn patch(&mut self, state: &State, section: Section, keys: &BTreeSet<Vec<u8>>) -> Result<bool, RootError> {
        let marker = salt_marker(state);
        let mut dirty: BTreeSet<usize> = BTreeSet::new();
        let mut hashed = 0u64;
        let tree = &mut self.sections[section.position()];
        for key in keys {
            let value = record_value(state, section, key)?;
            let at = tree.leaves.binary_search_by(|(k, _)| k.as_slice().cmp(key.as_slice()));
            match (value, at) {
                (Some(bytes), Ok(index)) => {
                    let salt = leaf_salt(&state.root_salt, marker, section, key, &value_digest(&bytes));
                    let hash = leaf_hash(&salt, section, index as u64, key, &bytes);
                    tree.leaves[index].1 = hash;
                    tree.levels[0][index] = hash;
                    dirty.insert(index);
                    hashed += 1;
                }
                (Some(bytes), Err(index)) if index == tree.leaves.len() => {
                    let salt = leaf_salt(&state.root_salt, marker, section, key, &value_digest(&bytes));
                    let hash = leaf_hash(&salt, section, index as u64, key, &bytes);
                    tree.leaves.push((key.clone(), hash));
                    tree.levels[0].push(hash);
                    dirty.insert(index);
                    hashed += 1;
                }
                // A key that was never here and still is not: a journal may
                // over-report, and one leaf not recomputed is the whole cost.
                (None, Err(_)) => {}
                // A removal, or an insert before the end.
                (None, Ok(_)) | (Some(_), Err(_)) => return Ok(false),
            }
        }
        tree.refold(section, dirty);
        self.leaves_hashed += hashed;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Key, MemberStatus};

    fn seeded(n: u8) -> State {
        let mut st = State::default();
        for i in 0..n {
            st.add_underwriter(vec![[i + 1; 32]], 2500.0).expect("founding underwriter");
        }
        st
    }

    /// The leaf a proof claims, recomputed the way `verify` does.
    fn claimed_leaf(p: &InclusionProof) -> [u8; 32] {
        leaf_hash(&p.leaf_salt, p.section, p.index, &p.key, &p.value)
    }

    #[test]
    fn the_root_is_deterministic_and_moves_with_the_ledger() {
        let a = seeded(4);
        let b = seeded(4);
        assert_eq!(state_root(&a).unwrap(), state_root(&b).unwrap(), "same ledger, same root");

        let mut c = seeded(4);
        c.members.get_mut(&0).unwrap().debt_out += 100;
        assert_ne!(state_root(&a).unwrap(), state_root(&c).unwrap(), "a changed record must move the root");
    }

    /// The three sections outside the record maps are what make this a
    /// commitment to the STATE rather than to its interesting parts. Without
    /// them a validator could move a credit limit, a reservation, a governed
    /// parameter or the replay cache with the root unmoved.
    #[test]
    fn state_outside_the_record_sections_is_committed_too() {
        let base = seeded(3);
        let root = state_root(&base).unwrap();
        let moved = |what: &str, st: &State| {
            assert_ne!(root, state_root(st).unwrap(), "{what} must move the root");
        };

        let mut params_changed = base.clone();
        params_changed.params.dust += 0.5;
        moved("params", &params_changed);

        let mut epoch_changed = base.clone();
        epoch_changed.epoch += 1;
        moved("epoch", &epoch_changed);

        let mut staked = base.clone();
        staked.edges.insert((0, 1), 500);
        moved("a stake edge", &staked);

        let mut reserved = staked.clone();
        reserved.reserved.insert((0, 1), 100);
        assert_ne!(state_root(&staked).unwrap(), state_root(&reserved).unwrap(), "a reservation must move the root");

        let mut committed = base.clone();
        committed.committed.insert(0, 250);
        moved("committed flow", &committed);

        let mut supply = base.clone();
        supply.underwriters.insert(0, 1);
        moved("a declared supply", &supply);

        let mut replay = base.clone();
        replay.applied_by_expiry.entry(base.epoch + 2).or_default().insert([7u8; 32]);
        moved("a replay id", &replay);

        let mut forfeit = base.clone();
        forfeit.forfeit_reserve.insert(0, 100);
        moved("the forfeit reserve", &forfeit);
    }

    /// The `Replay` section is defined over the WINDOW, so an absent bucket
    /// and an empty one commit to the same leaf: the root commits to the
    /// ledger rather than to a serialisation of it. A bucket outside the
    /// window has no position to take, and is refused rather than dropped.
    #[test]
    fn the_replay_section_is_the_window_and_refuses_a_bucket_outside_it() {
        let base = seeded(3);
        let mut empty_bucket = base.clone();
        empty_bucket.applied_by_expiry.insert(base.epoch + 2, BTreeSet::new());
        assert_eq!(
            state_root(&base).unwrap(),
            state_root(&empty_bucket).unwrap(),
            "an absent bucket and an empty one are one leaf"
        );

        let mut stray = base.clone();
        stray
            .applied_by_expiry
            .insert(base.epoch + k::MAX_TX_LIFETIME_EPOCHS + 1, BTreeSet::from([[1u8; 32]]));
        assert!(
            state_root(&stray).is_err(),
            "a bucket the section has no position for must be an error, never a silent drop"
        );
    }

    /// Two ledgers differing ONLY in `root_salt` must not share a root. The
    /// salt is never a leaf value (an inclusion proof for it would hand over
    /// the secret protecting every other leaf), so this is what shows it is
    /// still committed to — transitively, through every leaf's salt.
    #[test]
    fn the_root_salt_is_committed_transitively() {
        let a = seeded(3);
        let mut b = seeded(3);
        b.root_salt = [42u8; 32];
        assert_ne!(state_root(&a).unwrap(), state_root(&b).unwrap());
    }

    #[test]
    fn a_member_proof_verifies_against_the_root() {
        let st = seeded(5);
        let root = state_root(&st).unwrap();
        for id in 0..5u64 {
            let proof = prove_member(&st, id).unwrap().expect("member is present");
            assert!(verify(&root, &proof), "member {id}'s proof must verify");
        }
        assert!(prove_member(&st, 99).unwrap().is_none(), "absent member has no inclusion proof");
    }

    /// A stake row is proved like any other record: one leaf per account,
    /// carrying that creditor's out-edges and BOTH reservation layers — what
    /// outstanding credit holds on each arc and what the seats do.
    #[test]
    fn a_stake_row_is_provable_and_carries_both_reservations() {
        let mut st = seeded(3);
        st.edges.insert((0, 1), 500);
        st.edges.insert((0, 2), 300);
        st.reserved.insert((0, 2), 120);
        st.seat_reserved.insert((0, 1), 40);
        let root = state_root(&st).unwrap();
        let proof = prove(&st, Section::Stakes, &id_key(0)).unwrap().expect("account 0 has a row");
        assert!(verify(&root, &proof));
        let row: Vec<(u64, u64, u64, u64)> = crate::codec::decode(&proof.value).expect("the row decodes");
        assert_eq!(row, vec![(1, 500, 0, 40), (2, 300, 120, 0)], "debtor ascending, stake and both holds together");

        let empty = prove(&st, Section::Stakes, &id_key(2))
            .unwrap()
            .expect("every account has a row");
        assert!(verify(&root, &empty));
        assert_eq!(crate::codec::decode::<Vec<(u64, u64, u64, u64)>>(&empty.value).unwrap(), Vec::new());
    }

    /// A seat outlives the stake it was taken on, so the `Stakes` section must
    /// hold a leaf for a creditor the edge map no longer names — otherwise the
    /// root commits to less than the ledger holds.
    #[test]
    fn a_seat_arc_keeps_its_stake_row_after_the_edge_decays_away() {
        let mut st = seeded(3);
        st.seat_reserved.insert((7, 1), 2_000);
        assert!(stake_keys(&st).contains(&7), "the seat map names the row");
        let root = state_root(&st).unwrap();
        let proof = prove(&st, Section::Stakes, &id_key(7))
            .unwrap()
            .expect("the seat's creditor has a row");
        assert!(verify(&root, &proof));
        assert_eq!(
            crate::codec::decode::<Vec<(u64, u64, u64, u64)>>(&proof.value).unwrap(),
            vec![(1, 0, 0, 2_000)],
            "a stake of nothing, and the seat still held"
        );
    }

    /// Every section must be provable, including the one-leaf `Ledger`
    /// section, the fixed-width `Replay` one and an empty section's
    /// neighbours — odd counts at several levels are where a Merkle path
    /// implementation usually goes wrong.
    #[test]
    fn every_section_is_provable_at_awkward_sizes() {
        for n in [1u8, 2, 3, 5, 9, 17, 32, 33] {
            let st = seeded(n);
            let root = state_root(&st).unwrap();
            for id in 0..n as u64 {
                let proof = prove_member(&st, id).unwrap().expect("present");
                assert!(verify(&root, &proof), "n={n}, member {id}");
                let row = prove(&st, Section::Stakes, &id_key(id)).unwrap().expect("present");
                assert!(verify(&root, &row), "n={n}, stake row {id}");
            }
            for e in replay_window(st.epoch) {
                let bucket = prove(&st, Section::Replay, &id_key(e))
                    .unwrap()
                    .expect("the window is always full");
                assert!(verify(&root, &bucket), "n={n}, replay bucket {e}");
            }
            let ledger = prove(&st, Section::Ledger, &[])
                .unwrap()
                .expect("the ledger leaf is always present");
            assert!(verify(&root, &ledger), "n={n}, ledger leaf");
        }
    }

    #[test]
    fn a_tampered_proof_does_not_verify() {
        let st = seeded(6);
        let root = state_root(&st).unwrap();
        let good = prove_member(&st, 2).unwrap().expect("present");

        let mut wrong_value = good.clone();
        wrong_value.value.push(0);
        assert!(!verify(&root, &wrong_value), "a changed record must not verify");

        let mut wrong_salt = good.clone();
        wrong_salt.leaf_salt = [0u8; 32];
        assert!(!verify(&root, &wrong_salt), "a made-up salt must not verify");

        let mut wrong_key = good.clone();
        wrong_key.key = id_key(3).to_vec();
        assert!(!verify(&root, &wrong_key), "the same record under another key must not verify");

        let mut short_path = good.clone();
        short_path.path.pop();
        assert!(!verify(&root, &short_path), "a dropped sibling must not verify");

        let mut long_path = good.clone();
        long_path.path.push([9u8; 32]);
        assert!(!verify(&root, &long_path), "an appended sibling must not verify");

        let mut wrong_section = good.clone();
        wrong_section.section = Section::Contracts;
        assert!(!verify(&root, &wrong_section), "a leaf reassigned to another section must not verify");

        assert!(!verify(&[0u8; 32], &good), "a valid proof against the wrong root must not verify");
    }

    /// The leaf preimage is injective: length framing means the committed
    /// byte string `key ‖ value` has exactly ONE split that hashes to the
    /// committed leaf, even for a verifier handed the honest salt. An
    /// unframed preimage lets every split verify, which forges records that
    /// were never on the ledger and breaks the key ordering non-membership
    /// proofs rest on.
    #[test]
    fn the_key_value_boundary_inside_a_leaf_cannot_slide() {
        let st = seeded(4);
        let root = state_root(&st).unwrap();

        let good = prove_member(&st, 2).unwrap().expect("present");
        assert!(verify(&root, &good), "fixture: the honest split must verify");
        let joined: Vec<u8> = [good.key.clone(), good.value.clone()].concat();
        for cut in 0..=joined.len() {
            if cut == good.key.len() {
                continue;
            }
            let mut slid = good.clone();
            slid.key = joined[..cut].to_vec();
            slid.value = joined[cut..].to_vec();
            assert!(!verify(&root, &slid), "the boundary slid to {cut} must not verify");
        }

        // The ledger leaf is the extreme case: its committed key is EMPTY,
        // so an unframed preimage would let it verify under any key that
        // prefixes its value.
        let ledger = prove(&st, Section::Ledger, &[]).unwrap().expect("present");
        let mut forged = ledger.clone();
        forged.key = ledger.value[..16].to_vec();
        forged.value = ledger.value[16..].to_vec();
        assert!(!verify(&root, &forged), "the ledger leaf must not verify under a key carved from its value");
    }

    /// A leaf commits to its position, so a path cannot be replayed at
    /// another index. The load-bearing case is the promotion alias: the last
    /// leaf of a 3-leaf tree folds through the identical sibling sequence as
    /// leaf 1 of a 2-leaf tree, so only what the hashes commit to tells the
    /// two claims apart.
    #[test]
    fn a_proof_verifies_only_at_its_committed_position_and_tree_size() {
        let st = seeded(3);
        let root = state_root(&st).unwrap();
        let good = prove_member(&st, 2).unwrap().expect("present");
        assert_eq!((good.index, good.leaf_count, good.path.len()), (2, 3, 1), "fixture: the promoted leaf");
        assert!(verify(&root, &good));

        let mut alias = good.clone();
        alias.index = 1;
        alias.leaf_count = 2;
        assert!(!verify(&root, &alias), "the promotion alias — same siblings, smaller claimed tree — must not verify");

        let mut wrong_index = good.clone();
        wrong_index.index = 0;
        assert!(!verify(&root, &wrong_index), "another index must not verify");

        let mut wrong_count = good.clone();
        wrong_count.leaf_count = 4;
        assert!(!verify(&root, &wrong_count), "another tree size must not verify");

        let mut out_of_range = good.clone();
        out_of_range.index = 3;
        assert!(!verify(&root, &out_of_range), "an index outside the tree must not verify");
    }

    /// **The section's leaf count is bound at the section root, and it has to
    /// be.** Which side each sibling sits on is a function of the INDEX
    /// alone, and the presence pattern for index 0 is identical at three
    /// leaves and at four — so a proof for leaf 0 of a 3-leaf section folds,
    /// byte for byte, to the leaves root of a 4-leaf claim. The leaf hash
    /// cannot separate them: it no longer carries the count, which is exactly
    /// what lets a leaf survive its section growing. The section root
    /// separates them.
    ///
    /// Mutation that bites: drop `count` from `section_hash`. The resized
    /// claim below then verifies, and a section's shape becomes something a
    /// proof declares rather than something the root signed.
    ///
    /// The promotion alias fails twice over — at the leaf's index and at the
    /// section's count — which is the second assertion.
    #[test]
    fn a_leaf_count_is_bound_at_the_section_root() {
        let st = seeded(3);
        let root = state_root(&st).unwrap();

        let good = prove_member(&st, 0).unwrap().expect("present");
        assert_eq!((good.index, good.leaf_count, good.path.len()), (0, 3, 2), "fixture: index 0 of three leaves");
        assert!(verify(&root, &good));

        let mut resized = good.clone();
        resized.leaf_count = 4;
        assert_eq!(
            fold_path(claimed_leaf(&good), good.index, good.leaf_count, &good.path),
            fold_path(claimed_leaf(&resized), resized.index, resized.leaf_count, &resized.path),
            "fixture: the two claims fold to the SAME leaves root, so only the section's count can separate them"
        );
        assert!(!verify(&root, &resized), "a section resized under an unchanged leaf must not verify");

        let promoted = prove_member(&st, 2).unwrap().expect("present");
        let mut alias = promoted.clone();
        alias.index = 1;
        alias.leaf_count = 2;
        assert_ne!(claimed_leaf(&alias), claimed_leaf(&promoted), "the index is committed in the leaf");
        assert!(!verify(&root, &alias), "and the promotion alias fails at the count as well");
    }

    /// The property the salt exists for: sibling hashes on a path must not be
    /// brute-forceable from the record space. An attacker who knows the shape
    /// of a `Member` and tries every plausible one cannot match a sibling
    /// without `root_salt`, which is exactly what salting buys — so a proof
    /// discloses its own leaf and nobody else's.
    #[test]
    fn a_sibling_hash_cannot_be_reconstructed_without_the_chain_salt() {
        let st = seeded(4);
        let victim = st.members.get(&1).expect("member 1").clone();
        let victim_key = id_key(1).to_vec();
        let victim_bytes = encode("member", &victim).unwrap();

        // The attacker holds a proof for member 0, so they hold member 1's
        // leaf hash as a sibling, and they have guessed member 1's record
        // exactly. Without the chain salt they still cannot confirm it.
        let proof = prove_member(&st, 0).unwrap().expect("present");
        let real_leaf = leaf_hash(
            &leaf_salt(&st.root_salt, salt_marker(&st), Section::Members, &victim_key, &value_digest(&victim_bytes)),
            Section::Members,
            1,
            &victim_key,
            &victim_bytes,
        );
        assert!(
            proof.path.contains(&real_leaf),
            "fixture must actually put the victim's leaf on the path as a sibling"
        );

        let guessed_salt =
            leaf_salt(&[0u8; 32], salt_marker(&st), Section::Members, &victim_key, &value_digest(&victim_bytes));
        let guessed = leaf_hash(&guessed_salt, Section::Members, 1, &victim_key, &victim_bytes);
        assert_ne!(guessed, real_leaf, "a correct guess of the record must not reproduce the sibling hash");
    }

    /// A disclosed salt unblinds one epoch's value of one record, nothing
    /// more. The salt binds the VALUE, so the attacker below — who holds
    /// member 1's key, old value and old salt from a proof member 1 once
    /// handed them, has guessed the NEW value exactly, and holds a fresh
    /// proof of member 0's carrying the new leaf as a sibling — still cannot
    /// confirm the guess: the new value's salt derives from the new value,
    /// which only `root_salt` holders can compute.
    #[test]
    fn a_disclosed_salt_does_not_unblind_the_same_keys_later_value() {
        let before = seeded(4);
        let disclosed = prove_member(&before, 1).unwrap().expect("present");

        let mut after = before.clone();
        after.members.get_mut(&1).unwrap().debt_out += 13_750;
        let root_after = state_root(&after).unwrap();
        let neighbour = prove_member(&after, 0).unwrap().expect("present");
        assert!(verify(&root_after, &neighbour));

        let guessed_value = encode("member", &after.members.get(&1).unwrap()).unwrap();
        let confirmed = leaf_hash(&disclosed.leaf_salt, Section::Members, 1, &disclosed.key, &guessed_value);
        assert!(
            !neighbour.path.contains(&confirmed),
            "the old salt must not confirm a guess of the leaf's later value"
        );

        // Fixture control: the guess IS right — with the real salt the
        // leaf sits on the neighbour's path exactly where the attacker
        // looked.
        let real_salt = leaf_salt(
            &after.root_salt,
            salt_marker(&after),
            Section::Members,
            &disclosed.key,
            &value_digest(&guessed_value),
        );
        let real_leaf = leaf_hash(&real_salt, Section::Members, 1, &disclosed.key, &guessed_value);
        assert!(
            neighbour.path.contains(&real_leaf),
            "fixture: the victim's new leaf must be a sibling on the neighbour's path"
        );
    }

    /// **The salt binds the value through its digest**, which binds the same
    /// thing under collision resistance: two values deriving one salt is a
    /// SHA-256 collision. What it buys is that a large value is hashed once
    /// here instead of in full, twice.
    ///
    /// Mutation that bites: derive the salt from the key alone. Whoever once
    /// saw a proof for a leaf then holds that leaf's permanent salt, and can
    /// brute-force its value inside every later proof carrying it as a
    /// sibling — record values live in small spaces.
    #[test]
    fn the_salt_binds_the_value_through_its_digest() {
        let salt = |value: &[u8]| leaf_salt(&[7u8; 32], 1_000, Section::Members, b"k", &value_digest(value));
        let base = vec![1u8, 2, 3, 4];
        let mut moved = base.clone();
        moved[2] ^= 1;
        assert_ne!(salt(&base), salt(&moved), "one byte of the value moves the salt");
        assert_eq!(salt(&base), salt(&base.clone()), "and the same value derives the same salt");
        // And the digest is the binding, not a second input beside the value:
        // a caller cannot reach a salt without one.
        assert_eq!(salt(&base), leaf_salt(&[7u8; 32], 1_000, Section::Members, b"k", &value_digest(&base)));
    }

    /// **The root is the same on one thread and on the pool.** A leaf commits
    /// its own index and the tree folds the leaves in position, so the
    /// collect's ORDER is part of the format: a machine with a different core
    /// count computing a different root is a consensus fault.
    ///
    /// Mutation that bites: `par_bridge()` in place of `par_iter()`, which is
    /// an UNINDEXED parallel iterator and yields in completion order. The two
    /// roots differ, and so would two honest validators' with different core
    /// counts. A merely reordered but still deterministic collect does not
    /// bite this probe and does not need to: it would be a format change, and
    /// `proof-fixture-check` fails on one.
    #[test]
    fn the_root_is_the_same_serially_and_in_parallel() {
        let mut st = State::default();
        for i in 0..64u8 {
            st.add_underwriter(vec![[i + 1; 32]], 100.0 + i as f64).expect("member");
        }
        let parallel = state_root(&st).expect("root");
        let serial = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .expect("a one-thread pool")
            .install(|| state_root(&st))
            .expect("root");
        assert_eq!(parallel, serial);
    }

    /// **What makes the tree incremental.** The salt binds the epoch, so a
    /// record nobody touched keeps its leaf hash for the whole epoch, and a
    /// block rehashes the rows it wrote and their paths rather than the
    /// ledger. The block marker still moves — `last_begin_secs` sits in the
    /// scalar leaf and the root moves with it — but no record leaf does.
    ///
    /// Mutation that bites: bind `last_begin_secs` in `salt_marker`. Every
    /// leaf's salt then changes on every block that did work, the sibling
    /// below moves, and the cache has nothing left to keep.
    #[test]
    fn an_unchanged_record_keeps_its_leaf_hash_within_an_epoch() {
        let early = seeded(4);
        let before = prove_member(&early, 0).unwrap().expect("present");

        let mut later = early.clone();
        later.last_begin_secs += 3_600;
        assert_eq!(salt_marker(&later), salt_marker(&early), "fixture: the block moved, the epoch did not");
        let after = prove_member(&later, 0).unwrap().expect("present");

        assert_eq!(after.path, before.path, "an untouched record must keep its leaf hash inside its epoch");
        assert_ne!(
            state_root(&later).unwrap(),
            state_root(&early).unwrap(),
            "fixture: the block marker still moves the root, through the scalar leaf"
        );
    }

    /// The disclosure the epoch marker bounds: across an epoch boundary every
    /// salt changes, so someone holding a proof from one epoch learns nothing
    /// from a later epoch's siblings — not the neighbouring value, and not
    /// even "it is still the same".
    #[test]
    fn an_unchanged_record_is_not_recognisable_across_epochs() {
        let early = seeded(4);
        let disclosed = prove_member(&early, 1).unwrap().expect("present");

        let mut later = early.clone();
        later.epoch += 1;
        let root_later = state_root(&later).unwrap();
        let neighbour = prove_member(&later, 0).unwrap().expect("present");
        assert!(verify(&root_later, &neighbour));

        let stale = leaf_hash(&disclosed.leaf_salt, Section::Members, 1, &disclosed.key, &disclosed.value);
        assert!(
            !neighbour.path.contains(&stale),
            "an unchanged record must not present the same leaf hash in a later epoch"
        );
    }

    /// The split between the record path and the section path is part of
    /// the tree's committed structure, not the proof's to declare: each
    /// fold consumes exactly the siblings its shape dictates, so a hash
    /// moved across the declared boundary — in either direction, or all of
    /// them at once — must fail.
    #[test]
    fn a_sibling_cannot_cross_the_section_boundary() {
        let st = seeded(5);
        let root = state_root(&st).unwrap();
        let good = prove_member(&st, 0).unwrap().expect("present");
        assert!(verify(&root, &good));
        assert!(!good.path.is_empty() && !good.section_path.is_empty(), "fixture: both halves populated");

        let mut shifted = good.clone();
        let s = shifted.section_path.remove(0);
        shifted.path.push(s);
        assert!(!verify(&root, &shifted), "a top-tree sibling relabelled into the record path must not verify");

        let mut lifted = good.clone();
        let s = lifted.path.pop().expect("populated");
        lifted.section_path.insert(0, s);
        assert!(!verify(&root, &lifted), "a record-path sibling relabelled into the top tree must not verify");

        let mut flattened = good.clone();
        let rest = std::mem::take(&mut flattened.section_path);
        flattened.path.extend(rest);
        assert!(!verify(&root, &flattened), "a proof with no declared section boundary at all must not verify");
    }

    /// Promotion, not duplication, for odd levels (CVE-2012-2459): two
    /// different leaf lists must never produce the same section root.
    #[test]
    fn an_odd_level_promotes_rather_than_duplicating() {
        let a = [[1u8; 32], [2u8; 32], [3u8; 32]];
        let duplicated = [[1u8; 32], [2u8; 32], [3u8; 32], [3u8; 32]];
        assert_ne!(
            merkle(&a, None).0,
            merkle(&duplicated, None).0,
            "duplicating the last leaf must not reproduce the odd-length root"
        );
    }

    #[test]
    fn an_empty_section_has_a_domain_separated_root() {
        assert_eq!(merkle(&[], None).0, empty_section());
        assert_ne!(empty_section(), [0u8; 32], "an empty section must not hash to a value a real node could take");
    }

    // ------------------------------------------------ the incremental root ----

    /// A scene driver for the cache probes: a small community, an id
    /// counter, and the pair check after every step.
    struct Scene {
        st: State,
        cache: RootCache,
        next_id: u64,
    }

    impl Scene {
        /// `k` founding underwriters and `m` ordinary accounts, member `i`
        /// holding key `[i + 1; 32]`.
        fn new(k: u8, m: u8) -> Scene {
            let mut st = State::default();
            for i in 0..k {
                st.add_underwriter(vec![[i + 1; 32]], 2500.0).expect("founding underwriter");
            }
            for i in k..k + m {
                st.new_account(vec![[i + 1; 32]]);
            }
            Scene { st, cache: RootCache::default(), next_id: 0 }
        }

        fn id(&mut self) -> [u8; 32] {
            self.next_id += 1;
            let mut id = [0u8; 32];
            id[..8].copy_from_slice(&self.next_id.to_be_bytes());
            id
        }

        fn apply(&mut self, tx: crate::tx::Tx, signers: &[Key]) -> crate::errors::Res<()> {
            let id = self.id();
            let (not_after, now) = (self.st.epoch + 5, self.st.last_begin_secs);
            crate::apply(&mut self.st, tx, id, not_after, signers, now)
        }

        fn ok(&mut self, tx: crate::tx::Tx, signers: &[Key]) {
            self.apply(tx, signers).expect("the transition should have been accepted");
        }

        fn goto(&mut self, epoch: u64) {
            self.st.begin_block(epoch * k::EPOCH_SECS);
        }

        /// The definition and the cache, held equal. Returns the root so a
        /// caller can compare two steps.
        fn agree(&mut self, when: &str) -> [u8; 32] {
            let definition = state_root(&self.st);
            let cached = self.cache.refresh(&mut self.st);
            assert_eq!(definition, cached, "the cached root left the definition {when}");
            definition.expect("the scene stays inside the replay window")
        }
    }

    fn key(i: u8) -> Key {
        [i + 1; 32]
    }

    /// **The cache is the definition, over a scene that touches every
    /// journal.** Seating, settlement, an insured acceptance, a partial
    /// discharge, an epoch crank, the retirement sweep, and a round trip
    /// through `codec` into a cold cache.
    ///
    /// This is the scripted half. The driven half is `Driver::check`, which
    /// holds the same pair after every transition in the tree — which is
    /// where a mutation actually gets caught: remove the `touch` beside
    /// `flow::stake` and the two diverge on the first settlement.
    #[test]
    fn the_cached_root_is_the_definition() {
        use crate::tx::Tx;
        use crate::types::Party;

        let mut s = Scene::new(2, 1);
        s.agree("on a cold cache");

        // A seat: a key nobody has seen becomes a row, appending to `Members`
        // and to `Stakes`.
        let newcomer = key(9);
        let seated = s.st.next_contract;
        s.ok(
            Tx::Accept {
                debtor: Party::Key(newcomer),
                creditor: Party::Member(0),
                amount: 40.0,
                maturity_epochs: 30,
                arb: None,
            },
            &[key(0), newcomer],
        );
        s.agree("after a seat");

        // Settled in full: a stake edge, so the row now has standing.
        s.ok(Tx::Settle { contract: seated, amount: 40.0 }, &[key(0), newcomer]);
        s.agree("after a settlement");

        // An insured acceptance against that standing — this is what writes
        // `reserved`, and so the creditor's `Stakes` row.
        let debtor = s.st.member_of_key(&newcomer).expect("the seat took");
        let insured = s.st.next_contract;
        s.ok(
            Tx::Accept {
                debtor: Party::Member(debtor),
                creditor: Party::Member(1),
                amount: 20.0,
                maturity_epochs: 30,
                arb: None,
            },
            &[key(1), newcomer],
        );
        assert!(s.st.contracts[&insured].insured, "fixture: the acceptance must actually reserve flow");
        s.agree("after an insured acceptance");

        // A partial discharge releases the hold where it was taken.
        s.ok(Tx::Settle { contract: insured, amount: 5.0 }, &[key(1), newcomer]);
        s.agree("after a partial discharge");

        // The boundary: every salt changes, every stake decays, and the whole
        // tree is rebuilt.
        let builds = s.cache.full_builds();
        s.goto(1);
        s.agree("after an epoch crank");
        assert_eq!(s.cache.full_builds(), builds + 1, "an epoch boundary is exactly one full build");

        // The retirement sweep, which is the one place rows are REMOVED.
        s.ok(Tx::Settle { contract: insured, amount: 15.0 }, &[key(1), newcomer]);
        s.agree("after closing the row");
        s.goto(2 + k::CLOSED_RETENTION_EPOCHS);
        assert!(!s.st.contracts.contains_key(&insured), "fixture: the sweep must actually retire the row");
        s.agree("after the retirement sweep");

        // A state off the wire meets a cold cache, and the two still agree.
        let bytes = crate::codec::encode(&s.st).expect("encode");
        let mut restored: State = crate::codec::decode(&bytes).expect("decode");
        let mut cold = RootCache::default();
        assert_eq!(state_root(&restored).unwrap(), cold.refresh(&mut restored).unwrap(), "on a state off the wire");
        assert_eq!(state_root(&restored).unwrap(), state_root(&s.st).unwrap(), "and it is the same ledger");
    }

    /// **An ordinary block hashes what it touched.** One acceptance and one
    /// settlement in a community of a thousand rehash a handful of leaves —
    /// `Validators` and the scalar leaf are rebuilt and counted on every
    /// refresh, and the rest is what the block wrote. The epoch crank beside
    /// it rehashes the ledger.
    ///
    /// Mutation that bites: a `values_mut()` on a per-block path, or a
    /// `DerefMut` where a point operation belongs. The count goes from a
    /// constant to the row count.
    #[test]
    fn an_ordinary_block_hashes_what_it_touched() {
        use crate::tx::Tx;
        use crate::types::Party;

        let mut s = Scene::new(2, 0);
        for i in 2..1_000u64 {
            let mut k = [0u8; 32];
            k[..8].copy_from_slice(&(i + 1).to_be_bytes());
            s.st.new_account(vec![k]);
        }
        s.agree("on a cold cache");
        assert!(s.st.members.len() >= 1_000, "fixture: a community worth measuring");

        let before = s.cache.leaves_hashed();
        let row = s.st.next_contract;
        s.ok(
            Tx::Accept {
                debtor: Party::Member(2),
                creditor: Party::Member(0),
                amount: 40.0,
                maturity_epochs: 30,
                arb: None,
            },
            &[key(0), s.st.members[&2].keys[0]],
        );
        s.agree("after an acceptance");
        s.ok(Tx::Settle { contract: row, amount: 40.0 }, &[key(0), s.st.members[&2].keys[0]]);
        s.agree("after a settlement");
        let block = s.cache.leaves_hashed() - before;
        assert!(block <= 16, "two transitions in a community of 1,000 rehashed {block} leaves");

        let before = s.cache.leaves_hashed();
        s.goto(1);
        s.agree("after an epoch crank");
        let boundary = s.cache.leaves_hashed() - before;
        assert!(
            boundary >= s.st.members.len() as u64,
            "the boundary rebuilds the ledger: {boundary} leaves against {} rows",
            s.st.members.len()
        );
    }

    /// **No section moves inside an epoch.** Every removal a transition can
    /// make is at the boundary, and every insert is at the end, so a patch
    /// never finds the section reshaped under it. The counter is how that is
    /// WATCHED rather than asserted: a rebuild is still correct, so nothing
    /// else would notice.
    ///
    /// Mutation that bites: a `contracts.remove` outside the sweep.
    #[test]
    fn no_section_moves_inside_an_epoch() {
        use crate::tx::Tx;
        use crate::types::Party;

        let mut s = Scene::new(3, 2);
        s.agree("on a cold cache");
        s.goto(1);
        s.agree("at the boundary the scene starts from");

        // A seat, a settlement, an insured acceptance, a partial discharge, a
        // governance proposal, a supply reduction and a replay id, all inside
        // one epoch.
        let newcomer = key(9);
        let seat = s.st.next_contract;
        s.ok(
            Tx::Accept {
                debtor: Party::Key(newcomer),
                creditor: Party::Member(0),
                amount: 60.0,
                maturity_epochs: 30,
                arb: None,
            },
            &[key(0), newcomer],
        );
        s.agree("after a seat");
        s.ok(Tx::Settle { contract: seat, amount: 60.0 }, &[key(0), newcomer]);
        s.agree("after a settlement");

        let debtor = s.st.member_of_key(&newcomer).expect("the seat took");
        let insured = s.st.next_contract;
        s.ok(
            Tx::Accept {
                debtor: Party::Member(debtor),
                creditor: Party::Member(1),
                amount: 20.0,
                maturity_epochs: 30,
                arb: None,
            },
            &[key(1), newcomer],
        );
        s.agree("after an insured acceptance");
        s.ok(Tx::Settle { contract: insured, amount: 7.0 }, &[key(1), newcomer]);
        s.agree("after a partial discharge");
        s.ok(Tx::DeclareSupply { member: 2, supply: 1_000.0 }, &[key(2)]);
        s.agree("after a supply reduction");
        // A refused transition still spends its id, which is a `Replay` write.
        let _ = s.apply(Tx::Settle { contract: 999, amount: 1.0 }, &[key(0)]);
        s.agree("after a refusal");

        assert_eq!(s.cache.structural_rebuilds(), 0, "no section may be reshaped between two epoch boundaries");
        assert_eq!(s.cache.full_builds(), 2, "the cold warm-up and the one boundary, and nothing else");

        // Fixture control: the counter is not stuck at zero. A row removed
        // mid-epoch — which is what the mutation above amounts to — is
        // exactly what it is watching for, and the root stays right because
        // the answer to a reshaped section is a rebuild.
        s.st.contracts.remove(&seat);
        s.agree("after a removal no transition can make");
        assert_eq!(s.cache.structural_rebuilds(), 1, "a row removed inside an epoch reshapes its section");
    }

    /// A proof off the cache is the definition's proof, at every leaf of
    /// every section and at the sizes where a promotion tree goes wrong.
    #[test]
    fn a_proof_from_the_cache_is_the_definitions_proof() {
        for n in [1u8, 2, 3, 5, 9, 17, 32, 33] {
            let mut st = seeded(n);
            st.edges.insert((0, 1.min(n as usize - 1)), 500);
            let mut cache = RootCache::default();
            let root = cache.refresh(&mut st).expect("root");
            assert_eq!(root, state_root(&st).unwrap(), "n={n}");
            for section in Section::ALL {
                let keys: Vec<Vec<u8>> = match section {
                    Section::Replay => replay_window(st.epoch).map(|e| id_key(e).to_vec()).collect(),
                    Section::Ledger => vec![Vec::new()],
                    _ => (0..n as u64).map(|id| id_key(id).to_vec()).collect(),
                };
                for key in keys {
                    let mine = cache.prove(&st, section, &key).expect("prove");
                    let theirs = prove(&st, section, &key).expect("prove");
                    assert_eq!(mine, theirs, "n={n}, {section:?}, key {key:?}");
                    if let Some(p) = mine {
                        assert!(verify(&root, &p), "n={n}, {section:?}");
                    }
                }
            }
        }
    }

    /// **A read computes no root.** `Replica::state_hash` answers from the
    /// cached value, so an unauthenticated `/head` cannot buy a full build —
    /// which is what it did when the root was recomputed per request, bounded
    /// only by the per-IP read limiter.
    #[test]
    fn a_head_read_computes_no_root() {
        let mut st = seeded(8);
        let mut cache = RootCache::default();
        let root = cache.refresh(&mut st).expect("root");
        let (builds, hashed) = (cache.full_builds(), cache.leaves_hashed());
        for _ in 0..100 {
            assert_eq!(cache.refresh(&mut st).expect("root"), root, "a state nothing wrote keeps its root");
        }
        assert_eq!(cache.full_builds(), builds, "a hundred reads must not rebuild the tree");
        assert!(
            cache.leaves_hashed() - hashed <= 100 * (st.validators.len() as u64 + 1),
            "a read costs the scalar leaf and the validator set, and nothing else"
        );
    }

    /// The incremental fold must arrive at the same LEVELS a rebuild does,
    /// not merely the same root — a proof is read off those levels, so a
    /// level that folded to the right value by the wrong shape would serve
    /// paths nobody could verify.
    #[test]
    fn a_patched_tree_has_the_shape_a_rebuilt_one_has() {
        for n in 1..40u64 {
            let mut st = State::default();
            let mut cache = RootCache::default();
            for i in 0..n {
                let mut k = [0u8; 32];
                k[..8].copy_from_slice(&(i + 1).to_be_bytes());
                st.new_account(vec![k]);
                cache.refresh(&mut st).expect("root");
            }
            let patched = &cache.sections[Section::Members.position()];
            let rebuilt = SectionTree::built(Section::Members, section_leaves(&st, Section::Members).unwrap());
            assert_eq!(patched.levels, rebuilt.levels, "n={n}: the patched levels must be the rebuilt ones");
            assert_eq!(patched.root, rebuilt.root, "n={n}");
        }
    }

    /// A journal that over-reports costs a leaf hash and never a wrong root:
    /// a key touched but not written recomputes to the value it already had.
    #[test]
    fn an_over_marked_key_costs_a_leaf_and_changes_nothing() {
        let mut st = seeded(4);
        let mut cache = RootCache::default();
        let root = cache.refresh(&mut st).expect("root");
        st.members.touch(&2);
        st.members.touch(&99);
        st.edges.touch(&(1, 2));
        assert_eq!(cache.refresh(&mut st).expect("root"), root, "touching a key writes nothing");
        assert_eq!(cache.structural_rebuilds(), 0, "a key that was never there is not a removal");
    }

    /// A proof is a wire object: it travels from the member who produced it
    /// to whoever checks it.
    #[test]
    fn a_proof_round_trips_through_serde() {
        let mut st = seeded(3);
        st.members.get_mut(&0).unwrap().status = MemberStatus::Suspended;
        let root = state_root(&st).unwrap();
        let proof = prove_member(&st, 0).unwrap().expect("present");
        let bytes = crate::codec::encode(&proof).expect("serialize");
        let back: InclusionProof = crate::codec::decode(&bytes).expect("deserialize");
        assert_eq!(back, proof);
        assert!(verify(&root, &back));
    }
}
