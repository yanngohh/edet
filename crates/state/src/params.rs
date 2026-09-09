//! Governed constants, their safe ranges, and the re-denomination set.

use std::collections::BTreeMap;

use edet_kernel::constants as k;

use crate::types::ParamKey;

/// `PartialEq` because the write gate's memo is keyed on this whole struct: a
/// stored `seed_reach` is a lower bound only while nothing that can lower a
/// cut has moved, and `rescale` moves the graph and these together
/// (`bond::GateCache`). Comparing the struct rather than a chosen subset is
/// what keeps a new governed field from silently escaping the key.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Params {
    // Denomination-valued (rescaled by re-denomination).
    pub v_base: f64,
    pub dust: f64,
    // Dimensionless / structural.
    pub risk_k: f64,
    pub theta_adopt: f64,
    /// The bar for a proposal that changes who orders the ledger — the
    /// validator set, and the suspension or reinstatement of a member who
    /// holds voting power. Higher than `theta_adopt`, and frozen the same way
    /// (no `ParamKey` reaches it). See `k::THETA_ADOPT_VALIDATOR`.
    #[serde(default = "default_theta_adopt_validator")]
    pub theta_adopt_validator: f64,
    /// Charter visibility policy (1.0 = seal precise amounts; 0.0 = open).
    pub seal_amounts: f64,
    /// Per-epoch stake decay numerator, over [`k::DECAY_DEN`].
    ///
    /// Standing should reflect present backing rather than history, so an edge
    /// fades unless it is renewed by trade. It is a ratio rather than an
    /// amount because it must survive re-denomination untouched.
    pub stake_decay: f64,
    /// Operation-bond unit as a FRACTION of `v_base`, not an absolute amount.
    ///
    /// This is the only shape that survives re-denomination while staying
    /// governable. `safe_range` is a static table in genesis units, so a
    /// denomination-valued bond would drift out of its own constitutional
    /// range the first time `rescale` ran. Every governed constant is
    /// dimensionless for this reason.
    pub bond_fraction: f64,
    /// Seed-amendment rate β (§Governance): what fraction of the tracked external
    /// seed one epoch of amendments may add to it.
    ///
    /// Dimensionless, like every other governed constant, and for the same
    /// reason — `safe_range` is a static table in genesis units, so a
    /// denomination-valued rate would drift out of its own constitutional
    /// range at the first re-denomination. The AMOUNT an amendment carries is
    /// transition payload; this governs only how fast amendments may arrive.
    pub seed_rate: f64,
    /// The insured horizon, in epochs from a claim's acceptance
    /// (`ParamKey::InsuredHorizon`, `k::INSURED_HORIZON_EPOCHS`).
    ///
    /// An epoch count is dimensionless — the epoch is a protocol constant —
    /// so it survives re-denomination like every other governed constant.
    /// Read through `insured_horizon_epochs`, which clamps it into its range.
    #[serde(default = "default_insured_horizon")]
    pub insured_horizon: f64,
    /// Free bonded transitions per member per epoch (the allowance `A`).
    pub bond_free_allowance: u32,
    /// Epochs a bond stays encumbered before releasing (`T_b`).
    pub bond_release_epochs: u64,
    /// Consecutive saturated epochs before bonds may be forfeited (`F`).
    pub bond_forfeit_epochs: u64,
    pub min_maturity_epochs: u64,
    pub gov_cooldown_epochs: u64,
    pub redenom_band_ln: f64,
    /// The floor on how many validators the ledger will let any removal path —
    /// `Exit`, a suspension, a `ValidatorPower` of zero — leave standing.
    ///
    /// Genesis data, like the chain id: the ceremony knows which kind of chain
    /// it is founding and the state machine does not, so `genesis init` writes
    /// `k::MIN_VALIDATORS_REAL_CHAIN` here for a real chain and
    /// `k::MIN_VALIDATORS` for the dev one. Held only in the kernel constant,
    /// the floor bound the ceremony and nothing after it: a four-validator
    /// chain was a three after one member's free `Exit`, where any single
    /// crash halts it, and a one after three, on no vote at all — while the
    /// paper says changing who orders the ledger takes two thirds of the seed.
    #[serde(default = "default_min_validators")]
    pub min_validators: u64,
    // Governance bookkeeping.
    pub last_amend_epoch: BTreeMap<ParamKey, u64>,
    pub last_redenom_epoch: Option<u64>,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            v_base: k::BASE_CAPACITY,
            dust: k::DUST,
            risk_k: k::RISK_K,
            theta_adopt: k::THETA_ADOPT,
            theta_adopt_validator: k::THETA_ADOPT_VALIDATOR,
            seal_amounts: 0.0,
            stake_decay: k::STAKE_DECAY_NUM as f64,
            bond_fraction: k::BOND_UNIT_FRACTION,
            seed_rate: k::SEED_RATE,
            insured_horizon: k::INSURED_HORIZON_EPOCHS as f64,
            bond_free_allowance: k::BOND_FREE_ALLOWANCE,
            bond_release_epochs: k::BOND_RELEASE_EPOCHS,
            bond_forfeit_epochs: k::BOND_FORFEIT_EPOCHS,
            min_maturity_epochs: k::MIN_MATURITY_EPOCHS,
            gov_cooldown_epochs: k::GOV_COOLDOWN_EPOCHS,
            redenom_band_ln: k::REDENOM_BAND_LN,
            min_validators: default_min_validators(),
            last_amend_epoch: BTreeMap::new(),
            last_redenom_epoch: None,
        }
    }
}

impl Params {
    /// Constitutional safe range per governed constant.
    pub fn safe_range(key: ParamKey) -> (f64, f64) {
        match key {
            ParamKey::RiskK => (0.25, 5.0),
            ParamKey::SealAmounts => (0.0, 1.0),
            // Bounded above as strictly as below, and the ceiling is the
            // load-bearing half. A floor keeps the traffic bound meaningful;
            // the ceiling is what stops governance from pricing ordinary
            // members out of writing while every rule still reads as neutral.
            ParamKey::BondFraction => (0.001, 0.10),
            // Bounded on both sides for the same reason, in opposite
            // directions. Decay far below the floor freezes the graph, so
            // standing conferred once is conferred for ever and the community
            // cannot withdraw backing by simply ceasing to trade. Decay at the
            // ceiling erases standing faster than ordinary trade renews it,
            // which is a credit freeze enacted as a rounding rule.
            ParamKey::StakeDecay => (900.0, 999.0),
            // Bounded above so one epoch can never admit an explosion, and —
            // the half that is easy to miss — bounded strictly ABOVE ZERO so
            // the door cannot be governed shut. Amendment is the only
            // external-commitment door a ledger has after genesis (§Governance); a
            // community that set β to nothing would re-freeze the credit half
            // of its own genesis permanently, which is the exact defect the
            // mechanism exists to repair, enacted as a parameter change.
            ParamKey::SeedRate => (0.001, 0.05),
            // From the maturity floor — below it every claim is uninsured,
            // which is a credit freeze enacted as a term — to the maximum
            // horizon, where the dial does nothing. An underwriter's exposure
            // in TIME is what this bounds: a claim is insured only within this
            // many epochs of its acceptance, whatever signatures moved its
            // date since.
            ParamKey::InsuredHorizon => (k::MIN_MATURITY_EPOCHS as f64, k::MAX_HORIZON_EPOCHS as f64),
        }
    }

    pub fn get(&self, key: ParamKey) -> f64 {
        match key {
            ParamKey::RiskK => self.risk_k,
            ParamKey::SealAmounts => self.seal_amounts,
            ParamKey::BondFraction => self.bond_fraction,
            ParamKey::StakeDecay => self.stake_decay,
            ParamKey::SeedRate => self.seed_rate,
            ParamKey::InsuredHorizon => self.insured_horizon,
        }
    }

    pub fn set(&mut self, key: ParamKey, value: f64) {
        match key {
            ParamKey::RiskK => self.risk_k = value,
            ParamKey::SealAmounts => self.seal_amounts = value,
            ParamKey::BondFraction => self.bond_fraction = value,
            ParamKey::StakeDecay => self.stake_decay = value,
            ParamKey::SeedRate => self.seed_rate = value,
            ParamKey::InsuredHorizon => self.insured_horizon = value,
        }
    }

    /// The insured horizon as the epoch count the ledger measures with,
    /// clamped into its constitutional range for the reason `decay_ratio`
    /// clamps: a value written before a range change must not open the door
    /// wider than the constitution now allows, nor close it.
    pub fn insured_horizon_epochs(&self) -> u64 {
        let (lo, hi) = Params::safe_range(ParamKey::InsuredHorizon);
        if self.insured_horizon.is_finite() {
            self.insured_horizon.clamp(lo, hi) as u64
        } else {
            lo as u64
        }
    }

    /// The seed-amendment rate β, clamped into its safe range — read through
    /// here rather than off the field, for the same reason `decay_ratio`
    /// clamps: a value written before a range change must not be able to open
    /// the door wider than the constitution now allows, nor close it entirely.
    pub fn seed_rate_bounded(&self) -> f64 {
        let (lo, hi) = Params::safe_range(ParamKey::SeedRate);
        if self.seed_rate.is_finite() {
            self.seed_rate.clamp(lo, hi)
        } else {
            lo
        }
    }

    /// The bond for one unit of transition class, in current denomination.
    /// Derived from `v_base` rather than stored, so re-denomination carries
    /// it without a rescale entry of its own.
    pub fn bond_unit(&self) -> f64 {
        self.bond_fraction * self.v_base
    }

    /// The bond unit in MINOR UNITS, which is what the gate charges and what
    /// `Member.bonds` holds. Rounded the way every other amount crossing this
    /// boundary is, so the schedule and the ledger agree exactly.
    pub fn bond_unit_minor(&self) -> u64 {
        crate::state::State::to_minor(self.bond_unit())
    }

    /// The dust threshold in minor units — the reading every comparison
    /// against a stored amount uses, since stored amounts are integers. A
    /// comparison that converted the amount to `f64` instead would put the
    /// boundary back exactly where the integers removed it.
    ///
    /// `dust` itself stays an `f64`, deliberately: it is a governed PARAMETER
    /// alongside `v_base`, `bond_fraction`, `risk_k` and `theta_adopt`, it is
    /// written only at genesis and by a re-denomination, and it accumulates
    /// nothing — so it carries none of the drift that put the ledger's amounts
    /// on the grid. Making one member of the parameter block integer and
    /// leaving the rest real would be the less coherent of the two.
    ///
    /// A downward re-denomination can take it BELOW one minor unit, and then
    /// this reads zero: there is nothing left to forgive when the finest unit
    /// the ledger holds is larger than the threshold. A row then closes only
    /// when it is paid to exactly nothing, which is the honest answer rather
    /// than a rounding.
    pub fn dust_minor(&self) -> u64 {
        crate::state::State::to_minor(self.dust)
    }

    /// `v_base` in minor units, for the thresholds stated as a fraction of it.
    pub fn v_base_minor(&self) -> u64 {
        crate::state::State::to_minor(self.v_base)
    }

    /// Stake decay as `(numerator, denominator)` for the integer edge decay.
    /// Clamped into its safe range here so a value written before a range
    /// change cannot decay edges to nothing.
    pub fn decay_ratio(&self) -> (u64, u64) {
        let (lo, hi) = Params::safe_range(ParamKey::StakeDecay);
        (self.stake_decay.clamp(lo, hi) as u64, k::DECAY_DEN)
    }

    /// Apply the re-denomination factor to every denomination-valued constant.
    ///
    /// The stake graph is rescaled alongside these by `State::rescale`: edges
    /// and reservations are denomination-valued, and a rescale that moved the
    /// constants without them would silently reprice every credit limit.
    pub fn rescale(&mut self, pi: f64) {
        self.v_base *= pi;
        self.dust *= pi;
    }
}

/// `serde(default)` for `theta_adopt_validator`, so a params blob written
/// before the field existed loads at the genesis constant rather than at 0.0 —
/// which would be no bar at all, on the one threshold that most needs one.
fn default_theta_adopt_validator() -> f64 {
    k::THETA_ADOPT_VALIDATOR
}

/// `serde(default)` for `min_validators`: the state machine's own floor of
/// one, which is what a params blob written before the field existed ran
/// under, and what a dev chain founds with.
fn default_insured_horizon() -> f64 {
    k::INSURED_HORIZON_EPOCHS as f64
}

fn default_min_validators() -> u64 {
    k::MIN_VALIDATORS as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The constitutional freeze: "the constants that guard amendment must not
    /// be amendable through the door they guard." `theta_adopt` is exactly
    /// such a constant — it is the mass an `Assent` coalition needs to enact
    /// ANY `ParamChange`, including one that targets `theta_adopt` itself, so
    /// if it were reachable through `ParamKey` a coalition holding it could
    /// first lower it and then adopt anything else at the lowered bar. The
    /// freeze is enforced by omission: `ParamKey` has no `ThetaAdopt` variant.
    /// This test walks every OTHER governed key — the door — and confirms none
    /// of them is a back way in to the field the door itself is hinged on.
    #[test]
    fn the_door_cannot_amend_its_own_guard() {
        let every_other_key =
            [ParamKey::RiskK, ParamKey::SealAmounts, ParamKey::BondFraction, ParamKey::StakeDecay, ParamKey::SeedRate];
        let mut params = Params::default();
        let before = params.theta_adopt;
        let sentinel = 0.999_999;
        for key in every_other_key {
            params.set(key, sentinel);
        }
        assert_eq!(
            params.theta_adopt.to_bits(),
            before.to_bits(),
            "theta_adopt must be bit-unchanged: no ParamKey may set it"
        );
        assert_eq!(
            params.theta_adopt_validator.to_bits(),
            Params::default().theta_adopt_validator.to_bits(),
            "and neither may the validator bar, which guards the one change that is not recoverable"
        );
    }

    /// Decay must never be able to reach 1.0 (no decay) or 0 (instant
    /// erasure), whatever is written to the field.
    #[test]
    fn decay_ratio_is_always_a_proper_fraction() {
        let mut p = Params::default();
        for v in [-1e9, 0.0, 500.0, 999.9, 1e9] {
            p.stake_decay = v;
            let (num, den) = p.decay_ratio();
            assert!(num > 0 && num < den, "{v} produced {num}/{den}");
        }
    }
}
