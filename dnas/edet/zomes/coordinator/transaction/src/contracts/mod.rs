pub mod archival;
pub mod debt_balance;
pub mod expiration;
pub mod transfer;

use hdk::prelude::*;
use transaction_integrity::debt_contract::{ContractStatus, DebtContract};
use transaction_integrity::types::constants::{coordinator_contract_error, *};
use transaction_integrity::types::{timestamp_to_epoch, EpochBucketTag};
use transaction_integrity::*;

// Re-export all public items so existing `contracts::` paths keep working.
pub use archival::{archive_old_contracts, get_archived_contracts, ArchivalResult};
pub use debt_balance::{get_total_debt, rebuild_debt_balance, update_debt_balance};
pub use expiration::{get_next_debt_expiration, process_contract_expirations, ExpirationResult};
pub use transfer::{transfer_debt, DebtTransferResult};

// =========================================================================
//  Contract CRUD
// =========================================================================

#[derive(Serialize, Deserialize, Debug)]
pub struct CreateDebtContractInput {
    pub amount: f64,
    pub creditor: AgentPubKeyB64,
    pub debtor: AgentPubKeyB64,
    pub transaction_hash: ActionHash,
    pub is_trial: bool,
}

/// Create a debt contract when a transaction is accepted.
/// Called by the coordinator when a seller accepts a buyer's transaction.
///
/// Idempotency: if a contract already exists for the given `transaction_hash`
/// (detected by scanning the debtor's active contracts), the existing record
/// is returned immediately without creating a duplicate.  This handles the case
/// where `notify_buyer_of_accepted_transaction` is retried due to a network
/// timeout or the buyer's conductor restarting mid-call.
#[hdk_extern]
pub fn create_debt_contract(input: CreateDebtContractInput) -> ExternResult<Record> {
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    let debtor_agent: AgentPubKey = input.debtor.clone().into();

    // ── Idempotency guard ───────────────────────────────────────────────────
    // Scan the debtor's DebtorToContracts links to check whether a contract
    // for this exact transaction_hash was already created (e.g., due to a
    // network retry of notify_buyer_of_accepted_transaction).
    {
        let existing_links =
            get_links(LinkQuery::try_new(debtor_agent.clone(), LinkTypes::DebtorToContracts)?, GetStrategy::Local)?;
        for link in existing_links {
            let Some(original_hash) = link.target.clone().into_action_hash() else { continue };
            let Some(record) = get_latest_debt_contract_record(original_hash)? else { continue };
            if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
                if contract.transaction_hash == input.transaction_hash {
                    debug!(
                        "create_debt_contract: idempotency hit — contract for tx {:?} already exists, returning existing",
                        input.transaction_hash
                    );
                    return Ok(record);
                }
            }
        }
    }
    // ── End idempotency guard ───────────────────────────────────────────────

    let maturity = MIN_MATURITY;

    // Fetch the seller's (creditor's) support breakdown to populate co_signers.
    // Co-signers are the beneficiaries listed in the seller's breakdown — the nodes
    // whose debt will be drained during the seller's support cascade for this transaction.
    // Using the creditor's breakdown (rather than the debtor's) aligns with the whitepaper:
    // contagion is recorded against the beneficiaries the seller (supporter) chose to drain.
    let creditor_agent: AgentPubKey = input.creditor.clone().into();
    let co_signers = crate::support_cascade::get_support_breakdown_for_agent(creditor_agent.clone())?
        .map(|(bd, _record)| bd.addresses.into_iter().zip(bd.coefficients).collect());

    let contract = DebtContract {
        amount: input.amount,
        original_amount: input.amount,
        maturity,
        start_epoch: current_epoch,
        creditor: input.creditor.clone(),
        debtor: input.debtor.clone(),
        transaction_hash: input.transaction_hash,
        co_signers,
        status: ContractStatus::Active,
        is_trial: input.is_trial,
    };

    let contract_hash = create_entry(&EntryTypes::DebtContract(contract.clone()))?;

    // Link debtor -> contract
    create_link(debtor_agent.clone(), contract_hash.clone(), LinkTypes::DebtorToContracts, ())?;

    // Link creditor -> contract
    create_link(creditor_agent, contract_hash.clone(), LinkTypes::CreditorToContracts, ())?;

    // Create epoch-bucketed link for incremental queries (Workstream 3)
    let epoch_tag = EpochBucketTag { epoch: current_epoch };
    let epoch_tag_bytes =
        SerializedBytes::try_from(epoch_tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    create_link(
        debtor_agent.clone(),
        contract_hash.clone(),
        LinkTypes::AgentToContractsByEpoch,
        LinkTag(epoch_tag_bytes.bytes().clone()),
    )?;

    // Update the running debt balance for the debtor: +amount, +1 contract
    update_debt_balance(debtor_agent, input.amount, 1)?;

    let record = get(contract_hash, GetOptions::default())?.ok_or(wasm_error!(WasmErrorInner::Guest(
        coordinator_contract_error::CREATED_CONTRACT_NOT_FOUND.to_string()
    )))?;

    Ok(record)
}

/// Result of the open-trial gate check.
#[derive(Debug, PartialEq, Clone)]
pub enum TrialCheckResult {
    /// No block — trial may proceed.
    Allowed,
    /// An Active trial contract already exists for this (buyer, seller) pair.
    OpenTrialExists,
    /// The pair is permanently blocked due to a prior trial default.
    PermanentlyBlocked,
}

/// Check whether the buyer already has an Active trial DebtContract with the given seller,
/// or whether the (buyer, seller) pair is permanently blocked from trials.
///
/// This enforces the per-(buyer, seller) open-trial gate (Whitepaper §5.3, PATH 0):
/// - Only one trial slot is open at a time between a given buyer and seller.
/// - The slot is released when the DebtContract transitions to Transferred (successful repayment).
/// - Expiry or default does NOT release the slot — the buyer must repay to earn another trial.
/// - If the trial contract expired/defaulted, a permanent `DebtorToBlockedTrialSeller` link is
///   written and this pair is barred from all future trials (Change 1 — Permanent Trial Block).
///
/// Returns the appropriate `TrialCheckResult` variant.
///
/// Called from `create_transaction` on the buyer's cell; queries the buyer's own
/// DHT links (local, efficient).
pub fn check_open_trial_for_buyer(buyer: AgentPubKey, seller: AgentPubKey) -> ExternResult<TrialCheckResult> {
    let seller_key: AgentPubKeyB64 = seller.clone().into();

    // 1. Check permanent block first (cheap link query).
    let block_links =
        get_links(LinkQuery::try_new(buyer.clone(), LinkTypes::DebtorToBlockedTrialSeller)?, GetStrategy::default())?;
    for link in &block_links {
        if let Some(target_hash) = link.target.clone().into_any_dht_hash() {
            if let Some(target_agent) = target_hash.into_agent_pub_key() {
                if AgentPubKeyB64::from(target_agent) == seller_key {
                    return Ok(TrialCheckResult::PermanentlyBlocked);
                }
            }
        }
    }

    // 2. Check for an existing Active trial contract.
    // Follow update links to get the latest contract status (avoids false positives
    // from expired contracts that have been updated to Expired status).
    let links = get_links(LinkQuery::try_new(buyer, LinkTypes::DebtorToContracts)?, GetStrategy::default())?;

    for link in links {
        let Some(original_hash) = link.target.into_action_hash() else {
            continue;
        };
        let Some(record) = get_latest_debt_contract_record(original_hash)? else {
            continue;
        };
        if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
            // Use the `is_trial` flag rather than an amount threshold.
            // After partial debt transfer, a non-trial contract's residual amount can
            // drop below the trial threshold (TRIAL_FRACTION * BASE_CAPACITY), causing a
            // false positive that incorrectly blocks a legitimate new trial transaction.
            // The `is_trial` flag was introduced specifically to avoid this ambiguity.
            if contract.creditor == seller_key && contract.status == ContractStatus::Active && contract.is_trial {
                return Ok(TrialCheckResult::OpenTrialExists);
            }
        }
    }

    Ok(TrialCheckResult::Allowed)
}

/// Follow DebtContractUpdates links to get the latest version of a contract record.
/// Falls back to the original hash if no update links exist.
pub fn get_latest_debt_contract_record(original_hash: ActionHash) -> ExternResult<Option<Record>> {
    let update_links =
        get_links(LinkQuery::try_new(original_hash.clone(), LinkTypes::DebtContractUpdates)?, GetStrategy::default())?;
    let latest_hash = update_links
        .into_iter()
        .max_by(|a, b| a.timestamp.cmp(&b.timestamp))
        .and_then(|link| link.target.into_action_hash())
        .unwrap_or(original_hash);
    get(latest_hash, GetOptions::default())
}

/// Get all active debt contracts where the given agent is the debtor.
/// Returns a list of (original_action_hash, latest_record).
pub fn get_active_contracts_with_original_for_debtor(debtor: AgentPubKey) -> ExternResult<Vec<(ActionHash, Record)>> {
    let links = get_links(LinkQuery::try_new(debtor, LinkTypes::DebtorToContracts)?, GetStrategy::default())?;

    let mut results = Vec::new();
    for link in links {
        let Some(original_hash) = link.target.into_action_hash() else {
            continue;
        };
        let Some(record) = get_latest_debt_contract_record(original_hash.clone())? else {
            continue;
        };
        if record
            .entry()
            .to_app_option::<DebtContract>()
            .ok()
            .flatten()
            .is_some_and(|c| c.status == ContractStatus::Active)
        {
            results.push((original_hash, record));
        }
    }

    Ok(results)
}

#[hdk_extern]
pub fn get_active_contracts_for_debtor(debtor: AgentPubKey) -> ExternResult<Vec<Record>> {
    Ok(get_active_contracts_with_original_for_debtor(debtor)?
        .into_iter()
        .map(|(_, r)| r)
        .collect())
}

/// Get all active debt contracts where the given agent is the creditor.
#[hdk_extern]
pub fn get_active_contracts_for_creditor(creditor: AgentPubKey) -> ExternResult<Vec<Record>> {
    let links = get_links(LinkQuery::try_new(creditor, LinkTypes::CreditorToContracts)?, GetStrategy::default())?;

    let mut records = Vec::new();
    for link in links {
        let Some(original_hash) = link.target.into_action_hash() else {
            continue;
        };
        let Some(record) = get_latest_debt_contract_record(original_hash)? else {
            continue;
        };
        if record
            .entry()
            .to_app_option::<DebtContract>()
            .ok()
            .flatten()
            .is_some_and(|c| c.status == ContractStatus::Active)
        {
            records.push(record);
        }
    }

    Ok(records)
}

/// Get all non-archived debt contracts for an agent as debtor, resolved to their
/// latest version via DebtContractUpdates links.
///
/// Unlike `get_all_contracts_as_debtor` which returns original (stale) create
/// records, this follows the update chain so callers see the current status
/// (Active / Transferred / Expired) and the current residual amount.
///
/// Returns contracts in all non-archived statuses. Archived contracts are
/// on a separate AgentToArchivedContracts index and should be fetched via
/// `get_archived_contracts` for cold-storage history.
#[hdk_extern]
pub fn get_all_contracts_as_debtor_resolved(debtor: AgentPubKey) -> ExternResult<Vec<Record>> {
    let links = get_links(LinkQuery::try_new(debtor, LinkTypes::DebtorToContracts)?, GetStrategy::default())?;

    let mut records = Vec::new();
    for link in links {
        let Some(original_hash) = link.target.into_action_hash() else {
            continue;
        };
        let Some(record) = get_latest_debt_contract_record(original_hash)? else {
            continue;
        };
        records.push(record);
    }

    Ok(records)
}

/// Get all contracts for an agent as debtor, including archived ones.
/// Useful for full history queries.
///
/// NOTE: Each contract is fetched via `get_latest_debt_contract_record` to ensure
/// the returned record reflects the current status (Active, Transferred, Expired, etc.)
/// rather than the original create-time record, which would show stale state.
#[hdk_extern]
pub fn get_all_contracts_as_debtor(debtor: AgentPubKey) -> ExternResult<Vec<Record>> {
    let mut all_records = Vec::new();

    // Get active contracts — follow update links to get latest status
    let active_links =
        get_links(LinkQuery::try_new(debtor.clone(), LinkTypes::DebtorToContracts)?, GetStrategy::default())?;

    for link in active_links {
        let Some(original_hash) = link.target.into_action_hash() else { continue };
        let Some(record) = get_latest_debt_contract_record(original_hash)? else { continue };
        all_records.push(record);
    }

    // Get archived contracts — follow update links to get latest status
    let archived_links =
        get_links(LinkQuery::try_new(debtor, LinkTypes::AgentToArchivedContracts)?, GetStrategy::default())?;

    for link in archived_links {
        let Some(original_hash) = link.target.into_action_hash() else { continue };
        let Some(record) = get_latest_debt_contract_record(original_hash)? else { continue };
        all_records.push(record);
    }

    Ok(all_records)
}

/// Get contracts created within a specific epoch range [from_epoch, to_epoch] inclusive.
///
/// Uses the epoch-bucketed AgentToContractsByEpoch links for efficient
/// incremental queries. Only contracts created in the specified epochs are returned.
///
/// Returns O(contracts_in_range) instead of O(total_contracts).
///
/// `resolve_latest`: when `true`, each record is resolved to its **latest version**
/// via DebtContractUpdates links, so callers see current residual amounts (after
/// partial transfers) rather than stale create-time amounts. Use this for risk-score
/// computations (D_out in lambda_b).
///
/// When `false`, the original create-action record is returned. Use this for
/// reputation-claim building where `ActionType::Create` counting must match the
/// integrity validator's chain-scan counter exactly.
pub fn get_contracts_in_epoch_range(
    debtor: AgentPubKey,
    from_epoch: u64,
    to_epoch: u64,
    resolve_latest: bool,
) -> ExternResult<Vec<Record>> {
    let links = get_links(LinkQuery::try_new(debtor, LinkTypes::AgentToContractsByEpoch)?, GetStrategy::default())?;

    // Filter links by epoch tag, collecting original create-hashes.
    let mut matching_hashes: Vec<ActionHash> = Vec::new();
    for link in links {
        let tag_bytes = SerializedBytes::from(UnsafeBytes::from(link.tag.into_inner()));
        if let Ok(epoch_tag) = EpochBucketTag::try_from(tag_bytes) {
            if epoch_tag.epoch >= from_epoch && epoch_tag.epoch <= to_epoch {
                if let Some(hash) = link.target.into_action_hash() {
                    matching_hashes.push(hash);
                }
            }
        }
    }

    if matching_hashes.is_empty() {
        return Ok(Vec::new());
    }

    if resolve_latest {
        // Resolve each original hash to its latest version so that callers see
        // current residual amounts (after partial transfers) rather than the
        // stale create-time amounts.
        let mut records = Vec::with_capacity(matching_hashes.len());
        for original_hash in matching_hashes {
            if let Some(record) = get_latest_debt_contract_record(original_hash)? {
                records.push(record);
            }
        }
        Ok(records)
    } else {
        // Return original create-action records so ActionType::Create counting
        // in reputation-claim building matches the integrity validator's chain-scan.
        let get_inputs: Vec<GetInput> = matching_hashes
            .into_iter()
            .map(|hash| GetInput::new(hash.into(), GetOptions::default()))
            .collect();
        let records: Vec<Record> = HDK.with(|hdk| hdk.borrow().get(get_inputs))?.into_iter().flatten().collect();
        Ok(records)
    }
}
