use hdk::prelude::*;
use transaction_integrity::debt_contract::{ContractStatus, DebtContract};
use transaction_integrity::types::constants::*;
use transaction_integrity::types::timestamp_to_epoch;
use transaction_integrity::*;

/// Result of contract archival operation.
#[derive(Serialize, Deserialize, Debug)]
pub struct ArchivalResult {
    /// Number of contracts archived.
    pub archived_count: u32,
    /// Total debt amount in archived contracts.
    pub archived_amount: f64,
}

/// Archive old contracts that are no longer active (Transferred or Expired).
/// Contracts are eligible for archival after ARCHIVE_AFTER_EPOCHS epochs
/// since their start_epoch + maturity.
///
/// Archival moves contracts from DebtorToContracts/CreditorToContracts links
/// to AgentToArchivedContracts links, reducing active query overhead.
///
/// This is called automatically during transaction creation and claim publishing.
#[hdk_extern]
pub fn archive_old_contracts(_: ()) -> ExternResult<ArchivalResult> {
    let agent = agent_info()?.agent_initial_pubkey;
    let now = sys_time()?;
    let current_epoch = timestamp_to_epoch(now);

    // Get all contracts where agent is debtor
    let debtor_links =
        get_links(LinkQuery::try_new(agent.clone(), LinkTypes::DebtorToContracts)?, GetStrategy::default())?;

    let get_inputs: Vec<GetInput> = debtor_links
        .iter()
        .filter_map(|link| {
            link.target
                .clone()
                .into_action_hash()
                .map(|hash| GetInput::new(hash.into(), GetOptions::default()))
        })
        .collect();

    let records: Vec<Option<Record>> = HDK.with(|hdk| hdk.borrow().get(get_inputs))?;

    let mut archived_count = 0u32;
    let mut archived_amount = 0.0f64;

    for (link, maybe_record) in debtor_links.iter().zip(records.iter()) {
        let record = match maybe_record {
            Some(r) => r,
            None => continue,
        };

        // Follow update links to get the latest version of the contract.
        // Without this, we might read the original (Active) version even though
        // the contract has been updated to Transferred or Expired.
        let original_action_hash = record.action_address().clone();
        let latest_record = match super::get_latest_debt_contract_record(original_action_hash.clone())? {
            Some(r) => r,
            None => continue,
        };

        let contract: DebtContract = match latest_record.entry().to_app_option::<DebtContract>().ok().flatten() {
            Some(c) => c,
            None => continue,
        };

        // Only archive Transferred or Expired contracts
        if contract.status != ContractStatus::Transferred && contract.status != ContractStatus::Expired {
            continue;
        }

        // Check if contract is old enough to archive
        let contract_end_epoch = contract.start_epoch + contract.maturity;
        if current_epoch < contract_end_epoch + ARCHIVE_AFTER_EPOCHS {
            continue;
        }

        // Build the Archived version of the contract.
        let archived_contract = DebtContract { status: ContractStatus::Archived, ..contract.clone() };

        // Archive: update contract status to Archived.
        // Use the LATEST record's action hash (not the original create hash) as the
        // previous-entry pointer so the update chain stays linear and valid.
        let latest_action_hash = latest_record.action_address().clone();
        update_entry(latest_action_hash, &archived_contract)?;

        // Now that the contract is Archived, delete the old DebtorToContracts link.
        // Integrity validation permits this deletion only for Archived contracts,
        // so it must happen AFTER the update above is committed.
        delete_link(link.create_link_hash.clone(), GetOptions::default())?;

        // Create AgentToArchivedContracts link for the debtor
        let debtor_agent: AgentPubKey = contract.debtor.clone().into();
        create_link(debtor_agent, original_action_hash.clone(), LinkTypes::AgentToArchivedContracts, ())?;

        archived_count += 1;
        archived_amount += contract.amount;
    }

    // Also archive contracts where agent is creditor
    let creditor_links =
        get_links(LinkQuery::try_new(agent.clone(), LinkTypes::CreditorToContracts)?, GetStrategy::default())?;

    let get_inputs: Vec<GetInput> = creditor_links
        .iter()
        .filter_map(|link| {
            link.target
                .clone()
                .into_action_hash()
                .map(|hash| GetInput::new(hash.into(), GetOptions::default()))
        })
        .collect();

    let creditor_records: Vec<Option<Record>> = HDK.with(|hdk| hdk.borrow().get(get_inputs))?;

    for (link, maybe_record) in creditor_links.iter().zip(creditor_records.iter()) {
        let record = match maybe_record {
            Some(r) => r,
            None => continue,
        };

        // Follow update links to get the latest version of the contract.
        let original_action_hash_c = record.action_address().clone();
        let latest_record = match super::get_latest_debt_contract_record(original_action_hash_c)? {
            Some(r) => r,
            None => continue,
        };

        let contract: DebtContract = match latest_record.entry().to_app_option::<DebtContract>().ok().flatten() {
            Some(c) => c,
            None => continue,
        };

        // Only archive Transferred or Expired contracts (or already Archived, meaning debtor
        // already archived it and we just need to clean up the creditor-side link).
        if contract.status != ContractStatus::Transferred
            && contract.status != ContractStatus::Expired
            && contract.status != ContractStatus::Archived
        {
            continue;
        }

        // Check if contract is old enough to archive
        let contract_end_epoch = contract.start_epoch + contract.maturity;
        if current_epoch < contract_end_epoch + ARCHIVE_AFTER_EPOCHS {
            continue;
        }

        // If the contract is already Archived (debtor did it first), just clean up
        // the creditor-side link and create the archived link.
        // If not yet Archived (debtor offline / hasn't run archival), the creditor
        // may perform the Transferred→Archived or Expired→Archived transition directly —
        // integrity validation now allows creditor-initiated archival for these two
        // transitions specifically, unblocking the cross-agent deadlock.
        if contract.status != ContractStatus::Archived {
            let archived_contract = DebtContract { status: ContractStatus::Archived, ..contract.clone() };
            let latest_action_hash = latest_record.action_address().clone();
            // Creditor-initiated archival: update the contract to Archived.
            // The integrity `validate_update_debt_contract` allows this for
            // Transferred→Archived and Expired→Archived when author == creditor.
            update_entry(latest_action_hash, &archived_contract)?;
        }

        // Delete the old CreditorToContracts link (contract is now Archived)
        delete_link(link.create_link_hash.clone(), GetOptions::default())?;

        // Create AgentToArchivedContracts link for the creditor
        let creditor_agent: AgentPubKey = contract.creditor.clone().into();
        create_link(creditor_agent, record.action_address().clone(), LinkTypes::AgentToArchivedContracts, ())?;
    }

    Ok(ArchivalResult { archived_count, archived_amount })
}

/// Get archived contracts for an agent (for historical lookup).
#[hdk_extern]
pub fn get_archived_contracts(agent: AgentPubKey) -> ExternResult<Vec<Record>> {
    let links = get_links(LinkQuery::try_new(agent, LinkTypes::AgentToArchivedContracts)?, GetStrategy::default())?;

    let get_inputs: Vec<GetInput> = links
        .into_iter()
        .filter_map(|link| {
            link.target
                .into_action_hash()
                .map(|hash| GetInput::new(hash.into(), GetOptions::default()))
        })
        .collect();

    let records: Vec<Record> = HDK.with(|hdk| hdk.borrow().get(get_inputs))?.into_iter().flatten().collect();

    Ok(records)
}
