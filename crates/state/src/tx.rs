//! The transaction alphabet.
//!
//! Note what is absent: there is no admission, and there is no
//! creation either. An account is created by its first TRADE — a `Party::Key`
//! in an `Accept` or a `Sale` — and needs no approval because creation grants
//! nothing: an account with no incident stakes has a capacity of zero.
//!
//! That is not a smaller claim than "created by its first signature", it is the
//! same one with the free channel closed. Creation had to be unbillable, and an
//! unbillable transition is bounded by nothing, so it carried a ledger-wide
//! per-epoch cap that shut onboarding for everybody the moment anyone filled it.
//! Folding creation into a bonded transition keeps it unapprovable — nobody
//! decides whether the row appears — while making it cost what every other row
//! costs.

use crate::types::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Tx {
    // ---------------------------------------------------------- key custody
    /// Register the guardian set that may recover this account's keys.
    RegisterGuardians {
        member: MemberId,
        guardians: Vec<MemberId>,
        threshold: u32,
        veto_window_epochs: u64,
    },
    RotateRequest {
        member: MemberId,
        new_keys: Vec<Key>,
    },
    RotateVeto {
        member: MemberId,
    },
    RotateFinalize {
        member: MemberId,
    },
    /// Register or rotate the key this member's VALIDATOR signs consensus
    /// with — never one of `keys`, and never able to sign a transaction.
    ///
    /// Two keys because they live in different places and fail differently: a
    /// consensus key sits unencrypted on a server that answers the internet
    /// and signs a vote every second, a member key signs obligations from a
    /// device its holder carries. One key for both roles makes a validator host
    /// compromise an economic identity compromise, and leaves no rotation that
    /// does not also move the money.
    ///
    /// `key: None` retires the registration, which is how an operator stands
    /// their validator down. It is refused while they still hold voting
    /// power: a validator whose key the ledger cannot resolve is a validator
    /// no certificate can be verified against, and the set has to be able to
    /// name one for every member in it.
    SetConsensusKey {
        member: MemberId,
        key: Option<Key>,
    },
    /// Voluntary exit: valid once all obligations and bonds are cleared.
    Exit {
        member: MemberId,
    },

    // -------------------------------------------------------- support cascade
    /// Replace the supporter's beneficiary listing (weights are waterfill
    /// shares). Listing is the supporter's half of the consent; the
    /// beneficiary's half is `ApproveSupporter`, and drains need both.
    ListBeneficiaries {
        supporter: MemberId,
        entries: Vec<(MemberId, f64)>,
    },
    /// The beneficiary's moderation gate: drains from `supporter` execute only
    /// after approval, and can be closed again.
    ApproveSupporter {
        beneficiary: MemberId,
        supporter: MemberId,
        approved: bool,
    },
    /// The sale composite: discharge the buyer's existing obligations through
    /// the cascade, then book the remainder as fresh debt.
    ///
    /// Either side may be named by key, which is how a newcomer's first trade
    /// creates their account (see `Party`). A party named by key must sign, and
    /// both sides already had to.
    Sale {
        seller: Party,
        buyer: Party,
        amount: f64,
        maturity_epochs: u64,
    },

    // ------------------------------------------------------------ underwriting
    /// Lower or leave the underwriter role.
    ///
    /// **This transition cannot RAISE a declaration, at any capacity.** A
    /// supply is a promise from outside the community, and no quantity the
    /// community computed about itself can stand in for one: capping a raise
    /// by the declarer's own capacity reads correctly one party at a time and
    /// is hollow insurance in aggregate — twelve joiners wash-backing each
    /// other behind a seed of 100 declare 204,900 between them and borrow
    /// 204,800 of ledger-labelled INSURED credit. A supply rises through a
    /// ceremony and through nothing else: genesis, or a `SeedAmendment`
    /// (§Governance). Restating an unchanged supply is still legal, since
    /// `want == current` is not a raise.
    ///
    /// LOWERING is floored by the flow already committed through them
    /// (§Stability), because a withdrawal is decay applied to a source arc and
    /// takes the same floor: the debt did not shrink because the underwriter
    /// changed their mind.
    ///
    /// `supply: 0` from an underwriter carrying nothing leaves the role.
    DeclareSupply {
        member: MemberId,
        /// The new declared supply, in denomination units.
        supply: f64,
    },

    // -------------------------------------------------------------- contracts
    /// Book an obligation. Insured if it fits the debtor's capacity, in which
    /// case it reserves the flow that justified it; otherwise uninsured, at
    /// the creditor's own risk and with no community recourse.
    ///
    /// Either side may be named by key, which is how a newcomer's first trade
    /// creates their account (see `Party`). This is the transition the
    /// first-contact journey walks: nobody has backed the newcomer, so the
    /// obligation is uninsured and the established counterparty bears it alone
    /// — the same first risk §Recourse already named, now also carrying the bond for
    /// the row it brings into existence.
    Accept {
        debtor: Party,
        creditor: Party,
        amount: f64,
        maturity_epochs: u64,
        /// Consented arbitration terms, pinned here, immutable after.
        arb: Option<ArbTermsWire>,
    },
    /// Move the debtor of a claim: the old debtor discharges, the successor is
    /// booked against the new debtor's own standing.
    Transfer {
        contract: ContractId,
        new_debtor: MemberId,
    },
    Settle {
        contract: ContractId,
        amount: f64,
    },
    Extend {
        contract: ContractId,
        new_maturity_epoch: u64,
    },
    /// Permissionless default crank.
    MarkExpired {
        contract: ContractId,
    },
    /// Late discharge against an expired contract.
    Cure {
        contract: ContractId,
        amount: f64,
    },
    /// A panel arbiter's attestation of the disputed amount; the award mints
    /// when the quorum concurs (median, capped).
    ArbAttest {
        contract: ContractId,
        arbiter: MemberId,
        amount: f64,
    },

    // ------------------------------------------------------------- governance
    Propose {
        author: MemberId,
        kind: ProposalKind,
    },
    Assent {
        member: MemberId,
        proposal: ProposalId,
    },

    /// Permissionless forfeiture crank for a member in sustained exhaustion.
    ///
    /// Deliberately shaped like `MarkExpired` rather than like a governance
    /// vote: validity is a pure state check (the target's saturated-epoch
    /// counter has reached the threshold), so the sanction can neither be
    /// forged against a member who never exhausted its bonds nor suppressed by
    /// whoever would have had to call the vote. A discretionary forfeiture
    /// would also hand its caller a choice about *when* the abuser's debt
    /// lands, and the design's whole claim is that nobody chooses.
    ForfeitBonds {
        member: MemberId,
    },
}
