use hdi::chain::must_get_agent_activity;
use hdi::prelude::{wasm_error, ExternResult, LinkTag, SerializedBytes, UnsafeBytes};
use hdi::prelude::{ActionHash, AgentPubKey, ChainFilter, EntryType, RegisterAgentActivity, WasmErrorInner};

// use crate::types::transaction_validation_error;
use crate::types;

pub fn tag_to_ranking(tag: LinkTag) -> ExternResult<(i64, Option<SerializedBytes>, Vec<AgentPubKey>)> {
    let bytes = tag.into_inner();
    let sb = SerializedBytes::from(UnsafeBytes::from(bytes));

    let ranking = types::RankingTag::try_from(sb).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    Ok((ranking.ranking, ranking.custom_tag, ranking.agents))
}

pub fn previous_activity_matches<F, T>(
    author: AgentPubKey,
    chain_top: ActionHash,
    entry_type: Option<EntryType>,
    find_map: F,
) -> ExternResult<T>
where
    F: FnMut((usize, &RegisterAgentActivity)) -> Option<T>,
    T: Default,
{
    Ok(must_get_agent_activity(author, ChainFilter::new(chain_top))?
        .iter()
        .filter(|activity| {
            activity.action.action().entry_type().is_some_and(|action_entry_type| {
                entry_type.as_ref().is_none_or(|entry_type| action_entry_type == entry_type)
            })
        })
        .enumerate()
        .find_map(find_map)
        .unwrap_or_default())
}
