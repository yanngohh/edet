pub mod capacity;
pub mod checkpoint;
pub mod contracts;
mod ranking_index;
pub mod support_cascade;
pub mod transaction;
pub mod trust;
pub mod trust_cache;
mod types;
pub mod vouch;
pub mod wallet;

#[cfg(not(debug_assertions))]
#[macro_use]
pub mod macros {
    #[macro_export]
    macro_rules! debug {
        ($($arg:tt)*) => { if false { let _ = ::std::format_args!($($arg)*); } };
    }
    #[macro_export]
    macro_rules! info {
        ($($arg:tt)*) => { if false { let _ = ::std::format_args!($($arg)*); } };
    }
    #[macro_export]
    macro_rules! warn {
        ($($arg:tt)*) => { if false { let _ = ::std::format_args!($($arg)*); } };
    }
    #[macro_export]
    macro_rules! error {
        ($($arg:tt)*) => { if false { let _ = ::std::format_args!($($arg)*); } };
    }
}
use hdk::prelude::*;
use transaction_integrity::types::constants::link_resolution_error;
use transaction_integrity::*;
use wallet::{create_wallet, get_wallet_for_agent};

#[hdk_extern]
pub fn init() -> ExternResult<InitCallbackResult> {
    let agent_info = agent_info()?;
    let owner = agent_info.agent_initial_pubkey;
    if let (_, None) = get_wallet_for_agent(owner.to_owned())? {
        create_wallet(Wallet::new(&owner.to_owned().into()))?;
    }

    // grant unrestricted access to accept_cap_claim so other agents can send us claims
    // NOTE ON UNRESTRICTED ACCESS:
    // These grants use CapAccess::Unrestricted because callers are not known at init time
    // (any peer in the network may need to call these). This is the standard Holochain
    // pattern for peer-to-peer remote calls. Security relies on handler-level validation:
    //  - create_drain_request: validates the caller holds a valid SupportBreakdown naming them
    //  - notify_buyer/seller_of_accepted_transaction: validates caller is the expected
    //    buyer/seller on the transaction entry
    //  - notify_trust_row_refresh: triggers a local-only republication, no state mutation
    //  - receive_vouch_slash: validates the expired contract hash and debtor identity
    // All handlers are therefore safe to call by arbitrary peers — the worst a malicious
    // caller can do is trigger a no-op (trust refresh) or receive an authorization error.
    let mut fns = std::collections::HashSet::new();
    // Grant access for support cascade: other agents (sellers) can request
    // that we drain our own debt as part of their support breakdown.
    fns.insert((zome_info()?.name, "create_drain_request".into()));
    // Grant access for buyer contract creation: when a seller approves a
    // Pending transaction, they notify the buyer to run their side of
    // side-effects (contract creation for purchases, satisfaction for drains).
    fns.insert((zome_info()?.name, "notify_buyer_of_accepted_transaction".into()));
    // Grant access for seller-side effects: when a buyer creates an auto-accepted
    // (non-trial) transaction, they notify the seller to run cascade + acquaintances.
    fns.insert((zome_info()?.name, "notify_seller_of_accepted_transaction".into()));
    // Grant access for trust row refresh: after a drain cascade transfers debt,
    // the beneficiary notifies affected creditors to republish their trust rows
    // so the EigenTrust bidirectional trust loop is established.
    fns.insert((zome_info()?.name, "notify_trust_row_refresh".into()));
    // Grant access for vouch slashing: when a debtor's contract expires, the
    // debtor dispatches slash requests to each sponsor via call_remote. The
    // sponsor processes the slash on their own source chain.
    fns.insert((zome_info()?.name, "receive_vouch_slash".into()));
    let functions: GrantedFunctions = hdk::prelude::GrantedFunctions::Listed(fns);

    create_cap_grant(CapGrantEntry {
        tag: "".into(),
        // empty access converts to unrestricted
        access: ().into(),
        functions,
    })?;

    Ok(InitCallbackResult::Pass)
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum Signal {
    LinkCreated { action: SignedActionHashed, link_type: LinkTypes },
    LinkDeleted { action: SignedActionHashed, create_link_action: SignedActionHashed, link_type: LinkTypes },
    EntryCreated { action: SignedActionHashed, app_entry: EntryTypes },
    EntryUpdated { action: SignedActionHashed, app_entry: EntryTypes, original_app_entry: Box<EntryTypes> },
    EntryDeleted { action: SignedActionHashed, original_app_entry: EntryTypes },
}

#[hdk_extern(infallible)]
pub fn post_commit(committed_actions: Vec<SignedActionHashed>) {
    for action in committed_actions {
        if let Err(err) = signal_action(action.clone()) {
            error!("Error signaling new action: {:?}", err);
        }
    }
}

fn signal_action(action: SignedActionHashed) -> ExternResult<()> {
    match action.hashed.content.clone() {
        Action::CreateLink(create_link) => {
            if let Ok(Some(link_type)) = LinkTypes::from_type(create_link.zome_index, create_link.link_type) {
                emit_signal(Signal::LinkCreated { action, link_type })?;
            }
            Ok(())
        }
        Action::DeleteLink(delete_link) => {
            let record = get(delete_link.link_add_address.clone(), GetOptions::default())?.ok_or(wasm_error!(
                WasmErrorInner::Guest(link_resolution_error::FETCH_CREATE_LINK_FAILED.to_string())
            ))?;
            match record.action() {
                Action::CreateLink(create_link) => {
                    if let Ok(Some(link_type)) = LinkTypes::from_type(create_link.zome_index, create_link.link_type) {
                        emit_signal(Signal::LinkDeleted {
                            action,
                            link_type,
                            create_link_action: record.signed_action.clone(),
                        })?;
                    }
                    Ok(())
                }
                _ => Err(wasm_error!(WasmErrorInner::Guest(link_resolution_error::CREATE_LINK_NOT_FOUND.to_string()))),
            }
        }
        Action::Create(_create) => {
            if let Ok(Some(app_entry)) = get_entry_for_action(&action.hashed.hash) {
                emit_signal(Signal::EntryCreated { action, app_entry })?;
            }
            Ok(())
        }
        Action::Update(update) => {
            if let Ok(Some(app_entry)) = get_entry_for_action(&action.hashed.hash) {
                if let Ok(Some(original_app_entry)) = get_entry_for_action(&update.original_action_address) {
                    emit_signal(Signal::EntryUpdated {
                        action,
                        app_entry,
                        original_app_entry: Box::new(original_app_entry),
                    })?;
                }
            }
            Ok(())
        }
        Action::Delete(delete) => {
            if let Ok(Some(original_app_entry)) = get_entry_for_action(&delete.deletes_address) {
                emit_signal(Signal::EntryDeleted { action, original_app_entry })?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn get_entry_for_action(action_hash: &ActionHash) -> ExternResult<Option<EntryTypes>> {
    let record = match get_details(action_hash.clone(), GetOptions::default())? {
        Some(Details::Record(record_details)) => record_details.record,
        _ => return Ok(None),
    };
    let entry = match record.entry().as_option() {
        Some(entry) => entry,
        None => return Ok(None),
    };
    let (zome_index, entry_index) = match record.action().entry_type() {
        Some(EntryType::App(AppEntryDef { zome_index, entry_index, .. })) => (zome_index, entry_index),
        _ => return Ok(None),
    };
    EntryTypes::deserialize_from_type(*zome_index, *entry_index, entry)
}
