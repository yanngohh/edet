use hdk::prelude::*;
use support_integrity::*;

#[hdk_extern]
pub fn create_support_breakdown(support_breakdown: SupportBreakdown) -> ExternResult<Record> {
    info!("SUPPORT: create_support_breakdown for owner={}", support_breakdown.owner);
    let support_breakdown_hash = create_entry(&EntryTypes::SupportBreakdown(support_breakdown.clone()))?;
    let agent_hash: HoloHash<hash_type::Agent> = support_breakdown.owner.into();
    create_link(agent_hash, support_breakdown_hash.clone(), LinkTypes::OwnerToSupportBreakdown, ())?;
    for base in support_breakdown.addresses.clone() {
        let base: AgentPubKey = base.into();
        create_link(base, support_breakdown_hash.clone(), LinkTypes::AddressToSupportBreakdowns, ())?;
    }
    let record = get(support_breakdown_hash.clone(), GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest("Could not find the newly created SupportBreakdown".to_string())))?;
    Ok(record)
}

#[hdk_extern]
pub fn get_latest_support_breakdown(original_support_breakdown_hash: ActionHash) -> ExternResult<Option<Record>> {
    let links = get_links(
        LinkQuery::try_new(original_support_breakdown_hash.clone(), LinkTypes::SupportBreakdownUpdates)?,
        GetStrategy::default(),
    )?;
    let latest_link = links
        .into_iter()
        .max_by(|link_a, link_b| link_a.timestamp.cmp(&link_b.timestamp));
    let latest_support_breakdown_hash = match latest_link {
        Some(link) => link
            .target
            .clone()
            .into_action_hash()
            .ok_or(wasm_error!(WasmErrorInner::Guest("No action hash associated with link".to_string())))?,
        None => original_support_breakdown_hash.clone(),
    };
    get(latest_support_breakdown_hash, GetOptions::default())
}

#[hdk_extern]
pub fn get_original_support_breakdown(original_support_breakdown_hash: ActionHash) -> ExternResult<Option<Record>> {
    let Some(details) = get_details(original_support_breakdown_hash, GetOptions::default())? else {
        return Ok(None);
    };
    match details {
        Details::Record(details) => Ok(Some(details.record)),
        _ => Err(wasm_error!(WasmErrorInner::Guest("Malformed get details response".to_string()))),
    }
}

#[hdk_extern]
pub fn get_all_revisions_for_support_breakdown(
    original_support_breakdown_hash: ActionHash,
) -> ExternResult<Vec<Record>> {
    let Some(original_record) = get_original_support_breakdown(original_support_breakdown_hash.clone())? else {
        return Ok(vec![]);
    };
    let links = get_links(
        LinkQuery::try_new(original_support_breakdown_hash.clone(), LinkTypes::SupportBreakdownUpdates)?,
        GetStrategy::default(),
    )?;
    let get_input: Vec<GetInput> = links
        .into_iter()
        .map(|link| {
            Ok(GetInput::new(
                link.target
                    .into_action_hash()
                    .ok_or(wasm_error!(WasmErrorInner::Guest("No action hash associated with link".to_string())))?
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
pub struct UpdateSupportBreakdownInput {
    pub original_support_breakdown_hash: ActionHash,
    pub previous_support_breakdown_hash: ActionHash,
    pub updated_support_breakdown: SupportBreakdown,
}

#[hdk_extern]
pub fn update_support_breakdown(input: UpdateSupportBreakdownInput) -> ExternResult<Record> {
    // AddressToSupportBreakdowns links are append-only (integrity validation forbids deletion).
    // New links for every address in the updated breakdown are created below; stale links
    // from previous versions naturally accumulate but are filtered out at read time by
    // `get_support_breakdown_for_address`, which resolves any linked version to the latest
    // via the SupportBreakdownUpdates chain and checks that the queried address is still
    // present in the current breakdown before returning.

    let updated_support_breakdown_hash =
        update_entry(input.previous_support_breakdown_hash.clone(), &input.updated_support_breakdown)?;
    create_link(
        input.original_support_breakdown_hash.clone(),
        updated_support_breakdown_hash.clone(),
        LinkTypes::SupportBreakdownUpdates,
        (),
    )?;
    for base in input.updated_support_breakdown.addresses.clone() {
        let base: AgentPubKey = base.into();
        create_link(base, updated_support_breakdown_hash.clone(), LinkTypes::AddressToSupportBreakdowns, ())?;
    }
    let record = get(updated_support_breakdown_hash.clone(), GetOptions::default())?
        .ok_or(wasm_error!(WasmErrorInner::Guest("Could not find the newly updated SupportBreakdown".to_string())))?;
    Ok(record)
}

#[hdk_extern]
pub fn get_support_breakdown_for_address(address: AgentPubKey) -> ExternResult<Vec<Record>> {
    let address_b64: AgentPubKeyB64 = address.clone().into();
    let links =
        get_links(LinkQuery::try_new(address.clone(), LinkTypes::AddressToSupportBreakdowns)?, GetStrategy::default())?;

    // AddressToSupportBreakdowns links are append-only: old links from superseded breakdown
    // versions are never deleted. We must therefore:
    //  1. For each link, resolve the target to the *latest* version of its breakdown via
    //     the SupportBreakdownUpdates chain (starting from the original create hash).
    //  2. Check that the queried `address` is still present in the latest version.
    //     If it was removed in an update, the link is stale and must be ignored.
    //  3. De-duplicate: multiple links may point to different versions of the same breakdown;
    //     return at most one record per breakdown (the latest).

    // Collect (original_hash → latest_record) for all breakdowns this address appears in.
    // We key by original hash to avoid emitting the same breakdown twice when multiple
    // links exist for the same original (create + update links both present).
    let mut seen_originals: std::collections::HashMap<ActionHash, Record> = std::collections::HashMap::new();

    for link in links {
        let Some(linked_hash) = link.target.clone().into_action_hash() else { continue };

        // Step 1: find the original create hash and the latest version.
        // Attempt to get details on the linked hash to discover if it is a create or update.
        // The SupportBreakdownUpdates links go FROM the original hash TO each update hash,
        // so we need to walk backwards if the linked hash is an update.
        // Simpler approach: fetch the record; if it is an Update action, resolve its
        // original hash via the action's `original_action_address`.
        let Some(linked_record) = get(linked_hash.clone(), GetOptions::default())? else { continue };

        let original_hash: ActionHash = match linked_record.action() {
            Action::Update(update) => update.original_action_address.clone(),
            Action::Create(_) => linked_hash.clone(),
            _ => continue,
        };

        if seen_originals.contains_key(&original_hash) {
            continue; // Already resolved this breakdown — skip duplicate links.
        }

        // Step 2: resolve to the latest version via SupportBreakdownUpdates.
        let latest_record = {
            let update_links = get_links(
                LinkQuery::try_new(original_hash.clone(), LinkTypes::SupportBreakdownUpdates)?,
                GetStrategy::default(),
            )?;
            if let Some(latest_link) = update_links.into_iter().max_by(|a, b| a.timestamp.cmp(&b.timestamp)) {
                if let Some(latest_hash) = latest_link.target.into_action_hash() {
                    get(latest_hash, GetOptions::default())?.unwrap_or(linked_record)
                } else {
                    linked_record
                }
            } else {
                linked_record // No updates — the create IS the latest.
            }
        };

        // Step 3: check that the queried address is still listed in the latest version.
        let Some(latest_bd) = latest_record.entry().to_app_option::<SupportBreakdown>().ok().flatten() else {
            continue;
        };
        if latest_bd.addresses.contains(&address_b64) {
            seen_originals.insert(original_hash, latest_record);
        }
        // If address was removed in an update, the link is stale — do not include.
    }

    Ok(seen_originals.into_values().collect())
}

#[hdk_extern]
pub fn get_support_breakdown_for_owner(owner: AgentPubKey) -> ExternResult<(Option<ActionHash>, Option<Record>)> {
    let owner_b64: AgentPubKeyB64 = owner.clone().into();
    info!("SUPPORT: get_support_breakdown_for_owner: looking up breakdown for {}", owner_b64);
    match get_links(LinkQuery::try_new(owner, LinkTypes::OwnerToSupportBreakdown)?, GetStrategy::default())?
        .first()
        .and_then(|support_breakdown_to_owner_link| {
            support_breakdown_to_owner_link.target.to_owned().into_action_hash()
        }) {
        Some(original_support_breakdown_hash) => {
            info!("SUPPORT: found breakdown link for {} -> AH({})", owner_b64, original_support_breakdown_hash);
            let record = get_latest_support_breakdown(original_support_breakdown_hash.to_owned())?;
            Ok((Some(original_support_breakdown_hash), record))
        }
        None => {
            info!("SUPPORT: no breakdown link found for {}", owner_b64);
            Ok((None, None))
        }
    }
}
