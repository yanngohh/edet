use std::collections::BTreeMap;

mod types;
use hdk::prelude::*;
use transaction_integrity::{
    functions::tag_to_ranking,
    types::{constants::link_resolution_error, RankingCursorTagType, RankingTag},
};
pub use types::*;

/// Forked from https://github.com/holochain-open-dev/ranking-index
impl RankingIndex {
    pub fn new_with_default_mod(link_type: ScopedLinkType) -> Self {
        RankingIndex { link_type, index_interval: 100 }
    }

    /// Creates a new link between a path of the format
    /// `ranking_by_[RANKING_NAME].[INTERVAL_NUMBER]` and the specified
    /// entry with an optional custom tag.
    ///
    /// INTERVAL_NUMBER is the `ranking` as provided as argument divided
    /// by the `index_interval` of the [`RankingIndex`].
    ///
    /// If the path doesn't exist yet, it will be created on the fly.
    pub fn create_ranking(
        &self,
        hash: AnyLinkableHash,
        ranking: i64,
        tag: Option<SerializedBytes>,
        agents: Vec<AgentPubKey>,
    ) -> ExternResult<()> {
        let ranking_path = self.get_ranking_path(ranking);
        let typed_path = ranking_path.to_owned().into_typed(self.link_type);
        typed_path.ensure()?;

        create_link(typed_path.path_entry_hash()?, hash, self.link_type, ranking_to_tag(ranking, tag, agents)?)?;

        Ok(())
    }

    /// Deletes the link associated to an entry ranking for the specified entry.
    pub fn delete_ranking(&self, hash: AnyLinkableHash, ranking: i64) -> ExternResult<()> {
        // Get previous ranking
        let ranking_path = &self.get_ranking_path(ranking);
        let links = get_links(
            LinkQuery::new(
                ranking_path.path_entry_hash()?,
                LinkTypeFilter::Dependencies(vec![self.link_type.zome_index]),
            ),
            GetStrategy::default(),
        )?;

        let links_to_delete: Vec<ActionHash> = links
            .clone()
            .into_iter()
            .filter(|link| link.target.eq(&AnyLinkableHash::from(hash.clone())))
            .map(|link| link.create_link_hash)
            .collect();

        // Delete links for previous ranking
        for to_delete in links_to_delete {
            delete_link(to_delete, GetOptions::default())?;
        }

        Ok(())
    }

    /// Gets highest/lowest `count` ranked entries. The `direction` specifies
    /// whether to get the highest or the lowest ranked entries.
    ///
    /// The SQL analogue of `get_ranking_chunk(GetRankingDirection::Ascending, 10)`
    /// would be:
    /// `SELECT * FROM all_ranked_entries ORDER BY ranking ASC LIMIT 10`
    ///
    /// Optionally, a `cursor` can be specified in order to get the highest/lowest
    /// `count` ranked entries starting from the ranking specified in the cursor.
    ///
    /// The SQL analogue of
    /// `get_ranking_chunk(GetRankingDirection::Descending, 5, Some( GetRankingCursor { from_ranking: 350 }))`
    /// would be
    /// `WITH ranked_entries_subset AS (SELECT * FROM all_ranked_entries WHERE ranking < 350) SELECT * FROM ranked_entries_subset ORDER BY ranking DESC LIMIT 5`
    pub fn get_ranking_chunk<T: RankingCursorTagType>(
        &self,
        direction: GetRankingDirection,
        count: usize,
        cursor: Option<GetRankingCursor<T>>,
    ) -> ExternResult<Ranking> {
        let intervals = self.get_interval_paths()?;

        let mut ranking_map: Ranking = BTreeMap::new();
        let mut interval_index = initial_interval_index(&intervals, direction.clone(), cursor.clone()) as isize;

        let paths: Vec<&Path> = intervals.values().collect();

        while ranking_len(&ranking_map) < count && interval_index >= 0 && interval_index < intervals.len() as isize {
            let path_to_fetch = paths[interval_index as usize];
            let new_ranking = &self.get_ranking_from_interval_path(path_to_fetch)?;

            for (ranking, hashes) in new_ranking {
                for hash in hashes {
                    let is_inside_query_range =
                        is_inside_query_range(hash.clone(), *ranking, direction.clone(), cursor.clone());
                    if is_inside_query_range {
                        ranking_map
                            .entry(*ranking)
                            .and_modify(|hashes| {
                                hashes.retain(|existing| existing.hash != hash.hash);
                            })
                            .or_default()
                            .push(hash.clone());
                    }
                }
            }

            match direction {
                GetRankingDirection::Ascendent => {
                    interval_index += 1;
                }
                GetRankingDirection::Descendent => {
                    interval_index -= 1;
                }
            }
        }

        Ok(ranking_map)
    }

    fn get_interval_paths(&self) -> ExternResult<BTreeMap<i64, Path>> {
        let root_path = self.root_path().into_typed(self.link_type);
        let children_paths = root_path.children_paths()?;

        let mut interval_paths: BTreeMap<i64, Path> = BTreeMap::new();

        for path in children_paths {
            if let Some(component) = path.leaf() {
                if let Ok(ranking) = component_to_ranking(component) {
                    interval_paths.insert(ranking, path.path);
                }
            }
        }

        Ok(interval_paths)
    }

    fn get_ranking_from_interval_path(&self, interval_path: &Path) -> ExternResult<Ranking> {
        let links = get_links(
            LinkQuery::new(
                interval_path.path_entry_hash()?,
                LinkTypeFilter::Dependencies(vec![self.link_type.zome_index]),
            ),
            GetStrategy::default(),
        )?;

        let ranking = links
            .into_iter()
            .map(|link| {
                let ranking = tag_to_ranking(link.tag)?;
                Ok((ranking.0, link.target, ranking.1, ranking.2))
            })
            .collect::<ExternResult<Vec<(i64, AnyLinkableHash, Option<SerializedBytes>, Vec<AgentPubKey>)>>>()?;

        let mut ranking_map: Ranking = BTreeMap::new();

        for (ranking, hash, custom_tag, agents) in ranking {
            ranking_map.entry(ranking).or_default().push(HashWithTag {
                hash: AnyLinkableHash::from(hash),
                tag: custom_tag,
                agents,
            });
        }

        Ok(ranking_map)
    }

    fn ranking_interval(&self, ranking: i64) -> i64 {
        // Use Euclidean division so that negative rankings floor toward negative infinity
        // rather than truncating toward zero. Without this, negative rankings (e.g. -1)
        // would land in bucket 0 (same as 0..index_interval), colliding with positive
        // small-value rankings and corrupting descendent pagination.
        // NOTE: In production, rankings are unix timestamps in milliseconds and are always
        // positive. This fix is a safety measure for future-proofing and test scenarios.
        ranking.div_euclid(self.index_interval as i64)
    }

    fn get_ranking_path(&self, ranking: i64) -> Path {
        let mut path = Path::from(self.root_path_str());
        let ranking_interval: Component = self.ranking_interval(ranking).to_string().into();
        path.append_component(ranking_interval);
        path
    }

    fn root_path(&self) -> Path {
        Path::from(self.root_path_str())
    }

    fn root_path_str(&self) -> String {
        format!("ranking_index_{}_{:?}", self.link_type.zome_index, self.link_type.zome_type)
    }
}

fn ranking_to_tag(
    ranking: i64,
    custom_tag: Option<SerializedBytes>,
    agents: Vec<AgentPubKey>,
) -> ExternResult<LinkTag> {
    let bytes = SerializedBytes::try_from(RankingTag { ranking, custom_tag, agents })
        .map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;

    Ok(LinkTag(bytes.bytes().clone()))
}

fn component_to_ranking(c: &Component) -> ExternResult<i64> {
    let s = String::try_from(c).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))?;
    let ranking = s
        .parse::<i64>()
        .map_err(|_| wasm_error!(WasmErrorInner::Guest(link_resolution_error::BAD_RANKING_COMPONENT.to_string())))?;

    Ok(ranking)
}

fn ranking_len(ranking: &Ranking) -> usize {
    ranking.values().fold(0, |acc, next| acc + next.len())
}

fn initial_interval_index<T: RankingCursorTagType>(
    interval_paths: &BTreeMap<i64, Path>,
    direction: GetRankingDirection,
    maybe_cursor: Option<GetRankingCursor<T>>,
) -> usize {
    match maybe_cursor {
        None => match direction {
            GetRankingDirection::Ascendent => 0,
            GetRankingDirection::Descendent => {
                if interval_paths.is_empty() {
                    0
                } else {
                    interval_paths.len() - 1
                }
            }
        },
        Some(cursor) => {
            let ordered_keys: Vec<i64> = interval_paths.keys().cloned().collect();
            if !interval_paths.is_empty() {
                for i in 0..(interval_paths.len() - 1) {
                    if ordered_keys[i] <= cursor.from_ranking && cursor.from_ranking < ordered_keys[i + 1] {
                        return i;
                    }
                }
            }
            match direction {
                GetRankingDirection::Ascendent => 0,
                GetRankingDirection::Descendent => {
                    if interval_paths.is_empty() {
                        0
                    } else {
                        interval_paths.len() - 1
                    }
                }
            }
        }
    }
}

fn is_inside_query_range<T: RankingCursorTagType>(
    hash: HashWithTag,
    ranking: i64,
    direction: GetRankingDirection,
    maybe_cursor: Option<GetRankingCursor<T>>,
) -> bool {
    match maybe_cursor {
        None => true,
        Some(cursor) => {
            let from_ranking = cursor.from_ranking;
            let matches_from_ranking = match direction {
                GetRankingDirection::Ascendent => ranking >= from_ranking,
                GetRankingDirection::Descendent => ranking <= from_ranking,
            };
            let matches_agent = hash.agents.contains(&cursor.agent_pubkey);
            match (cursor.tag, hash.tag) {
                (Some(cursor_bytes), Some(hash_bytes)) => {
                    let cursor_tag: Option<T> = cursor_bytes.try_into().ok();
                    let hash_tag: Option<T> = hash_bytes.try_into().ok();
                    match (cursor_tag, hash_tag) {
                        // Both the tag filter AND the from_ranking cursor boundary must be
                        // satisfied. Previously `matches_from_ranking` was omitted here,
                        // which meant that when a tag filter was active the cursor position
                        // was ignored and all matching entries were returned on every page.
                        (Some(cursor_tag), Some(hash_tag)) => {
                            cursor_tag == hash_tag && matches_agent && matches_from_ranking
                        }
                        _ => false,
                    }
                }
                _ => matches_from_ranking && matches_agent,
            }
        }
    }
}
