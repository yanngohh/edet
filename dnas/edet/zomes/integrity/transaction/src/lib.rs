pub mod checkpoint;
pub mod debt_contract;
pub mod functions;
pub mod reputation_claim;
pub mod transaction;
pub mod validation;
pub mod vouch;
pub use transaction::*;
pub use vouch::*;
pub mod types;
pub mod wallet;
pub use checkpoint::ChainCheckpoint;
use debt_contract::*;
use hdi::prelude::*;
pub use reputation_claim::ReputationClaim;
pub use wallet::*;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
#[hdk_entry_types]
#[unit_enum(UnitEntryTypes)]
pub enum EntryTypes {
    Wallet(Wallet),
    Transaction(Transaction),
    DebtContract(DebtContract),
    ReputationClaim(ReputationClaim),
    ChainCheckpoint(ChainCheckpoint),
    Vouch(Vouch),
}

#[derive(Serialize, Deserialize)]
#[hdk_link_types]
pub enum LinkTypes {
    OwnerToWallet,
    WalletUpdates,
    WalletToTransactions,
    TransactionToParent,
    /// Debtor -> DebtContract (find active contracts for a debtor)
    DebtorToContracts,
    /// Creditor -> DebtContract (find contracts where agent is creditor)
    CreditorToContracts,
    /// DebtContract updates chain
    DebtContractUpdates,
    /// Agent(i) -> Agent(j) with tag = TrustLinkTag { c_ij, epoch }
    /// Published local trust rows for Subjective Local Expansion (whitepaper line 640)
    AgentToLocalTrust,
    /// Agent(i) -> Agent(j) for acquaintance set A_i (whitepaper Definition 3)
    AgentToAcquaintance,
    /// Agent -> ReputationClaim action hash (find latest claim for an agent)
    /// Tag contains the epoch for efficient lookup.
    AgentToReputationClaim,
    /// Agent -> Archived DebtContract (for historical lookup only).
    /// Archived contracts are excluded from active scans for scalability.
    AgentToArchivedContracts,
    /// Agent -> ChainCheckpoint (for efficient checkpoint lookup).
    /// Tag contains the epoch and sequence number.
    AgentToCheckpoint,
    /// Agent -> Agent (self-link) with tag = DebtBalanceTag.
    /// Maintains a running total of outstanding debt for O(1) capacity checks.
    AgentToDebtBalance,
    /// Agent -> DebtContract with tag = EpochBucketTag.
    /// Enables epoch-scoped contract queries for incremental claim updates.
    AgentToContractsByEpoch,
    /// Agent (Entrant) -> Vouch (to calculate capacity)
    EntrantToVouch,
    /// Agent (Sponsor) -> Vouch (to track stakes)
    SponsorToVouch,
    /// Vouch updates chain (for status transitions)
    VouchUpdates,
    /// Agent(creditor) -> Agent(debtor) with tag = FailureObservationTag { amount, epoch }
    /// Published when a creditor observes a debtor default. Enables community-wide
    /// contagion: other nodes can query who has observed a given debtor default.
    /// Observations are DHT-verifiable via the debtor's expired contracts.
    AgentToFailureObservation,
    /// Path-based anchor for reverse lookup of failure observations.
    /// FailureObservationAnchor -> Agent(debtor) with tag = FailureObservationIndexTag
    /// Enables efficient queries of "who has observed this debtor default?"
    FailureObservationIndex,
    /// Agent(debtor) -> Agent(seller) — permanent block written when a trial contract expires/defaults.
    /// Once this link exists, the (buyer, seller) pair is permanently barred from future trials.
    /// This prevents Sybil identity cycling via repeated trial defaults.
    /// Append-only; never deleted.
    DebtorToBlockedTrialSeller,
    /// Agent(beneficiary) -> Transaction(drain) with tag = SupportSatisfactionTag.
    /// Created when a drain transaction is accepted and the cascade successfully reduces
    /// the beneficiary's debt. Enables compute_sf_counters to recognize support events
    /// as satisfaction evidence: S_{beneficiary←supporter} = drained_amount.
    AgentToSupportSatisfaction,
}

#[hdk_extern]
pub fn genesis_self_check(_data: GenesisSelfCheckData) -> ExternResult<ValidateCallbackResult> {
    // NOTE: No membrane proof is required for joining the edet network.
    // The protocol uses an open-network model: any agent can join without pre-authorization.
    // Trust is established through trial transactions (the real bootstrap mechanism),
    // not through admission control. Future deployments MAY implement membrane proofs
    // for closed-community networks without changing the trust protocol.
    Ok(ValidateCallbackResult::Valid)
}

pub fn validate_agent_joining(
    _agent_pub_key: AgentPubKey,
    _membrane_proof: &Option<MembraneProof>,
) -> ExternResult<ValidateCallbackResult> {
    // Open network — no membrane proof required. See genesis_self_check comment above.
    Ok(ValidateCallbackResult::Valid)
}

#[hdk_extern]
pub fn validate(op: Op) -> ExternResult<ValidateCallbackResult> {
    validation::validate(op)
}
