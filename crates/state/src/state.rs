//! The ledger state and its per-epoch machinery.

use std::collections::{BTreeMap, BTreeSet};

use edet_kernel::constants as k;
use edet_kernel::flow;

use crate::journal::Journal;
use crate::params::Params;
use crate::types::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct State {
    pub params: Params,
    pub epoch: u64,
    pub members: Journal<MemberId, Member>,
    pub next_member: MemberId,
    pub key_index: BTreeMap<Key, MemberId>,
    /// What creditors have staked on each other, in minor units.
    ///
    /// This is the whole reputation system. An edge is not volume traded —
    /// settlement takes two signatures and no delivery, so volume is free to
    /// fabricate — but what a creditor put behind the debtor, capped by the
    /// creditor's own capacity at the moment they accepted.
    pub edges: Journal<(usize, usize), u64>,
    /// Flow held against outstanding insured credit, in minor units. A unit of
    /// standing backs one obligation at a time.
    pub reserved: Journal<(usize, usize), u64>,
    /// Flow drawn through each underwriter's supply arc, in minor units.
    ///
    /// The supply arc is a real arc of the network and it is shared by every
    /// debtor that underwriter backs. Leaving it uncharged is the difference
    /// between a bound and a suggestion: reservations recorded against
    /// distinct stake edges never meet, so one declared supply is lent once
    /// per debtor — measured at 250,000 against a true cut of 2,500 with a
    /// hundred of them.
    ///
    /// It is also the community's outstanding insured credit, since every
    /// insured unit crosses exactly one supply arc (§Verification invariant 2), and the
    /// floor a withdrawal may not go below (§Stability, invariant 5).
    pub committed: flow::Committed,
    /// Flow the live seats hold on stake arcs, minor units.
    ///
    /// **The write layer's reservation map, and capacity never reads it.** A
    /// row is a stock and its price is a reservation no transition releases:
    /// `Member.seat` names the arcs, this is their sum, and the one thing that
    /// gives one back is the sweep retiring a row that holds nothing, owes
    /// nothing and is named by nothing (`apply::retire_empty_rows`).
    ///
    /// An arc may decay BELOW what this holds, and that is deliberate rather
    /// than an oversight. `flow::decay` floors an edge at its CREDIT
    /// reservation and at nothing else, so the stake behind a seat fades like
    /// any other; the residual clamps at zero (`Network::build` reads
    /// `w.saturating_sub(held)`) and the seat is still spent. Flooring the
    /// arc here instead would hand a sponsor 500.00 of permanent capacity for
    /// 25 seats, which is the exploit rather than the fix — so invariant 7
    /// deliberately does NOT claim `seat_reserved <= edges`.
    #[serde(default)]
    pub seat_reserved: Journal<(usize, usize), u64>,
    /// Flow the live seats hold on supply arcs, minor units. Its total is one
    /// bond unit per row that carries a `seat`, which is what invariant 7
    /// conserves — and the ceiling the community reaches is
    /// `Σ supply / unit` rows, after which nobody seats until a ceremony
    /// raises the seed.
    #[serde(default)]
    pub seat_committed: flow::Committed,
    /// The underwriters and the supply each has declared, in minor units.
    /// **Every unit of it arrived through a ceremony**: genesis, or a seed
    /// amendment the community endorsed (§Governance). There is no other door.
    ///
    /// An underwriter is a member who has accepted a liability: if the members
    /// they back fail, this much of the loss is theirs. It is not a rank and
    /// not a privilege, and the set is deliberately OPEN — a community whose
    /// credit could only ever originate with its founders would wind down as
    /// they aged out. What opens it is `seed::enact`, not `DeclareSupply`:
    /// `DeclareSupply` may only ever LOWER a declaration.
    ///
    /// **A declaration made against capacity the community itself conferred
    /// would be hollow insurance**, which is why there is no door for one.
    /// Such a declaration becomes a source arc feeding the next member's
    /// capacity, which becomes their declaration: a seed of 100 and twelve
    /// joiners wash-backing each other reach a declared 204,900 between them,
    /// every unit of it a source arc, and the ledger then issues — LABELLED
    /// INSURED — 204,800 of credit to accounts the coalition controls, with
    /// every invariant satisfied. The consolation that suggests itself ("what
    /// those twelve can owe together stays at 100") is a statement about the
    /// DECLARERS as a set and says nothing about the sybils they back, which
    /// is who the credit goes to. `tests/adversarial.rs` measures both halves.
    ///
    /// Three properties make the set safe to leave open, and all three are
    /// arithmetic rather than policy. Supply bounds the AGGREGATE, so an
    /// underwriter who backs twenty accounts still risks only what they
    /// declared. A coalition cannot underwrite itself, because capacity draws
    /// supply only from underwriters OUTSIDE the set being measured. And a
    /// coalition cannot underwrite the accounts it CONTROLS either, because
    /// nothing the ledger itself confers can become a supply.
    ///
    /// So this is the community's real insured credit, and `external_seed`
    /// reads exactly this map. A separate field recording which part of it was
    /// external could never differ from it, and a fact stored twice is a fact
    /// that can drift.
    pub underwriters: BTreeMap<MemberId, u64>,
    /// External seed admitted by amendment in the open epoch, minor units;
    /// reset at the boundary. The epoch's rate bound is measured against it
    /// (`crate::seed`).
    #[serde(default)]
    pub seed_amended_this_epoch: u64,
    pub contracts: Journal<ContractId, Contract>,
    pub next_contract: ContractId,
    /// Forfeited operation bonds, keyed by the member that forfeited them.
    ///
    /// **`bond_headroom` consumes this, and that is the whole of the sanction**
    /// a forfeiture is a reservation that never returns. It was the loss
    /// pool's first-loss layer, and when the pool was retired (§Recourse) this became
    /// a number nothing read — while `forfeit_bonds` handed the headroom back on
    /// the spot, which made the "sanction" a release. Measured then: an abuser
    /// forfeited 2500, its headroom went 0 → 2231, and it wrote 22 more
    /// transitions at once.
    ///
    /// Nothing is minted, nobody is owed it, and no position anywhere improves,
    /// so `prop:no-rent` is intact. It is not absorbing either: the free
    /// allowance survives, the recovery path is priced at zero, and the way back
    /// is the model's own — outgrow it.
    ///
    /// Held per member, not as a scalar, because the amount is a *potential*
    /// obligation of a named abuser rather than a fund. `BTreeMap` so any
    /// future draw order is deterministic across validators.
    pub forfeit_reserve: BTreeMap<MemberId, u64>,
    pub proposals: Journal<ProposalId, Proposal>,
    pub next_proposal: ProposalId,
    /// Validator voting power (consensus driver reads this at epoch
    /// boundaries; changes are governance transitions).
    pub validators: BTreeMap<MemberId, u64>,
    /// Binds every signed payload (transactions via `SignedTx::id`,
    /// consensus messages via `engine_context::sign_bytes`) to THIS network,
    /// so a signature produced against one edet chain is never valid on
    /// another — the dev/testnet genesis seeds from published entropy
    /// (`block::dev_seed`), so without this a transaction signed against a
    /// testnet would be byte-identical to one signed against a real network
    /// sharing a founder's identity. Part of the replicated, hashed state,
    /// so every honest node agrees on it by construction — it is genesis
    /// data, not something a running network can quietly change.
    pub chain_id: String,
    /// Ids of transactions already applied — the replay defence — bucketed by
    /// the epoch after which each may be forgotten (`not_after_epoch -> ids`).
    ///
    /// `apply` records an id here BEFORE dispatch, unconditionally (see its
    /// doc comment), and refuses one already present. The bucketing is the
    /// bound: `begin_block` drops every bucket whose key falls behind the
    /// epoch it just closed, so this holds at most one
    /// `MAX_TX_LIFETIME_EPOCHS` window rather than the whole chain's history.
    ///
    /// **One structure, because the window is inside the digest.** An
    /// envelope's `not_after_epoch` is covered by the digest that names it
    /// (`SignedTx::id`), so an id can only ever have been recorded in the one
    /// bucket its own window names, and the replay test is a single lookup
    /// there (`is_applied`). A flat set beside this would answer the same
    /// question at the same cost, be hashed into the state root a second
    /// time, and be a fact stored twice — which is a fact that can drift.
    ///
    /// One leaf per bucket in the root's `Replay` section, over the fixed
    /// window `[epoch, epoch + MAX_TX_LIFETIME_EPOCHS]`, so a recorded id
    /// rehashes the epoch it expires in and nothing else.
    pub applied_by_expiry: Journal<u64, BTreeSet<[u8; 32]>>,
    /// The `now_secs` of the most recent `begin_block` that did work. Makes
    /// the epoch-advance clamp a per-BLOCK bound instead of a per-call one —
    /// see `begin_block`. Replicated (every node processes the same block
    /// stream, so every node holds the same value) and therefore part of the
    /// state hash, like every other field here.
    pub last_begin_secs: u64,
    /// The per-chain secret every state-root leaf is salted from
    /// (`root::leaf_salt`, the paper's §Implementation).
    ///
    /// Genesis data, like `chain_id`: every validator must hold the identical
    /// value or they compute different roots and the chain cannot agree. It
    /// never appears as a leaf VALUE — an inclusion proof for it would hand
    /// over the secret that keeps every other leaf unguessable — but it is
    /// committed to transitively, since every leaf's salt derives from it.
    ///
    /// Secret from anyone who was not given the ledger, not from members:
    /// validators and full nodes hold it, and the read views already
    /// gives validators blanket visibility. What it defends is the sibling
    /// hashes travelling inside somebody else's inclusion proof, whose
    /// preimages (amounts, ids, statuses) are otherwise a small enough space
    /// to brute-force.
    pub root_salt: [u8; 32],
}

impl Default for State {
    fn default() -> Self {
        State {
            params: Params::default(),
            epoch: 0,
            members: Default::default(),
            next_member: 0,
            key_index: BTreeMap::new(),
            edges: Default::default(),
            reserved: Default::default(),
            committed: Default::default(),
            seat_reserved: Default::default(),
            seat_committed: Default::default(),
            underwriters: BTreeMap::new(),
            seed_amended_this_epoch: 0,
            contracts: Default::default(),
            next_contract: 0,
            forfeit_reserve: BTreeMap::new(),
            proposals: Default::default(),
            next_proposal: 0,
            validators: BTreeMap::new(),
            chain_id: "edet-dev".to_string(),
            applied_by_expiry: Default::default(),
            last_begin_secs: 0,
            root_salt: DEV_ROOT_SALT,
        }
    }
}

/// The state-root salt a dev/test chain uses (`root_salt`). Published, like
/// every other dev secret in this tree (`block::dev_seed`), because a dev
/// chain's privacy is already nil and a fixed value keeps dev roots
/// reproducible across runs. A real genesis draws 32 random bytes instead —
/// `edet-node genesis init`.
pub const DEV_ROOT_SALT: [u8; 32] = *b"edet-dev-root-salt-v1___________";

/// How close to a whole minor unit a product has to land before
/// [`State::to_minor`] reads it as that unit rather than as the unit below.
///
/// Sized between the two things it has to separate, and there are eleven
/// orders of magnitude between them: an f64 round-trip artefact is one ulp,
/// relative 2⁻⁵³ ≈ 1.1e-16, while the nearest thing an amount can be to a
/// boundary and still mean the unit below is half a unit, relative 0.5/u.
/// Anything in that gap works; 1e-9 is far enough above the noise to survive a
/// few arithmetic steps and far enough below half a unit to never move a real
/// amount.
const MINOR_SNAP: f64 = 1e-9;

/// Re-denominate one minor-unit quantity, rounded DOWN.
///
/// Down, not nearest, at every boundary into the capacity path: rounding up
/// would let a reservation claim a unit of standing no stake actually carries,
/// and the cut bound is an inequality that must never be crossed by rounding.
fn scale_minor(v: u64, pi: f64) -> u64 {
    if !pi.is_finite() || pi <= 0.0 {
        return 0;
    }
    ((v as f64) * pi).floor().max(0.0) as u64
}

/// Re-denominate what one SEAT holds, at the route level.
///
/// A `Held` is a flow, not a list of numbers, and the difference shows the
/// moment anything divides one: flooring arc by arc leaves a node where three
/// arcs of 1 arrive and one of 3 departs holding 0 in and 1 out, which is no
/// longer a flow — and invariant 7 asks of every seat that it be one, because
/// that is what makes the net inward seat flow across a set's boundary equal
/// `unit x seats`. A path can be split at any value and every piece still is a
/// path, so the division happens there and conservation survives exactly.
///
/// An obligation's hold is floored arc by arc instead, and rightly: the audit
/// asks it for arc-level equality against the caches and reads its AMOUNT off
/// the supply side, while `shave_holds_to_what_the_arcs_deliver` then trims
/// supply entries alone. The seat layer has no shave — a seat arc may sit above
/// the stake beneath it by design — so the route level is available to it.
///
/// A hold that does not decompose is floored arc by arc. Nothing produces one:
/// `flow::reserve_with_own_supply` returns a flow and this preserves the
/// property, so the fallback is what keeps a corrupt hold a refusal at the
/// audit rather than a panic here.
fn rescale_seat_held(held: &mut flow::Held, sponsor: usize, pi: f64) {
    match flow::decompose(held, sponsor) {
        Ok(mut paths) => {
            for p in paths.iter_mut() {
                p.value = scale_minor(p.value, pi);
            }
            *held = flow::from_paths(&paths);
        }
        Err(_) => rescale_held(held, pi),
    }
}

/// Re-denominate what one obligation holds, arc by arc. An arc that rounds to
/// nothing is dropped, which is safe in the only direction that matters: the
/// obligation ends up holding less than it did, never more.
///
/// The caller then reads the obligation's AMOUNT back off this, rather than
/// scaling the amount on its own — see `State::rescale`. Scaling both
/// independently makes them disagree by a minor unit, which halts the chain.
fn rescale_held(held: &mut flow::Held, pi: f64) {
    for (_, amount) in held.edges.iter_mut() {
        *amount = scale_minor(*amount, pi);
    }
    for (_, amount) in held.supply.iter_mut() {
        *amount = scale_minor(*amount, pi);
    }
    held.edges.retain(|&(_, a)| a > 0);
    held.supply.retain(|&(_, a)| a > 0);
}

impl State {
    /// Advance to the epoch containing `now_secs`, closing intermediate
    /// epochs (clock, counters) and recomputing the network quantities at
    /// each boundary.
    ///
    /// `target` is clamped to `self.epoch + MAX_EPOCH_ADVANCE_PER_BLOCK`
    /// regardless of how far `now_secs` claims to be — this function stays
    /// infallible (it is called from `apply`, deep inside the commit path,
    /// where returning an error would need a large refactor) and clamping is
    /// the correct behaviour for a safety net: applying a block can never
    /// hang the process no matter what timestamp it carries, it can only
    /// under-advance the epoch. In a healthy network this branch never
    /// fires — `crate::block`'s validity rule rejects any block whose
    /// timestamp would demand it, before this ever runs. See the constant's
    /// own doc comment for why the bound is unreachable in practice.
    ///
    /// The early return is what makes that clamp a PER-BLOCK bound rather than
    /// a per-call one, and it is load-bearing. `apply` calls `begin_block`
    /// again for every transaction, on top of the once-per-block call
    /// `Replica::apply_block_to` already makes. Before the clamp existed that
    /// redundancy was free: an unclamped target is a fixed function of
    /// `now_secs`, so repeat calls with the same timestamp found `self.epoch`
    /// already at the target and did nothing. The clamp broke that, because
    /// `self.epoch + MAX` is a MOVING target — each repeat call could
    /// re-clamp and grind through another full allowance, making the true
    /// bound `(N_txs + 1) x MAX_EPOCH_ADVANCE_PER_BLOCK` and handing an
    /// attacker back a block-size-scaled version of the very hang the clamp closes
    /// (measured: 2000 trivial transactions and `u64::MAX` drove the epoch to
    /// 2001x the ceiling and took 42s for ONE block). Skipping a repeat call
    /// for a timestamp already processed restores the no-op property the
    /// clamp accidentally removed, and does it here — in the structural
    /// layer — so it holds however many times, and from wherever, this is
    /// called. Do not "optimise" this check away.
    pub fn begin_block(&mut self, now_secs: u64) {
        if now_secs == self.last_begin_secs {
            return;
        }
        self.last_begin_secs = now_secs;
        let raw_target = now_secs / k::EPOCH_SECS;
        let target = raw_target.min(self.epoch.saturating_add(k::MAX_EPOCH_ADVANCE_PER_BLOCK));
        let advanced = self.epoch < target;
        while self.epoch < target {
            for m in self.members.values_mut() {
                m.rep.d_in = 0.0;
                m.rep.d_out = 0.0;
                // Fold the epoch's denial record into the saturation counter
                // BEFORE clearing it: a member denied at any point in the
                // closing epoch advances toward forfeiture, and one that was
                // never denied resets to zero outright. The reset is what
                // keeps an honest burst — throttled for one epoch, clear the
                // next — from ever accumulating toward a sanction.
                if m.bond_denied_this_epoch {
                    m.bond_saturated_epochs += 1;
                } else {
                    m.bond_saturated_epochs = 0;
                }
                m.bond_denied_this_epoch = false;
                m.bond_free_used = 0;
            }
            self.epoch += 1;
            self.seed_amended_this_epoch = 0;
            // Standing fades unless trade renews it. Never below what is
            // reserved: the debt did not shrink because the evidence aged, so
            // the collateral behind it may not either.
            let (num, den) = self.params.decay_ratio();
            flow::decay(&mut self.edges, &self.reserved, num, den);
            self.prune_expired_applied_ids();
        }
        // The permissionless cranks, run by the epoch itself rather than by
        // whoever happens to care (`apply::sweep_cranks`, the paper's §Recourse).
        //
        // ONCE, after the loop, and never inside it: a fresh chain closes up
        // to `MAX_EPOCH_ADVANCE_PER_BLOCK` epochs in a single block while its
        // clock catches up with the wall, and a per-epoch sweep would walk the
        // contract book ten thousand times for one block. Expiry is a
        // comparison against the epoch this call ends at, so one pass at the
        // end sees exactly what every intermediate pass would have seen, and
        // the bond-saturation counters the second half reads are folded by the
        // loop above before it runs.
        if advanced {
            crate::apply::sweep_cranks(self);
            // **After the sweep, never before it.** `release_bonds` inside the
            // loop above hands every bond back before `ForfeitBonds` looks, and
            // at the genesis release period of one epoch that makes forfeiture
            // unreachable outright: a member saturated for seven consecutive
            // epochs, re-exhausting its headroom every one of them, keeps
            // `forfeit_reserve` empty throughout and goes on writing 21 to 24
            // transitions an epoch for ever.
            //
            // Moving it out of the loop changes nothing about which bonds
            // release. The cutoff is `epoch + 1` and the map is keyed by release
            // epoch, so one pass at the epoch this call ends at hands back
            // exactly what a pass per epoch would have — the same argument that
            // puts `sweep_cranks` here rather than inside the loop.
            self.release_bonds();
        }
    }

    /// Drop every applied transaction id whose claimed validity window
    /// (`not_after_epoch`) is now behind the just-closed epoch. Run once per
    /// epoch closed inside `begin_block`'s loop (not once per call) so a
    /// single block that closes many epochs at once still prunes every
    /// bucket that falls due along the way, not just the final one.
    ///
    /// Safe to prune the moment a bucket's key is `< self.epoch`: `apply`
    /// independently refuses `not_after_epoch < state.epoch` as
    /// `ET_TX_EXPIRED`, so once an id's window has elapsed it could never
    /// have been successfully replayed anyway — forgetting it here does not
    /// reopen a hole, it only stops paying to remember something the expiry
    /// check would refuse regardless. This is the load-bearing interaction
    /// between the two mechanisms: the set bounds membership cost, the
    /// window bounds correctness once membership is forgotten.
    fn prune_expired_applied_ids(&mut self) {
        let stale: Vec<u64> = self.applied_by_expiry.range(..self.epoch).map(|(&k, _)| k).collect();
        for k in stale {
            self.applied_by_expiry.remove(&k);
        }
    }

    /// Record that `tx_id` has been applied, retained until
    /// `not_after_epoch` (inclusive) has fully elapsed. Called from `apply`
    /// UNCONDITIONALLY, before dispatch — see that function's doc comment for
    /// why a transaction that goes on to fail must still be recorded.
    pub(crate) fn record_applied(&mut self, tx_id: [u8; 32], not_after_epoch: u64) {
        self.applied_by_expiry.entry(not_after_epoch).or_default().insert(tx_id);
    }

    /// Whether this envelope has already been applied.
    ///
    /// One bucket, never a scan: the digest that produced `tx_id` covers
    /// `not_after_epoch`, so no other bucket could hold it.
    pub fn is_applied(&self, tx_id: &[u8; 32], not_after_epoch: u64) -> bool {
        self.applied_by_expiry
            .get(&not_after_epoch)
            .is_some_and(|ids| ids.contains(tx_id))
    }

    /// How many ids the replay cache is holding, across every live bucket.
    /// What a probe about the cost of a write reads.
    pub fn applied_count(&self) -> usize {
        self.applied_by_expiry.values().map(|ids| ids.len()).sum()
    }

    /// Undo one `record_applied`, for an envelope that turned out to authorise
    /// nothing (`apply`'s `ET_MEM_NOT_SIGNER` release — see its note on why
    /// that case, and only that case, may be released).
    ///
    /// An emptied bucket is removed rather than left behind: an absent
    /// bucket and an empty one commit to the same leaf, so leaving it would
    /// be a row nothing reads, and the whole point of the window is that the
    /// cache holds only what it must.
    pub(crate) fn forget_applied(&mut self, tx_id: &[u8; 32], not_after_epoch: u64) {
        if let Some(bucket) = self.applied_by_expiry.get_mut(&not_after_epoch) {
            bucket.remove(tx_id);
            if bucket.is_empty() {
                self.applied_by_expiry.remove(&not_after_epoch);
            }
        }
    }

    /// Return every operation bond whose release epoch has arrived.
    ///
    /// Run once per epoch closed inside `begin_block`'s loop, not once per
    /// call, so a block that closes several epochs at once returns every bond
    /// that fell due along the way rather than only the last epoch's. A bond
    /// that failed to release would be indistinguishable from a fee, which is
    /// the one thing this mechanism must never become.
    fn release_bonds(&mut self) {
        // Split at `epoch + 1`, not at `epoch`: a bond keyed to release AT
        // this epoch is due NOW, so it must fall on the released side. Using
        // `epoch` here holds every bond exactly one epoch too long, which for
        // the default `T_b = 1` means bonds that never release at all —
        // silently turning the reservation into a fee.
        let cutoff = self.epoch.saturating_add(1);
        for m in self.members.values_mut() {
            // `split_off` hands back everything at or above the cutoff (the
            // bonds still held); what stays behind is the release.
            let still_held = m.bonds.split_off(&cutoff);
            m.bonds = still_held;
        }
    }

    // ----------------------------------------------------------- capacity --

    /// Minor units for a denomination amount, rounded DOWN — with the floor
    /// taken on the QUANTITY, never on an f64 artefact of it.
    ///
    /// Down, not nearest, at every boundary into the capacity path: rounding
    /// up would let a reservation claim a unit of standing that no stake
    /// actually carries, and the cut bound is an inequality that must never be
    /// crossed by rounding. That half is unchanged, and the snap below never
    /// touches it — a genuine 0.289 is a third of a unit clear of any boundary
    /// and still floors to 28.
    ///
    /// What a plain `floor` got wrong is the other half. `x * MINOR` is not the
    /// real product but the nearest double to it, and for **6.5% of minor
    /// values — 16% in the 2400–2500 band a realistic seed sits in** —
    /// `from_minor(u) * MINOR` lands one ulp BELOW `u`, so the floor takes a
    /// whole unit off a quantity that was exact to begin with.
    /// `to_minor(from_minor(29))` was 28.
    ///
    /// That is not a conservative rounding, it is a wrong answer, and it is
    /// wrong in the one direction that halts a chain: a stake capped one unit
    /// under what the creditor may confer, and §Verification invariant 1
    /// comparing the drawn flow against a cut one unit under the real one. A
    /// fully drawn community whose cut is one of those values fails its own
    /// audit, on ordinary traffic and with nothing wrong.
    ///
    /// So: snap to the nearest unit when the product is within a relative 1e-9
    /// of it, floor otherwise. `round` and `floor` are exact IEEE operations,
    /// so this stays a function of the bits and §Implementation is untouched.
    ///
    /// **This function is the BOUNDARY, and it is crossed once.** Amounts are
    /// `u64` minor units everywhere in state, so the round trip this corrects
    /// happens where a transaction's payload enters `apply` and where a view
    /// leaves `serve` — and nowhere in between. A quantity that crosses here
    /// twice is a defect: the ledger's own arithmetic never leaves the
    /// integers, which is what lets §Verification ask for exact equality
    /// instead of a tolerance.
    ///
    /// **It CLAMPS at both ends, and a clamp is not a refusal.** A NaN and a
    /// negative floor to zero, and the closing `as` cast saturates, so a finite
    /// `1e300` is `u64::MAX` minor units. Neither is a rounding of what the
    /// signer wrote, so neither may be booked: the bottom is refused by
    /// `to_minor(x) == 0` at every call site, and the top by
    /// [`State::amount_representable`], asked BEFORE this function at every
    /// ingress that books what it names — or, in `settle`, `cure` and
    /// `declare_supply`, by the comparison against the book that follows the
    /// conversion, which a clamped value cannot pass.
    pub fn to_minor(x: f64) -> u64 {
        if !x.is_finite() || x <= 0.0 {
            return 0;
        }
        let scaled = x * k::MINOR;
        let nearest = scaled.round();
        if nearest >= 0.0 && (scaled - nearest).abs() <= MINOR_SNAP * nearest.max(1.0) {
            return nearest as u64;
        }
        scaled.floor() as u64
    }

    /// Denomination amount for minor units. Exactly inverted by
    /// [`State::to_minor`] — see there for why that had to be made true.
    pub fn from_minor(u: u64) -> f64 {
        u as f64 / k::MINOR
    }

    /// Whether a wire amount may be booked at all: finite, non-negative, and
    /// at or under `MAX_AMOUNT_MINOR` once it is on the grid.
    ///
    /// **Asked BEFORE [`State::to_minor`], because `to_minor` clamps in both
    /// directions and a clamp is not a refusal.** Above the ceiling the cast
    /// saturates, so the conversion itself can no longer tell an amount from
    /// the largest number there is — which is why the test is stated on the
    /// conversion's OUTPUT and applied at the ingress, where refusing is still
    /// possible.
    ///
    /// It does not appear in §Verification, and must not: `rescale` at the top
    /// of the re-denomination band legitimately carries a ceiling-sized amount
    /// past it, and an audit clause here would halt a chain on a lawful
    /// re-denomination. This is an ingress bound on what a payload may name,
    /// not a property of the book.
    pub fn amount_representable(x: f64) -> bool {
        x.is_finite() && x >= 0.0 && Self::to_minor(x) <= k::MAX_AMOUNT_MINOR
    }

    /// Flow indices are member ids. Ids are dense from zero and never reused,
    /// so the ledger's own identifier IS the graph index and no side table can
    /// drift out of step with the member map.
    pub fn flow_n(&self) -> usize {
        self.next_member as usize
    }

    /// The underwriters and their supplies, in the shape the kernel takes.
    ///
    /// Built in `BTreeMap` order, so the flow network is a function of state
    /// rather than of insertion history — one half of §Implementation's determinism, the
    /// other being that nothing here touches floating point.
    ///
    /// Every declared supply appears, whatever the underwriter's STATUS. A
    /// supply is an accepted liability, not a privilege, and suspending a
    /// member does not release the credit already standing on them — the same
    /// reason §Stability floors a withdrawal at the committed flow. Suspension revokes
    /// origination, which `capacity_of` enforces on the borrowing side.
    fn uw(&self) -> Vec<(usize, u64)> {
        self.underwriters.iter().map(|(&id, &supply)| (id as usize, supply)).collect()
    }

    /// Capacity of an account set: the maximum flow the underwriters can push
    /// into it, over stakes net of what outstanding credit already reserves.
    ///
    /// The set form is the security statement. Because this is a cut, the
    /// bound holds for a coalition as a whole — splitting across identities
    /// gains nothing, since stakes internal to the set never cross its own
    /// boundary, and underwriters inside the set supply it nothing.
    pub fn capacity_of_set(&self, ids: &[MemberId]) -> f64 {
        Self::from_minor(self.capacity_of_set_minor(ids))
    }

    /// `capacity_of_set` in minor units — the reading every internal caller
    /// uses. The graph is integer, the ledger is integer, and the `f64` above
    /// exists for the API edge alone.
    pub fn capacity_of_set_minor(&self, ids: &[MemberId]) -> u64 {
        let targets: Vec<usize> = ids.iter().map(|&id| id as usize).collect();
        flow::capacity(&self.edges, &self.reserved, &self.committed, &self.uw(), &targets, self.flow_n(), u64::MAX)
    }

    /// Capacity of an account set on a **pristine residual**: the gross cut,
    /// with nothing reserved out of it.
    ///
    /// This is the quantity §Verification invariant 1 compares outstanding credit
    /// against, and it must not be confused with `capacity_of_set`. That one
    /// answers "how much MORE can this set borrow", which is the gross cut
    /// minus what is already drawn — so comparing credit outstanding against
    /// it compares a number with itself subtracted out, and reads zero at
    /// exactly the moment the ceiling is fully and legitimately drawn.
    pub fn gross_capacity_of_set(&self, ids: &[MemberId]) -> f64 {
        Self::from_minor(self.gross_capacity_of_set_minor(ids))
    }

    /// `gross_capacity_of_set` in minor units.
    pub fn gross_capacity_of_set_minor(&self, ids: &[MemberId]) -> u64 {
        let targets: Vec<usize> = ids.iter().map(|&id| id as usize).collect();
        let (pristine_r, pristine_c) = (flow::Reservations::new(), flow::Committed::new());
        flow::capacity(&self.edges, &pristine_r, &pristine_c, &self.uw(), &targets, self.flow_n(), u64::MAX)
    }

    /// Capacity of one account, with no status gate: the raw graph answer.
    ///
    /// This is what `conferrable` reads and what the invariant audit compares
    /// against, because both are questions about the GRAPH. `capacity_of` is
    /// the origination test and gates on status; the two are deliberately
    /// different readings of the same quantity.
    pub fn capacity_raw(&self, id: MemberId) -> f64 {
        self.capacity_of_set(&[id])
    }

    /// `capacity_raw` in minor units.
    pub fn capacity_raw_minor(&self, id: MemberId) -> u64 {
        self.capacity_of_set_minor(&[id])
    }

    /// Capacity of one account: what the community has put behind them.
    ///
    /// Zero for an account nobody has staked on, by arithmetic rather than by
    /// rule — which is why account creation needs no approval and no rate
    /// limit. A suspended or exited account is refused outright: suspension
    /// revokes origination, and capacity is what origination consults.
    pub fn capacity_of(&self, id: MemberId) -> f64 {
        Self::from_minor(self.capacity_of_minor(id))
    }

    /// `capacity_of` in minor units, status gate and all.
    pub fn capacity_of_minor(&self, id: MemberId) -> u64 {
        let Some(m) = self.members.get(&id) else { return 0 };
        if !matches!(m.status, MemberStatus::Active) {
            return 0;
        }
        self.capacity_raw_minor(id)
    }

    /// What a member may confer on somebody else: their declared supply if
    /// they are an underwriter, otherwise their own capacity.
    ///
    /// The two must not be conflated. Capacity is how much the community will
    /// carry YOU; supply is how much you have promised to carry others. Supply
    /// is not re-derived at read time — it was capped by capacity when
    /// declared and stands until changed, because a promise that silently
    /// shrank would not be one — and it is not netted against what the
    /// underwriter has already committed, because what they may confer is a
    /// limit and limits propagate freely. Only simultaneous use is bounded.
    pub fn conferrable(&self, id: MemberId) -> f64 {
        Self::from_minor(self.conferrable_minor(id))
    }

    /// `conferrable` in minor units.
    pub fn conferrable_minor(&self, id: MemberId) -> u64 {
        flow::conferrable(&self.edges, &self.reserved, &self.committed, &self.uw(), id as usize, self.flow_n())
    }

    /// **What the community's external seed reaches this account for** — §Standing's
    /// max-flow over the underwriters' source arcs, plus this account's own
    /// declared supply.
    ///
    /// This is the write floor's basis, and the reason it is not
    /// `conferrable` is measured. `conferrable` for an underwriter is their
    /// DECLARED supply, and a declaration made against capacity the community
    /// itself conferred — a promise on the strength of another promise —
    /// inflates **geometrically**, each member's declaration becoming a source
    /// arc feeding the next member's capacity, which becomes their
    /// declaration. Measured on six founders at 2500 with a single honest 300
    /// trade behind the whole chain, at the constitutional bond ceiling: the
    /// coalition's write headroom **doubles per accomplice** — 2,400 at four
    /// links, 19,200 at seven, 153,600 at ten, **614,400 at twelve** — while
    /// the real cut over the whole set stays **300** throughout. Free accounts
    /// are free, so that is not a 64× amplification, it is an unbounded write
    /// channel, and it makes "traffic is bounded by earned standing" vacuous.
    ///
    /// **`DeclareSupply` refuses every raise**, so no such chain can be built:
    /// every supply arc is ceremony-seated and `underwriters` IS the external
    /// seed. This function therefore agrees with `conferrable_gross` on every
    /// account that is not itself an underwriter, and exceeds it by the
    /// account's own supply where it is. It is kept as the write floor's basis
    /// rather than folded into `conferrable`, because the two answer different
    /// questions and only one of them may ever be widened: this one is what a
    /// member has to LOSE, and `conferrable` is what they may place on
    /// somebody else.
    ///
    /// **Why capacity alone is no defence, and why this is not "read the seed".** A
    /// founding underwriter's capacity is ZERO — a cut into an underwriter draws
    /// on the *other* underwriters, and at genesis there are none — which is
    /// exactly why the write floor read `conferrable` in the first place. It was
    /// concluded from that that the write floor could not move to the external
    /// seed "because then only underwriters could write at all", and that is
    /// true of reading a member's own supply alone. It is not true of reading
    /// the max-flow OVER the seeded arcs, which is the same distinction
    /// §Governance makes between a member's own seed and what the seed reaches.
    /// Measured on the same construction: every accomplice reads exactly
    /// **300**, its real reach through the one honest trade, rather than the
    /// 300…307,200 a declaration-based reading gives it, while a **founder
    /// stays at 2500** and an **honest member backed for 300 stays at 300**.
    /// The exponential is linear in the number of accounts, and linear at
    /// 1.00× — a coalition of k free keys reached through one honest edge
    /// writes exactly what k honest members with that same backing write,
    /// which is the standard the rest of the model already holds itself to.
    ///
    /// The per-member figure still over-counts a SET that shares one bottleneck,
    /// and that is inherent rather than a residue: the honest case over-counts
    /// identically, the bounding quantity is a cut, and a cut is defined over a
    /// set nobody names at write time.
    ///
    /// **It is GROSS of live credit, and that is the half a write floor is
    /// easiest to get wrong.** Mirroring `conferrable`'s netting here, on the
    /// argument that swapping one basis for the other should change only the
    /// basis. `conferrable` is a credit limit, where netting is the whole point
    /// — a unit of standing backs one obligation at a time (§Standing). A write floor
    /// is not a limit on simultaneous use: it is what a member has to LOSE, and
    /// `bond_headroom` already subtracts what they owe. Mirroring the netting
    /// therefore did two things nobody decided:
    ///
    /// - **It subtracted a member's own debt twice.** Their obligation reserves
    ///   the arcs into them, so the residual reach falls by the amount, and then
    ///   `− debt_out` takes the same amount again. Measured: a member reached for
    ///   500 who borrows 200 had reach 300 and headroom **100**, where
    ///   `prop:write-floor` says 500 − 200 = **300**.
    /// - **It subtracted everybody else's debt as well.** A member who has
    ///   borrowed nothing lost their floor because another member drew the arc
    ///   that reaches them. Measured at **20%** community utilisation: one
    ///   member draws a 500 underwriter, and a second member hanging off it goes
    ///   headroom 500 → 0, allowance 32 → 0, having done nothing at all. Drawn to
    ///   its ceiling the community stopped trading altogether — `Accept`, `Sale`
    ///   and a newcomer's first trade all refused `ET-BND-001` — which is the
    ///   failure the comment on `bond_headroom` says must not happen, and the
    ///   opposite of what §Standing says a full ceiling does (it withholds INSURANCE,
    ///   it does not withhold trade) and of what the client promised on the
    ///   network page.
    ///
    /// So the reached part is flat, exactly like the account's own external part
    /// beside it, and the only netting is `bond_headroom`'s own. Nothing about
    /// The write floor's basis changes: the arcs are still the seeded ones, a key nobody has
    /// backed still reaches none of them, and the k-accomplice parity is
    /// re-measured under this reading rather than assumed
    /// (`tests/bonds.rs`).
    pub fn seed_reach(&self, id: MemberId) -> f64 {
        Self::from_minor(self.seed_reach_minor(id))
    }

    /// `seed_reach` in minor units — what `bond_headroom` subtracts from, and
    /// therefore the only reading in which the write floor's arithmetic is
    /// exact.
    pub fn seed_reach_minor(&self, id: MemberId) -> u64 {
        let reached = flow::capacity(
            &self.edges,
            &flow::Reservations::new(),
            &flow::Committed::new(),
            &self.uw(),
            &[id as usize],
            self.flow_n(),
            u64::MAX,
        );
        reached + self.underwriters.get(&id).copied().unwrap_or(0)
    }

    /// **What one row costs**: one bond unit of flow, in minor units.
    ///
    /// No constant of its own. `Accept` and `Sale` — the two transitions that
    /// can seat — carry a work multiple of 1.0, so the seat's slope IS the
    /// bond unit, and moving it is the `ParamKey::BondFraction` question that
    /// already exists rather than a second one nobody governs.
    pub fn seat_price_minor(&self) -> u64 {
        self.params.bond_unit_minor()
    }

    /// **The write layer's reach**: the seed's flow to `id` over the stake
    /// graph, gross of live credit, net of every live seat, the member's own
    /// supply arc included.
    ///
    /// Three readings of one graph, and they differ in exactly what they net.
    /// `capacity_of` nets outstanding CREDIT and excludes a target's own
    /// supply, because nobody underwrites their own borrowing. `seed_reach`
    /// nets nothing, because a write floor is what a member has to LOSE.
    /// This one nets SEATS, because a seat is spent and never returns — and it
    /// counts the member's own supply, because a founding underwriter holds
    /// supply and no in-stakes and has to be able to bring the first members
    /// in.
    ///
    /// Not memoised. `GateCache` holds lower bounds on `seed_reach` keyed on
    /// the three things that lower a cut, and a seat lowers this one without
    /// moving any of them — a bound that went stale downward here would admit
    /// a seat the bound forbids, which is the one direction a memo may not
    /// err in. It is asked only by a transaction that seats.
    pub fn seat_reach_minor(&self, id: MemberId) -> u64 {
        flow::capacity_with_own_supply(
            &self.edges,
            &self.seat_reserved,
            &self.seat_committed,
            &self.uw(),
            &[id as usize],
            self.flow_n(),
            u64::MAX,
        )
    }

    /// `seat_reach_minor` in denomination units — what `/member` serves, so a
    /// client can say how many more newcomers a member's standing carries.
    pub fn seat_reach(&self, id: MemberId) -> f64 {
        Self::from_minor(self.seat_reach_minor(id))
    }

    /// Whether `rows` seats fit on `id`'s reach — asked the way `seat_pair`
    /// will take them, one reservation after another on the residual the one
    /// before it left, so that the gate and the write cannot disagree.
    ///
    /// One row is one flow, asked with the limit so the search stops as soon
    /// as the answer is known. Two rows are NOT one flow of two units: a
    /// maximum flow reroutes through the reverse arcs of its own earlier
    /// augmentation, and `reserve` never reroutes an earlier hold. On a graph
    /// where two unit paths into the sponsor share nothing but a cross arc —
    /// U→A, U→B, A→X, A→Y, B→Y, X→S, Y→S, one unit each — a two-unit flow
    /// fits by pushing the second unit back along A→Y, while the first unit
    /// taken alone runs U→A→Y→S and leaves no path for the second. A gate that
    /// asked the two-unit flow admitted that trade and the write refused it
    /// with the work bond already charged. So this takes the same sequence of
    /// holds on a scratch copy of the two seat maps: the maps only, cloned
    /// once per two-row trade, which is the rarest transaction there is.
    /// `tests/seats.rs` holds the two equal over that shape and over the plain
    /// ones.
    pub fn can_seat_minor(&self, id: MemberId, rows: u64) -> bool {
        let unit = self.seat_price_minor();
        if rows == 0 || unit == 0 {
            return true;
        }
        let uw = self.uw();
        let n = self.flow_n();
        if rows == 1 {
            return flow::capacity_with_own_supply(
                &self.edges,
                &self.seat_reserved,
                &self.seat_committed,
                &uw,
                &[id as usize],
                n,
                unit,
            ) >= unit;
        }
        let mut reserved: flow::Reservations = (*self.seat_reserved).clone();
        let mut committed = self.seat_committed.clone();
        (0..rows).all(|_| {
            flow::reserve_with_own_supply(&self.edges, &mut reserved, &mut committed, &uw, id as usize, n, unit)
                .is_some()
        })
    }

    /// Take one seat's flow against `sponsor`. `None`, changing nothing, if
    /// the residual cannot carry it.
    ///
    /// `raw_mut` then `touch` exactly the keys the `Held` names — the shape
    /// `reserve_capacity_minor` uses, for the same reason: `&mut` would mark
    /// the whole map, which is one of the three the root's `Stakes` leaves are
    /// built from.
    ///
    /// It never reroutes an earlier seat. A full re-solve could sometimes fit
    /// one more row than this refuses, which is the same first-come
    /// conservatism the insured tier already carries — and it can never admit
    /// a seat the bound forbids, which is the direction that matters.
    pub(crate) fn reserve_seat(&mut self, sponsor: MemberId) -> Option<flow::Held> {
        let want = self.seat_price_minor();
        if want == 0 {
            return Some(flow::Held::default());
        }
        let uw = self.uw();
        let n = self.flow_n();
        let held = flow::reserve_with_own_supply(
            &self.edges,
            self.seat_reserved.raw_mut(),
            &mut self.seat_committed,
            &uw,
            sponsor as usize,
            n,
            want,
        );
        if let Some(h) = &held {
            for &(key, _) in &h.edges {
                self.seat_reserved.touch(&key);
            }
        }
        held
    }

    /// Give back one seat's flow — for a reservation taken inside a transition
    /// that then refused, and for the sweep retiring an empty row
    /// (`apply::retire_empty_rows`), and for nothing else.
    ///
    /// **No transition releases a live seat**, which is the whole of why the
    /// bound has no time in it; a row that is empty is not a stock, and the
    /// sweep is the one caller that says so.
    pub(crate) fn release_seat(&mut self, held: &flow::Held) {
        for &(key, _) in &held.edges {
            self.seat_reserved.touch(&key);
        }
        flow::release(self.seat_reserved.raw_mut(), &mut self.seat_committed, held);
    }

    /// Whether `amount` fits within `debtor`'s free capacity — the insured
    /// test, asked without computing the maximum.
    pub fn fits_capacity(&self, debtor: MemberId, amount: f64) -> bool {
        let Some(m) = self.members.get(&debtor) else { return false };
        if !matches!(m.status, MemberStatus::Active) {
            return false;
        }
        let want = Self::to_minor(amount);
        want == 0
            || flow::capacity(
                &self.edges,
                &self.reserved,
                &self.committed,
                &self.uw(),
                &[debtor as usize],
                self.flow_n(),
                want,
            ) >= want
    }

    /// Whether `debtor` could carry `amount` once `held` has been given back.
    ///
    /// The question a `Transfer` must answer BEFORE it mutates anything, and it
    /// cannot be answered by `fits_capacity` alone: the outgoing debtor's own
    /// reservation is still standing at that point, and it may run along arcs
    /// the incoming debtor needs. Asking on the live residual would report "no
    /// room" for a transfer that in fact has room the moment the old hold is
    /// released — and since the answer decides whether the creditor's signature
    /// is required, a conservative reading is a signature demanded for nothing.
    ///
    /// Answered on COPIES, so it stays a pure read. `dispatch` has no rollback,
    /// so every question whose answer gates a mutation has to be asked before
    /// the first write.
    pub fn fits_capacity_after_release(&self, debtor: MemberId, amount: f64, held: &flow::Held) -> bool {
        self.fits_capacity_minor_after_release(debtor, Self::to_minor(amount), held)
    }

    /// `fits_capacity_after_release` in minor units.
    pub fn fits_capacity_minor_after_release(&self, debtor: MemberId, want: u64, held: &flow::Held) -> bool {
        self.insurable_minor_after_release(debtor, want, held) >= want
    }

    /// How much of `want` `debtor` could carry insured once `held` is given
    /// back — the same question as `fits_capacity_after_release`, answered
    /// with the AMOUNT rather than the verdict.
    ///
    /// The cascade needs the amount. A `Transfer` moves a whole claim and so
    /// only ever needs yes or no, but a discharge that cannot be carried whole
    /// may still be carried in part, and the part the successor cannot carry
    /// simply stays with its original debtor. `flow::capacity` is already
    /// limit-capped, so this is min(residual, want) for free.
    pub(crate) fn insurable_minor_after_release(&self, debtor: MemberId, want: u64, held: &flow::Held) -> u64 {
        if want == 0 {
            return 0;
        }
        let Some(m) = self.members.get(&debtor) else { return 0 };
        if !matches!(m.status, MemberStatus::Active) {
            return 0;
        }
        let (mut reserved, mut committed) = (self.reserved.clone(), self.committed.clone());
        flow::release(&mut reserved, &mut committed, held);
        flow::capacity(&self.edges, &reserved, &committed, &self.uw(), &[debtor as usize], self.flow_n(), want)
    }

    /// `insurable_minor_after_release` in denomination units.
    pub fn insurable_after_release(&self, debtor: MemberId, amount: f64, held: &flow::Held) -> f64 {
        Self::from_minor(self.insurable_minor_after_release(debtor, Self::to_minor(amount), held))
    }

    /// Hold flow against a new insured obligation, returning exactly the arcs
    /// the augmentation consumed. `None` — changing nothing — if the residual
    /// cannot carry it, which is not an error: the obligation is then
    /// uninsured, and the creditor bears it alone (§Recourse).
    ///
    /// The caller MUST store what comes back on the obligation. Both halves of
    /// the path are charged, stake edges and supply arcs alike, and settlement
    /// gives back precisely this and nothing else.
    pub fn reserve_capacity(&mut self, debtor: MemberId, amount: f64) -> Option<flow::Held> {
        self.reserve_capacity_minor(debtor, Self::to_minor(amount))
    }

    /// `reserve_capacity` in minor units — what every transition calls, since
    /// the amount has already crossed the boundary by the time it books.
    pub fn reserve_capacity_minor(&mut self, debtor: MemberId, want: u64) -> Option<flow::Held> {
        if want == 0 {
            return Some(flow::Held::default());
        }
        let uw = self.uw();
        let n = self.flow_n();
        // `raw_mut`, then touch exactly what came back. `flow::reserve`
        // writes the arcs of the `Held` it returns and nothing else, so the
        // keys are namable — and `&mut` would mark the whole reservation map,
        // which is the map the root's `Stakes` leaves are built from.
        let held =
            flow::reserve(&self.edges, self.reserved.raw_mut(), &mut self.committed, &uw, debtor as usize, n, want);
        if let Some(h) = &held {
            for &(key, _) in &h.edges {
                self.reserved.touch(&key);
            }
        }
        held
    }

    /// Give back exactly what an obligation held.
    pub(crate) fn release_capacity(&mut self, held: &flow::Held) {
        // The released arcs are the ones the obligation held, and `release`
        // touches no others — including the `retain` that drops an arc back
        // to nothing, which is a value change on a `Stakes` leaf and never a
        // leaf of its own.
        for &(key, _) in &held.edges {
            self.reserved.touch(&key);
        }
        flow::release(self.reserved.raw_mut(), &mut self.committed, held);
    }

    /// The least an underwriter may reduce their supply to: the flow currently
    /// drawn through them (§Stability, §Verification invariant 5).
    ///
    /// A withdrawal is decay applied to a source arc and takes the same floor —
    /// the debt did not shrink because the underwriter changed their mind.
    /// Without it, leaving while credit stands on you breaks the cut bound
    /// outright: one of six underwriters walking away from a fully drawn
    /// community left capacity 12,500 against 15,000 outstanding.
    pub fn supply_floor(&self, underwriter: MemberId) -> u64 {
        flow::supply_floor(&self.committed, underwriter as usize)
    }

    /// Record a settled obligation as stake, capped by what the creditor may
    /// confer.
    ///
    /// This is the only way the stake graph grows, and the cap is what makes
    /// fabricated settlement worthless: between two accounts that may confer
    /// nothing, every settlement stakes zero however much passes between them.
    pub(crate) fn record_stake(
        &mut self,
        creditor: MemberId,
        debtor: MemberId,
        amount: u64,
        creditor_conferrable: u64,
    ) {
        let key = (creditor as usize, debtor as usize);
        self.edges.touch(&key);
        flow::stake(self.edges.raw_mut(), key.0, key.1, amount, creditor_conferrable);
    }

    /// Place a stake exactly as a settlement would, capped by what the
    /// creditor may actually confer.
    ///
    /// For genesis fixtures and harnesses that need a community with standing
    /// in it without driving every founding trade through `apply`. It cannot
    /// manufacture standing and is safe to expose for that reason: the cap is
    /// READ from the graph rather than supplied by the caller, so between two
    /// accounts that may confer nothing this places exactly zero — which is
    /// the same thing `Settle` does, and the same reason wash trading is
    /// worthless rather than merely detectable.
    pub fn place_stake(&mut self, creditor: MemberId, debtor: MemberId, amount: f64) {
        let conferrable = self.conferrable_minor(creditor);
        self.record_stake(creditor, debtor, Self::to_minor(amount), conferrable);
    }

    /// Total reservation held across stake edges.
    pub fn reserved_total(&self) -> f64 {
        Self::from_minor(flow::reserved_total(&self.reserved))
    }

    /// The community's outstanding insured credit: the flow drawn through the
    /// underwriters. Every insured unit crosses exactly one supply arc, so the
    /// supply side IS the total, which is what §Verification invariant 2 conserves.
    pub fn committed_total(&self) -> f64 {
        Self::from_minor(flow::committed_total(&self.committed))
    }

    /// The external seed, `Σ supply`: what the community's underwriters have
    /// undertaken to carry, every unit of it seated by a ceremony — genesis,
    /// plus every §Governance amendment since.
    ///
    /// This is the honest figure §Standing states in prose: *a community's real
    /// insured credit is its genesis seed, plus whatever later underwriters
    /// can pay from outside it.* There is no larger "declared total" to show
    /// beside it — that figure answers "how much has been promised on the
    /// strength of another promise", and a declaration cannot be raised at
    /// all, so the two are one.
    pub fn external_seed(&self) -> f64 {
        Self::from_minor(self.external_minor())
    }

    /// The external seed in minor units — the rate bound's own reading.
    pub(crate) fn external_minor(&self) -> u64 {
        self.underwriters.values().sum()
    }

    /// Utilisation, `Σ committed / Σ supply` — one of the three figures §Adoption
    /// says an underwriter needs in order to revise a declaration. Pinned at
    /// 1.0 means the ceiling is binding and credit is being rationed
    /// first-come-first-served.
    pub fn utilisation(&self) -> f64 {
        let supply: u64 = self.underwriters.values().sum();
        if supply == 0 {
            return 0.0;
        }
        flow::committed_total(&self.committed) as f64 / supply as f64
    }

    /// Bond headroom: what this member has to lose, net of debt, of encumbrance
    /// they are already carrying, and of what they have forfeited.
    ///
    /// Read from **`seed_reach`** — what the community's external seed reaches
    /// this account for — and NOT from capacity, and not from `conferrable`
    /// either. Both alternatives were tried and both are measured wrong.
    ///
    /// Capacity cannot be it: a founding underwriter has capacity ZERO, because
    /// the cut into them draws only on the OTHER underwriters and at genesis
    /// nobody has backed anybody, so the one member who has demonstrably
    /// accepted a real liability could not send a single transition and the
    /// community could never make its first trade.
    ///
    /// `conferrable` is not it either, and that is the reading this one exists
    /// to replace: for an underwriter it is their DECLARED supply, a promise
    /// made on the strength of another promise, and a chain of them doubles the
    /// write channel with every accomplice — 614,400 at twelve against a real
    /// cut of 300. `seed_reach` keeps what `conferrable` has right (a founder
    /// writes against what the ceremony seated) and drops what it has wrong.
    ///
    /// It stays a different reading from the credit gate. The bond gate is an
    /// admission rule for the write surface; capacity measures what the
    /// community underwrites. They share a ceiling but must not share a
    /// reading, or a member near their credit limit would be unable to send
    /// the very transitions that settle their way out of it. A `seed_reach`
    /// that netted live reservations would give the two readings the only part
    /// that matters in common, and the subtraction below would then take the
    /// member's own debt a second time. It is gross, and the whole of the
    /// netting is here.
    ///
    /// Neither reading is free. `seed_reach` is a max-flow over arcs a ceremony
    /// seated, so a key nobody has backed reaches none of it — which is what
    /// makes the write surface Sybil-proof by the same arithmetic as the credit
    /// surface, rather than by a quota. Measured at the set quantifier: k
    /// accomplices hold exactly what k honestly-backed members hold, 1.00×.
    pub fn bond_headroom(&self, id: MemberId) -> f64 {
        Self::from_minor(self.bond_headroom_minor(id))
    }

    /// `bond_headroom` in minor units — the reading the write gate charges in.
    pub fn bond_headroom_minor(&self, id: MemberId) -> u64 {
        let Some(m) = self.members.get(&id) else { return 0 };
        if !matches!(m.status, MemberStatus::Active) {
            return 0;
        }
        // `forfeit_reserve` is the third term, and subtracting it is what
        // makes a forfeiture mean anything. Handing the headroom back at
        // forfeiture instead makes the "sanction" a release: an abuser
        // forfeiting 2500 goes from headroom 0 to 2231 and immediately writes
        // 22 more transitions.
        //
        // The forfeited amount stays encumbered, permanently, and that is
        // the whole of the penalty: nothing is minted, nobody is owed it, and no
        // position anywhere improves — `prop:no-rent` intact. It is a
        // reservation that never returns, which is what the word already meant
        // and what §Recourse already claimed ("returns to the member on schedule
        // *unless forfeited*"). The way back is the model's own: earn more
        // standing and `conferrable` grows past it. And the recovery path stays
        // free either way, because `bond::due` prices `Settle`, `Cure`,
        // `Transfer`, `Exit` and `DeclareSupply` at zero — a forfeited member can
        // always discharge what it owes.
        let forfeited = self.forfeit_reserve.get(&id).copied().unwrap_or(0);
        // `seed_reach`, not `conferrable`. See `seed_reach` for the
        // measurement: reading the declared supply made this channel double with
        // every accomplice a coalition added, on one honest trade.
        self.seed_reach_minor(id)
            .saturating_sub(m.debt_out)
            .saturating_sub(m.bond_enc())
            .saturating_sub(forfeited)
    }

    /// Uniform re-denomination of every denomination-valued quantity.
    ///
    /// The stake graph is in the rescale set and that is load-bearing: a
    /// re-denomination that moved the constants and the debts but not the
    /// stakes would silently reprice every credit limit in the community while
    /// appearing to be a change of unit. Scaling every stake and reservation
    /// by pi scales every cut by pi, hence capacity by pi, so each comparison
    /// of amount against capacity has both sides scaled alike and its outcome
    /// is preserved exactly.
    pub fn rescale(&mut self, pi: f64) {
        self.params.rescale(pi);
        for c in self.contracts.values_mut() {
            c.outstanding = scale_minor(c.outstanding, pi);
            c.original = scale_minor(c.original, pi);
            if let Some(a) = c.arb.as_mut() {
                a.award_cap = scale_minor(a.award_cap, pi);
            }
            for v in c.arb_attestations.values_mut() {
                *v = scale_minor(*v, pi);
            }
        }
        for m in self.members.values_mut() {
            m.rep.open_default = scale_minor(m.rep.open_default, pi);
            m.rep.d_in *= pi;
            m.rep.d_out *= pi;
            // `debt_out` is deliberately NOT scaled here. An insured
            // obligation's amount after a re-denomination is whatever its arcs
            // can still carry, which is not its old amount times pi, so the
            // cache cannot be scaled — it is re-derived from the book at the
            // end of this function, which is the only figure it is ever
            // allowed to disagree with by nothing.
            // Encumbrance is denomination-valued: it is measured against
            // capacity, which rescales, so a bond that did not rescale would
            // silently change size in real terms at every re-denomination.
            for v in m.bonds.values_mut() {
                *v = scale_minor(*v, pi);
            }
        }
        for v in self.forfeit_reserve.values_mut() {
            *v = scale_minor(*v, pi);
        }
        // The capacity path, by the same factor and rounded DOWN everywhere.
        //
        // Order and method are both load-bearing. Stakes, supplies and every
        // obligation's `Held` are scaled by flooring, and then `reserved` and
        // `committed` are REBUILT as the exact sums of those helds rather than
        // scaled in their own right. Scaling them independently would put
        // `floor(Σ h)` on one side and `Σ floor(h)` on the other, so the two
        // views of one fact would drift apart at the first re-denomination:
        // releasing every obligation would leave dust permanently reserved,
        // and §Verification invariant 2 — which asks for exactness, not closeness —
        // would be false with nothing having gone wrong.
        //
        // Flooring keeps both floors safe by construction. A held amount can
        // only round down at least as far as the arc that carries it, so
        // `reserved ≤ stake` and `committed ≤ supply` survive: an obligation
        // can never end up holding more than the arc beneath it.
        for w in self.edges.values_mut() {
            *w = scale_minor(*w, pi);
        }
        for s in self.underwriters.values_mut() {
            *s = scale_minor(*s, pi);
        }
        // The epoch's amendment counter is a share of the same
        // denomination-valued quantity as the supplies above, so it moves by
        // the same factor and rounds the same way.
        self.seed_amended_this_epoch = scale_minor(self.seed_amended_this_epoch, pi);
        // **An insured obligation owes exactly what it holds**, and this is
        // where that stops being a consequence and becomes the definition.
        // `outstanding` is an f64 and the hold is integer, and §Verification invariant 2
        // compares the two for EXACT equality after `to_minor` — so the only
        // safe order is to re-denominate the hold first and read the amount
        // off it, never to scale the amount and hope the arcs agree. They do
        // not: `Σ floor(arc·π)` against `floor(Σ arc·π)` differs on a third of
        // two-arc holds at 2/3, and the ledger halted on the first
        // re-denomination that met one.
        for c in self.contracts.values_mut() {
            rescale_held(&mut c.held, pi);
        }
        // The seat layer takes the same treatment for the same reason: each
        // seat's `Held` is scaled arc by arc and the two maps are then rebuilt
        // as the exact sums, because `floor(Σ h)` and `Σ floor(h)` differ and
        // invariant 7 asks for equality. There is no shave and no floor — a
        // seat arc is allowed to sit above the stake beneath it by design, so
        // nothing here has to be brought back down to what the arcs deliver.
        for m in self.members.values_mut() {
            if let Some(seat) = m.seat.as_mut() {
                rescale_seat_held(&mut seat.held, seat.sponsor as usize, pi);
            }
        }
        self.shave_holds_to_what_the_arcs_deliver();
        for c in self.contracts.values_mut() {
            let live = matches!(c.status, ContractStatus::Active | ContractStatus::Expired);
            if !(c.insured && live) {
                continue;
            }
            if c.held.supply.is_empty() {
                // Its arcs rounded away to nothing, or they never reached a
                // whole minor unit to begin with (`dust` rescales and the
                // minor unit does not, so a downward re-denomination makes
                // sub-minor amounts legal). An obligation that holds nothing
                // is not insured — that is the definition rather than a
                // downgrade — and the debt itself stays exactly where it is,
                // scaled with everything else. Writing `outstanding` down to
                // the empty hold instead would erase the claim altogether,
                // which is the larger of the two losses by far.
                c.insured = false;
                c.held = Default::default();
            } else {
                c.outstanding = c.held.amount();
            }
        }
        self.rebuild_reservations();
        self.rebuild_seat_reservations();
        self.edges.retain(|_, w| *w > 0);
        self.underwriters.retain(|_, s| *s > 0);
        // The cached debt, counted rather than scaled — see the member loop
        // above for why it could not be scaled with everything else.
        let mut by_debtor: BTreeMap<MemberId, u64> = BTreeMap::new();
        for c in self.contracts.values() {
            if matches!(c.status, ContractStatus::Active | ContractStatus::Expired) {
                *by_debtor.entry(c.debtor).or_insert(0) += c.outstanding;
            }
        }
        for (id, m) in self.members.iter_mut() {
            m.debt_out = by_debtor.get(id).copied().unwrap_or(0);
        }
    }

    /// After a re-denomination has floored every arc: bring each debtor's
    /// supply claim back down to what their floored stake arcs can actually
    /// deliver.
    ///
    /// Flooring arc by arc is not enough on its own, and the reason is that a
    /// `Held` is not a list of numbers — the reservation caches are a FLOW, and
    /// conservation at every hop is what makes them one. `flow::reserve`
    /// records ONE supply entry per underwriter however many paths the
    /// augmentation took, so an underwriter reaching a debtor down two paths
    /// carrying 1000 and 1001 minor units floors at 2/3 to a supply claim of
    /// 1334 while the four stake arcs beneath it floor to 666 + 667 = 1333.
    /// The obligation would claim a unit of supply that no longer reaches its
    /// debtor; §Verification invariant 1 reads the supply side against a cut computed on
    /// the floored graph, and **1334 against 1333 halts every honest node.**
    /// Measured on a plain four-arc community with one underwriter — the third
    /// face of the same defect as the two named before it, and the one the
    /// original prescription would have left open.
    ///
    /// Read per DEBTOR, and that is load-bearing: the union of one debtor's
    /// holds is a flow, which no single one of them need be. `loss::split_edges`
    /// hands a subrogated claim a SHARE of each arc rather than an augmenting
    /// path of its own, and its own doc says so — re-solving a single `Held`
    /// against its own arcs would cut such a claim in half. The union is
    /// exactly what invariant 1 sums, so the union is what has to stay
    /// feasible.
    ///
    /// The shave comes off the largest claim first, one minor unit at a time,
    /// ties by contract then underwriter — a function of state, and it lands on
    /// the arcs with the most to give. The EDGE side is deliberately left where
    /// the flooring put it: it is bounded by the floored stakes so invariant 4
    /// holds, and leaving it high only means an obligation reserves a shade
    /// more of a stake edge than it is worth, which settlement gives back
    /// exactly as it took.
    fn shave_holds_to_what_the_arcs_deliver(&mut self) {
        let n = self.flow_n();
        let mut by_debtor: BTreeMap<MemberId, Vec<ContractId>> = BTreeMap::new();
        for c in self.contracts.values() {
            if c.insured && matches!(c.status, ContractStatus::Active | ContractStatus::Expired) {
                by_debtor.entry(c.debtor).or_default().push(c.id);
            }
        }
        for (debtor, ids) in by_debtor {
            let mut arcs: flow::Edges = Default::default();
            let mut sources: BTreeMap<usize, u64> = BTreeMap::new();
            for id in &ids {
                let held = &self.contracts[id].held;
                for &(key, amount) in &held.edges {
                    *arcs.entry(key).or_insert(0) += amount;
                }
                for &(u, amount) in &held.supply {
                    *sources.entry(u).or_insert(0) += amount;
                }
            }
            let claimed: u64 = sources.values().sum();
            if claimed == 0 {
                continue;
            }
            let uw: Vec<(usize, u64)> = sources.into_iter().collect();
            let (reserved, committed) = (flow::Reservations::new(), flow::Committed::new());
            let deliverable = flow::capacity(&arcs, &reserved, &committed, &uw, &[debtor as usize], n, claimed);
            let excess = claimed.saturating_sub(deliverable);
            if excess == 0 {
                continue;
            }
            // (contract, underwriter, claim), in contract-then-underwriter
            // order — which is the order the maps and `flow::reserve` already
            // guarantee.
            let mut claims: Vec<(ContractId, usize, u64)> = ids
                .iter()
                .flat_map(|&id| self.contracts[&id].held.supply.iter().map(move |&(u, a)| (id, u, a)))
                .collect();
            for _ in 0..excess {
                let Some(i) = (0..claims.len())
                    .filter(|&i| claims[i].2 > 0)
                    .max_by_key(|&i| (claims[i].2, std::cmp::Reverse(i)))
                else {
                    break;
                };
                claims[i].2 -= 1;
            }
            for (id, u, amount) in claims {
                let Some(c) = self.contracts.get_mut(&id) else { continue };
                if let Some(slot) = c.held.supply.iter_mut().find(|(x, _)| *x == u) {
                    slot.1 = amount;
                }
            }
            for id in &ids {
                if let Some(c) = self.contracts.get_mut(id) {
                    c.held.supply.retain(|&(_, a)| a > 0);
                }
            }
        }
    }

    /// Recompute `reserved` and `committed` from what the obligations actually
    /// hold. The two are a cache of one fact — every outstanding `Held` — and
    /// this is the one place that fact is re-derived rather than maintained.
    fn rebuild_reservations(&mut self) {
        self.reserved.clear();
        self.committed.clear();
        for h in self.contracts.values().map(|c| &c.held) {
            for &(key, amount) in &h.edges {
                *self.reserved.entry(key).or_insert(0) += amount;
            }
            for &(u, amount) in &h.supply {
                *self.committed.entry(u).or_insert(0) += amount;
            }
        }
        self.reserved.retain(|_, v| *v > 0);
        self.committed.retain(|_, v| *v > 0);
    }

    /// The seat maps as the exact sums of what the live seats hold — the same
    /// construction `rebuild_reservations` performs, over the other layer.
    fn rebuild_seat_reservations(&mut self) {
        self.seat_reserved.clear();
        self.seat_committed.clear();
        let mut edges: BTreeMap<(usize, usize), u64> = BTreeMap::new();
        for h in self.members.values().filter_map(|m| m.seat.as_ref()).map(|s| &s.held) {
            for &(key, amount) in &h.edges {
                *edges.entry(key).or_insert(0) += amount;
            }
            for &(u, amount) in &h.supply {
                *self.seat_committed.entry(u).or_insert(0) += amount;
            }
        }
        edges.retain(|_, v| *v > 0);
        for (key, v) in edges {
            self.seat_reserved.insert(key, v);
        }
        self.seat_committed.retain(|_, v| *v > 0);
    }

    pub fn member_of_key(&self, key: &Key) -> Option<MemberId> {
        self.key_index.get(key).copied()
    }

    /// Insert a founding underwriter and their declared supply (the genesis
    /// manifest, §Adoption).
    ///
    /// **Genesis underwriting is not a default. It is the only seed.** A
    /// community may choose to underwrite nothing and it will work — every
    /// obligation uninsured, borne bilaterally, which is an ordinary mutual
    /// credit network. What it can never do is leave that state: with no
    /// underwriter there is no source arc, so no capacity, so nothing may be
    /// conferred, so no settlement stakes anything, so no member can ever
    /// declare a supply. Zero is absorbing, and the free-signature bound says
    /// no amount of subsequent trading substitutes for the seed.
    ///
    /// Founders are given no stakes in one another. A community begins with
    /// people willing to stand behind it, not with a graph — a founder's own
    /// capacity is what the OTHER underwriters have put behind them, which
    /// they earn the same way everyone else does.
    ///
    /// Size the seed to **peak simultaneous** insured credit, never annual
    /// volume: the seed is not consumed, it is reserved, released and reserved
    /// again, so it turns over once per settlement term — 12× a year at the
    /// maturity floor of 30 epochs, which is the fastest any chain admits, and
    /// 4× on 90-day terms (`examples/seed_table.rs`, gated by `just
    /// seed-table-check`). Err low — understating costs friction that the
    /// uninsured tier absorbs, while overstating is the error with no later
    /// correction.
    pub fn add_underwriter(&mut self, keys: Vec<Key>, supply: f64) -> crate::errors::Res<MemberId> {
        use crate::errors::{Error, ET_ADM_DUP_KEY};
        if keys.is_empty()
            || keys
                .iter()
                .any(|k| self.key_index.contains_key(k) || self.members.values().any(|m| m.consensus_key == Some(*k)))
        {
            return Err(Error(ET_ADM_DUP_KEY));
        }
        // A ceremony can mis-type a number, and this is the one door that
        // seats a supply with no signature to check it against: a saturating
        // `1e300` here would be the whole roll, past every later bound.
        if !Self::amount_representable(supply) {
            return Err(Error(crate::errors::ET_CTR_BAD_AMOUNT));
        }
        let id = self.new_account(keys);
        let minor = Self::to_minor(supply);
        if minor > 0 {
            // Genesis supply is EXTERNAL by definition, and this is the one
            // place that can be said without checking anything. The ledger
            // cannot verify externality — that is the free-signature bound
            // (§Security), which is why genesis was a ceremony and not a
            // verification. What it can do is admit supply only where a
            // ceremony seated it, and this is the first of them; §Governance's
            // amendments are the rest, and there is no third door.
            self.underwriters.insert(id, minor);
        }
        Ok(id)
    }

    /// Create an account. Not a transition and not an admission: this is the
    /// bookkeeping every path shares, and it grants nothing — a fresh account
    /// has no incident stakes, hence a capacity of zero.
    pub fn new_account(&mut self, keys: Vec<Key>) -> MemberId {
        let id = self.next_member;
        self.next_member += 1;
        for key in &keys {
            self.key_index.insert(*key, id);
        }
        self.members.insert(
            id,
            Member {
                id,
                keys,
                consensus_key: None,
                status: MemberStatus::Active,
                joined_epoch: self.epoch,
                guardian: None,
                pending_rotation: None,
                beneficiaries: Default::default(),
                supporters_of: Default::default(),
                approved_supporters: Default::default(),
                rep: Reputation::default(),
                debt_out: 0,
                bonds: Default::default(),
                bond_free_used: 0,
                bond_saturated_epochs: 0,
                bond_denied_this_epoch: false,
                // Filled by `apply::seat_pair`, where the reservation that
                // bought the row is taken. A ceremony's own rows carry `None`,
                // and no slot: the slot is what a seat paid for.
                seat: None,
                seat_slot: false,
            },
        );
        id
    }

    /// Register the key a member's validator signs consensus with (the
    /// genesis manifest's half of `Tx::SetConsensusKey`).
    ///
    /// It is deliberately not a `keys` entry and not in `key_index`: a
    /// consensus key lives on a server and must never be able to sign an
    /// obligation. Refused if the key already belongs to a member or to
    /// another validator.
    pub fn set_consensus_key(&mut self, id: MemberId, key: Key) -> crate::errors::Res<()> {
        use crate::errors::{Error, ET_MEM_UNKNOWN, ET_VAL_KEY_IN_USE};
        if self.key_index.contains_key(&key) || self.members.values().any(|m| m.consensus_key == Some(key)) {
            return Err(Error(ET_VAL_KEY_IN_USE));
        }
        let m = self.members.get_mut(&id).ok_or(Error(ET_MEM_UNKNOWN))?;
        m.consensus_key = Some(key);
        Ok(())
    }

    /// Designate a founding validator (genesis manifest). The member must
    /// already exist, be Active, and have registered a consensus key: a
    /// validator whose signing key the ledger cannot name is one no
    /// certificate can be verified against.
    pub fn set_genesis_validator(&mut self, id: MemberId, power: u64) -> crate::errors::Res<()> {
        use crate::errors::{Error, ET_VAL_NOT_ELIGIBLE, ET_VAL_NO_CONSENSUS_KEY};
        let eligible = self
            .members
            .get(&id)
            .map(|m| matches!(m.status, MemberStatus::Active))
            .unwrap_or(false);
        if !eligible || power == 0 {
            return Err(Error(ET_VAL_NOT_ELIGIBLE));
        }
        if self.members[&id].consensus_key.is_none() {
            return Err(Error(ET_VAL_NO_CONSENSUS_KEY));
        }
        self.validators.insert(id, power);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_member_state() -> State {
        let mut st = State::default();
        let id = st.add_underwriter(vec![[1u8; 32]], 2500.0).expect("founding underwriter");
        st.set_consensus_key(id, [0xC1u8; 32]).expect("consensus key");
        st.set_genesis_validator(id, 1).expect("genesis validator");
        st
    }

    /// Zero is absorbing, and it is the first thing to get right: a community
    /// that underwrites nothing has no source arc, so no member can ever earn
    /// capacity, so no member can ever declare a supply. There is no path out
    /// of it from inside the ledger — which is the free-signature bound in its
    /// most concrete form.
    #[test]
    fn a_community_with_no_underwriters_stays_at_zero() {
        let mut st = State::default();
        let a = st.new_account(vec![[1u8; 32]]);
        let b = st.new_account(vec![[2u8; 32]]);
        // Every settlement between them, forever.
        for _ in 0..1000 {
            let conf = st.conferrable_minor(a);
            st.record_stake(a, b, State::to_minor(1_000_000.0), conf);
            let conf = st.conferrable_minor(b);
            st.record_stake(b, a, State::to_minor(1_000_000.0), conf);
        }
        assert!(st.edges.is_empty(), "nothing may be conferred, so nothing is staked");
        assert_eq!(st.capacity_of(a), 0.0);
        assert_eq!(st.capacity_of_set(&[a, b]), 0.0);
    }

    /// The founding seed reaches a newcomer through ordinary trade, and the
    /// set of everyone it reached can still only owe what one supply carried.
    #[test]
    fn the_seed_propagates_but_the_cut_does_not_move() {
        let mut st = State::default();
        let u = st.add_underwriter(vec![[1u8; 32]], 2500.0).expect("underwriter");
        let chain: Vec<MemberId> = (0..5).map(|i| st.new_account(vec![[10 + i as u8; 32]])).collect();
        // The underwriter backs the first, who backs the second, and so on.
        let mut creditor = u;
        for &d in &chain {
            let conf = st.conferrable_minor(creditor);
            st.record_stake(creditor, d, State::to_minor(1_000_000.0), conf);
            creditor = d;
        }
        for &d in &chain {
            assert_eq!(st.capacity_of(d), 2500.0, "limits propagate undiminished");
        }
        assert_eq!(st.capacity_of_set(&chain), 2500.0, "but simultaneous use is one supply");
    }

    /// A re-denomination must carry the reservations exactly, or the two
    /// views of one fact drift and both floors quietly stop holding.
    #[test]
    fn redenomination_keeps_the_reservations_exact() {
        let mut st = State::default();
        let u = st.add_underwriter(vec![[1u8; 32]], 2500.0).expect("underwriter");
        let d = st.new_account(vec![[2u8; 32]]);
        let conf = st.conferrable_minor(u);
        st.record_stake(u, d, State::to_minor(2500.0), conf);
        let held = st.reserve_capacity(d, 2000.0).expect("within the cut");
        st.contracts.insert(
            0,
            Contract {
                id: 0,
                debtor: d,
                creditor: u,
                outstanding: State::to_minor(2000.0),
                original: State::to_minor(2000.0),
                maturity_epoch: 30,
                status: ContractStatus::Active,
                created_epoch: 0,
                accepted_epoch: 0,
                insured: true,
                held,
                arb: None,
                arb_attestations: Default::default(),
                arb_awarded: false,
            },
        );
        st.rescale(0.5);
        let c = &st.contracts[&0];
        assert_eq!(st.committed_total(), State::from_minor(c.held.amount()), "invariant 2 is exact");
        assert!(st.supply_floor(u) <= st.underwriters[&u], "invariant 5 survives the rescale");
        for (k, r) in &st.reserved {
            assert!(*r <= st.edges[k], "invariant 4: no edge below its live reservation");
        }
        // And releasing gives every unit back, with nothing stranded.
        let held = st.contracts[&0].held.clone();
        st.release_capacity(&held);
        assert_eq!(st.reserved_total(), 0.0);
        assert_eq!(st.committed_total(), 0.0);
    }

    /// The clamp's structural backstop, and the anti-hang proof: applying a block
    /// carrying `time_secs = u64::MAX` must return promptly and advance the
    /// epoch by EXACTLY `MAX_EPOCH_ADVANCE_PER_BLOCK`, not the ~2.135e14
    /// epochs the raw timestamp would otherwise demand (at `EPOCH_SECS =
    /// 86_400`, that unbounded loop is measured elsewhere at ~2.76 µs/epoch —
    /// on the order of 18 years of CPU for one call). Without the clamp this
    /// test would not fail an assertion, it would simply never return; the
    /// wall-clock bound below is what actually catches a regression.
    #[test]
    fn begin_block_clamps_a_u64_max_timestamp_to_the_structural_bound() {
        // **The property is that the call RETURNS**, and a wall-clock budget
        // could never assert it: unclamped this does not run slowly, it does
        // not come back, and the assertion after it never executes. So the
        // call goes on a thread and the test waits with a deadline — which
        // FAILS, naming the clamp, where a budget would have hung the run.
        //
        // The deadline is a liveness bound and not a measurement: ~60 s
        // against the ~28 ms this costs (`MAX_EPOCH_ADVANCE_PER_BLOCK` epochs
        // at the ~2.76 µs/epoch measured above), so roughly 2,000x. The old
        // `< 2.0 s` was 70x, which is a figure about this box — and the loaded
        // runner that turned the ingress flood probe red is exactly where a
        // margin like that goes.
        let (done, wait) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut st = one_member_state();
            st.begin_block(u64::MAX);
            let _ = done.send(st.epoch);
        });
        let epoch = wait
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("begin_block must RETURN on a u64::MAX timestamp — unclamped, it never does");

        assert_eq!(
            epoch,
            k::MAX_EPOCH_ADVANCE_PER_BLOCK,
            "epoch must advance by EXACTLY the clamp, not the raw (astronomical) target"
        );
    }

    /// The clamp bounds the ADVANCE per call, not an absolute epoch ceiling:
    /// a second oversized block from an already-advanced epoch still only
    /// advances by another clamp's worth, proving `target` is computed
    /// relative to `self.epoch` (`self.epoch.saturating_add(..)`), not from
    /// zero.
    #[test]
    fn begin_block_clamp_is_relative_to_the_current_epoch_not_absolute() {
        let mut st = one_member_state();

        st.begin_block(k::MAX_EPOCH_ADVANCE_PER_BLOCK * k::EPOCH_SECS);
        assert_eq!(st.epoch, k::MAX_EPOCH_ADVANCE_PER_BLOCK);

        st.begin_block(u64::MAX);
        assert_eq!(
            st.epoch,
            2 * k::MAX_EPOCH_ADVANCE_PER_BLOCK,
            "a second oversized block must advance by another clamp's worth, not jump straight to the raw target"
        );
    }

    /// No regression on the ordinary path: a ledger-plausible timestamp well
    /// under the clamp still advances to the exact target epoch.
    #[test]
    fn begin_block_advances_to_the_exact_target_when_well_under_the_clamp() {
        let mut st = one_member_state();
        st.begin_block(10 * k::EPOCH_SECS);
        assert_eq!(st.epoch, 10);
    }
}
