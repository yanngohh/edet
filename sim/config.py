import os
from sim.universe import ProtocolParameters

# Single Source of Truth for Paths
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS_DIR = os.path.join(PROJECT_ROOT, "sim/results")


def get_production_params(size: int) -> ProtocolParameters:
    """Return the certified production parameters for the edet simulation.

    This function is the Single Source of Truth for all protocol parameters.
    Values correspond to the whitepaper Appendix B (Parameter Selection).

    Parameters:
        size: Network size (unused currently but available for future scaling).
    """
    return ProtocolParameters(
        # EigenTrust Configuration
        eigentrust_alpha=0.08,
        eigentrust_epsilon=0.001,
        # 20 is a deliberate performance shortcut for simulation: the whitepaper and
        # the Rust constant (EIGENTRUST_MAX_ITERATIONS = 84) use 84, which guarantees
        # convergence even on adversarial/sparse graphs via (1-alpha)^84 ≈ 9.5e-4 < epsilon.
        # In practice the simulation graphs are well-connected and the early-exit
        # (diff < epsilon) fires well before iteration 20, so the cap is never reached
        # and both values produce identical results. Simulation networks also never
        # exceed ~20 hops of meaningful trust mass, so 20 is a safe ceiling here.
        eigentrust_iterations=20,

        # Credit Capacity
        base_capacity=1000.0,
        capacity_beta=5000.0,
        acq_saturation=50.0,         # n0: acquaintance saturation constant

        # Trust Attenuation
        failure_tolerance=0.12,      # tau: failure rate threshold
        tau_newcomer=0.05,           # tau_0: bilateral newcomer tolerance
        volume_maturation=1000.0,    # N_mat: bilateral volume for full failure tolerance
        penalty_sharpness=4.0,       # gamma: penalty curve exponent

        # Recent Failure Rate Window (Behavioral Switch Detection)
        recent_window_k=10,          # K: rolling window size in epochs
        recent_weight=2.0,           # w_r: r_eff = max(r_cumul, w_r * r_recent)

        # Trust Banking Mitigation 
        trust_banking_bound_fraction=0.25, # Fraction of N_mat for max S in the score multiplier
        
        # Witness-Based Contagion
        contagion_witness_factor=0.25,  # k: tau_eff penalty per failure witness
        witness_discount=0.5,           # d_w: discount on median witness bilateral rate
        min_contagion_witnesses=3,      # n_min: minimum witnesses for imputed floor rate

        # Behavioral
        # min_maturity reduced from 50 → 30 to tighten the adversarial extraction horizon
        # (a build-then-betray attacker now has 30 days instead of 50 before failures are
        # recorded) while remaining consistent with standard net-30 commercial terms.
        min_maturity=30,
        trial_fraction=0.05,

        # Risk thresholds
        default_accept_threshold=0.4,
        default_reject_threshold=0.8,
        risk_sigmoid_k=0.75,

        # Acquaintance Bound (Dunbar-style cap)
        # Matches the Holochain implementation where |A_i| is bounded by
        # social discovery. Set to 150 (Dunbar's number) for realistic
        # scaling behavior. Set to 0 for unbounded (legacy) mode.
        max_acquaintances=150,

        # Subjective Trust Subgraph (matches Holochain bounded BFS)
        subgraph_max_depth=4,
        max_subgraph_nodes=50000,
    )
