use hdk::prelude::*;
use transaction_integrity::debt_contract::{ContractStatus, DebtContract};
use transaction_integrity::types::constants::DUST_THRESHOLD;
use transaction_integrity::*;

use super::debt_balance::update_debt_balance;

// =========================================================================
//  Debt Transfer (Whitepaper Definition 2)
// =========================================================================

/// Result of a debt transfer operation during a sale.
#[derive(Serialize, Deserialize, Debug)]
pub struct DebtTransferResult {
    /// Total amount of debt successfully transferred.
    pub transferred: f64,
    /// Per-creditor breakdown: (creditor, amount_transferred).
    /// These should be used to update S counters.
    pub creditor_transfers: Vec<(AgentPubKeyB64, f64)>,
}

/// Transfer debt when a seller sells goods/services.
/// Iterates through the seller's active contracts (as debtor), reducing
/// their amounts and recording which creditors had debt transferred.
///
/// Returns the amount actually transferred and per-creditor breakdown
/// for S counter updates.
#[hdk_extern]
pub fn transfer_debt(input: (AgentPubKey, f64)) -> ExternResult<DebtTransferResult> {
    let (seller, amount) = input;
    let mut contracts = super::get_active_contracts_with_original_for_debtor(seller.clone())?;
    // Drain contracts in ascending-maturity order (earliest expiry first).
    // Without this, later-maturity contracts could be drained while near-expiry
    // contracts wait and eventually expire unpaid — increasing failure count and
    // penalising the debtor more than necessary.
    // start_epoch + maturity gives the absolute expiry epoch; we sort ascending so
    // the soonest-to-expire contracts are cleared first.
    contracts.sort_by(|(_, rec_a), (_, rec_b)| {
        let epoch_a = rec_a
            .entry()
            .to_app_option::<DebtContract>()
            .ok()
            .flatten()
            .map(|c| c.start_epoch.saturating_add(c.maturity))
            .unwrap_or(u64::MAX);
        let epoch_b = rec_b
            .entry()
            .to_app_option::<DebtContract>()
            .ok()
            .flatten()
            .map(|c| c.start_epoch.saturating_add(c.maturity))
            .unwrap_or(u64::MAX);
        epoch_a.cmp(&epoch_b)
    });
    let mut remaining = amount;
    let mut creditor_transfers: Vec<(AgentPubKeyB64, f64)> = Vec::new();
    let mut total_transferred_amount = 0.0f64;
    let mut contracts_fully_resolved = 0i64;

    for (original_action_hash, record) in contracts {
        if remaining <= DUST_THRESHOLD {
            break;
        }

        let contract: DebtContract = match record.entry().to_app_option::<DebtContract>().ok().flatten() {
            Some(c) if c.status == ContractStatus::Active => c,
            _ => continue,
        };

        let transfer = contract.amount.min(remaining);
        if transfer <= DUST_THRESHOLD {
            continue;
        }

        // Update the contract: reduce amount
        let new_amount = contract.amount - transfer;
        let new_status =
            if new_amount <= DUST_THRESHOLD { ContractStatus::Transferred } else { ContractStatus::Active };
        let is_fully_transferred = new_status == ContractStatus::Transferred;

        let updated_contract = DebtContract {
            amount: if new_amount <= DUST_THRESHOLD { 0.0 } else { new_amount },
            status: new_status,
            ..contract.clone()
        };

        let latest_action_hash = record.action_address().clone();
        let updated_hash = update_entry(latest_action_hash.clone(), &updated_contract)?;
        create_link(original_action_hash, updated_hash.clone(), LinkTypes::DebtContractUpdates, ())?;

        // Link update to current epoch bucket for incremental claim scan (Workstream 3 Fix)
        let now = sys_time()?;
        let current_epoch = transaction_integrity::types::timestamp_to_epoch(now);
        let epoch_tag = transaction_integrity::types::EpochBucketTag { epoch: current_epoch };
        let epoch_tag_bytes =
            SerializedBytes::try_from(epoch_tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
        let debtor_agent: AgentPubKey = contract.debtor.clone().into();
        create_link(
            debtor_agent,
            updated_hash,
            LinkTypes::AgentToContractsByEpoch,
            LinkTag(epoch_tag_bytes.bytes().clone()),
        )?;

        total_transferred_amount += transfer;
        if is_fully_transferred {
            contracts_fully_resolved += 1;
        }

        // Track per-creditor transfer for S counter updates
        let creditor = contract.creditor.clone();
        if let Some(existing) = creditor_transfers.iter_mut().find(|(c, _)| *c == creditor) {
            existing.1 += transfer;
        } else {
            creditor_transfers.push((creditor, transfer));
        }

        remaining -= transfer;
    }

    // Update running debt balance: decrease by total transferred, adjust contract count
    if total_transferred_amount > DUST_THRESHOLD {
        update_debt_balance(seller, -total_transferred_amount, -contracts_fully_resolved)?;
        // Phase 3: Add acquaintances upon successful debt transfer (repayment).
        // This grants reputation-based capacity only after observable economic evidence.
        crate::trust::update_acquaintances_from_evidence(&creditor_transfers, &[])?;
    }

    Ok(DebtTransferResult { transferred: amount - remaining, creditor_transfers })
}
