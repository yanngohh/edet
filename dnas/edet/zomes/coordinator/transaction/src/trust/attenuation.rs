use std::collections::HashMap;

use hdk::prelude::*;
use transaction_integrity::types::constants::*;
use transaction_integrity::VouchStatus;

use super::sf_counters::SFCounters;

// =========================================================================
//  Trust Attenuation (Whitepaper Definition 5/6)
//
//  phi(r) = max(0, 1 - (r / tau)^gamma)
//  s_ij = S_ij * phi(r_ij)
// =========================================================================

/// Compute the effective bilateral volume capped by age-weighted volume maturation.
/// n_mat_eff = min(n_ij, MAX_VOLUME_PER_EPOCH × max(1, age_epochs))
///
/// This prevents wash-trading from accelerating tolerance growth beyond real time.
fn compute_n_mat_eff(bilateral_volume: f64, first_seen_epoch: u64) -> f64 {
    if first_seen_epoch < u64::MAX {
        let age: f64 = if let Ok(now) = sys_time() {
            let current_epoch = transaction_integrity::types::timestamp_to_epoch(now);
            (current_epoch.saturating_sub(first_seen_epoch) as f64).max(1.0)
        } else {
            // sys_time() should never fail on a healthy Holochain node.
            // Fall back conservatively to age = 1 epoch so that the maturation cap
            // remains tight (n_mat_eff = MAX_VOLUME_PER_EPOCH × 1) rather than
            // granting full maturation based on volume alone, which would allow
            // wash-trading to bypass the time-weighted tolerance cap.
            1.0_f64
        };
        bilateral_volume.min(MAX_VOLUME_PER_EPOCH * age)
    } else {
        bilateral_volume
    }
}

/// Trust attenuation function phi(r, n_ij) per Definition 5 of the whitepaper.
/// Uses bilateral volume-scaled failure tolerance: new relationships face
/// stricter thresholds, scaling logarithmically with interaction volume.
///
/// tau_eff(n_ij) = TAU_NEWCOMER + (TAU - TAU_NEWCOMER) * min(ln(1+n_ij) / ln(1+N_mat), 1)
/// phi(r, n_ij) = max(0, 1 - (r / tau_eff)^gamma)
///
/// `first_seen_epoch` is the earliest epoch we observed the debtor.
/// Used to enforce time-weighted maturation: n_mat_eff = min(n_ij, MAX_VOLUME_PER_EPOCH * age).
/// This prevents wash-trading from accelerating tolerance growth beyond the passage of real time.
/// Matches sim/universe.py: compute_local_trust_row() age-cap on n_mat_eff.
pub fn trust_attenuation(
    failure_rate: f64,
    bilateral_volume: f64,
    first_seen_epoch: u64,
    recent_s: f64,
    recent_f: f64,
) -> f64 {
    let n_mat_eff = compute_n_mat_eff(bilateral_volume, first_seen_epoch);

    let vol_ratio = ((1.0 + n_mat_eff).ln() / (1.0 + VOLUME_MATURATION_THRESHOLD).ln()).min(1.0);
    let tau_eff = TAU_NEWCOMER + (FAILURE_TOLERANCE - TAU_NEWCOMER) * vol_ratio;
    if tau_eff <= 0.0 {
        return if failure_rate > 0.0 { 0.0 } else { 1.0 };
    }

    // Recent failure rate window (behavioral switch detection)
    let r_recent = {
        let win_total = recent_s + recent_f;
        if win_total > 0.0 {
            recent_f / win_total
        } else {
            0.0
        }
    };
    let r_eff = failure_rate.max(RECENT_WEIGHT * r_recent);

    let ratio = r_eff / tau_eff;
    let phi = (1.0 - ratio.powf(PENALTY_SHARPNESS)).max(0.0);
    // Downgraded from info! to debug!: this function is called for every
    // (creditor, debtor) pair in the trust subgraph — potentially 150+ times per
    // reputation query. info! level would flood production logs even with the
    // cfg(debug_assertions) gate disabled, because Holochain's conductor may
    // forward info-level WASM logs to the operator console.
    debug!(
        "Trust attenuation: debtor={:?} r_eff={:.4}, tau_eff={:.4}, phi={:.4} (n_mat_eff={:.1})",
        "??", // we don't have debtor here, but r_eff is enough
        r_eff,
        tau_eff,
        phi,
        n_mat_eff
    );
    phi
}

/// Trust attenuation with witness-based contagion.
/// tau_eff' = tau_eff / (1 + k * witnesses)
/// where k = CONTAGION_WITNESS_FACTOR (default 0.25)
///
/// Additionally applies the aggregate witness rate floor:
/// r_eff = max(r_bilateral, d_w * r_witness_median)
/// where d_w = WITNESS_DISCOUNT. This closes the selective defaulting gap:
/// when bilateral r = 0, the median of witnesses' rates provides a nonzero floor.
///
/// `first_seen_epoch` is used for age-based volume maturation cap (same as trust_attenuation).
/// `aggregate_witness_rate` is the median bilateral F/(S+F) across failure witnesses.
pub fn trust_attenuation_with_contagion(
    failure_rate: f64,
    bilateral_volume: f64,
    witness_count: u32,
    first_seen_epoch: u64,
    recent_s: f64,
    recent_f: f64,
    aggregate_witness_rate: f64,
) -> f64 {
    let n_mat_eff = compute_n_mat_eff(bilateral_volume, first_seen_epoch);

    let vol_ratio = ((1.0 + n_mat_eff).ln() / (1.0 + VOLUME_MATURATION_THRESHOLD).ln()).min(1.0);
    let base_tau_eff = TAU_NEWCOMER + (FAILURE_TOLERANCE - TAU_NEWCOMER) * vol_ratio;

    // Apply contagion penalty: more witnesses = stricter tolerance
    let contagion_factor = 1.0 + CONTAGION_WITNESS_FACTOR * (witness_count as f64);
    let tau_eff = base_tau_eff / contagion_factor;

    // Recent failure rate window (behavioral switch detection)
    let r_recent = {
        let win_total = recent_s + recent_f;
        if win_total > 0.0 {
            recent_f / win_total
        } else {
            0.0
        }
    };
    let mut r_eff = failure_rate.max(RECENT_WEIGHT * r_recent);

    // Aggregate witness contagion floor
    if aggregate_witness_rate > 0.0 {
        r_eff = r_eff.max(WITNESS_DISCOUNT * aggregate_witness_rate);
    }

    let ratio = r_eff / tau_eff;
    let phi = (1.0 - ratio.powf(PENALTY_SHARPNESS)).max(0.0);
    debug!(
        "Trust attenuation (contagion): r_eff={:.4}, tau_eff={:.4}, phi={:.4} (witnesses={}, witness_rate={:.4})",
        r_eff, tau_eff, phi, witness_count, aggregate_witness_rate
    );
    phi
}

/// Compute the normalized local trust row from pre-computed SF counters.
/// Returns { debtor => c_ij } where c_ij = w_i * (s_ij / Sigma_i) + (1-w_i) * p_j.
///
/// w_i = min(1, Sigma_i / N_mat) where Sigma_i = sum(s_ij) is the sum of attenuated scores
/// (Whitepaper Definition 3.7: Confidence-Weighted Local Trust)
pub fn compute_local_trust_row_from_sf(
    sf_counters: &HashMap<AgentPubKeyB64, SFCounters>,
) -> ExternResult<HashMap<AgentPubKeyB64, f64>> {
    let mut raw_trust: HashMap<AgentPubKeyB64, f64> = HashMap::new();
    let mut total_mass = 0.0;

    // Build raw attenuated trust
    for (debtor, counters) in sf_counters {
        let total_interactions = counters.satisfaction + counters.failure;

        if total_interactions > 0.0 && counters.satisfaction > 0.0 {
            let failure_rate = counters.failure / total_interactions;
            let phi = trust_attenuation(
                failure_rate,
                total_interactions,
                counters.first_seen_epoch,
                counters.recent_satisfaction,
                counters.recent_failure,
            );

            // Note: f_bank capping is retained for Trust Banking mitigation
            let s_eff = counters
                .satisfaction
                .min(VOLUME_MATURATION_THRESHOLD * TRUST_BANKING_BOUND_FRACTION);
            let score = s_eff * phi;
            if score > 0.0 {
                raw_trust.insert(debtor.clone(), score);
                total_mass += score;
            }
        }
    }

    // Confidence-Weighted Trust (Whitepaper Definition 3.7):
    // w_i = min(1, Sigma_i / N_mat) where Sigma_i = sum of attenuated scores s_ij
    // Using total_mass (sum of s_ij) correctly matches the whitepaper definition.
    // Nodes undergoing trust collapse (rising failures -> phi -> 0) see total_mass
    // shrink, causing w_i to fall and pre-trust weight to increase — the intended behaviour.
    let w_i = (total_mass / VOLUME_MATURATION_THRESHOLD).min(1.0);
    let pre_trust_weight = 1.0 - w_i;

    let mut normalized: HashMap<AgentPubKeyB64, f64> = HashMap::new();

    // 1. Add weighted local evidence
    if total_mass > 0.0 && w_i > 0.0 {
        for (debtor, score) in &raw_trust {
            normalized.insert(debtor.clone(), w_i * (score / total_mass));
        }
    }

    // 2. Add weighted pre-trust baseline
    if pre_trust_weight > 0.0 {
        let p = super::get_pre_trust_distribution(None)?; // Uniform for now if no observer context
        for (debtor, p_val) in p {
            let current = normalized.get(&debtor).unwrap_or(&0.0);
            normalized.insert(debtor, current + pre_trust_weight * p_val);
        }
    }

    Ok(normalized)
}

/// Compute the normalized local trust row from SF counters with witness-based contagion.
/// This variant queries the DHT for failure witnesses and applies contagion penalties.
/// More expensive than the basic version due to DHT lookups.
///
/// w_i = min(1, Sigma_i / N_mat) where Sigma_i = sum(s_ij) is the sum of attenuated scores
/// (Whitepaper Definition 3.7: Confidence-Weighted Local Trust)
pub fn compute_local_trust_row_from_sf_with_contagion(
    sf_counters: &HashMap<AgentPubKeyB64, SFCounters>,
) -> ExternResult<HashMap<AgentPubKeyB64, f64>> {
    let mut raw_trust: HashMap<AgentPubKeyB64, f64> = HashMap::new();
    let mut total_mass = 0.0;

    // Build raw attenuated trust
    for (debtor, counters) in sf_counters {
        let total_interactions = (counters.satisfaction + counters.failure).max(0.0);

        if total_interactions > 0.0 {
            let failure_rate = if total_interactions > 0.0 { counters.failure / total_interactions } else { 1.0 };

            let debtor_agent: AgentPubKey = debtor.clone().into();
            let (witness_count, agg_rate) =
                crate::trust_cache::get_cached_witness_contagion(&debtor_agent).unwrap_or((0, 0.0));

            let phi = trust_attenuation_with_contagion(
                failure_rate,
                total_interactions,
                witness_count,
                counters.first_seen_epoch,
                counters.recent_satisfaction,
                counters.recent_failure,
                agg_rate,
            );

            // S_eff is the interaction score base. If satisfaction is 0, this is 0.
            let s_eff = counters
                .satisfaction
                .min(VOLUME_MATURATION_THRESHOLD * TRUST_BANKING_BOUND_FRACTION);
            let score = s_eff * phi;

            // Store attenuation factor so it can be applied to vouches later
            raw_trust.insert(debtor.clone(), score);
            // We'll store phi briefly to apply to vouches if S=0.
            // Better: just apply it to everything in the same map.
            total_mass += score;
        }
    }

    // ── VOUCH AS LOCAL TRUST (Whitepaper §5.1) ──
    // A vouch is an explicit credit link that contributes to the local trust row Sigma_i.
    // It is ALSO subject to behavioral attenuation if the agent has interactions.
    // Each Active vouch contributes VOUCH_TRUST_BASE × φ(vouchee_failure_rate) to the
    // sponsor's total mass. This is summed (not maxed) with any SF-based score so that
    // sponsors who also traded directly with the vouchee accumulate stronger endorsement.
    let vouches = crate::vouch::get_vouches_given(())?;
    for record in vouches {
        if record.vouch.status == VouchStatus::Active {
            let entrant_key: AgentPubKeyB64 = record.vouch.entrant.clone().into();

            // Re-calculate phi for the vouchee if they have interactions
            let phi = if let Some(counters) = sf_counters.get(&entrant_key) {
                let total_interactions = (counters.satisfaction + counters.failure).max(0.0);
                if total_interactions > 0.0 {
                    let failure_rate = counters.failure / total_interactions;
                    let debtor_agent: AgentPubKey = record.vouch.entrant.clone();
                    let (witness_count, agg_rate) =
                        crate::trust_cache::get_cached_witness_contagion(&debtor_agent).unwrap_or((0, 0.0));

                    trust_attenuation_with_contagion(
                        failure_rate,
                        total_interactions,
                        witness_count,
                        counters.first_seen_epoch,
                        counters.recent_satisfaction,
                        counters.recent_failure,
                        agg_rate,
                    )
                } else {
                    1.0
                }
            } else {
                1.0
            };

            let vouch_score = VOUCH_TRUST_BASE * phi;

            // Combine: s_ij_att + v_ij_att (Whitepaper §5.1, Definition 3.7)
            // Sum SF-based and vouch-based contributions so a sponsor who also traded
            // with the vouchee accumulates stronger endorsement than one who vouched only.
            // Apply the trust-banking cap (β_bank · N_mat) to the COMBINED total,
            // not just to the SF component. Per Whitepaper Def 3.5, the cap applies to
            // min(S_ij, β_bank · N_mat) before multiplying by φ. The vouch score is
            // treated as an additional S contribution (it represents the sponsor's stake
            // as an implicit S signal), so the joint ceiling should hold.
            if vouch_score > 0.0 {
                let existing = raw_trust.get(&entrant_key).cloned().unwrap_or(0.0);
                let banking_cap = VOLUME_MATURATION_THRESHOLD * TRUST_BANKING_BOUND_FRACTION;
                let uncapped = existing + vouch_score;
                let new_val = uncapped.min(banking_cap);
                let actual_increase = new_val - existing;
                if actual_increase > 0.0 {
                    total_mass += actual_increase;
                    raw_trust.insert(entrant_key, new_val);
                }
            }
        }
    }

    // Confidence-Weighted Trust (Whitepaper Definition 3.7):
    // w_i = min(1, Sigma_i / N_mat) where Sigma_i = sum of attenuated scores s_ij
    let w_i = (total_mass / VOLUME_MATURATION_THRESHOLD).min(1.0);
    let pre_trust_weight = 1.0 - w_i;

    let mut normalized: HashMap<AgentPubKeyB64, f64> = HashMap::new();

    // 1. Add weighted local evidence
    if total_mass > 0.0 && w_i > 0.0 {
        for (debtor, score) in &raw_trust {
            normalized.insert(debtor.clone(), w_i * (score / total_mass));
        }
    }

    // 2. Add weighted pre-trust baseline
    if pre_trust_weight > 0.0 {
        // Here we use the actual observer's pre-trust context
        let agent = agent_info()?.agent_initial_pubkey;
        let p = super::get_pre_trust_distribution(Some(agent))?;
        for (debtor, p_val) in p {
            let current = normalized.get(&debtor).unwrap_or(&0.0);
            normalized.insert(debtor, current + pre_trust_weight * p_val);
        }
    }

    Ok(normalized)
}

/// Compute the normalized local trust row for the current agent.
/// Returns { debtor => c_ij } where c_ij = s_ij / sum_k(s_ik).
/// Uses witness-based contagion for stricter tolerance of known defaulters.
pub fn compute_local_trust_row(agent: AgentPubKey) -> ExternResult<HashMap<AgentPubKeyB64, f64>> {
    let sf_counters = super::sf_counters::compute_sf_counters(agent)?;
    compute_local_trust_row_from_sf_with_contagion(&sf_counters)
}

/// Compute the personalized pre-trust distribution for the given observer.
/// If observer is None, returns a uniform distribution over the entire (known) network.
/// Otherwise, returns a uniform distribution over self + evidenced acquaintances.
pub fn get_pre_trust_distribution(observer: Option<AgentPubKey>) -> ExternResult<HashMap<AgentPubKeyB64, f64>> {
    let mut p: HashMap<AgentPubKeyB64, f64> = HashMap::new();
    match observer {
        Some(obs) => {
            let acquaintances = super::acquaintances::get_acquaintances(())?;
            let our_sf = super::sf_counters::compute_sf_counters(obs.clone())?;

            for acq in &acquaintances {
                let acq_key: AgentPubKeyB64 = acq.clone().into();
                if *acq == obs {
                    continue;
                }

                // Evidence Gate + Contagion Gate + Attenuation Fix
                let counters_opt = our_sf.get(&acq_key);
                let s_ij = counters_opt.map_or(0.0, |c| c.satisfaction);
                let f_ij = counters_opt.map_or(0.0, |c| c.failure);
                let total_interactions = s_ij + f_ij;
                let first_seen = counters_opt.map_or(0, |c| c.first_seen_epoch);

                let (witness_count, agg_rate) =
                    crate::trust_cache::get_cached_witness_contagion(acq).unwrap_or((0, 0.0));

                if total_interactions > 0.0 && s_ij > 0.0 {
                    let failure_rate = f_ij / total_interactions;
                    let phi = trust_attenuation_with_contagion(
                        failure_rate,
                        total_interactions,
                        witness_count,
                        first_seen,
                        counters_opt.map_or(0.0, |c| c.recent_satisfaction),
                        counters_opt.map_or(0.0, |c| c.recent_failure),
                        agg_rate,
                    );

                    if phi > 0.0 {
                        // POINT F: VOLUME-WEIGHTED PRE-TRUST (Attenuated)
                        // w_ij = s_ij / N_mat  (unnormalized ratio, no per-entry min(1.0) cap)
                        let s_eff = s_ij.min(VOLUME_MATURATION_THRESHOLD * TRUST_BANKING_BOUND_FRACTION);
                        let score = s_eff * phi;
                        let w_ij = score / VOLUME_MATURATION_THRESHOLD; // no min(1.0) here
                        p.insert(acq_key.clone(), w_ij);
                    }
                }
            }

            let total_assigned_mass: f64 = p.values().sum();
            let obs_key: AgentPubKeyB64 = obs.into();
            let obs_mass = (1.0 - total_assigned_mass).max(0.0);

            // Observer Self-Mass Assignment:
            // This function calculates the pre-trust distribution for the observer's subjective view.
            // It assigns any unallocated probability mass (1.0 - assigned_mass) to the observer themselves.
            //
            // SECURITY NOTE:
            // The policy preventing a target from benefiting from their own pre-trust mass
            // (e.g., to prevent "I trust myself 100%" capacity inflation) is enforced at the caller
            // level (e.g., in `get_subjective_reputation_as_observer`), where both target and observer
            // are known. This function simply returns the correct distribution.

            let current_obs = p.get(&obs_key).unwrap_or(&0.0);
            p.insert(obs_key, current_obs + obs_mass);

            // Z_i normalization (Whitepaper Definition 13):
            // Normalize to a valid probability distribution in all cases.
            // When total > 1.0 (well-connected nodes), this is the Z_i denominator.
            // When total < 1.0 (subgraph truncation dropped some acquaintances),
            // normalization prevents downstream bias from inflating surviving nodes.
            let total_p: f64 = p.values().sum();
            if total_p > 0.0 && (total_p - 1.0).abs() > 1e-9 {
                for val in p.values_mut() {
                    *val /= total_p;
                }
            }
        }
        None => {
            // Global uniform fallback
            let obs = agent_info()?.agent_initial_pubkey;
            p.insert(obs.into(), 1.0);
        }
    }

    Ok(p)
}
