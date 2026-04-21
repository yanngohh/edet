use hdk::prelude::*;
use transaction_integrity::types::constants::coordinator_wallet_error;
use transaction_integrity::*;

#[hdk_extern]
pub fn create_wallet(wallet: Wallet) -> ExternResult<Record> {
    let wallet_hash = create_entry(&EntryTypes::Wallet(wallet.clone()))?;
    let agent_hash: HoloHash<hash_type::Agent> = wallet.owner.to_owned().into();
    create_link(agent_hash, wallet_hash.clone(), LinkTypes::OwnerToWallet, ())?;
    let record = get(wallet_hash.clone(), GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_wallet_error::CREATED_WALLET_NOT_FOUND.to_string())))?;

    Ok(record)
}

#[hdk_extern]
pub fn get_latest_wallet(original_wallet_hash: ActionHash) -> ExternResult<Option<Record>> {
    let links =
        get_links(LinkQuery::try_new(original_wallet_hash.clone(), LinkTypes::WalletUpdates)?, GetStrategy::default())?;
    let latest_link = links
        .into_iter()
        .max_by(|link_a, link_b| link_a.timestamp.cmp(&link_b.timestamp));
    let latest_wallet_hash = match latest_link {
        Some(link) => link.target.clone().into_action_hash().ok_or(wasm_error!(WasmErrorInner::Guest(
            coordinator_wallet_error::WALLET_LINK_NO_ACTION_HASH.to_string()
        )))?,
        None => original_wallet_hash.clone(),
    };
    get(latest_wallet_hash, GetOptions::default())
}

#[hdk_extern]
pub fn get_original_wallet(original_wallet_hash: ActionHash) -> ExternResult<Option<Record>> {
    let Some(details) = get_details(original_wallet_hash, GetOptions::default())? else {
        return Ok(None);
    };
    match details {
        Details::Record(details) => Ok(Some(details.record)),
        _ => {
            Err(wasm_error!(WasmErrorInner::Guest(coordinator_wallet_error::WALLET_GET_DETAILS_MALFORMED.to_string())))
        }
    }
}

#[hdk_extern]
pub fn get_all_revisions_for_wallet(original_wallet_hash: ActionHash) -> ExternResult<Vec<Record>> {
    let Some(original_record) = get_original_wallet(original_wallet_hash.clone())? else {
        return Ok(vec![]);
    };
    let links =
        get_links(LinkQuery::try_new(original_wallet_hash.clone(), LinkTypes::WalletUpdates)?, GetStrategy::default())?;
    let get_input: Vec<GetInput> = links
        .into_iter()
        .map(|link| {
            Ok(GetInput::new(
                link.target
                    .into_action_hash()
                    .ok_or(wasm_error!(WasmErrorInner::Guest(
                        coordinator_wallet_error::WALLET_LINK_NO_ACTION_HASH.to_string()
                    )))?
                    .into(),
                GetOptions::default(),
            ))
        })
        .collect::<ExternResult<Vec<GetInput>>>()?;
    let records = HDK.with(|hdk| hdk.borrow().get(get_input))?;
    let mut records: Vec<Record> = records.into_iter().flatten().collect();
    records.insert(0, original_record);
    Ok(records)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UpdateWalletInput {
    pub original_wallet_hash: ActionHash,
    pub previous_wallet_hash: ActionHash,
    pub updated_wallet: Wallet,
}

#[hdk_extern]
pub fn update_wallet(input: UpdateWalletInput) -> ExternResult<Record> {
    let updated_wallet_hash = update_entry(input.previous_wallet_hash.clone(), &input.updated_wallet)?;
    create_link(input.original_wallet_hash.clone(), updated_wallet_hash.clone(), LinkTypes::WalletUpdates, ())?;
    let record = get(updated_wallet_hash.clone(), GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest(coordinator_wallet_error::UPDATED_WALLET_NOT_FOUND.to_string())))?;
    Ok(record)
}

#[hdk_extern]
pub fn get_wallet_for_agent(owner: AgentPubKey) -> ExternResult<(Option<ActionHash>, Option<Record>)> {
    match get_links(LinkQuery::try_new(owner, LinkTypes::OwnerToWallet)?, GetStrategy::default())?
        .first()
        .and_then(|wallet_to_owner_link| wallet_to_owner_link.target.to_owned().into_action_hash())
    {
        Some(original_wallet_hash) => {
            let record = get_latest_wallet(original_wallet_hash.to_owned())?;
            Ok((Some(original_wallet_hash), record))
        }
        None => Ok((None, None)),
    }
}
