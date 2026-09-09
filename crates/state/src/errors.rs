//! Named rejection codes. Every code here is REACHABLE and has at least one
//! regression test in `crates/state/tests/`.
//!
//! Both halves are the claim. `ET-CTR-003` ("not a party") and `ET-GOV-002`
//! ("unknown parameter") lived here for a release without a single `return`
//! naming either: the first was answered by requiring both parties' signatures,
//! the second by making `ParamKey` an enum, so each was closed by construction
//! and the constant outlived the check. A published code no path can produce is
//! worse than none — it is a documented behaviour that never happens, and the
//! client translated it into six languages. They are gone, translations with
//! them. Do not add a code before the `return` that uses it.

pub type Code = &'static str;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(pub Code);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for Error {}

pub type Res<T> = Result<T, Error>;

/// A key already spoken for — by a member, or by a rotation waiting out its
/// veto window. Naming it as a party resolves to its holder; it can never seat
/// a second account on it.
pub const ET_ADM_DUP_KEY: Code = "ET-ADM-002";
/// A malformed key list: empty, or longer than one account may hold
/// (`MAX_KEYS_PER_ACCOUNT`). `RotateRequest` is the one transition that writes
/// a key list, so it is the one that has to carry the bound.
pub const ET_ADM_BAD_KEYS: Code = "ET-ADM-003";
// ET-ADM-004 was the account-creation burst bound: a LEDGER-WIDE per-epoch
// counter on an unbonded transition, so whoever filled it first shut
// onboarding for everybody at zero cost. Retired with `OpenAccount` itself
// — an account is seated by the bonded trade that names it,
// and the bond is the bound. The code is deliberately not reused: a stale
// client asserting on it should find nothing rather than something else.
pub const ET_MEM_UNKNOWN: Code = "ET-MEM-001";
pub const ET_MEM_NOT_ACTIVE: Code = "ET-MEM-002";
pub const ET_MEM_NOT_SIGNER: Code = "ET-MEM-003";
/// A Suspended seller's `Sale` would leave a remainder past what mutual
/// netting against the buyer already covers. Suspension leaves discharge
/// open (a suspended member can always pay down what they owe), but a sale's
/// cascade and any genesis remainder are new credit the seller would be
/// *originating* — the thing suspension revokes, not the thing it leaves
/// open. Distinct from `ET_MEM_NOT_ACTIVE` (which still refuses a
/// Probationary or Exited seller outright): a Suspended seller is not
/// inactive here, they are over the netting line.
pub const ET_MEM_SUSPENDED_NO_ORIGINATION: Code = "ET-MEM-004";

// Contracts
pub const ET_CTR_UNKNOWN: Code = "ET-CTR-001";
pub const ET_CTR_BAD_STATE: Code = "ET-CTR-002";
pub const ET_CTR_BAD_AMOUNT: Code = "ET-CTR-004";
pub const ET_CTR_MATURITY_TOO_SHORT: Code = "ET-CTR-005";
pub const ET_CTR_NOT_DUE: Code = "ET-CTR-006";
pub const ET_CTR_SELF_DEAL: Code = "ET-CTR-007";
pub const ET_CTR_MATURITY_TOO_LONG: Code = "ET-CTR-008";

// Underwriting (§Standing, §Stability)
/// A `DeclareSupply` that raises the declaration. **Every raise is refused**:
/// supply is seated by a ceremony (genesis, or a §Governance amendment) and by
/// nothing else, so this transition may only lower one.
///
/// The name is the cap it replaces. Capping a declaration by the declarer's own
/// CAPACITY keeps it from being a free signature — an account nobody backs has
/// capacity zero — and is a supply of hollow insurance in aggregate, because
/// capacity is what the community conferred: twelve joiners wash-backing each
/// other behind a seed of 100 reach a declared 204,900 and borrow 204,800 of
/// ledger-labelled INSURED credit against it.
pub const ET_UWR_ABOVE_CAPACITY: Code = "ET-UWR-001";
/// A withdrawal below the flow already committed through this underwriter
/// (§Stability). Measured, when the floor was missing: one of six underwriters leaving
/// a fully drawn community left capacity 12,500 against 15,000 outstanding.
pub const ET_UWR_BELOW_COMMITTED: Code = "ET-UWR-002";
/// An `Exit` by a member still carrying a declared supply. Leaving the
/// community and leaving the underwriter role are two acts, and the second has
/// a floor the first must not be able to jump.
pub const ET_UWR_STILL_DECLARED: Code = "ET-UWR-003";

// Seed amendments (§Governance)
/// An amendment beyond what this epoch may still admit: β × the external seed
/// tracked when the epoch opened, less what has already been admitted in it.
///
/// The base is the EXTERNAL seed and never the declared total, which is the
/// whole security content of the bound — §Standing's declared total inflates
/// geometrically from inside the community, and a rate computed on it would
/// convert that inflation into amendment headroom.
pub const ET_SEED_RATE: Code = "ET-SED-001";
/// A member assenting to their own seed amendment. The author of a
/// `SeedAmendment` IS its beneficiary (the kind names nobody), so their assent
/// is a vote on their own supply — and under a governance weight that counts an
/// underwriter's declared supply, a dominant underwriter could otherwise raise
/// themselves on their own weight, epoch after epoch, each raise enlarging the
/// weight that carries the next.
pub const ET_SEED_CONFLICTED: Code = "ET-SED-002";

// Pool

// Rotation
pub const ET_ROT_NO_GUARDIANS: Code = "ET-ROT-001";
pub const ET_ROT_THRESHOLD: Code = "ET-ROT-002";
pub const ET_ROT_NO_REQUEST: Code = "ET-ROT-003";
pub const ET_ROT_WINDOW_OPEN: Code = "ET-ROT-004";
// ET-ROT-005 ("the rotation was vetoed") is retired and not reused: a veto
// deletes the request, so a finalize after one finds no request (ET-ROT-003)
// and a second veto finds nothing to veto. A stale client asserting on it
// should find nothing rather than something else.
pub const ET_ROT_BAD_WINDOW: Code = "ET-ROT-006";

// Support cascade
pub const ET_CAS_BAD_WEIGHT: Code = "ET-CAS-001";
// ET-CAS-002 (self-listing) is deliberately unused and not reused: a self entry
// is the legal share of a sale that clears the supporter's own debts.
pub const ET_CAS_TOO_MANY: Code = "ET-CAS-003";
pub const ET_CAS_NOT_LISTED: Code = "ET-CAS-004";

// Arbitration
pub const ET_ARB_NO_TERMS: Code = "ET-ARB-001";
pub const ET_ARB_NOT_PANEL: Code = "ET-ARB-002";
pub const ET_ARB_WINDOW_CLOSED: Code = "ET-ARB-003";
pub const ET_ARB_ALREADY_ATTESTED: Code = "ET-ARB-004";
pub const ET_ARB_ALREADY_AWARDED: Code = "ET-ARB-005";
pub const ET_ARB_BAD_TERMS: Code = "ET-ARB-006";
/// The award crank asked before the attestation window has closed. The award is
/// the median over every attestation the window received, so there is nothing
/// to take a median OF until it has run.
pub const ET_ARB_WINDOW_OPEN: Code = "ET-ARB-007";

// Lifecycle
pub const ET_LIF_OUTSTANDING_DEBT: Code = "ET-LIF-001";
pub const ET_LIF_OUTSTANDING_BONDS: Code = "ET-LIF-002";

// Validators
pub const ET_VAL_NOT_ELIGIBLE: Code = "ET-VAL-001";
/// Refused whenever removing a validator (via `Exit`, `Suspend`, or
/// `ValidatorPower { power: 0 }`) would drop the validator set below
/// `edet_kernel::constants::MIN_VALIDATORS`. See that constant's doc comment
/// for why the floor exists at all.
pub const ET_VAL_LAST_VALIDATOR: Code = "ET-VAL-002";
/// A `ValidatorPower` beyond `MAX_VALIDATOR_POWER`, or one that would make
/// the set's total voting power overflow the `u64` consensus sums it into.
pub const ET_VAL_POWER_TOO_HIGH: Code = "ET-VAL-003";
/// Voting power for a member who has registered no consensus key, or a
/// `SetConsensusKey { key: None }` from a member who still holds power.
///
/// A validator the ledger cannot name a signing key for is a validator no
/// certificate can be verified against, and `EdetValidatorSet::build` fails
/// closed rather than quietly returning a smaller set with a lower quorum. So
/// the registration comes first and the power second, in both directions.
pub const ET_VAL_NO_CONSENSUS_KEY: Code = "ET-VAL-004";
/// A consensus key already registered by another member, or one that is
/// somebody's MEMBER key. Both would make one key two identities, and the
/// second would put an economic signature back on a validator host — which is
/// the separation this key exists for.
pub const ET_VAL_KEY_IN_USE: Code = "ET-VAL-005";

// Governance
pub const ET_GOV_NOT_ESTABLISHED: Code = "ET-GOV-001";
pub const ET_GOV_OUT_OF_RANGE: Code = "ET-GOV-003";
pub const ET_GOV_COOLDOWN: Code = "ET-GOV-004";
pub const ET_GOV_UNKNOWN_PROPOSAL: Code = "ET-GOV-005";
pub const ET_GOV_BAND: Code = "ET-GOV-006";
/// An assent from outside the electorate: the member holds no external supply,
/// so their weight under that measure is exactly zero.
///
/// **Refused rather than silently discounted**, which is the same rule
/// `ET-SED-002` already follows: a recorded assent that did not count would be
/// a vote the ledger shows and does not use, and ledger state carries what
/// must be enforced rather than what a member prefers. A member who is later
/// endorsed re-assents, which is a fresh decision rather than a formality —
/// the same reasoning §Governance gives for having no vesting.
///
/// Numbered 007 rather than reusing the retired 002: an old client that still
/// maps 002 to "unknown parameter" would translate this into something false.
pub const ET_GOV_NO_MANDATE: Code = "ET-GOV-007";
/// An assent from a member already on the proposal's record. Refused rather
/// than absorbed: the assent set took the duplicate silently, so a re-assent
/// was admitted without limit, free, and a durable replay id each time. A
/// zero-priced transition must destroy its own precondition, and this is what
/// refuses the second call.
pub const ET_GOV_ALREADY_ASSENTED: Code = "ET-GOV-008";

// Operation bonds (enforced in `apply` after the envelope checks and before
// dispatch — see `bond_gate`).
/// The submitter's remaining headroom cannot cover this transition's bond.
/// This is the "network denies to execute" point, and it is deliberately the
/// SAME ceiling that already refuses debt beyond capacity rather than a
/// second, independent limit.
pub const ET_BOND_EXHAUSTED: Code = "ET-BND-001";
/// A `ForfeitBonds` crank against a member that is not in sustained
/// exhaustion. A pure state check, exactly like `MarkExpired`: the sanction
/// cannot be forged against a member who never saturated the gate.
pub const ET_BOND_NOT_SATURATED: Code = "ET-BND-002";
/// A `ForfeitBonds` crank against a member holding nothing to forfeit.
pub const ET_BOND_NOTHING_HELD: Code = "ET-BND-003";
/// A priced transition with no signer that resolves to a member, so there is
/// nobody to charge. Refused rather than waved through, because a
/// trade may name a party by KEY: without this, two fresh keys signing an
/// `Accept` between themselves would write two account rows and a contract for
/// nothing, which is the free channel deleting `OpenAccount` exists to close.
///
/// The honest reading for a client is "you have nobody to trade with yet" —
/// the newcomer's first transaction is carried by the established member who
/// chose to trade with them, exactly as `bonded_party` describes.
pub const ET_BOND_NO_PAYER: Code = "ET-BND-004";
/// The payer's own write budget is zero because their STATUS zeroed it, not
/// because they spent it. `bond_headroom` returns 0.00 for any member that is
/// not `Active`, so without a code of its own every priced transition a
/// suspended member signs for themselves arrives as `ET-BND-001` — which tells
/// them to wait for bonds to release, and no bond was ever charged, so waiting
/// is not the remedy. Worse, `ET-BND-001` arms the saturation counter and the
/// epoch sweep forfeits on it: a suspended member holding 20.00 encumbered from
/// before the suspension loses all of it, permanently, for nothing but trying
/// to use their own wallet.
///
/// The honest reading for a client is "your status has suspended your write
/// budget": what a non-Active member may still do — settle, cure, transfer out,
/// register guardians, rotate, approve a supporter, exit — a co-signer with
/// headroom can pay for, because the bill goes to the first signer in
/// canonical order who CAN pay and a suspended member never can.
pub const ET_BOND_STATUS: Code = "ET-BND-005";
/// The sponsor's SEAT REACH is spent: no bond unit of the seed's flow reaches
/// them on the write layer once every live seat is netted out, so this trade
/// cannot bring a new row into the ledger.
///
/// Distinct from `ET-BND-001` on both counts that matter. The remedy is
/// different — a work bond releases on schedule and nothing releases a seat,
/// so only new backing reaching the sponsor opens one — and it **must not arm
/// the saturation counter**: a seat refusal is a member having brought in as
/// many people as their standing carries, and arming forfeiture on it would
/// let the epoch sweep take a member's bonds for exactly that.
///
/// The honest reading for a client is "you have brought in as many people as
/// your standing carries; more backing on you opens more".
pub const ET_BOND_SEAT_UNBACKED: Code = "ET-BND-006";

// Transaction envelope (replay/expiry/window — enforced in `apply`
// before dispatch, ahead of every other check).
pub const ET_TX_REPLAY: Code = "ET-TX-001";
pub const ET_TX_EXPIRED: Code = "ET-TX-002";
pub const ET_TX_WINDOW_TOO_LONG: Code = "ET-TX-003";
/// The caller (`Replica::apply_block_to`) could not compute this
/// transaction's id (`SignedTx::id`) at all — a defensive code path, since
/// `codec`-encoding a plain in-memory struct into a `Vec` cannot fail in
/// practice; recorded as an ordinary failed outcome rather than silently
/// dropping the transaction (see that function's doc comment).
pub const ET_TX_UNDIGESTABLE: Code = "ET-TX-004";
