use hdk::prelude::*;
use transaction_integrity::types::constants::*;
use transaction_integrity::*;

use super::sf_counters::compute_sf_counters;
use crate::trust_cache::get_cached_acquaintances;

// =========================================================================
//  Acquaintance Set Management (Whitepaper Definition 3)
//
//  A_i = { peers successfully transacted with } ∪ { self }
//  Grows through successful transactions, prunes on default.
// =========================================================================

/// Get the current agent's acquaintance set.
/// Uses caching for performance - cached data is valid within an epoch.
#[hdk_extern]
pub fn get_acquaintances(_: ()) -> ExternResult<Vec<AgentPubKey>> {
    get_cached_acquaintances()
}

/// Add a peer to the acquaintance set.
/// Enforces the Dunbar-style acquaintance cap (MAX_ACQUAINTANCES = 150).
/// When the cap is exceeded, the acquaintance with the lowest bilateral satisfaction
/// S_ij is evicted. Note: eviction only removes the AgentToAcquaintance link (affecting
/// the pre-trust vector p^(i)); underlying S/F counters are NOT deleted and continue
/// to contribute to the local trust row c_ij. This preserves correctness of Sybil isolation
/// (Corollary SubjectiveSybilResistance) because evicted S>0 peers still flow trust through
/// C^T; only the teleportation probability p^(i)_j is zeroed.
#[hdk_extern]
pub fn add_acquaintance(peer: AgentPubKey) -> ExternResult<()> {
    let agent = agent_info()?.agent_initial_pubkey;

    // Check if already an acquaintance
    let links = get_links(LinkQuery::try_new(agent.clone(), LinkTypes::AgentToAcquaintance)?, GetStrategy::default())?;
    let already_exists = links
        .iter()
        .any(|link| link.target.clone().into_agent_pub_key().is_some_and(|a| a == peer));

    if already_exists {
        return Ok(());
    }

    // Enforce acquaintance cap: if at or above MAX_ACQUAINTANCES, evict the weakest link
    if links.len() >= MAX_ACQUAINTANCES {
        // Compute S counters to find weakest acquaintance to evict
        let sf = compute_sf_counters(agent.clone())?;

        // Find the acquaintance with the lowest S_ij (weakest relationship)
        let mut min_s = f64::MAX;
        let mut weakest_link: Option<(ActionHash, AgentPubKey)> = None;

        for link in &links {
            if let Some(acq) = link.target.clone().into_agent_pub_key() {
                let acq_b64: AgentPubKeyB64 = acq.clone().into();
                let s_val = sf.get(&acq_b64).map_or(0.0, |c| c.satisfaction);
                if s_val < min_s {
                    min_s = s_val;
                    weakest_link = Some((link.create_link_hash.clone(), acq));
                }
            }
        }

        // Evict the weakest acquaintance
        if let Some((hash, _evicted)) = weakest_link {
            delete_link(hash, GetOptions::default())?;
        }
    }

    create_link(agent, peer, LinkTypes::AgentToAcquaintance, ())?;
    crate::trust_cache::invalidate_all_caches();

    Ok(())
}

/// Remove a peer from the acquaintance set.
pub fn remove_acquaintance(peer: AgentPubKey) -> ExternResult<()> {
    let agent = agent_info()?.agent_initial_pubkey;
    let links = get_links(LinkQuery::try_new(agent, LinkTypes::AgentToAcquaintance)?, GetStrategy::default())?;

    for link in links {
        if link.target.clone().into_agent_pub_key().is_some_and(|a| a == peer) {
            delete_link(link.create_link_hash, GetOptions::default())?;
        }
    }
    crate::trust_cache::invalidate_all_caches();

    Ok(())
}

/// Update acquaintance set based on S/F evidence.
/// Called after trust state changes (debt transfer, expiration).
/// Adds peers with positive S, removes peers where F > S.
pub fn update_acquaintances_from_evidence(
    creditor_transfers: &[(AgentPubKeyB64, f64)],
    creditor_failures: &[(AgentPubKeyB64, f64)],
) -> ExternResult<()> {
    let agent = agent_info()?.agent_initial_pubkey;

    // For transfers: ensure they are acquaintances
    for (debtor_key, _amount) in creditor_transfers {
        let debtor: AgentPubKey = debtor_key.clone().into();
        if debtor != agent {
            add_acquaintance(debtor)?;
        }
    }

    // For failures: just add them too as evidence of interaction.
    // The policy of *removing* based on F > S is now handled in publish_trust_row
    // during the proactive maintenance phase if desired.
    // For now, any evidence (positive or negative) creates an acquaintance
    // so that the relationship is tracked in the trust system.
    for (debtor_key, _amount) in creditor_failures {
        let debtor: AgentPubKey = debtor_key.clone().into();
        if debtor != agent {
            add_acquaintance(debtor)?;
        }
    }

    Ok(())
}
