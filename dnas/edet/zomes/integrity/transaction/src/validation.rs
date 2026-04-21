use crate::checkpoint::*;
use crate::debt_contract::*;
use crate::reputation_claim::*;
use crate::transaction::*;
use crate::types;
use crate::types::link_validation_error;
use crate::vouch::*;
use crate::wallet::*;
use crate::{EntryTypes, LinkTypes};
use hdi::prelude::*;

/// Core validation dispatch logic, called by the `#[hdk_extern] validate` in lib.rs.
pub fn validate(op: Op) -> ExternResult<ValidateCallbackResult> {
    match op.flattened::<EntryTypes, LinkTypes>()? {
        FlatOp::StoreEntry(store_entry) => match store_entry {
            OpEntry::CreateEntry { app_entry, action } => match app_entry {
                EntryTypes::Wallet(wallet) => validate_create_wallet(EntryCreationAction::Create(action), wallet),
                EntryTypes::Transaction(transaction) => {
                    validate_create_transaction(EntryCreationAction::Create(action), transaction)
                }
                EntryTypes::DebtContract(contract) => {
                    validate_create_debt_contract(EntryCreationAction::Create(action), contract)
                }
                EntryTypes::ReputationClaim(claim) => {
                    validate_create_reputation_claim(EntryCreationAction::Create(action), claim)
                }
                EntryTypes::ChainCheckpoint(checkpoint) => {
                    validate_create_checkpoint(EntryCreationAction::Create(action), checkpoint)
                }
                EntryTypes::Vouch(vouch) => validate_create_vouch(EntryCreationAction::Create(action), vouch),
            },
            OpEntry::UpdateEntry { app_entry, action, .. } => match app_entry {
                EntryTypes::Wallet(wallet) => validate_create_wallet(EntryCreationAction::Update(action), wallet),
                EntryTypes::Transaction(transaction) => {
                    validate_create_transaction(EntryCreationAction::Update(action), transaction)
                }
                EntryTypes::DebtContract(contract) => {
                    // Skip validate_create_debt_contract for updates: the create validator
                    // rejects non-Active status, blocking Expired/Transferred/Archived transitions.
                    let original_record = must_get_valid_record(action.original_action_address.clone())?;
                    let original_action_cloned = original_record.action().clone();
                    let original_create_action = match EntryCreationAction::try_from(original_action_cloned) {
                        Ok(a) => a,
                        Err(e) => {
                            return Ok(ValidateCallbackResult::Invalid(format!(
                                "Expected EntryCreationAction for contract update: {e:?}"
                            )));
                        }
                    };
                    let original_contract: Option<DebtContract> =
                        original_record.entry().to_app_option().map_err(|e| wasm_error!(e))?;
                    let original_contract = match original_contract {
                        Some(c) => c,
                        None => {
                            return Ok(ValidateCallbackResult::Invalid(
                                link_validation_error::CONTRACT_UPDATE_ORIGINAL_NOT_DEBT_CONTRACT.to_string(),
                            ));
                        }
                    };
                    validate_update_debt_contract(action, contract, original_create_action, original_contract)
                }
                EntryTypes::ReputationClaim(claim) => {
                    validate_create_reputation_claim(EntryCreationAction::Update(action), claim)
                }
                EntryTypes::ChainCheckpoint(checkpoint) => {
                    validate_create_checkpoint(EntryCreationAction::Update(action), checkpoint)
                }
                EntryTypes::Vouch(vouch) => {
                    // For vouch updates in StoreEntry, delegate to validate_update_vouch
                    // (which allows sponsor OR debtor for slash updates) instead of
                    // validate_create_vouch (which only allows the sponsor).
                    let original_record = must_get_valid_record(action.original_action_address.clone())?;
                    let original_action = original_record.action().clone();
                    let original_create_action = match EntryCreationAction::try_from(original_action) {
                        Ok(a) => a,
                        Err(e) => {
                            return Ok(ValidateCallbackResult::Invalid(format!(
                                "Expected EntryCreationAction for vouch update: {e:?}"
                            )));
                        }
                    };
                    let original_vouch: Option<Vouch> =
                        original_record.entry().to_app_option().map_err(|e| wasm_error!(e))?;
                    let original_vouch = match original_vouch {
                        Some(v) => v,
                        None => {
                            return Ok(ValidateCallbackResult::Invalid(
                                link_validation_error::VOUCH_UPDATE_ORIGINAL_NOT_VOUCH.to_string(),
                            ));
                        }
                    };
                    validate_update_vouch(action, vouch, original_create_action, original_vouch)
                }
            },
            _ => Ok(ValidateCallbackResult::Valid),
        },
        FlatOp::RegisterUpdate(update_entry) => match update_entry {
            OpUpdate::Entry { app_entry, action } => {
                let original_action = must_get_action(action.clone().original_action_address)?.action().to_owned();
                let original_create_action = match EntryCreationAction::try_from(original_action) {
                    Ok(action) => action,
                    Err(e) => {
                        return Ok(ValidateCallbackResult::Invalid(format!(
                            "Expected to get EntryCreationAction from Action: {e:?}"
                        )));
                    }
                };
                match app_entry {
                    EntryTypes::Transaction(transaction) => {
                        let original_app_entry = must_get_valid_record(action.clone().original_action_address)?;
                        let original_transaction = match Transaction::try_from(original_app_entry) {
                            Ok(entry) => entry,
                            Err(e) => {
                                return Ok(ValidateCallbackResult::Invalid(format!(
                                    "Expected to get Transaction from Record: {e:?}"
                                )));
                            }
                        };
                        validate_update_transaction(action, transaction, original_create_action, original_transaction)
                    }
                    EntryTypes::Wallet(wallet) => {
                        let original_app_entry = must_get_valid_record(action.clone().original_action_address)?;
                        let original_wallet = match Wallet::try_from(original_app_entry) {
                            Ok(entry) => entry,
                            Err(e) => {
                                return Ok(ValidateCallbackResult::Invalid(format!(
                                    "Expected to get Wallet from Record: {e:?}"
                                )));
                            }
                        };
                        validate_update_wallet(action, wallet, original_create_action, original_wallet)
                    }
                    EntryTypes::DebtContract(contract) => {
                        let original_app_entry = must_get_valid_record(action.clone().original_action_address)?;
                        let original_contract = match DebtContract::try_from(original_app_entry) {
                            Ok(entry) => entry,
                            Err(e) => {
                                return Ok(ValidateCallbackResult::Invalid(format!(
                                    "Expected to get DebtContract from Record: {e:?}"
                                )));
                            }
                        };
                        validate_update_debt_contract(action, contract, original_create_action, original_contract)
                    }
                    EntryTypes::ReputationClaim(claim) => {
                        let original_app_entry = must_get_valid_record(action.clone().original_action_address)?;
                        let original_claim = match ReputationClaim::try_from(original_app_entry) {
                            Ok(entry) => entry,
                            Err(e) => {
                                return Ok(ValidateCallbackResult::Invalid(format!(
                                    "Expected to get ReputationClaim from Record: {e:?}"
                                )));
                            }
                        };
                        validate_update_reputation_claim(action, claim, original_create_action, original_claim)
                    }
                    EntryTypes::ChainCheckpoint(checkpoint) => {
                        let original_app_entry = must_get_valid_record(action.clone().original_action_address)?;
                        let original_checkpoint = match ChainCheckpoint::try_from(original_app_entry) {
                            Ok(entry) => entry,
                            Err(e) => {
                                return Ok(ValidateCallbackResult::Invalid(format!(
                                    "Expected to get ChainCheckpoint from Record: {e:?}"
                                )));
                            }
                        };
                        validate_update_checkpoint(action, checkpoint, original_create_action, original_checkpoint)
                    }
                    EntryTypes::Vouch(vouch) => {
                        // Vouch updates not allowed, but we need the arm
                        let original_app_entry = must_get_valid_record(action.clone().original_action_address)?;
                        let original_vouch = match Vouch::try_from(original_app_entry) {
                            Ok(entry) => entry,
                            Err(e) => return Ok(ValidateCallbackResult::Invalid(format!("Expected Vouch: {e:?}"))),
                        };
                        validate_update_vouch(action, vouch, original_create_action, original_vouch)
                    }
                }
            }
            _ => Ok(ValidateCallbackResult::Valid),
        },
        FlatOp::RegisterDelete(delete_entry) => {
            let original_action_hash = delete_entry.clone().action.deletes_address;
            let original_record = must_get_valid_record(original_action_hash)?;
            let original_record_action = original_record.action().clone();
            let original_action = match EntryCreationAction::try_from(original_record_action) {
                Ok(action) => action,
                Err(e) => {
                    return Ok(ValidateCallbackResult::Invalid(format!(
                        "Expected to get EntryCreationAction from Action: {e:?}"
                    )));
                }
            };
            let app_entry_type = match original_action.entry_type() {
                EntryType::App(app_entry_type) => app_entry_type,
                _ => {
                    return Ok(ValidateCallbackResult::Valid);
                }
            };
            let entry = match original_record.entry().as_option() {
                Some(entry) => entry,
                None => {
                    return Ok(ValidateCallbackResult::Invalid(
                        link_validation_error::DELETE_RECORD_NO_ENTRY.to_string(),
                    ));
                }
            };
            let original_app_entry = match EntryTypes::deserialize_from_type(
                app_entry_type.zome_index,
                app_entry_type.entry_index,
                entry,
            )? {
                Some(app_entry) => app_entry,
                None => {
                    return Ok(ValidateCallbackResult::Invalid(
                        link_validation_error::DELETE_UNKNOWN_ENTRY_TYPE.to_string(),
                    ));
                }
            };
            match original_app_entry {
                EntryTypes::Transaction(original_transaction) => {
                    validate_delete_transaction(delete_entry.clone().action, original_action, original_transaction)
                }
                EntryTypes::Wallet(original_wallet) => {
                    validate_delete_wallet(delete_entry.clone().action, original_action, original_wallet)
                }
                EntryTypes::DebtContract(original_contract) => {
                    validate_delete_debt_contract(delete_entry.clone().action, original_action, original_contract)
                }
                EntryTypes::ReputationClaim(original_claim) => {
                    validate_delete_reputation_claim(delete_entry.clone().action, original_action, original_claim)
                }
                EntryTypes::ChainCheckpoint(original_checkpoint) => {
                    validate_delete_checkpoint(delete_entry.clone().action, original_action, original_checkpoint)
                }
                EntryTypes::Vouch(original_vouch) => {
                    validate_delete_vouch(delete_entry.clone().action, original_action, original_vouch)
                }
            }
        }
        FlatOp::RegisterCreateLink { link_type, base_address, target_address, tag, action } => match link_type {
            LinkTypes::OwnerToWallet => validate_create_link_owner_to_wallet(action, base_address, target_address, tag),
            LinkTypes::WalletUpdates => validate_create_link_wallet_updates(action, base_address, target_address, tag),
            LinkTypes::WalletToTransactions => {
                validate_create_link_wallet_to_transactions(action, base_address, target_address, tag)
            }
            LinkTypes::TransactionToParent => {
                validate_create_link_transaction_to_parent(action, base_address, target_address, tag)
            }
            LinkTypes::DebtorToContracts | LinkTypes::CreditorToContracts | LinkTypes::DebtContractUpdates => {
                // DebtorToContracts: base is debtor, author must be the debtor.
                // CreditorToContracts: base is creditor, but the debtor creates this link.
                //   We verify the author matches the debtor field of the target contract.
                // DebtContractUpdates: base is original contract hash, author must be debtor.
                match link_type {
                    LinkTypes::DebtorToContracts => {
                        // Author must equal the base agent (the debtor)
                        if let Some(base_agent) = base_address.into_agent_pub_key() {
                            if action.author != base_agent {
                                return Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::CONTRACT_DEBTOR_AUTHOR_MISMATCH
                                        .to_string(),
                                ));
                            }
                        } else {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::TYPE_MISMATCH.to_string(),
                            ));
                        }
                        Ok(ValidateCallbackResult::Valid)
                    }
                    LinkTypes::CreditorToContracts => {
                        // Look up the target contract and verify the action author is the debtor.
                        let target_hash = match ActionHash::try_from(target_address) {
                            Ok(h) => h,
                            Err(_) => {
                                return Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::TYPE_MISMATCH.to_string(),
                                ))
                            }
                        };
                        if let Ok(record) = must_get_valid_record(target_hash) {
                            if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
                                let debtor_key: AgentPubKey = contract.debtor.clone().into();
                                if action.author != debtor_key {
                                    return Ok(ValidateCallbackResult::Invalid(
                                        types::constants::link_validation_error::CONTRACT_CREDITOR_LINK_NOT_DEBTOR
                                            .to_string(),
                                    ));
                                }
                            }
                        }
                        Ok(ValidateCallbackResult::Valid)
                    }
                    LinkTypes::DebtContractUpdates => {
                        // Base is original contract hash; look up contract to check debtor.
                        let base_hash = match ActionHash::try_from(base_address) {
                            Ok(h) => h,
                            Err(_) => {
                                return Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::TYPE_MISMATCH.to_string(),
                                ))
                            }
                        };
                        if let Ok(record) = must_get_valid_record(base_hash) {
                            if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
                                let debtor_key: AgentPubKey = contract.debtor.clone().into();
                                if action.author != debtor_key {
                                    return Ok(ValidateCallbackResult::Invalid(
                                        types::constants::link_validation_error::CONTRACT_UPDATE_LINK_NOT_DEBTOR
                                            .to_string(),
                                    ));
                                }
                            }
                        }
                        Ok(ValidateCallbackResult::Valid)
                    }
                    _ => Ok(ValidateCallbackResult::Valid),
                }
            }
            LinkTypes::AgentToAcquaintance | LinkTypes::AgentToArchivedContracts => {
                // Author must match the base agent (only you can manage your own acquaintances/archives)
                if let Some(base_agent) = base_address.into_agent_pub_key() {
                    if action.author != base_agent {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::AUTHOR_NOT_LINK_BASE.to_string(),
                        ))
                    } else {
                        Ok(ValidateCallbackResult::Valid)
                    }
                } else {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::TYPE_MISMATCH.to_string(),
                    ))
                }
            }
            LinkTypes::AgentToLocalTrust => {
                // Author must match the base agent (only you can publish your trust row)
                let base_agent = match base_address.clone().into_agent_pub_key() {
                    Some(a) => a,
                    None => {
                        return Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::TYPE_MISMATCH.to_string(),
                        ));
                    }
                };
                if action.author != base_agent {
                    return Ok(ValidateCallbackResult::Invalid(
                        types::constants::trust_link_validation_error::AUTHOR_NOT_BASE_AGENT.to_string(),
                    ));
                }
                // Range-check the trust value embedded in the link tag.
                // Without this check a malicious node can publish c_ij = NaN,
                // c_ij = 1e300, c_ij = -5, etc. — corrupting EigenTrust on
                // every peer that ingests the row. The error code
                // `TRUST_VALUE_OUT_OF_RANGE` was already declared but never
                // emitted prior to this fix.
                let parsed: Result<types::TrustLinkTag, _> =
                    SerializedBytes::from(UnsafeBytes::from(tag.0.clone())).try_into();
                let parsed_tag = match parsed {
                    Ok(t) => t,
                    Err(_) => {
                        return Ok(ValidateCallbackResult::Invalid(
                            types::constants::trust_link_validation_error::TRUST_VALUE_OUT_OF_RANGE.to_string(),
                        ));
                    }
                };
                if !parsed_tag.trust_value.is_finite() || parsed_tag.trust_value < 0.0 || parsed_tag.trust_value > 1.0 {
                    return Ok(ValidateCallbackResult::Invalid(
                        types::constants::trust_link_validation_error::TRUST_VALUE_OUT_OF_RANGE.to_string(),
                    ));
                }
                Ok(ValidateCallbackResult::Valid)
            }
            LinkTypes::AgentToReputationClaim => {
                // Author must match the base agent (only you can publish your own claim link)
                if let Some(base_agent) = base_address.into_agent_pub_key() {
                    if action.author != base_agent {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::INVALID_REPUTATION_CLAIM_LINK.to_string(),
                        ))
                    } else {
                        Ok(ValidateCallbackResult::Valid)
                    }
                } else {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::TYPE_MISMATCH.to_string(),
                    ))
                }
            }
            LinkTypes::AgentToCheckpoint => {
                // Author must match the base agent (only you can publish your own checkpoint link)
                if let Some(base_agent) = base_address.into_agent_pub_key() {
                    if action.author != base_agent {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::INVALID_CHECKPOINT_LINK.to_string(),
                        ))
                    } else {
                        Ok(ValidateCallbackResult::Valid)
                    }
                } else {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::TYPE_MISMATCH.to_string(),
                    ))
                }
            }
            LinkTypes::AgentToDebtBalance => {
                // Author must match the base agent (only you can update your own debt balance)
                if let Some(base_agent) = base_address.into_agent_pub_key() {
                    if action.author != base_agent {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::INVALID_DEBT_BALANCE_LINK.to_string(),
                        ))
                    } else {
                        Ok(ValidateCallbackResult::Valid)
                    }
                } else {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::TYPE_MISMATCH.to_string(),
                    ))
                }
            }
            LinkTypes::AgentToContractsByEpoch => {
                // Author must match the base agent (only you can create your own epoch-bucketed contract links)
                if let Some(base_agent) = base_address.into_agent_pub_key() {
                    if action.author != base_agent {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::INVALID_EPOCH_BUCKET_LINK.to_string(),
                        ))
                    } else {
                        Ok(ValidateCallbackResult::Valid)
                    }
                } else {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::TYPE_MISMATCH.to_string(),
                    ))
                }
            }
            LinkTypes::EntrantToVouch => {
                // Only the entrant (or sponsor on their behalf) can create EntrantToVouch links.
                // The vouch target must be fetched to verify the base matches the entrant.
                if let Some(base_agent) = base_address.into_agent_pub_key() {
                    // Verify that the link author is the sponsor of the vouch
                    // (the sponsor creates both links during create_vouch)
                    let target_hash = match ActionHash::try_from(target_address) {
                        Ok(hash) => hash,
                        Err(_) => {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::ENTRANT_VOUCH_TARGET_NOT_ACTION.to_string(),
                            ))
                        }
                    };
                    let record = must_get_valid_record(target_hash)?;
                    if let Some(vouch) = record.entry().to_app_option::<Vouch>().map_err(|e| wasm_error!(e))? {
                        if vouch.entrant != base_agent {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::ENTRANT_VOUCH_BASE_MISMATCH.to_string(),
                            ));
                        }
                        if action.author != vouch.sponsor {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::ENTRANT_VOUCH_AUTHOR_NOT_SPONSOR.to_string(),
                            ));
                        }
                        Ok(ValidateCallbackResult::Valid)
                    } else {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::ENTRANT_VOUCH_TARGET_NOT_VOUCH.to_string(),
                        ))
                    }
                } else {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::ENTRANT_VOUCH_BASE_NOT_AGENT.to_string(),
                    ))
                }
            }
            LinkTypes::SponsorToVouch => {
                if let Some(base_agent) = base_address.into_agent_pub_key() {
                    // Sponsor must be the author and the base must be the sponsor
                    if action.author != base_agent {
                        return Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::SPONSOR_VOUCH_AUTHOR_MISMATCH.to_string(),
                        ));
                    }
                    Ok(ValidateCallbackResult::Valid)
                } else {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::SPONSOR_VOUCH_BASE_NOT_AGENT.to_string(),
                    ))
                }
            }
            LinkTypes::VouchUpdates => {
                // Vouch update links can be created by the sponsor OR by the debtor (entrant)
                // for slash updates. The base is the original vouch hash, target is the updated vouch hash.
                if let Some(base_hash) = base_address.into_action_hash() {
                    let record = must_get_valid_record(base_hash)?;
                    if let Some(vouch) = record.entry().to_app_option::<Vouch>().ok().flatten() {
                        if action.author != vouch.sponsor && action.author != vouch.entrant {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::VOUCH_UPDATE_AUTHOR_NOT_SPONSOR.to_string(),
                            ));
                        }
                        Ok(ValidateCallbackResult::Valid)
                    } else {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::VOUCH_UPDATE_BASE_NOT_VOUCH.to_string(),
                        ))
                    }
                } else {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::VOUCH_UPDATE_BASE_NOT_ACTION.to_string(),
                    ))
                }
            }
            LinkTypes::AgentToFailureObservation => {
                // Base = creditor agent, target = debtor agent.
                // The link is created by the debtor's cell (which runs process_contract_expirations)
                // on behalf of the creditor, so we do NOT check action.author == base_agent.
                // Instead, we verify the expired contract's creditor field matches the link base,
                // and the debtor field matches the link target. The contract hash in the tag is the
                // unforgeable proof: it must reference a real Expired/Archived DebtContract on the DHT.
                if let Some(base_agent) = base_address.into_agent_pub_key() {
                    let debtor_agent = match target_address.into_agent_pub_key() {
                        Some(a) => a,
                        None => {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::FAILURE_OBS_TARGET_NOT_AGENT.to_string(),
                            ));
                        }
                    };
                    // Decode tag and verify expired contract
                    let tag_bytes = SerializedBytes::from(UnsafeBytes::from(tag.into_inner()));
                    let obs_tag = match types::FailureObservationTag::try_from(tag_bytes) {
                        Ok(t) => t,
                        Err(_) => {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::FAILURE_OBS_TAG_MALFORMED.to_string(),
                            ));
                        }
                    };
                    let contract_record = match must_get_valid_record(obs_tag.expired_contract_hash) {
                        Ok(r) => r,
                        Err(_) => {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::FAILURE_OBS_CONTRACT_NOT_FOUND.to_string(),
                            ));
                        }
                    };
                    let contract: DebtContract =
                        match contract_record.entry().to_app_option().map_err(|e| wasm_error!(e))? {
                            Some(c) => c,
                            None => {
                                return Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::FAILURE_OBS_NOT_CONTRACT.to_string(),
                                ));
                            }
                        };
                    if contract.status != ContractStatus::Expired && contract.status != ContractStatus::Archived {
                        return Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::FAILURE_OBS_CONTRACT_NOT_EXPIRED.to_string(),
                        ));
                    }
                    let contract_debtor: AgentPubKey = contract.debtor.clone().into();
                    if contract_debtor != debtor_agent {
                        return Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::FAILURE_OBS_DEBTOR_MISMATCH.to_string(),
                        ));
                    }
                    // Verify the link base (creditor) matches the contract's creditor field.
                    let contract_creditor: AgentPubKey = contract.creditor.clone().into();
                    if contract_creditor != base_agent {
                        return Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::FAILURE_OBS_AUTHOR_NOT_CREDITOR.to_string(),
                        ));
                    }
                    Ok(ValidateCallbackResult::Valid)
                } else {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::FAILURE_OBS_BASE_NOT_AGENT.to_string(),
                    ))
                }
            }
            LinkTypes::FailureObservationIndex => {
                // Path-based anchor links for reverse lookup
                // Base is a path entry hash, target is an agent (debtor)
                // Any agent can create these as part of publishing their observation
                if target_address.into_agent_pub_key().is_none() {
                    return Ok(ValidateCallbackResult::Invalid(
                        link_validation_error::FAILURE_OBS_TARGET_NOT_AGENT.to_string(),
                    ));
                }
                Ok(ValidateCallbackResult::Valid)
            }
            LinkTypes::DebtorToBlockedTrialSeller => {
                // Author (debtor) must match base agent (only the debtor writes their own block list).
                // Target must be an agent (the seller being permanently blocked).
                // Append-only: this link type is never deleted.
                if let Some(base_agent) = base_address.into_agent_pub_key() {
                    if action.author != base_agent {
                        return Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::BLOCKED_TRIAL_AUTHOR_NOT_DEBTOR.to_string(),
                        ));
                    }
                    if target_address.into_agent_pub_key().is_none() {
                        return Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::BLOCKED_TRIAL_TARGET_NOT_AGENT.to_string(),
                        ));
                    }
                    Ok(ValidateCallbackResult::Valid)
                } else {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::BLOCKED_TRIAL_BASE_NOT_AGENT.to_string(),
                    ))
                }
            }
            LinkTypes::AgentToSupportSatisfaction => {
                // Author (beneficiary) must match base agent.
                // Only the beneficiary can record their own support satisfaction evidence.
                let base_agent = match base_address.clone().into_agent_pub_key() {
                    Some(a) => a,
                    None => return Ok(ValidateCallbackResult::Valid),
                };
                if action.author != base_agent {
                    return Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::AUTHOR_NOT_LINK_BASE.to_string(),
                    ));
                }
                // Verify the tag's amount is finite and non-negative.
                // We cannot cross-check the tag against an actual drain transaction
                // without get_links (not available in HDI), but we can at least
                // ensure the amount is a valid f64. A forged satisfaction tag with
                // NaN or negative amount would corrupt the pre-trust distribution
                // fed into EigenTrust.
                let parsed: Result<types::SupportSatisfactionTag, _> =
                    SerializedBytes::from(UnsafeBytes::from(tag.0.clone())).try_into();
                match parsed {
                    Ok(sat_tag) if sat_tag.amount.is_finite() && sat_tag.amount >= 0.0 => {
                        Ok(ValidateCallbackResult::Valid)
                    }
                    Ok(_) | Err(_) => Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::AUTHOR_NOT_LINK_BASE.to_string(),
                    )),
                }
            }
        },
        FlatOp::RegisterDeleteLink { link_type, base_address, target_address, tag, original_action, action } => {
            match link_type {
                LinkTypes::OwnerToWallet => {
                    validate_delete_link_owner_to_wallet(action, original_action, base_address, target_address, tag)
                }
                LinkTypes::WalletUpdates => {
                    validate_delete_link_wallet_updates(action, original_action, base_address, target_address, tag)
                }
                LinkTypes::WalletToTransactions => validate_delete_link_wallet_to_transactions(
                    action,
                    original_action,
                    base_address,
                    target_address,
                    tag,
                ),
                LinkTypes::TransactionToParent => validate_delete_link_transaction_to_parent(
                    action,
                    original_action,
                    base_address,
                    target_address,
                    tag,
                ),
                // Trust, acquaintance, archived contract, checkpoint, debt balance, reputation claim,
                // failure observation, and support satisfaction links can be deleted (pruning, republishing)
                LinkTypes::AgentToLocalTrust
                | LinkTypes::AgentToAcquaintance
                | LinkTypes::AgentToReputationClaim
                | LinkTypes::AgentToArchivedContracts
                | LinkTypes::AgentToCheckpoint
                | LinkTypes::AgentToDebtBalance
                | LinkTypes::AgentToFailureObservation
                | LinkTypes::FailureObservationIndex
                | LinkTypes::AgentToSupportSatisfaction => Ok(ValidateCallbackResult::Valid),
                LinkTypes::EntrantToVouch | LinkTypes::SponsorToVouch => {
                    // Only the original link creator can delete vouch links
                    if action.author != original_action.author {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::VOUCH_LINK_DELETE_NOT_CREATOR.to_string(),
                        ))
                    } else {
                        Ok(ValidateCallbackResult::Valid)
                    }
                }
                LinkTypes::VouchUpdates => {
                    // Vouch update links cannot be deleted (append-only)
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::VOUCH_UPDATE_LINK_NOT_DELETABLE.to_string(),
                    ))
                }
                LinkTypes::DebtorToBlockedTrialSeller => {
                    // Permanent block — cannot be deleted
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::BLOCKED_TRIAL_LINK_NOT_DELETABLE.to_string(),
                    ))
                }
                // Contract index links can be deleted ONLY when the target contract is in a
                // terminal+archived state, allowing the archival mechanism to remove them
                // and keep active scans O(active) instead of O(all-time).
                // DebtContractUpdates and AgentToContractsByEpoch are still immutable —
                // they are append-only metadata and must never be pruned.
                LinkTypes::DebtContractUpdates | LinkTypes::AgentToContractsByEpoch => {
                    Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::CONTRACT_LINK_NOT_DELETABLE.to_string(),
                    ))
                }
                LinkTypes::DebtorToContracts | LinkTypes::CreditorToContracts => {
                    // These index links may be deleted when the target contract has been
                    // Transferred, Expired, or Archived (terminal states)
                    //
                    // Simplest approach possible since we can't get links in integrity zone:
                    // walk the original action's updates via must_get_agent_activity
                    // on the contract debtor to find if any update has a terminal status.
                    let target_hash = match ActionHash::try_from(target_address) {
                        Ok(h) => h,
                        Err(_) => {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::CONTRACT_LINK_NOT_DELETABLE.to_string(),
                            ))
                        }
                    };
                    let original_record = match must_get_valid_record(target_hash.clone()) {
                        Ok(r) => r,
                        Err(_) => {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::CONTRACT_LINK_DELETE_NOT_ARCHIVED.to_string(),
                            ))
                        }
                    };
                    let original_contract: Option<DebtContract> =
                        original_record.entry().to_app_option::<DebtContract>().ok().flatten();
                    // If the original contract (Create record) is already in a terminal state,
                    // the delete is fine (this is unusual but valid).
                    // If it's Active, we must find a terminal-state Update on the debtor's chain.
                    let is_terminal = original_contract
                        .as_ref()
                        .map(|c| !matches!(c.status, ContractStatus::Active))
                        .unwrap_or(false);

                    if !is_terminal {
                        // Original is Active. Walk the debtor's chain to find a terminal update.
                        let debtor = match original_contract.as_ref() {
                            Some(c) => AgentPubKey::from(c.debtor.clone()),
                            None => {
                                return Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::CONTRACT_LINK_DELETE_NOT_ARCHIVED
                                        .to_string(),
                                ))
                            }
                        };
                        let activity = must_get_agent_activity(
                            debtor,
                            ChainFilter::new(original_record.action_address().clone()),
                        )?;
                        let mut found_terminal = false;
                        for item in &activity {
                            if let Action::Update(upd) = item.action.action() {
                                if upd.original_action_address == target_hash {
                                    if let Some(entry_hash) = item.action.action().entry_hash() {
                                        if let Ok(entry) = must_get_entry(entry_hash.clone()) {
                                            if let Ok(updated) = DebtContract::try_from(entry.content) {
                                                if !matches!(updated.status, ContractStatus::Active) {
                                                    found_terminal = true;
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if !found_terminal {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::CONTRACT_LINK_DELETE_NOT_ARCHIVED.to_string(),
                            ));
                        }
                    }
                    Ok(ValidateCallbackResult::Valid)
                }
            }
        }
        FlatOp::StoreRecord(store_record) => match store_record {
            OpRecord::CreateEntry { app_entry, action } => match app_entry {
                EntryTypes::Wallet(wallet) => validate_create_wallet(EntryCreationAction::Create(action), wallet),
                EntryTypes::Transaction(transaction) => {
                    validate_create_transaction(EntryCreationAction::Create(action), transaction)
                }
                EntryTypes::DebtContract(contract) => {
                    validate_create_debt_contract(EntryCreationAction::Create(action), contract)
                }
                EntryTypes::ReputationClaim(claim) => {
                    validate_create_reputation_claim(EntryCreationAction::Create(action), claim)
                }
                EntryTypes::ChainCheckpoint(checkpoint) => {
                    validate_create_checkpoint(EntryCreationAction::Create(action), checkpoint)
                }
                EntryTypes::Vouch(vouch) => validate_create_vouch(EntryCreationAction::Create(action), vouch),
            },
            OpRecord::UpdateEntry { original_action_hash, app_entry, action, .. } => {
                let original_record = must_get_valid_record(original_action_hash)?;
                let original_action = original_record.action().clone();
                let original_action = match original_action {
                    Action::Create(create) => EntryCreationAction::Create(create),
                    Action::Update(update) => EntryCreationAction::Update(update),
                    _ => {
                        return Ok(ValidateCallbackResult::Invalid(
                            link_validation_error::UPDATE_ORIGINAL_NOT_CREATE.to_string(),
                        ));
                    }
                };
                match app_entry {
                    EntryTypes::Wallet(wallet) => {
                        let result =
                            validate_create_wallet(EntryCreationAction::Update(action.clone()), wallet.clone())?;
                        if let ValidateCallbackResult::Valid = result {
                            let original_wallet: Option<Wallet> =
                                original_record.entry().to_app_option().map_err(|e| wasm_error!(e))?;
                            let original_wallet = match original_wallet {
                                Some(wallet) => wallet,
                                None => {
                                    return Ok(ValidateCallbackResult::Invalid(
                                        "The updated entry type must be the same as the original entry type"
                                            .to_string(),
                                    ));
                                }
                            };
                            validate_update_wallet(action, wallet, original_action, original_wallet)
                        } else {
                            Ok(result)
                        }
                    }
                    EntryTypes::Transaction(transaction) => {
                        let result = validate_create_transaction(
                            EntryCreationAction::Update(action.clone()),
                            transaction.clone(),
                        )?;
                        if let ValidateCallbackResult::Valid = result {
                            let original_transaction: Option<Transaction> =
                                original_record.entry().to_app_option().map_err(|e| wasm_error!(e))?;
                            let original_transaction = match original_transaction {
                                Some(transaction) => transaction,
                                None => {
                                    return Ok(ValidateCallbackResult::Invalid(
                                        "The updated entry type must be the same as the original entry type"
                                            .to_string(),
                                    ));
                                }
                            };
                            validate_update_transaction(action, transaction, original_action, original_transaction)
                        } else {
                            Ok(result)
                        }
                    }
                    EntryTypes::DebtContract(contract) => {
                        // Skip validate_create_debt_contract for updates: the create
                        // validator rejects any non-Active status, which would block
                        // legitimate Expired/Transferred/Archived transitions.
                        // validate_update_debt_contract handles all update-specific rules.
                        let original_contract: Option<DebtContract> =
                            original_record.entry().to_app_option().map_err(|e| wasm_error!(e))?;
                        let original_contract = match original_contract {
                            Some(contract) => contract,
                            None => {
                                return Ok(ValidateCallbackResult::Invalid(
                                    link_validation_error::UPDATE_TYPE_MISMATCH.to_string(),
                                ));
                            }
                        };
                        validate_update_debt_contract(action, contract, original_action, original_contract)
                    }
                    EntryTypes::ReputationClaim(claim) => {
                        let result = validate_create_reputation_claim(
                            EntryCreationAction::Update(action.clone()),
                            claim.clone(),
                        )?;
                        if let ValidateCallbackResult::Valid = result {
                            let original_claim: Option<ReputationClaim> =
                                original_record.entry().to_app_option().map_err(|e| wasm_error!(e))?;
                            let original_claim = match original_claim {
                                Some(claim) => claim,
                                None => {
                                    return Ok(ValidateCallbackResult::Invalid(
                                        "The updated entry type must be the same as the original entry type"
                                            .to_string(),
                                    ));
                                }
                            };
                            validate_update_reputation_claim(action, claim, original_action, original_claim)
                        } else {
                            Ok(result)
                        }
                    }
                    EntryTypes::ChainCheckpoint(checkpoint) => {
                        let result = validate_create_checkpoint(
                            EntryCreationAction::Update(action.clone()),
                            checkpoint.clone(),
                        )?;
                        if let ValidateCallbackResult::Valid = result {
                            let original_checkpoint: Option<ChainCheckpoint> =
                                original_record.entry().to_app_option().map_err(|e| wasm_error!(e))?;
                            let original_checkpoint = match original_checkpoint {
                                Some(checkpoint) => checkpoint,
                                None => {
                                    return Ok(ValidateCallbackResult::Invalid(
                                        "The updated entry type must be the same as the original entry type"
                                            .to_string(),
                                    ));
                                }
                            };
                            validate_update_checkpoint(action, checkpoint, original_action, original_checkpoint)
                        } else {
                            Ok(result)
                        }
                    }
                    EntryTypes::Vouch(vouch) => {
                        let original_vouch: Option<Vouch> =
                            original_record.entry().to_app_option().map_err(|e| wasm_error!(e))?;
                        let original_vouch = match original_vouch {
                            Some(v) => v,
                            None => {
                                return Ok(ValidateCallbackResult::Invalid(
                                    link_validation_error::TYPE_MISMATCH.to_string(),
                                ))
                            }
                        };
                        validate_update_vouch(action, vouch, original_action, original_vouch)
                    }
                }
            }
            OpRecord::DeleteEntry { original_action_hash, action, .. } => {
                let original_record = must_get_valid_record(original_action_hash)?;
                let original_action = original_record.action().clone();
                let original_action = match original_action {
                    Action::Create(create) => EntryCreationAction::Create(create),
                    Action::Update(update) => EntryCreationAction::Update(update),
                    _ => {
                        return Ok(ValidateCallbackResult::Invalid(
                            link_validation_error::DELETE_ORIGINAL_NOT_CREATE.to_string(),
                        ));
                    }
                };
                let app_entry_type = match original_action.entry_type() {
                    EntryType::App(app_entry_type) => app_entry_type,
                    _ => {
                        return Ok(ValidateCallbackResult::Valid);
                    }
                };
                let entry = match original_record.entry().as_option() {
                    Some(entry) => entry,
                    None => {
                        return Ok(ValidateCallbackResult::Invalid(
                            link_validation_error::DELETE_RECORD_NO_ENTRY.to_string(),
                        ));
                    }
                };
                let original_app_entry = match EntryTypes::deserialize_from_type(
                    app_entry_type.zome_index,
                    app_entry_type.entry_index,
                    entry,
                )? {
                    Some(app_entry) => app_entry,
                    None => {
                        return Ok(ValidateCallbackResult::Invalid(
                            link_validation_error::DELETE_UNKNOWN_ENTRY_TYPE.to_string(),
                        ));
                    }
                };
                match original_app_entry {
                    EntryTypes::Wallet(original_wallet) => {
                        validate_delete_wallet(action, original_action, original_wallet)
                    }
                    EntryTypes::Transaction(original_transaction) => {
                        validate_delete_transaction(action, original_action, original_transaction)
                    }
                    EntryTypes::DebtContract(original_contract) => {
                        validate_delete_debt_contract(action, original_action, original_contract)
                    }
                    EntryTypes::ReputationClaim(original_claim) => {
                        validate_delete_reputation_claim(action, original_action, original_claim)
                    }
                    EntryTypes::ChainCheckpoint(original_checkpoint) => {
                        validate_delete_checkpoint(action, original_action, original_checkpoint)
                    }
                    EntryTypes::Vouch(original_vouch) => validate_delete_vouch(action, original_action, original_vouch),
                }
            }
            OpRecord::CreateLink { base_address, target_address, tag, link_type, action } => match link_type {
                LinkTypes::OwnerToWallet => {
                    validate_create_link_owner_to_wallet(action, base_address, target_address, tag)
                }
                LinkTypes::WalletUpdates => {
                    validate_create_link_wallet_updates(action, base_address, target_address, tag)
                }
                LinkTypes::WalletToTransactions => {
                    validate_create_link_wallet_to_transactions(action, base_address, target_address, tag)
                }
                LinkTypes::TransactionToParent => {
                    validate_create_link_transaction_to_parent(action, base_address, target_address, tag)
                }
                // Agent-scoped links: author must match the base agent (defense-in-depth,
                // mirroring RegisterCreateLink checks)
                LinkTypes::AgentToLocalTrust
                | LinkTypes::AgentToAcquaintance
                | LinkTypes::AgentToReputationClaim
                | LinkTypes::AgentToArchivedContracts
                | LinkTypes::AgentToCheckpoint
                | LinkTypes::AgentToDebtBalance
                | LinkTypes::AgentToContractsByEpoch
                | LinkTypes::AgentToSupportSatisfaction => {
                    if let Some(base_agent) = base_address.into_agent_pub_key() {
                        if action.author != base_agent {
                            Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::STORE_RECORD_AUTHOR_MISMATCH.to_string(),
                            ))
                        } else {
                            Ok(ValidateCallbackResult::Valid)
                        }
                    } else {
                        Ok(ValidateCallbackResult::Valid)
                    }
                }
                // Contract links: enforce author-party checks mirroring RegisterCreateLink.
                LinkTypes::DebtorToContracts | LinkTypes::CreditorToContracts | LinkTypes::DebtContractUpdates => {
                    match link_type {
                        LinkTypes::DebtorToContracts => {
                            if let Some(base_agent) = base_address.into_agent_pub_key() {
                                if action.author != base_agent {
                                    return Ok(ValidateCallbackResult::Invalid(
                                        types::constants::link_validation_error::CONTRACT_DEBTOR_AUTHOR_MISMATCH
                                            .to_string(),
                                    ));
                                }
                            }
                            Ok(ValidateCallbackResult::Valid)
                        }
                        LinkTypes::CreditorToContracts => {
                            let target_hash = match ActionHash::try_from(target_address) {
                                Ok(h) => h,
                                Err(_) => return Ok(ValidateCallbackResult::Valid),
                            };
                            if let Ok(record) = must_get_valid_record(target_hash) {
                                if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
                                    let debtor_key: AgentPubKey = contract.debtor.clone().into();
                                    if action.author != debtor_key {
                                        return Ok(ValidateCallbackResult::Invalid(
                                            types::constants::link_validation_error::CONTRACT_CREDITOR_LINK_NOT_DEBTOR
                                                .to_string(),
                                        ));
                                    }
                                }
                            }
                            Ok(ValidateCallbackResult::Valid)
                        }
                        LinkTypes::DebtContractUpdates => {
                            let base_hash = match ActionHash::try_from(base_address) {
                                Ok(h) => h,
                                Err(_) => return Ok(ValidateCallbackResult::Valid),
                            };
                            if let Ok(record) = must_get_valid_record(base_hash) {
                                if let Some(contract) = record.entry().to_app_option::<DebtContract>().ok().flatten() {
                                    let debtor_key: AgentPubKey = contract.debtor.clone().into();
                                    if action.author != debtor_key {
                                        return Ok(ValidateCallbackResult::Invalid(
                                            types::constants::link_validation_error::CONTRACT_UPDATE_LINK_NOT_DEBTOR
                                                .to_string(),
                                        ));
                                    }
                                }
                            }
                            Ok(ValidateCallbackResult::Valid)
                        }
                        _ => Ok(ValidateCallbackResult::Valid),
                    }
                }
                // Vouch links: author must be the sponsor
                LinkTypes::EntrantToVouch | LinkTypes::SponsorToVouch => {
                    // The sponsor creates both links. For SponsorToVouch, base must be author.
                    if let Some(base_agent) = base_address.into_agent_pub_key() {
                        match link_type {
                            LinkTypes::SponsorToVouch => {
                                if action.author != base_agent {
                                    Ok(ValidateCallbackResult::Invalid(
                                        types::constants::link_validation_error::STORE_RECORD_SPONSOR_MISMATCH
                                            .to_string(),
                                    ))
                                } else {
                                    Ok(ValidateCallbackResult::Valid)
                                }
                            }
                            _ => Ok(ValidateCallbackResult::Valid),
                        }
                    } else {
                        Ok(ValidateCallbackResult::Valid)
                    }
                }
                // Vouch update links: author must be the sponsor or debtor (entrant)
                // Mirror the RegisterCreateLink check — base is original vouch ActionHash.
                LinkTypes::VouchUpdates => {
                    if let Some(base_hash) = base_address.into_action_hash() {
                        let record = must_get_valid_record(base_hash)?;
                        if let Some(vouch) = record.entry().to_app_option::<Vouch>().ok().flatten() {
                            if action.author != vouch.sponsor && action.author != vouch.entrant {
                                Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::VOUCH_UPDATE_AUTHOR_NOT_SPONSOR
                                        .to_string(),
                                ))
                            } else {
                                Ok(ValidateCallbackResult::Valid)
                            }
                        } else {
                            Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::VOUCH_UPDATE_BASE_NOT_VOUCH.to_string(),
                            ))
                        }
                    } else {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::VOUCH_UPDATE_BASE_NOT_ACTION.to_string(),
                        ))
                    }
                }
                // Failure observation links: full proof verification (mirrors RegisterCreateLink)
                // NOTE: The link is created by the debtor's cell on behalf of the creditor, so
                // we do NOT require action.author == base_agent. The contract hash in the tag
                // is the unforgeable proof; we verify the contract's creditor matches the base.
                LinkTypes::AgentToFailureObservation => {
                    if let Some(base_agent) = base_address.into_agent_pub_key() {
                        let debtor_agent = match target_address.into_agent_pub_key() {
                            Some(a) => a,
                            None => {
                                return Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::FAILURE_OBS_TARGET_NOT_AGENT.to_string(),
                                ));
                            }
                        };
                        let tag_bytes = SerializedBytes::from(UnsafeBytes::from(tag.into_inner()));
                        let obs_tag = match types::FailureObservationTag::try_from(tag_bytes) {
                            Ok(t) => t,
                            Err(_) => {
                                return Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::FAILURE_OBS_TAG_MALFORMED.to_string(),
                                ));
                            }
                        };
                        let contract_record = match must_get_valid_record(obs_tag.expired_contract_hash) {
                            Ok(r) => r,
                            Err(_) => {
                                return Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::FAILURE_OBS_CONTRACT_NOT_FOUND.to_string(),
                                ));
                            }
                        };
                        let contract: DebtContract =
                            match contract_record.entry().to_app_option().map_err(|e| wasm_error!(e))? {
                                Some(c) => c,
                                None => {
                                    return Ok(ValidateCallbackResult::Invalid(
                                        types::constants::link_validation_error::FAILURE_OBS_NOT_CONTRACT.to_string(),
                                    ));
                                }
                            };
                        if contract.status != ContractStatus::Expired && contract.status != ContractStatus::Archived {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::FAILURE_OBS_CONTRACT_NOT_EXPIRED.to_string(),
                            ));
                        }
                        let contract_debtor: AgentPubKey = contract.debtor.clone().into();
                        if contract_debtor != debtor_agent {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::FAILURE_OBS_DEBTOR_MISMATCH.to_string(),
                            ));
                        }
                        let contract_creditor: AgentPubKey = contract.creditor.clone().into();
                        if contract_creditor != base_agent {
                            return Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::FAILURE_OBS_AUTHOR_NOT_CREDITOR.to_string(),
                            ));
                        }
                        Ok(ValidateCallbackResult::Valid)
                    } else {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::FAILURE_OBS_BASE_NOT_AGENT.to_string(),
                        ))
                    }
                }
                // Failure observation index: path-based anchor, any agent can create
                LinkTypes::FailureObservationIndex => Ok(ValidateCallbackResult::Valid),
                // Blocked trial seller: author (debtor) must match base agent
                LinkTypes::DebtorToBlockedTrialSeller => {
                    if let Some(base_agent) = base_address.into_agent_pub_key() {
                        if action.author != base_agent {
                            Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::BLOCKED_TRIAL_AUTHOR_NOT_DEBTOR.to_string(),
                            ))
                        } else {
                            Ok(ValidateCallbackResult::Valid)
                        }
                    } else {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::BLOCKED_TRIAL_BASE_NOT_AGENT.to_string(),
                        ))
                    }
                } // Support satisfaction is covered by the agent-scoped group above
            },
            OpRecord::DeleteLink { original_action_hash, base_address, action } => {
                let record = must_get_valid_record(original_action_hash)?;
                let create_link = match record.action() {
                    Action::CreateLink(create_link) => create_link.clone(),
                    _ => {
                        return Ok(ValidateCallbackResult::Invalid(
                            link_validation_error::DELETE_ACTION_NOT_CREATE.to_string(),
                        ));
                    }
                };
                let link_type = match LinkTypes::from_type(create_link.zome_index, create_link.link_type)? {
                    Some(lt) => lt,
                    None => {
                        return Ok(ValidateCallbackResult::Valid);
                    }
                };
                match link_type {
                    LinkTypes::OwnerToWallet => validate_delete_link_owner_to_wallet(
                        action,
                        create_link.clone(),
                        base_address,
                        create_link.target_address,
                        create_link.tag,
                    ),
                    LinkTypes::WalletUpdates => validate_delete_link_wallet_updates(
                        action,
                        create_link.clone(),
                        base_address,
                        create_link.target_address,
                        create_link.tag,
                    ),
                    LinkTypes::WalletToTransactions => validate_delete_link_wallet_to_transactions(
                        action,
                        create_link.clone(),
                        base_address,
                        create_link.target_address,
                        create_link.tag,
                    ),
                    LinkTypes::TransactionToParent => validate_delete_link_transaction_to_parent(
                        action,
                        create_link.clone(),
                        base_address,
                        create_link.target_address,
                        create_link.tag,
                    ),
                    LinkTypes::AgentToLocalTrust
                    | LinkTypes::AgentToAcquaintance
                    | LinkTypes::AgentToReputationClaim
                    | LinkTypes::AgentToArchivedContracts
                    | LinkTypes::AgentToCheckpoint
                    | LinkTypes::AgentToDebtBalance
                    | LinkTypes::EntrantToVouch
                    | LinkTypes::SponsorToVouch
                    | LinkTypes::AgentToFailureObservation
                    | LinkTypes::FailureObservationIndex
                    | LinkTypes::AgentToSupportSatisfaction => Ok(ValidateCallbackResult::Valid),
                    LinkTypes::VouchUpdates => {
                        // Vouch update links are append-only and cannot be deleted.
                        // This mirrors the RegisterDeleteLink check (line 629-633) which
                        // correctly rejects VouchUpdates deletions. The StoreRecord path
                        // must apply the same rule — omitting it here would allow a
                        // malicious node to bypass the RegisterDeleteLink check by crafting
                        // a StoreRecord op instead.
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::VOUCH_UPDATE_LINK_NOT_DELETABLE.to_string(),
                        ))
                    }
                    LinkTypes::DebtorToBlockedTrialSeller => Ok(ValidateCallbackResult::Invalid(
                        types::constants::link_validation_error::BLOCKED_TRIAL_LINK_NOT_DELETABLE.to_string(),
                    )),
                    LinkTypes::DebtContractUpdates | LinkTypes::AgentToContractsByEpoch => {
                        Ok(ValidateCallbackResult::Invalid(
                            types::constants::link_validation_error::CONTRACT_LINK_NOT_DELETABLE.to_string(),
                        ))
                    }
                    LinkTypes::DebtorToContracts | LinkTypes::CreditorToContracts => {
                        // These index links may be deleted for archival. The coordinator
                        // enforces the age threshold and Archived status check.
                        // We only verify the target is a valid DebtContract.
                        // (Cannot check Archived status here: must_get_valid_record returns
                        // the original Create record, not the latest Update, and get_links
                        // is unavailable in the integrity/HDI zome.)
                        let target_hash = match create_link.target_address.clone().into_action_hash() {
                            Some(h) => h,
                            None => {
                                return Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::CONTRACT_LINK_DELETE_NOT_ARCHIVED
                                        .to_string(),
                                ))
                            }
                        };
                        match must_get_valid_record(target_hash) {
                            Ok(record) => match record.entry().to_app_option::<DebtContract>() {
                                Ok(Some(_)) => Ok(ValidateCallbackResult::Valid),
                                _ => Ok(ValidateCallbackResult::Invalid(
                                    types::constants::link_validation_error::CONTRACT_LINK_DELETE_NOT_ARCHIVED
                                        .to_string(),
                                )),
                            },
                            Err(_) => Ok(ValidateCallbackResult::Invalid(
                                types::constants::link_validation_error::CONTRACT_LINK_DELETE_NOT_ARCHIVED.to_string(),
                            )),
                        }
                    }
                }
            }
            OpRecord::CreatePrivateEntry { .. } => Ok(ValidateCallbackResult::Valid),
            OpRecord::UpdatePrivateEntry { .. } => Ok(ValidateCallbackResult::Valid),
            OpRecord::CreateCapClaim { .. } => Ok(ValidateCallbackResult::Valid),
            OpRecord::CreateCapGrant { .. } => Ok(ValidateCallbackResult::Valid),
            OpRecord::UpdateCapClaim { .. } => Ok(ValidateCallbackResult::Valid),
            OpRecord::UpdateCapGrant { .. } => Ok(ValidateCallbackResult::Valid),
            OpRecord::Dna { .. } => Ok(ValidateCallbackResult::Valid),
            OpRecord::OpenChain { .. } => Ok(ValidateCallbackResult::Valid),
            OpRecord::CloseChain { .. } => Ok(ValidateCallbackResult::Valid),
            OpRecord::InitZomesComplete { .. } => Ok(ValidateCallbackResult::Valid),
            _ => Ok(ValidateCallbackResult::Valid),
        },
        FlatOp::RegisterAgentActivity(agent_activity) => match agent_activity {
            OpActivity::CreateAgent { agent, action } => {
                let previous_action = must_get_action(action.prev_action)?;
                match previous_action.action() {
                    Action::AgentValidationPkg(AgentValidationPkg { membrane_proof, .. }) => {
                        crate::validate_agent_joining(agent, membrane_proof)
                    }
                    _ => Ok(ValidateCallbackResult::Invalid(
                        link_validation_error::CREATE_AGENT_PREV_NOT_AVP.to_string(),
                    )),
                }
            }
            _ => Ok(ValidateCallbackResult::Valid),
        },
    }
}
