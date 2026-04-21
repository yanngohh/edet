pub mod constants;
pub use constants::*;
use hdi::prelude::{encode::blake2b_128, *};

use crate::debt_contract::{ContractStatus, DebtContract};
use crate::{EntryTypes, Party, Transaction, TransactionSide, TransactionStatus, Wallet};
use hdi::prelude::{
    wasm_error, AgentPubKey, AgentPubKeyB64, AppEntryDef, EntryType, EntryVisibility, ExternResult,
    ScopedEntryDefIndex, WasmError, WasmErrorInner, HOLO_HASH_UNTYPED_LEN,
};
impl Default for Wallet {
    fn default() -> Self {
        Self {
            owner: AgentPubKey::from_raw_36(vec![0xdb; HOLO_HASH_UNTYPED_LEN]).into(),
            auto_reject_threshold: Default::default(),
            auto_accept_threshold: Default::default(),
            total_slashed_as_sponsor: 0.0,
            trial_tx_count: 0,
            last_trial_epoch: 0,
        }
    }
}

impl Wallet {
    pub fn new(owner: &AgentPubKeyB64) -> Self {
        Wallet {
            owner: owner.to_owned(),
            auto_reject_threshold: WALLET_DEFAULT_AUTO_REJECT_THRESHOLD,
            auto_accept_threshold: WALLET_DEFAULT_AUTO_ACCEPT_THRESHOLD,
            total_slashed_as_sponsor: 0.0,
            trial_tx_count: 0,
            last_trial_epoch: 0,
        }
    }

    pub fn entry_type() -> ExternResult<EntryType> {
        let ScopedEntryDefIndex { zome_index, zome_type: entry_def_index } = Wallet::entry_def()?;
        let visibility = EntryVisibility::from(&EntryTypes::Wallet(Wallet::default()));
        Ok(EntryType::App(AppEntryDef::new(entry_def_index, zome_index, visibility)))
    }

    pub fn entry_def() -> ExternResult<ScopedEntryDefIndex> {
        (&EntryTypes::Wallet(Wallet::default())).try_into()
    }
}

impl TryFrom<&EntryHash> for Wallet {
    type Error = WasmError;

    fn try_from(value: &EntryHash) -> Result<Self, Self::Error> {
        must_get_entry(value.to_owned())?
            .as_app_entry()
            .map(|entry| entry.clone().into_sb())
            .ok_or(wasm_error!(WasmErrorInner::Guest(String::from("Cannot retrieve Wallet entry serialized bytes"))))?
            .try_into()
            .map_err(|_| wasm_error!("Cannot convert serialized bytes to Wallet"))
    }
}

impl Default for Transaction {
    fn default() -> Self {
        Transaction {
            id: None,
            buyer: Party {
                side: TransactionSide::Buyer,
                pubkey: AgentPubKey::from_raw_36(vec![0xdb; HOLO_HASH_UNTYPED_LEN]).into(),
                previous_transaction: None,
                wallet: ActionHash::from_raw_36(vec![0xdb; HOLO_HASH_UNTYPED_LEN]),
            },
            seller: Party {
                side: TransactionSide::Seller,
                pubkey: AgentPubKey::from_raw_36(vec![0xdb; HOLO_HASH_UNTYPED_LEN]).into(),
                previous_transaction: None,
                wallet: ActionHash::from_raw_36(vec![0xdb; HOLO_HASH_UNTYPED_LEN]),
            },
            debt: 0f64,
            description: String::new(),
            parent: None,
            updated_action: None,
            status: TransactionStatus::Testing,
            is_trial: false,
            drain_metadata: None,
        }
    }
}

impl Transaction {
    pub fn initial(agent: &AgentPubKeyB64, wallet: &ActionHash) -> Transaction {
        let mut initial = Transaction::default();
        initial.buyer.pubkey = agent.to_owned();
        initial.buyer.wallet = wallet.to_owned();
        initial.seller.pubkey = agent.to_owned();
        initial.seller.wallet = wallet.to_owned();
        initial.status = TransactionStatus::Accepted;
        initial.description = "Initial transaction".into();
        initial
    }

    pub fn is_initial(&self) -> bool {
        self.buyer.pubkey == self.seller.pubkey && self.debt == 0f64 && self.status == TransactionStatus::Accepted
    }

    pub fn setup(&mut self, timestamp: Timestamp, seller_wallet: ActionHash, buyer_wallet: ActionHash) {
        let (seconds, nanos) = timestamp.as_seconds_and_nanos();
        let mut hash: Vec<u8> = Vec::new();
        hash.append(&mut [0u8; 16].to_vec());
        hash.append(&mut blake2b_128(format!("{seller_wallet}{buyer_wallet}{seconds}{nanos}").as_bytes()));

        let mut hash_bytes: Vec<u8> = Vec::new();
        hash_bytes.append(&mut encode::holo_dht_location_bytes(&hash));
        hash_bytes.append(&mut hash);
        let id: ExternalHash = HoloHash::from_raw_36_and_type(hash_bytes, hash_type::External);
        self.id = Some(id);

        self.buyer.wallet = buyer_wallet;
        self.seller.wallet = seller_wallet;
    }

    pub fn get_party_address(&self, agent: &AgentPubKeyB64) -> ExternResult<AgentPubKeyB64> {
        if self.buyer.pubkey == *agent {
            Ok(self.seller.pubkey.to_owned())
        } else if self.seller.pubkey == *agent {
            Ok(self.buyer.pubkey.to_owned())
        } else {
            Err(wasm_error!(WasmErrorInner::Guest(type_resolution_error::AGENT_NOT_TRANSACTION_PARTY.to_string())))
        }
    }

    pub fn entry_type() -> ExternResult<EntryType> {
        let ScopedEntryDefIndex { zome_index, zome_type: entry_def_index } = Transaction::entry_def()?;
        let visibility = EntryVisibility::from(&EntryTypes::Transaction(Transaction::default()));
        Ok(EntryType::App(AppEntryDef::new(entry_def_index, zome_index, visibility)))
    }

    pub fn entry_def() -> ExternResult<ScopedEntryDefIndex> {
        (&EntryTypes::Transaction(Transaction::default())).try_into()
    }
}

impl TryFrom<&EntryHash> for Transaction {
    type Error = WasmError;

    fn try_from(value: &EntryHash) -> Result<Self, Self::Error> {
        must_get_entry(value.to_owned())?
            .as_app_entry()
            .map(|entry| entry.clone().into_sb())
            .ok_or(wasm_error!(WasmErrorInner::Guest(String::from(
                "Cannot retrieve Transaction entry serialized bytes"
            ))))?
            .try_into()
            .map_err(|err| {
                wasm_error!(WasmErrorInner::Guest(format!("Cannot convert serialized bytes to Transaction: {err}")))
            })
    }
}

#[derive(Serialize, Deserialize, SerializedBytes, Debug)]
pub struct RankingTag {
    pub ranking: i64,
    pub custom_tag: Option<SerializedBytes>,
    pub agents: Vec<AgentPubKey>,
}

#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum TransactionStatusTag {
    Pending,
    Finalized,
}

pub trait RankingCursorTagType: Clone + TryFrom<SerializedBytes> + PartialEq {}

impl RankingCursorTagType for TransactionStatusTag {}

// =========================================================================
//  DebtContract Types (Whitepaper Definition 1)
// =========================================================================

impl Default for DebtContract {
    fn default() -> Self {
        DebtContract {
            amount: 0.0,
            original_amount: 0.0,
            maturity: MIN_MATURITY,
            start_epoch: 0,
            creditor: AgentPubKey::from_raw_36(vec![0xdb; HOLO_HASH_UNTYPED_LEN]).into(),
            debtor: AgentPubKey::from_raw_36(vec![0xdb; HOLO_HASH_UNTYPED_LEN]).into(),
            transaction_hash: ActionHash::from_raw_36(vec![0xdb; HOLO_HASH_UNTYPED_LEN]),
            co_signers: None,
            status: ContractStatus::Active,
            is_trial: false,
        }
    }
}

impl DebtContract {
    pub fn entry_type() -> ExternResult<EntryType> {
        let ScopedEntryDefIndex { zome_index, zome_type: entry_def_index } = DebtContract::entry_def()?;
        let visibility = EntryVisibility::from(&EntryTypes::DebtContract(DebtContract::default()));
        Ok(EntryType::App(AppEntryDef::new(entry_def_index, zome_index, visibility)))
    }

    pub fn entry_def() -> ExternResult<ScopedEntryDefIndex> {
        (&EntryTypes::DebtContract(DebtContract::default())).try_into()
    }
}

// =========================================================================
//  Trust Link Tag (Whitepaper Section 7.1 - DHT Publication)
// =========================================================================

/// Serialized tag for AgentToLocalTrust links.
/// Each agent publishes their normalized local trust row as links on the DHT:
/// Agent(i) -> Agent(j) with tag containing the trust value c_ij and the
/// epoch at which it was computed.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone)]
pub struct TrustLinkTag {
    /// Normalized local trust value c_ij in [0, 1].
    pub trust_value: f64,
    /// Epoch at which this trust value was computed.
    pub epoch: u64,
}

// =========================================================================
//  Failure Observation Tag (Witness-Based Contagion)
// =========================================================================

/// Serialized tag for AgentToFailureObservation links.
/// Published when a creditor observes a debtor default, enabling community-wide
/// contagion: other nodes can query who has observed a given debtor default.
/// Observations are DHT-verifiable: the `expired_contract_hash` field encodes
/// the ActionHash of the expired DebtContract that triggered the observation.
/// Integrity validation verifies this contract exists, is Expired/Archived,
/// and that the debtor matches the link target.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone)]
pub struct FailureObservationTag {
    /// Amount that failed (for potential future weighting; currently unused).
    pub amount: f64,
    /// Epoch when the failure was observed.
    pub epoch: u64,
    /// ActionHash of the expired DebtContract serving as proof of the default.
    pub expired_contract_hash: ActionHash,
    /// The witness's bilateral F/(S+F) with the debtor at the time of observation.
    /// Used by other nodes to compute the aggregate witness rate for contagion.
    /// Defaults to 0.0 for backwards compatibility with pre-existing observations.
    #[serde(default)]
    pub witness_bilateral_rate: f64,
}

/// Serialized tag for FailureObservationIndex links.
/// Used for reverse lookup: path -> debtor, with creditor encoded in tag.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone)]
pub struct FailureObservationIndexTag {
    /// The creditor who observed the failure.
    pub creditor: AgentPubKeyB64,
    /// Epoch when the observation was published.
    pub epoch: u64,
    /// The creditor's bilateral F/(S+F) at observation time.
    /// Used to compute the aggregate witness rate for the imputed floor.
    /// Defaults to 0.0 for backwards compatibility with pre-existing observations.
    #[serde(default)]
    pub witness_bilateral_rate: f64,
}

// =========================================================================
//  Support Satisfaction Tag (Drain-Based Reputation Evidence)
// =========================================================================

/// Serialized tag for AgentToSupportSatisfaction links.
/// Created when a drain transaction is accepted and the cascade successfully
/// reduces the beneficiary's debt. Enables `compute_sf_counters` to recognize
/// support events as satisfaction evidence: the beneficiary records S from the
/// supporter, populating the pre-trust vector so EigenTrust can propagate trust
/// back to the beneficiary.
///
/// Semantics: when supporter A's cascade drains beneficiary B's debt by `amount`,
/// B creates a link B -> drain_tx_hash with this tag. When B later computes
/// sf_counters(B), it scans these links and records S_{B←A} = amount.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone)]
pub struct SupportSatisfactionTag {
    /// The supporter (buyer in the drain transaction) who provided the drain.
    pub supporter: AgentPubKeyB64,
    /// Amount of debt successfully drained (transferred) by this support event.
    pub amount: f64,
    /// Epoch when the support satisfaction was recorded.
    pub epoch: u64,
}

// =========================================================================
//  Debt Balance Tag (Scalability: O(1) Debt Lookups)
// =========================================================================

/// Serialized tag for AgentToDebtBalance links.
/// Maintains a running total of an agent's outstanding debt, avoiding
/// the need to scan all active contracts on every capacity check.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone)]
pub struct DebtBalanceTag {
    /// Total outstanding debt (sum of active contract amounts).
    pub total_debt: f64,
    /// Epoch when this balance was last updated.
    pub epoch: u64,
    /// Number of active contracts contributing to this balance.
    pub contract_count: u64,
    /// Monotonically-increasing sequence counter. Incremented on every write.
    ///
    /// This enables optimistic-concurrency detection: after deleting the old
    /// balance link and before creating the new one, a concurrent write by
    /// another zome call (e.g., a simultaneous contract creation and expiration
    /// processing) would produce a link with sequence N+1 while this call also
    /// tries to write N+1. The resulting duplicate is harmless — `get_links`
    /// returns both, and callers always take `links.first()` which is
    /// deterministically ordered by the Holochain DHT. The `sequence` field
    /// therefore acts as a diagnostic aid: diverging sequences indicate
    /// concurrent writes and can trigger a `rebuild_debt_balance` call to
    /// reconcile from the authoritative contract list.
    #[serde(default)]
    pub sequence: u64,
}

/// Serialized tag for AgentToContractsByEpoch links.
/// Enables epoch-scoped contract queries for true incremental claim updates.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone)]
pub struct EpochBucketTag {
    /// The epoch in which this contract was created.
    pub epoch: u64,
}

// =========================================================================
//  Distributed Epochs (Whitepaper Section 7.1)
// =========================================================================

/// Convert a Holochain Timestamp to a distributed epoch number.
/// E = floor(unix_timestamp_secs / EPOCH_DURATION_SECS)
pub fn timestamp_to_epoch(timestamp: Timestamp) -> u64 {
    let (secs, _nanos) = timestamp.as_seconds_and_nanos();
    if secs < 0 {
        0
    } else {
        secs as u64 / EPOCH_DURATION_SECS
    }
}

/// Check whether a timestamp falls within [Δ_drift] seconds of an epoch boundary.
///
/// Used for boundary-window epoch validation (Whitepaper Property: Epoch Unambiguity):
/// when an action is authored near midnight (within CLOCK_DRIFT_MAX_SECS of an epoch
/// boundary), the author's declared epoch could be either E or E+1 depending on their
/// clock. Validators must accept both adjacent epochs to avoid spurious rejections.
///
/// Returns true if `|timestamp_secs mod EPOCH_DURATION_SECS| < CLOCK_DRIFT_MAX_SECS`
/// OR `|EPOCH_DURATION_SECS - (timestamp_secs mod EPOCH_DURATION_SECS)| < CLOCK_DRIFT_MAX_SECS`.
pub fn timestamp_near_epoch_boundary(timestamp: Timestamp) -> bool {
    let (secs, _nanos) = timestamp.as_seconds_and_nanos();
    if secs < 0 {
        return false;
    }
    let secs_u64 = secs as u64;

    #[allow(clippy::modulo_one)]
    let offset_within_epoch = secs_u64 % EPOCH_DURATION_SECS;
    // Distance from start of epoch (boundary at epoch beginning)
    let dist_from_start = offset_within_epoch;
    // Distance from end of epoch (boundary at epoch end / next epoch beginning)
    let dist_from_end = EPOCH_DURATION_SECS - offset_within_epoch;
    dist_from_start < CLOCK_DRIFT_MAX_SECS || dist_from_end <= CLOCK_DRIFT_MAX_SECS
}
