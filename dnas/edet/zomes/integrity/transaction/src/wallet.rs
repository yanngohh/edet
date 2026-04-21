use hdi::prelude::*;

use crate::types::{owner_to_wallet_validation_error, type_resolution_error, wallet_validation_error};

#[derive(Clone, PartialEq)]
#[hdk_entry_helper]
pub struct Wallet {
    pub owner: AgentPubKeyB64,
    /// Risk score below which transactions are auto-accepted
    pub auto_accept_threshold: f64,
    /// Risk score above which transactions are auto-rejected
    pub auto_reject_threshold: f64,
    /// Cumulative staking capacity permanently consumed by vouchee defaults.
    /// Once slashed, this amount can never be re-locked for new vouches, matching
    /// the simulation's `staked_capacity[sponsor] -= X * default_amount` invariant.
    /// Initialized to 0.0; incremented by `slash_vouch_for_entrant`.
    pub total_slashed_as_sponsor: f64,
    /// Number of trial transactions accepted in the current epoch.
    /// Bound by TRIAL_VELOCITY_LIMIT_PER_EPOCH.
    pub trial_tx_count: u32,
    /// The epoch in which trial_tx_count was last updated.
    pub last_trial_epoch: u64,
}

pub fn validate_create_wallet(action: EntryCreationAction, wallet: Wallet) -> ExternResult<ValidateCallbackResult> {
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
        Ok(ValidateCallbackResult::Invalid(wallet_validation_error::OWNER_ASSOCIATION_EXISTS.into()))
    } else if action.action_type() == ActionType::Create && wallet != Wallet::new(&wallet.owner) {
        Ok(ValidateCallbackResult::Invalid(wallet_validation_error::INVALID_INIT_STATE.into()))
    } else if action.author() != &wallet.owner.into() {
        Ok(ValidateCallbackResult::Invalid(wallet_validation_error::ACTION_AUTHOR_NOT_OWNER.into()))
    } else if wallet.auto_accept_threshold.is_nan()
        || wallet.auto_accept_threshold.is_infinite()
        || wallet.auto_reject_threshold.is_nan()
        || wallet.auto_reject_threshold.is_infinite()
        || wallet.auto_accept_threshold > wallet.auto_reject_threshold
        || wallet.auto_accept_threshold < 0.0
        || wallet.auto_reject_threshold > 1.0
    {
        Ok(ValidateCallbackResult::Invalid(wallet_validation_error::INVALID_THRESHOLDS.to_string()))
    } else {
        Ok(ValidateCallbackResult::Valid)
    }
}

pub fn validate_update_wallet(
    action: Update,
    wallet: Wallet,
    _original_action: EntryCreationAction,
    original_wallet: Wallet,
) -> ExternResult<ValidateCallbackResult> {
    if action.author != original_wallet.owner.clone().into() {
        Ok(ValidateCallbackResult::Invalid(wallet_validation_error::ACTION_AUTHOR_NOT_OWNER.into()))
    } else if wallet.owner != original_wallet.owner {
        Ok(ValidateCallbackResult::Invalid(wallet_validation_error::OWNER_IMMUTABLE.into()))
    } else {
        Ok(ValidateCallbackResult::Valid)
    }
}

pub fn validate_delete_wallet(
    _action: Delete,
    _original_action: EntryCreationAction,
    _original_wallet: Wallet,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(wallet_validation_error::WALLETS_NOT_DELETABLE.to_string()))
}

pub fn validate_create_link_owner_to_wallet(
    action: CreateLink,
    _base_address: AnyLinkableHash,
    target_address: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    let wallet_action_hash = target_address
        .into_action_hash()
        .ok_or(wasm_error!(WasmErrorInner::Guest(type_resolution_error::LINK_NO_ACTION_HASH.to_string())))?;
    let wallet_action = must_get_action(wallet_action_hash.to_owned())?;
    let record = must_get_valid_record(wallet_action_hash)?;
    let wallet: crate::Wallet = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest(type_resolution_error::LINK_NO_ENTRY.to_string())))?;
    if wallet_action.action().action_type() != ActionType::Create {
        Ok(ValidateCallbackResult::Invalid(owner_to_wallet_validation_error::TARGET_NOT_ON_CREATE_WALLET_ACTION.into()))
    } else if action.author != wallet.owner.into() {
        Ok(ValidateCallbackResult::Invalid(owner_to_wallet_validation_error::AUTHOR_NOT_WALLET_OWNER.into()))
    } else {
        Ok(ValidateCallbackResult::Valid)
    }
}

pub fn validate_delete_link_owner_to_wallet(
    _action: DeleteLink,
    _original_action: CreateLink,
    _base: AnyLinkableHash,
    _target: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(String::from("OwnerToWallet links cannot be deleted")))
}

pub fn validate_create_link_wallet_updates(
    _action: CreateLink,
    base_address: AnyLinkableHash,
    target_address: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    let action_hash = base_address
        .into_action_hash()
        .ok_or(wasm_error!(WasmErrorInner::Guest(type_resolution_error::LINK_NO_ACTION_HASH.to_string())))?;
    let record = must_get_valid_record(action_hash)?;
    let _wallet: crate::Wallet = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest(type_resolution_error::LINK_NO_ENTRY.to_string())))?;
    let action_hash = target_address
        .into_action_hash()
        .ok_or(wasm_error!(WasmErrorInner::Guest(type_resolution_error::LINK_NO_ACTION_HASH.to_string())))?;
    let record = must_get_valid_record(action_hash)?;
    let _wallet: crate::Wallet = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest(type_resolution_error::LINK_NO_ENTRY.to_string())))?;
    Ok(ValidateCallbackResult::Valid)
}

pub fn validate_delete_link_wallet_updates(
    _action: DeleteLink,
    _original_action: CreateLink,
    _base: AnyLinkableHash,
    _target: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(String::from("WalletUpdates links cannot be deleted")))
}
