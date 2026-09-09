//! Genesis values of the protocol constants.
//!
//! Governed constants live in the ledger's `Params` state; these are their
//! genesis values and the fixed (non-governed) algorithm parameters.

/// Reference denomination V_base.
pub const BASE_CAPACITY: f64 = 1000.0;
/// Dust threshold.
pub const DUST: f64 = 0.01;

/// Minor units per denomination unit — the scale at which the capacity path
/// works in integers.
///
/// Everything outside that path is `f64`, and this is the conversion at the
/// boundary. Two decimal places: the smallest unit a member can be asked to
/// think about, and small enough that rounding a stake down loses nothing that
/// matters against a credit limit.
pub const MINOR: f64 = 100.0;

/// The largest amount the ledger will book, in MINOR units.
///
/// **A clamp is not a refusal, and `to_minor` clamps at both ends.** It floors
/// a NaN and a negative to zero, and it ends in a float-to-integer `as` cast,
/// which SATURATES: a finite `1e300` on the wire is `u64::MAX` minor units. A
/// signed payload naming it books a debt of eighteen quintillion, and the
/// conservation sum in the audit — which runs on the commit path, on every
/// node — overflows on the next live contract. This is the bound that refuses
/// such an amount BEFORE the cast, where the refusal is still possible.
///
/// Its value is where the ledger's own boundary stops being exact. The wire
/// carries major units as an `f64` and the ledger stores minor units, so every
/// amount makes the trip `to_minor(from_minor(u))`: a view leaves as a float
/// and comes back in the payload that pays the row off. That trip is two
/// roundings of at most half an ulp each, so the figure that returns is within
/// `u * 2^-52` of `u` — under `2^51` that is less than half a minor unit and
/// the trip is the identity, and above it it is not. Measured on the octave
/// above: 7% of minor values between `2^51` and `2^52` come back a different
/// number, and 16% of those between `2^52` and `2^53`. A debtor paying the
/// exact figure the ledger served them is then told they have over-paid, or
/// pays in full and leaves a residue.
///
/// `2^51 - 1`, so the argument is strict. It leaves room for 8,192
/// ceiling-sized amounts in a `u64` sum, which the audit's own `u128` sums
/// then make moot. Structural rather than governed: a governed figure would
/// move with re-denomination, and this bound is a property of the number
/// format.
pub const MAX_AMOUNT_MINOR: u64 = 2_251_799_813_685_247;

/// Per-epoch stake decay, as `STAKE_DECAY_NUM / DECAY_DEN`.
///
/// Standing reflects present backing, so an edge fades unless trade renews it.
/// At this ratio an unrenewed edge halves in about thirty epochs, which is the
/// same order as the trading rhythm the community is built around — fast
/// enough that backing withdrawn in practice is withdrawn on the ledger, slow
/// enough that a member who trades seasonally is not erased between seasons.
pub const STAKE_DECAY_NUM: u64 = 977;
pub const DECAY_DEN: u64 = 1000;

/// Cascade drain coefficient ν — the paper's citation of the multiple.
///
/// The ledger multiplies by the rational pair below and never by this float:
/// a stake is an integer of minor units, and scaling it through an `f64` and
/// back was a third crossing of the boundary the ledger crosses exactly twice.
/// `edet_kernel::cascade`'s tests hold the three equal.
pub const NU_DRAIN: f64 = 1.0;
/// ν as the ratio the ledger applies to a stake in minor units.
pub const NU_DRAIN_NUM: u64 = 1;
pub const NU_DRAIN_DEN: u64 = 1;
/// Support-cascade recursion ceiling.
pub const MAX_CASCADE_DEPTH: u32 = 20;

/// Operation-bond unit, as a FRACTION of `BASE_CAPACITY` rather than an
/// absolute amount, so the genesis bond survives re-denomination unchanged in
/// real terms (the params field it seeds is itself rescaled).
pub const BOND_UNIT_FRACTION: f64 = 0.02;
/// Free transitions per member per epoch (the allowance `A`).
///
/// This is what carries a new account. Capacity starts at zero and an account
/// with no backing can bond nothing, so the allowance — not the bond — is what
/// lets somebody who just joined transact at all.
pub const BOND_FREE_ALLOWANCE: u32 = 32;
/// Epochs a bond stays encumbered before it releases (`T_b`).
pub const BOND_RELEASE_EPOCHS: u64 = 1;
/// Consecutive saturated epochs before a member's bonds may be forfeited
/// (`F`). A member is saturated in an epoch iff the gate DENIED it at least
/// once and it never came back under the ceiling.
pub const BOND_FORFEIT_EPOCHS: u64 = 3;
/// Risk sigmoid midpoint K (client-advisory only) and the claim pin.
pub const RISK_K: f64 = 0.75;
pub const K_CLAIM: f64 = 20.0;
/// Arbitration panel size cap.
pub const N_ARB: u32 = 16;

/// Governance: adoption threshold and cooldown.
pub const THETA_ADOPT: f64 = 0.5;
/// The adoption threshold for a proposal that changes WHO ORDERS THE LEDGER:
/// two thirds of the external seed, against a half for everything else.
///
/// At one threshold for every kind, whoever holds half the seed can — alone,
/// and in one epoch — remove every other validator down to the ledger's own
/// floor and suspend anyone who objects. The parameters are recoverable: a
/// constant moved too far is moved back by the same door, inside its
/// constitutional range, and every value in that range is one the ledger keeps
/// working at. The validator set is not recoverable in the same way, because
/// the coalition that holds it decides which blocks exist, including the ones
/// that would undo it.
///
/// Two thirds because that is the bar the consensus below it already uses: a
/// BFT quorum is `2f + 1` of `3f + 1`, so a set that can be changed by less
/// than two thirds can be changed by a minority the protocol was already
/// willing to tolerate as faulty. Written as a decimal rather than `2.0 / 3.0`
/// so the paper's constant table can read it (`scripts/paper-constants.py`) and
/// so every node computes the same bits from the same source text.
///
/// Frozen exactly like `THETA_ADOPT` — no `ParamKey` reaches either — because
/// a constant that guards amendment must not be amendable through the door it
/// guards.
pub const THETA_ADOPT_VALIDATOR: f64 = 0.6666666666666666;
pub const GOV_COOLDOWN_EPOCHS: u64 = 90;
/// Seed-amendment rate β: how much external seed one epoch may admit, as a
/// fraction of the external seed already tracked.
///
/// Dimensionless, like every governed constant, so it survives
/// re-denomination untouched. It is the bound that preserves the security
/// SHAPE of §Governance: a capacity-majority can still assent to a phantom
/// commitment — no rule stops a community lying to itself — but the bound
/// turns an explosion into a slow, public, attributable leak, during which
/// the only real victims (lenders, who must still choose to deliver goods
/// against the inflated figure) can simply stop extending credit.
///
/// Its base is the EXTERNAL seed and never the declared total, which is the
/// whole security content of the number: §Standing's declared total inflates
/// geometrically from inside the community (seed 100, twelve joiners, 409,600
/// declared), and a rate computed on it would convert that inflation into
/// amendment headroom.
pub const SEED_RATE: f64 = 0.02;
/// Re-denomination band: max |ln pi| per event.
pub const REDENOM_BAND_LN: f64 = 0.5;
/// The length of an epoch, in seconds — a constant of the PROTOCOL, not a
/// choice a community makes.
///
/// An epoch number is `unix_secs / EPOCH_SECS`, so it is absolute rather than
/// relative to any genesis: epoch N names the same stretch of time in every
/// community there is. That is what makes an epoch usable as a unit ACROSS a
/// boundary — the correspondent's two legs (§Model) are ordered by comparing two
/// integers, and the comparison is only meaningful because the integers mean
/// the same thing on both sides.
///
/// It was per-community state once, ungoverned and never changed, which made
/// the alignment true in fact and unguaranteed in principle: two communities
/// could differ, nothing would notice, and every cross-boundary deadline would
/// be silently wrong. A client cannot check it either — it reads the number,
/// not the clock behind it. Fixing it here makes the comparison sound by
/// construction and deletes the conversion that stood in for the guarantee.
///
/// A community wanting a different rhythm has the dials that actually mean
/// that: `stake_decay` for how fast standing fades, and per-contract maturity
/// for how long credit runs. Neither is the clock.
pub const EPOCH_SECS: u64 = 86_400;
/// Minimum contract maturity, in epochs.
pub const MIN_MATURITY_EPOCHS: u64 = 30;

/// Guardian threshold minimum; veto window (epochs).
pub const GUARDIAN_MIN: u32 = 2;
pub const VETO_WINDOW_EPOCHS: u64 = MIN_MATURITY_EPOCHS;
/// Ceiling on any epoch horizon a transaction may name for itself — a
/// contract's maturity, an arbitration window, a guardian veto window.
pub const MAX_HORIZON_EPOCHS: u64 = 10_000;
/// The insured horizon at genesis, in epochs: how far past its ACCEPTANCE a
/// claim may mature and still hold a reservation. Governed
/// (`ParamKey::InsuredHorizon`) between `MIN_MATURITY_EPOCHS` and
/// `MAX_HORIZON_EPOCHS`, and measured from the acceptance epoch — which a
/// transfer and a routed successor inherit — so no chain of extensions rolls
/// it: past the horizon an acceptance is booked uninsured and an extension
/// drops the insurance, on the creditor's own signature either way, and the
/// way to keep a claim insured longer is to settle and re-accept it against
/// the current cut. A year, erring short: understating it costs friction —
/// a long claim falls to the uninsured tier, where its creditor signed for
/// it — while overstating it locks an underwriter's supply on a performing
/// claim for a term nobody priced to them.
pub const INSURED_HORIZON_EPOCHS: u64 = 365;
/// Ceiling on any single validator's voting power.
pub const MAX_VALIDATOR_POWER: u64 = 1_000_000;
/// The validator floor a chain founds with when its genesis names none — the
/// dev chain's. One, because a solo harness chain is a legitimate thing to
/// run.
///
/// The floor the ledger enforces is `Params::min_validators`, genesis data
/// the ceremony writes: `MIN_VALIDATORS_REAL_CHAIN` for a real chain, this
/// for the dev one. Every removal path — `Exit`, a suspension, a
/// `ValidatorPower` of zero — refuses to leave fewer standing.
pub const MIN_VALIDATORS: usize = 1;
/// The floor for a chain that is not the dev chain: **4**, founded with and
/// then held by the ledger.
///
/// BFT tolerates `f` faults out of `3f + 1`, so a set of 4 is the smallest
/// that tolerates one — and one is the smallest number of faults worth
/// designing for, since a set that tolerates none is a single point of failure
/// wearing a quorum. At 1, 2 or 3 validators the safety argument every
/// certificate in this tree rests on is vacuous.
///
/// Decided at `genesis init`, because the state machine has no way to know
/// which kind of chain it is running and the ceremony does; written into
/// `Params::min_validators`, because a floor the ceremony alone held bound
/// the founding and nothing after it — a four was a three on one member's
/// free `Exit`, on no vote at all.
pub const MIN_VALIDATORS_REAL_CHAIN: usize = 4;
/// Client wallet default thresholds (advisory).
pub const WALLET_ACCEPT: f64 = 0.40;
pub const WALLET_REJECT: f64 = 0.80;
/// Hard ceiling on epochs a single block may close. A block whose timestamp
/// is wildly out of range (a poisoned WAL, a bug upstream of the timestamp
/// rule) would otherwise turn one `apply` call into an unbounded loop. This
/// is UNREACHABLE in a healthy network — the block timestamp validity rule
/// refuses such a block long before `begin_block` sees it — and exists purely
/// as a structural net, turning an unbounded hang into a shrug.
pub const MAX_EPOCH_ADVANCE_PER_BLOCK: u64 = 10_000;
/// **The longest ring of defaults the epoch sweep will net**, in hops.
///
/// Ring discovery is a bounded depth-first walk over the defaulted book, and
/// the bound is what keeps a sweep's cost a function of the constants rather
/// than of the graph: without one, a book with a long chain of defaults costs
/// a walk proportional to it on every epoch boundary, on every validator.
///
/// Eight because netting is worth most where it is common, and a ring of
/// obligations that closes at all closes short: A owes B who owes C who owes A
/// is the shape trade produces. A longer one is not refused as illegitimate —
/// it is simply not found, and the parties may still cure or settle.
pub const NETTING_MAX_RING: usize = 8;
/// **How many rings one epoch boundary nets.**
///
/// Netting is zero-priced and permissionless — nobody signs it — so it must be
/// bounded per sweep like every other crank. What is not netted this epoch is
/// still there next epoch: the precondition is a property of the book rather
/// than of a moment, and every ring netted destroys its own (at least one hop
/// closes), so a sweep cannot repeat one.
pub const NETTING_MAX_RINGS_PER_EPOCH: usize = 64;

/// Longest validity window a transaction may claim, in epochs. Bounds the
/// replay cache: no applied transaction id is retained longer than this many
/// epochs past the epoch it was applied in.
pub const MAX_TX_LIFETIME_EPOCHS: u64 = 30;

/// **The most installments an obligation may be discharged in.** A partial
/// `Settle` or `Cure` is at least this fraction of the original amount; the
/// payment that closes the row is any size.
///
/// A discharge is free, and "bounded by the amount on a row that was bonded
/// when it was created" was the sentence that priced it. An uninsured amount
/// is bounded by nothing but the ingress ceiling, so one allowance slot bought
/// `2^51` free settles of one minor unit — each a durable replay id, and on an
/// insured row each a re-hold on every validator. Pricing the settle by class
/// instead would refuse a member at their ceiling curing in parts, which is
/// the absorbing default the schedule exists to prevent; a floor on the SHARE
/// bounds the count and leaves the recovery path free. A five-year monthly
/// plan is sixty installments.
pub const MAX_INSTALLMENTS: u64 = 128;

/// How long a CLOSED obligation — settled, transferred, or cured — is kept in
/// the ledger before the epoch sweep drops it, counted from its creation and
/// never shorter than its own arbitration window.
///
/// **A closed row is a stock nothing else retires.** It is hashed into the
/// state root on every block, walked by every sweep, and served by the views,
/// while the only thing it still answers is "this was paid" — which the block
/// history already records, and which the stake the settlement conferred
/// already reflects. An inclusion proof taken while the row was live stays
/// valid against the root it was taken against, because a leaf salt binds the
/// snapshot it belongs to.
///
/// A year. Long enough that a dispute over a settled trade has an on-ledger
/// row for as long as anybody is plausibly still arguing about it, and short
/// enough that the book does not grow without bound.
pub const CLOSED_RETENTION_EPOCHS: u64 = 365;

/// How long a row that holds nothing is kept before the epoch sweep retires
/// it, counted from the epoch it was seated in.
///
/// **A seat is the one bond that does not return, because a row is a stock —
/// and a row that is empty is not one.** Empty is a property nobody can
/// impose on another member: an edge into a row is written by a creditor
/// settling with it, a contract by both parties, a bond by the member, a
/// supply by a ceremony, and the sweep asks for all of them to be gone —
/// every edge in either direction, every contract in any status, every
/// bond, default, supply, vote, pending rotation and every reference from a
/// panel, a proposal or a guardian roll — so the rule is not a lever anyone
/// can pull, and a member keeps their row by trading, which is what a row is
/// for. What retirement gives back is the seat, to the sponsor's reach, and
/// the key, which a later trade may seat again for the price of a seat.
///
/// Without it a community that seated its ceiling holds its dead for ever: at
/// a churn of a fifth a year, two fifths of its rows are alive in year four
/// and nobody can be seated until a ceremony. What a farm gains by it is
/// nothing it can use — a row the farm lets go empty was carrying no
/// allowance and no capacity, and the seats it returns re-seat at most the
/// same live count — so the bound on live rows holds and the total seated
/// over time is bounded by the ceiling times one plus the run's length over
/// this window.
///
/// A year: the same window a closed obligation is kept, which is the last
/// thing that names a row after its last trade, and the window in which an
/// edge of one denomination unit decays out of the graph.
pub const ROW_RETENTION_EPOCHS: u64 = 365;

/// How long a proposal is kept after it stops being actionable: an enacted one
/// past this, and one nobody carried past this from the epoch it opened.
///
/// Same reasoning as `CLOSED_RETENTION_EPOCHS`, on the other permanent row a
/// member can write. An enacted proposal changes nothing by staying — the
/// change is in `Params` and the cooldown is in `last_amend_epoch` — and an
/// un-enacted one that has sat unassented for a year is not going to carry.
pub const PROPOSAL_RETENTION_EPOCHS: u64 = 365;
