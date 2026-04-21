use hdk::prelude::*;
use std::{collections::BTreeMap, marker::PhantomData};
use transaction_integrity::types::RankingCursorTagType;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum GetRankingDirection {
    Ascendent,
    Descendent,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GetRankingCursor<T: RankingCursorTagType> {
    pub from_ranking: i64,
    pub tag: Option<SerializedBytes>,
    pub tag_type: PhantomData<T>,
    pub agent_pubkey: AgentPubKey,
}
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
pub struct HashWithTag {
    pub hash: AnyLinkableHash,
    pub tag: Option<SerializedBytes>,
    pub agents: Vec<AgentPubKey>,
}

pub struct RankingIndex {
    pub link_type: ScopedLinkType,
    pub index_interval: u64,
}

pub type Ranking = BTreeMap<i64, Vec<HashWithTag>>;
