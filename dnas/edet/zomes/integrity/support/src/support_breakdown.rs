use hdi::prelude::*;

use crate::types::{owner_to_support_breakdown_validation_error, support_breakdown_validation_error};

#[derive(Clone, PartialEq)]
#[hdk_entry_helper]
pub struct SupportBreakdown {
    pub owner: AgentPubKeyB64,
    pub addresses: Vec<AgentPubKeyB64>,
    pub coefficients: Vec<f64>,
}

pub fn validate_create_support_breakdown(
    action: EntryCreationAction,
    support_breakdown: SupportBreakdown,
) -> ExternResult<ValidateCallbackResult> {
    if action.action_type() == ActionType::Create
        && must_get_agent_activity(action.author().clone(), ChainFilter::new(action.prev_action().clone()))?
            .iter()
            .any(|activity| {
                activity.action.action().action_type() == action.action_type()
                    && activity
                        .action
                        .action()
                        .entry_type()
                        .is_some_and(|entry_type| entry_type == action.entry_type())
            })
    {
        Ok(ValidateCallbackResult::Invalid(support_breakdown_validation_error::OWNER_ASSOCIATION_EXISTS.to_string()))
    } else {
        let author: AgentPubKeyB64 = action.author().to_owned().into();
        let sum: f64 = support_breakdown.coefficients.iter().sum();
        let mut dedup_addresses = support_breakdown.addresses.clone();
        dedup_addresses.sort();
        dedup_addresses.dedup();
        if !(0.99999999f64..=1.00000001f64).contains(&sum) {
            Ok(ValidateCallbackResult::Invalid(
                support_breakdown_validation_error::COEFFICIENTS_NOT_SUMMING_UP.to_string(),
            ))
        } else if support_breakdown.coefficients.iter().any(|c| *c < 0f64 || *c > 1f64) {
            Ok(ValidateCallbackResult::Invalid(
                support_breakdown_validation_error::COEFFICIENT_NOT_IN_RANGE.to_string(),
            ))
        } else if support_breakdown.owner != author {
            Ok(ValidateCallbackResult::Invalid(support_breakdown_validation_error::ACTION_AUTHOR_NOT_OWNER.to_string()))
        } else if !support_breakdown.addresses.contains(&author) {
            Ok(ValidateCallbackResult::Invalid(
                support_breakdown_validation_error::ADDRESSES_MUST_CONTAIN_OWNER.to_string(),
            ))
        } else if dedup_addresses.len() != support_breakdown.addresses.len() {
            Ok(ValidateCallbackResult::Invalid(
                support_breakdown_validation_error::ADDRESSES_CONTAIN_DUPLICATE.to_string(),
            ))
        } else {
            Ok(ValidateCallbackResult::Valid)
        }
    }
}

pub fn validate_update_support_breakdown(
    action: Update,
    support_breakdown: SupportBreakdown,
    _original_action: EntryCreationAction,
    original_support_breakdown: SupportBreakdown,
) -> ExternResult<ValidateCallbackResult> {
    // The update author must be the owner of the support breakdown.
    // Without this check, any agent who knows the original entry hash could submit
    // an update to someone else's breakdown.
    let author: AgentPubKeyB64 = action.author.to_owned().into();
    if support_breakdown.owner != author {
        return Ok(ValidateCallbackResult::Invalid(
            support_breakdown_validation_error::ACTION_AUTHOR_NOT_OWNER.to_string(),
        ));
    }

    if original_support_breakdown
        .addresses
        .iter()
        .any(|address| !support_breakdown.addresses.contains(address))
    {
        Ok(ValidateCallbackResult::Invalid(support_breakdown_validation_error::ADDRESS_REMOVED_IN_UPDATE.to_string()))
    } else {
        Ok(ValidateCallbackResult::Valid)
    }
}

pub fn validate_delete_support_breakdown(
    _action: Delete,
    _original_action: EntryCreationAction,
    _original_support_breakdown: SupportBreakdown,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(
        support_breakdown_validation_error::SUPPORT_BREAKDOWNS_NOT_DELETABLE.to_string(),
    ))
}

pub fn validate_create_link_owner_to_support_breakdowns(
    action: CreateLink,
    _base_address: AnyLinkableHash,
    target_address: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    let support_breakdown_action_hash = target_address
        .into_action_hash()
        .ok_or(wasm_error!(WasmErrorInner::Guest("No action hash associated with link".to_string())))?;
    let support_breakdown_action = must_get_action(support_breakdown_action_hash.to_owned())?;
    let record = must_get_valid_record(support_breakdown_action_hash)?;
    let support_breakdown: crate::SupportBreakdown = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest("Linked action must reference an entry".to_string())))?;
    if support_breakdown_action.action().action_type() != ActionType::Create {
        Ok(ValidateCallbackResult::Invalid(
            owner_to_support_breakdown_validation_error::TARGET_NOT_ON_CREATE_WALLET_ACTION.to_string(),
        ))
    } else if action.author != support_breakdown.owner.into() {
        Ok(ValidateCallbackResult::Invalid(
            owner_to_support_breakdown_validation_error::AUTHOR_NOT_WALLET_OWNER.to_string(),
        ))
    } else {
        Ok(ValidateCallbackResult::Valid)
    }
}

pub fn validate_delete_link_owner_to_support_breakdowns(
    _action: DeleteLink,
    _original_action: CreateLink,
    _base: AnyLinkableHash,
    _target: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(
        support_breakdown_validation_error::OWNER_TO_SUPPORT_BREAKDOWN_LINK_NOT_DELETABLE.to_string(),
    ))
}

pub fn validate_create_link_support_breakdown_updates(
    action: CreateLink,
    base_address: AnyLinkableHash,
    target_address: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    // Check the entry type for the given action hash (base = original breakdown)
    let base_action_hash = base_address.into_action_hash().ok_or(wasm_error!(WasmErrorInner::Guest(
        support_breakdown_validation_error::SUPPORT_BREAKDOWN_UPDATES_LINK_NOT_DELETABLE.to_string()
    )))?;
    let base_record = must_get_valid_record(base_action_hash)?;
    let base_breakdown: crate::SupportBreakdown = base_record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest(
            support_breakdown_validation_error::SUPPORT_BREAKDOWNS_NOT_DELETABLE.to_string()
        )))?;

    // Check the entry type for the given action hash (target = updated breakdown)
    let target_action_hash = target_address.into_action_hash().ok_or(wasm_error!(WasmErrorInner::Guest(
        support_breakdown_validation_error::SUPPORT_BREAKDOWN_UPDATES_LINK_NOT_DELETABLE.to_string()
    )))?;
    let target_record = must_get_valid_record(target_action_hash)?;
    let target_breakdown: crate::SupportBreakdown = target_record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest(
            support_breakdown_validation_error::SUPPORT_BREAKDOWNS_NOT_DELETABLE.to_string()
        )))?;

    // Validation rules:
    // 1. Link author must be the owner of both breakdowns
    let author_b64: AgentPubKeyB64 = action.author.clone().into();
    if base_breakdown.owner != author_b64 {
        return Ok(ValidateCallbackResult::Invalid(
            support_breakdown_validation_error::UPDATE_AUTHOR_NOT_OWNER.to_string(),
        ));
    }
    if target_breakdown.owner != author_b64 {
        return Ok(ValidateCallbackResult::Invalid(
            support_breakdown_validation_error::TARGET_OWNER_MISMATCH.to_string(),
        ));
    }

    // 2. Both breakdowns must have the same owner (already checked above implicitly)

    Ok(ValidateCallbackResult::Valid)
}

pub fn validate_delete_link_support_breakdown_updates(
    _action: DeleteLink,
    _original_action: CreateLink,
    _base: AnyLinkableHash,
    _target: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(
        support_breakdown_validation_error::SUPPORT_BREAKDOWN_UPDATES_LINK_NOT_DELETABLE.to_string(),
    ))
}

pub fn validate_delete_link_owner_to_support_breakdown(
    _action: DeleteLink,
    _original_action: CreateLink,
    _base: AnyLinkableHash,
    _target: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(
        support_breakdown_validation_error::OWNER_TO_SUPPORT_BREAKDOWN_LINK_NOT_DELETABLE.to_string(),
    ))
}
pub fn validate_create_link_address_to_support_breakdowns(
    action: CreateLink,
    base_address: AnyLinkableHash,
    target_address: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    // Check the entry type for the given action hash
    let action_hash = target_address
        .into_action_hash()
        .ok_or(wasm_error!(WasmErrorInner::Guest(String::from("No action hash associated with link"))))?;
    let record = must_get_valid_record(action_hash)?;
    let support_breakdown: crate::SupportBreakdown = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest(String::from("Linked action must reference an entry"))))?;

    // Validation rules:
    // 1. Link author must be the owner of the support breakdown
    let author_b64: AgentPubKeyB64 = action.author.clone().into();
    if support_breakdown.owner != author_b64 {
        return Ok(ValidateCallbackResult::Invalid(
            support_breakdown_validation_error::UPDATE_AUTHOR_NOT_OWNER.to_string(),
        ));
    }

    // 2. The base address (beneficiary) must be in the breakdown's addresses list
    let base_agent = base_address.into_agent_pub_key().ok_or(wasm_error!(WasmErrorInner::Guest(
        support_breakdown_validation_error::BASE_ADDRESS_NOT_BENEFICIARY.to_string()
    )))?;
    let base_b64: AgentPubKeyB64 = base_agent.into();
    if !support_breakdown.addresses.contains(&base_b64) {
        return Ok(ValidateCallbackResult::Invalid(
            support_breakdown_validation_error::BASE_ADDRESS_NOT_BENEFICIARY.to_string(),
        ));
    }

    Ok(ValidateCallbackResult::Valid)
}
pub fn validate_delete_link_address_to_support_breakdowns(
    _action: DeleteLink,
    _original_action: CreateLink,
    _base: AnyLinkableHash,
    _target: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(
        support_breakdown_validation_error::ADDRESS_TO_SUPPORT_BREAKDOWNS_LINK_NOT_DELETABLE.to_string(),
    ))
}
