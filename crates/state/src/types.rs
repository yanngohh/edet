//! State objects.

use std::collections::{BTreeMap, BTreeSet};

use edet_kernel::flow;

pub type MemberId = u64;
pub type ContractId = u64;
pub type ProposalId = u64;
pub type Key = [u8; 32];

/// There is no admission ladder. An account exists as soon as a key signs,
/// and it is worth exactly what the community has staked on it — which for a
/// key that has done nothing is zero, by arithmetic rather than by rule.
///
/// What remains is the governance sanction and the member's own withdrawal.
/// Neither is a tier: `Active` is the resting state of every account that has
/// not been suspended or left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MemberStatus {
    Active,
    Suspended,
    Exited,
}

/// A party to a trade: an account that already exists, or the key of one that
/// comes into existence here.
///
/// **A row is seated by a trade, and there is no creation transition.** A
/// transition of its own would have to be unbonded — a key nobody knows has no
/// headroom, and charging for creation would need somebody's permission, which
/// §Model refuses — and an unbonded transition is bounded by nothing, so it
/// would need a ledger-wide per-epoch counter. That counter is a censorship
/// lever rather than a quota: an attacker holding no standing at all takes the
/// whole 1024 in one epoch for a bond spend of zero, and every honest newcomer
/// is refused until the boundary.
///
/// Naming the key inside a BONDED transition closes that and the storage term
/// together, because a row then only ever appears alongside a write somebody
/// paid for — and creation stays unapprovable, because nobody decides whether
/// the row appears: the trade does.
///
/// **Why the ids stay.** A member's keys move (`RotateFinalize` retires the old
/// ones), so a key is a name that can go stale while a member id never does.
/// A client that has resolved a counterparty names them by id; naming a key is
/// the deliberate statement "this is a new account if it is not one already",
/// which is exactly the case `/whois` answers `null` for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Party {
    Member(MemberId),
    Key(Key),
}

impl From<MemberId> for Party {
    fn from(id: MemberId) -> Self {
        Party::Member(id)
    }
}

impl From<Key> for Party {
    fn from(key: Key) -> Self {
        Party::Key(key)
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GuardianConfig {
    pub guardians: BTreeSet<MemberId>,
    pub threshold: u32,
    pub veto_window_epochs: u64,
}

/// A guardian rotation waiting out its veto window. A veto deletes it rather
/// than marking it: a marked request was a precondition the free veto could
/// consume again, and a durable replay id each time.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingRotation {
    pub new_keys: Vec<Key>,
    pub opened_epoch: u64,
}

/// Consented arbitration terms as a transaction carries them.
///
/// This is a WIRE type: `award_cap` is in major units and is an `f64`, like
/// every other amount a signed transaction carries, because the transaction is
/// the API edge and the wallet composes it. It is validated and converted
/// exactly once, at acceptance, into the stored [`ArbTerms`] beside it — which
/// is where the NaN and negative refusals live, since `to_minor` clamps both
/// to zero and a clamp is not a refusal.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ArbTermsWire {
    pub arbiters: BTreeSet<MemberId>,
    pub quorum: u32,
    pub window_epochs: u64,
    /// Award ceiling, major units.
    pub award_cap: f64,
}

/// Consented arbitration terms as the ledger stores them, pinned at acceptance
/// and immutable after.
///
/// The same terms as [`ArbTermsWire`] with the ceiling in MINOR UNITS, so the
/// comparison against a median attestation is integer on both sides — and
/// the two parties and the amount the panel was consented FOR. A row's own
/// `debtor`, `creditor` and `original` move at substitution: the row becomes
/// the first underwriter's claim, with the underwriter as creditor and their
/// share as the amount. The panel does not move with them — it names the
/// buyer's remedy against the SELLER for non-delivery — so the parties it
/// binds are recorded here, on the terms, and the award is minted between
/// them and bounded by this amount whatever the row says now
/// (`apply::arb_award`). Recorded on the terms rather than on every contract
/// because only a row with a panel ever reads them.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ArbTerms {
    pub arbiters: BTreeSet<MemberId>,
    pub quorum: u32,
    pub window_epochs: u64,
    /// Award ceiling, minor units.
    pub award_cap: u64,
    /// The buyer at acceptance: who an award is owed TO.
    pub debtor: MemberId,
    /// The seller at acceptance: who an award is owed BY.
    pub creditor: MemberId,
    /// The obligation's original amount, minor units: the third bound on an
    /// award, beside the median and the ceiling.
    pub amount: u64,
}

impl ArbTermsWire {
    /// Cross the boundary: the wire's ceiling, rounded the same way every
    /// other amount entering the ledger is, beside the parties and the amount
    /// the terms bind. Call only after `check_arb` has refused a ceiling that
    /// is not a finite non-negative number, and after the parties have ids.
    pub fn into_stored(self, award_cap: u64, debtor: MemberId, creditor: MemberId, amount: u64) -> ArbTerms {
        ArbTerms {
            arbiters: self.arbiters,
            quorum: self.quorum,
            window_epochs: self.window_epochs,
            award_cap,
            debtor,
            creditor,
            amount,
        }
    }
}

/// What remains of reputation once standing is a cut.
///
/// `open_default` is in MINOR UNITS. It is adjusted incrementally — up at a
/// default, down at a cure — and a running `f64` sum drifts from what a
/// recount says. See `Member.debt_out`.
///
/// Settled volume, per-creditor evidence, underwriting yield and their decayed
/// counters were all proxies for "how much does the community back this
/// account", and the stake graph answers that directly. What is left is the
/// part no cut can express: whether this account is currently in default, and
/// the advisory velocity counters the client shows.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Reputation {
    /// Outstanding defaulted amount, minor units (cured by late discharge).
    pub open_default: u64,
    /// Advisory debt-velocity counters for the current epoch. These are the
    /// one pair of quantities here that nothing compares for equality — the
    /// client scores a ratio of them — so they stay `f64`, and are reset at
    /// every epoch boundary rather than accumulated indefinitely.
    pub d_in: f64,
    pub d_out: f64,
}

/// What bought a row, and what it holds for as long as the row exists.
///
/// **A row is a stock, and its price is a reservation on the graph.** A write
/// budget bounds a RATE — it refills every epoch — while a row is permanent,
/// hashed into every state root and retired by nothing, so the two are priced
/// differently: the seat takes one bond unit of flow from the community's
/// seed to the SPONSOR, on the stake graph itself, on a reservation pair the
/// credit layer never reads, and it is never given back.
///
/// Shared and permanent, both, or the bound is not one. Per-account headroom
/// reads the same backing once per child, so a farm behind one edge of 500.00
/// compounds; a seat that shrank with decay would be renewed by an
/// accomplice's free trades at 2.3% of the backing an epoch. Held on the
/// shared graph and never released, `unit x seats(S)` cannot exceed the
/// all-time peak of the arcs into any superset of `S` that holds no
/// underwriter: 25 rows behind one edge of 500.00, 250 behind 5,000.00.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Seat {
    /// The member whose reach paid for the row.
    pub sponsor: MemberId,
    /// Exactly which arcs the seat holds, and how much on each — the same
    /// shape an obligation's hold has, and released by exactly one line when
    /// the sweep retires the row for standing empty (`State::release_seat`).
    /// No transition releases it: a row that holds anything keeps its seat
    /// for as long as it holds it.
    pub held: flow::Held,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Member {
    pub id: MemberId,
    pub keys: Vec<Key>,
    /// The key this member's VALIDATOR signs consensus with, if they run one.
    ///
    /// **It is not one of `keys`, and that is the whole of it.** A validator
    /// key lives unencrypted on a server that answers the internet; a member
    /// key signs `Accept`, `Settle` and `DeclareSupply`. They were the same
    /// key: seating a validator's consensus key as its member key makes a
    /// validator host compromise hand the attacker the operator's economic
    /// identity, and makes restoring that recovery phrase into the wallet turn
    /// a phone's vault into a hot consensus key.
    ///
    /// `None` for every member who runs no validator, which is nearly all of
    /// them. A member cannot be given voting power without one
    /// (`ET-VAL-004`), and `SetConsensusKey` is how an operator registers or
    /// rotates it — rotating the box without touching the money.
    #[serde(default)]
    pub consensus_key: Option<Key>,
    pub status: MemberStatus,
    pub joined_epoch: u64,
    pub guardian: Option<GuardianConfig>,
    pub pending_rotation: Option<PendingRotation>,
    /// Support-cascade listing: beneficiaries this member routes discharge
    /// toward, with waterfill weights.
    pub beneficiaries: BTreeMap<MemberId, f64>,
    /// Reverse index: members who list this member as a beneficiary.
    pub supporters_of: BTreeSet<MemberId>,
    /// Supporters whose drains this member has approved.
    ///
    /// Called a "moderation gate" until it was measured, which was the
    /// paper's framing and the smaller half of what it does. A drain cannot
    /// cost the member drained toward any money, and it is not what keeps
    /// strangers out — the drain cap is zero for a pair that has never settled
    /// anything, approved or not. What it decides is that the member's
    /// obligations are discharged by somebody else, and since a routed claim is
    /// a debtor swap **no stake is written for it**: 80 cleared by a
    /// supporter confers 0 where the debtor paying the same 80 confers 80. So
    /// this field carries the member's answer to **relief now against standing
    /// later**, which is why the refused proposal to replace it with a per-sale
    /// signature is refused rather than deferred.
    pub approved_supporters: BTreeSet<MemberId>,
    pub rep: Reputation,
    /// Cached total outstanding debt as debtor, MINOR UNITS (active + expired
    /// uncured).
    ///
    /// **A running sum in `f64` drifts from a recount, and the audit compares
    /// the two.** Every acceptance adds and every discharge subtracts, so the
    /// error accumulates with an account's traffic while the recomputed book
    /// is a fresh sum each time; at institutional volumes the difference
    /// crosses any tolerance the audit could pick, and then every honest node
    /// halts at the same height on a ledger where nothing went wrong. Integers
    /// remove the question rather than tightening the tolerance: the
    /// comparison in `invariants` is exact.
    pub debt_out: u64,
    /// Operation bonds encumbered and not yet released, minor units, keyed by
    /// the epoch at which they release. Reserved headroom, never a balance:
    /// nothing here is owed to anyone, and every entry returns to the member
    /// on schedule unless forfeited.
    pub bonds: BTreeMap<u64, u64>,
    /// Bonded transitions already spent from this epoch's free allowance.
    pub bond_free_used: u32,
    /// Consecutive epochs in which the gate denied this member and it never
    /// returned under its ceiling.
    pub bond_saturated_epochs: u64,
    /// Whether the gate denied this member during the open epoch.
    pub bond_denied_this_epoch: bool,
    /// What bought this row: the sponsor and the arcs their seat holds.
    /// `None` for a row a ceremony seated — genesis and `seed::enact` write
    /// their own underwriters, and a ceremony is the door the seat prices.
    #[serde(default)]
    pub seat: Option<Seat>,
    /// One self-act the seat paid for, held until the row spends it on a
    /// transition about itself and nobody else — registering guardians is the
    /// one a newcomer wants — and spent by a refusal too. A stock, not a rate:
    /// bounded by seats, which are bounded by the seed, so N rows hold N and
    /// no more. `false` for a ceremony's own rows.
    #[serde(default)]
    pub seat_slot: bool,
}

impl Member {
    /// Total encumbrance, minor units: the sum of unreleased bonds.
    pub fn bond_enc(&self) -> u64 {
        self.bonds.values().sum()
    }
    pub fn has_key(&self, k: &Key) -> bool {
        self.keys.iter().any(|x| x == k)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ContractStatus {
    Active,
    Transferred,
    Settled,
    Expired,
    Cured,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Contract {
    pub id: ContractId,
    pub debtor: MemberId,
    pub creditor: MemberId,
    /// What is still owed, MINOR UNITS. Integer for the same reason
    /// `Member.debt_out` is — it is decremented by every partial discharge —
    /// and for one more: an insured obligation owes exactly what it HOLDS, and
    /// the hold is integer, so the audit's clause is an equality rather than a
    /// comparison against a tolerance.
    pub outstanding: u64,
    /// What it was booked at, minor units.
    pub original: u64,
    pub maturity_epoch: u64,
    pub status: ContractStatus,
    pub created_epoch: u64,
    /// The epoch the claim was ACCEPTED, which a transfer, a routed successor
    /// and a subrogated piece inherit where `created_epoch` restarts: the base
    /// the insured horizon is measured from, so that no debtor swap refreshes
    /// it. `created_epoch` stays the row's own — the arbitration window and the
    /// retention sweep read it.
    pub accepted_epoch: u64,
    /// Whether this obligation reserved flow at acceptance.
    ///
    /// Capacity bounds what the community UNDERWRITES, not what a member may
    /// choose to risk. An insured obligation holds a reservation and the
    /// recourse machinery stands behind it; an uninsured one reserves nothing,
    /// triggers no community recourse, and the creditor bears it alone.
    pub insured: bool,
    /// Exactly what this obligation holds: which stake edges, which supply
    /// arcs, how much on each. Empty for an uninsured obligation.
    ///
    /// Stored on the contract rather than recomputed, because settlement must
    /// be the exact INVERSE of acceptance. Releasing proportionally across the
    /// arcs incident to the debtor is not the inverse of anything: it strands
    /// the upstream half of every multi-hop path, so settling an obligation
    /// destroys capacity nothing was behind. Holding the augmentation itself
    /// also closes the old release-ORDER question by construction — there is
    /// no order to choose when the answer is "precisely what was taken".
    ///
    /// A default deliberately does NOT release it (`mark_expired`): the flow a
    /// defaulter committed stays committed, which is why stealing through a
    /// default costs the thief exactly what they hold and cannot be repeated.
    pub held: flow::Held,
    /// Consented arbitration (None = channel structurally closed).
    pub arb: Option<ArbTerms>,
    /// Panel attestations collected so far (arbiter -> attested amount).
    /// Minor units, so the median the award is taken from is exact.
    pub arb_attestations: BTreeMap<MemberId, u64>,
    /// The award has been minted (once-only).
    pub arb_awarded: bool,
}

// **A contract records no `co_signers`, and must not.** Such a field would say
// which members' obligations a cascade cleared into a successor: written by
// `cascade::sale`, cleared by `loss::substitute`, rescaled at every
// re-denomination, carried into the state root on every contract row — and read
// by nothing, because what it would be provenance FOR does not exist. The
// cascade couples PRODUCTION (a supporter's sale clears somebody else's debts
// instead of their own) rather than failure, and §Stability says so in as many
// words. A field carrying evidence for a coupling the model does not perform is
// a cost on every row, in the hashed state, forever. **Ledger state carries what
// must be ENFORCED.**

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum ProposalKind {
    ParamChange {
        key: ParamKey,
        value: f64,
    },
    Redenominate {
        num: u64,
        den: u64,
    },
    Suspend {
        member: MemberId,
    },
    Unsuspend {
        member: MemberId,
    },
    /// Validator voting power (0 removes the validator).
    ValidatorPower {
        member: MemberId,
        power: u64,
    },
    /// Seed amendment (§Governance): the AUTHOR declares an external commitment of
    /// `amount`, and the community endorses it.
    ///
    /// **It names no beneficiary, and that is the whole of its
    /// authorization.** A supply is a signed, standing consent to inherit the
    /// debts of those the community's stakes reach through you (§Recourse), so an
    /// amendment naming somebody else would volunteer a member to underwrite —
    /// the one thing no discharge in this alphabet does. The beneficiary is
    /// the proposal's author, by construction rather than by a check, so the
    /// consent is the author's own signature on the `Propose` and cannot be
    /// forgotten, spoofed, or refactored away.
    ///
    /// The amount is transition payload rather than a governed constant: what
    /// governance settles is the RATE (`ParamKey::SeedRate`), not the size of
    /// any one commitment.
    SeedAmendment {
        amount: f64,
    },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Proposal {
    pub id: ProposalId,
    pub kind: ProposalKind,
    pub author: MemberId,
    pub assents: BTreeSet<MemberId>,
    pub enacted: bool,
    /// The epoch this proposal was written in, so the sweep can retire it.
    /// Without it a proposal is a permanent row bought with one bond, and
    /// `Propose` is priced as a rate.
    pub opened_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum ParamKey {
    RiskK,
    /// Charter visibility policy: 1.0 seals precise amounts to parties +
    /// validators (others see pow2 buckets); 0.0 = member-visible precise.
    SealAmounts,
    /// Operation-bond unit as a fraction of `v_base`. Its safe range is
    /// bounded ABOVE as strictly as below: a bond set too high is censorship
    /// by arithmetic — every rule still reads as neutral while ordinary
    /// members are priced out of writing.
    BondFraction,
    /// Per-epoch stake decay numerator, over `DECAY_DEN`. Standing should
    /// reflect present backing rather than history.
    StakeDecay,
    /// Seed-amendment rate β (§Governance): what fraction of the tracked external
    /// seed one epoch of amendments may add. Bounded away from zero as well as
    /// from above — see `Params::safe_range`.
    SeedRate,
    /// The insured horizon, in epochs from a claim's ACCEPTANCE: a claim
    /// maturing past it is booked or left uninsured (`k::INSURED_HORIZON_EPOCHS`).
    /// Read at acceptance, at extension and where a claim moves debtor, never
    /// by an invariant — a stored claim compared against a live dial is a halt
    /// waiting for the dial.
    InsuredHorizon,
}
