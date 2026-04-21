use hdk::prelude::*;

use crate::capacity::compute_credit_capacity;
use crate::vouch::get_vouched_capacity;

// =========================================================================
//  Credit Capacity Wrappers (depend on trust + vouch modules)
//
//  The pure `compute_credit_capacity()` function lives in crate::capacity
//  to break the trust -> contracts -> vouch -> trust circular dependency.
//  These wrappers add DHT query logic on top.
// =========================================================================

/// Get the credit capacity of a specific agent.
#[hdk_extern]
pub fn get_credit_capacity(agent: AgentPubKey) -> ExternResult<f64> {
    let rep = super::get_subjective_reputation(agent.clone())?;
    let vouched = get_vouched_capacity(agent.clone())?;
    if vouched < 0.0 {
        warn!("get_credit_capacity: negative vouched capacity ({}) — clamping to 0", vouched);
    }
    let base = vouched.max(0.0);
    let cap = compute_credit_capacity(rep.trust, rep.acquaintance_count, base);
    if base == 0.0 && cap > 0.0 {
        debug!("GRADUATION: agent={} has earned reputation-based capacity even without being vouched (trust={}, acqs={}, vouched={}, result={})", agent, rep.trust, rep.acquaintance_count, vouched, cap);
    }
    Ok(cap)
}

/// Compute credit capacity for an agent (convenience wrapper).
/// Used by vouch.rs to check sponsor's available capacity.
pub fn compute_credit_capacity_for_agent(agent: AgentPubKey) -> ExternResult<f64> {
    get_credit_capacity(agent)
}
