"""Strategic Oscillation suite.

Tests Theorem 5.3: Any node whose failure rate meets or exceeds tau is fully excluded.
Also analyzes the 'yield' of strategic defaulting.

Behavioral approach (v2):
    Instead of synthetically injecting F values, the attacker's default rate
    emerges naturally from its selling behavior:

    - Every epoch the attacker BUYS from honest peers (accumulating debt contracts).
    - In a fraction (1-r) of epochs the attacker also SELLS to honest peers,
      which triggers debt-transfer and increments S for the original creditors.
    - In the remaining fraction r of epochs the attacker does NOT sell, so
      those debt contracts sit idle until maturity, at which point tick()
      increments F.

    Over many epochs the ratio F/(S+F) converges to approximately r, which is
    exactly the input default_rate.  This tests the full protocol pipeline
    (contract creation, debt transfer, maturity expiration, phi(r) attenuation)
    rather than just the trust formula in isolation.
"""
import csv
import os
import math
import tempfile
import shutil
import numpy as np
from sim.universe import Universe, DebtContract, as_numpy
# from tqdm import tqdm


def execute_oscillation_test(size, seed, gpu_threshold, default_rate, target_epoch, progress=None, sub_task=None, univ=None, use_disk=True, result_dir=None, task_id=None):
    """
    Run a simulation where a strategic node defaults at a specific rate
    via behavioral selling/not-selling decisions.
    """
    start_epoch = univ.epoch if univ else 0
    from sim.config import get_production_params
    from sim.universe import Universe
    params = get_production_params(size)
    
    if univ is None:
        univ = Universe(size, gpu_threshold, params=params, seed=seed, use_disk=use_disk, result_dir=result_dir, task_id=task_id)
        # Vouch all genesis nodes so they can transact (direct bootstrap assignment)
        amount = params.base_capacity
        nodes = list(range(univ.size))
        for node in nodes:
            candidates = [j for j in nodes if j != node]
            if candidates:
                sponsor = univ.rng.choice(candidates)
                univ.staked_capacity[node] += amount
                univ.vouchers[node][sponsor] = univ.vouchers[node].get(sponsor, 0.0) + amount
        univ.update_credit_capacity()

    # Reuse or pick attacker
    if 'oscillation_roles' not in univ.suite_state:
        attacker = univ.rng.randint(0, size - 1)
        honest_nodes = [i for i in range(size) if i != attacker]
        univ.suite_state['oscillation_roles'] = {
            'attacker': attacker,
            'honest_nodes': honest_nodes
        }
    
    roles = univ.suite_state['oscillation_roles']
    attacker = roles['attacker']
    honest_nodes = roles['honest_nodes']

    # Force attacker to be fully self-supporting so debt transfer is clean
    breakdown = {k: 0.0 for k in univ.support_breakdown[attacker]}
    breakdown[attacker] = 1.0
    univ.set_support_breakdown(attacker, breakdown)

    # Track metrics
    extracted_value = 0.0

    # --- Selling schedule ---
    # We use a 20-epoch cycle.  In each cycle, the first round((1-r)*cycle)
    # epochs are "sell" epochs and the rest are "idle" epochs.
    # With maturity=50, debt accumulated in idle epochs expires after 50 ticks,
    # well after the cycle repeats, giving F a chance to accumulate.
    cycle = 20
    sell_epochs_per_cycle = round((1.0 - default_rate) * cycle)

    for epoch in range(univ.epoch, target_epoch):
        if progress and sub_task is not None:
            progress.advance(sub_task, 1)

        # Local epoch for phase calculation
        local_epoch = epoch - start_epoch

        # 1. Background honest activity (Sub-linear scaling O(sqrt(N)) for speed)
        for _ in range(3 * math.isqrt(size)):
            b, s = univ.rng.sample(honest_nodes, 2)
            univ.propose_transaction(b, s, univ.rng.uniform(10, 50))

        # Determine whether this epoch is a "sell" or "idle" epoch.
        phase_index = local_epoch % cycle
        is_sell_epoch = phase_index < sell_epochs_per_cycle

        # 2. Attacker SELLS to honest peers (transfers its debt, building S).
        #    Only happens in sell epochs.  Volume is high enough to drain most
        #    outstanding contracts, mimicking an attacker who actively works
        #    to maintain reputation.
        if is_sell_epoch:
            for _ in range(12):
                buyer = univ.rng.choice(honest_nodes)
                univ.propose_transaction(buyer, attacker, univ.rng.uniform(50, 150))

        # 3. Attacker BUYS from honest peers every epoch (accumulates debt).
        #    The debt that is NOT transferred before maturity becomes F.
        for _ in range(3):
            seller = univ.rng.choice(honest_nodes)
            amount = 100.0
            success, _ = univ.propose_transaction(attacker, seller, amount)
            if success:
                extracted_value += amount

        univ.tick()

        # Periodic checkpointing (Fix 5: Robust Resumption)
        if (epoch + 1) % 25 == 0 and univ.result_dir and task_id:
            checkpoint_path = os.path.join(univ.result_dir, f"checkpoint_{task_id}_interrupted")
            univ.save_state(checkpoint_path)

        # Re-assert self-support each epoch so randomize_support doesn't
        # change the attacker's breakdown. Preserve all keys to respect append-only constraints.
        breakdown = {k: 0.0 for k in univ.support_breakdown[attacker]}
        breakdown[attacker] = 1.0
        univ.set_support_breakdown(attacker, breakdown)

    # Use subjective trust from a representative honest creditor (protocol-accurate)
    honest_observer = honest_nodes[0]
    
    return {
        'final_trust': float(univ.get_subjective_reputation(honest_observer, attacker)),
        'global_trust': float(univ.global_trust[attacker]),
        'extracted_value': float(extracted_value),
        'final_capacity': float(univ.credit_capacity[attacker])
    }


def step(universe, epoch):
    """
    Standard step function for animated graph.
    Uses a default oscillation pattern (r=0.20) for visualization.

    Behavioral approach: the attacker sells in sell-phase epochs and idles
    in idle-phase epochs, producing a natural ~20% default rate.
    """
    if epoch == 0:
        # Pick a random attacker based on the universe seed
        attacker = universe.rng.randint(0, universe.size - 1)
        honest_nodes = [i for i in range(universe.size) if i != attacker]
        universe.suite_state['oscillation_roles'] = {
            'attacker': attacker,
            'honest_nodes': honest_nodes
        }
        breakdown = {k: 0.0 for k in universe.support_breakdown[attacker]}
        breakdown[attacker] = 1.0
        universe.set_support_breakdown(attacker, breakdown)
        print(f"[INFO] Oscillation Suite: Attacker={attacker}")

    roles = universe.suite_state['oscillation_roles']
    attacker = roles['attacker']
    honest_nodes = roles['honest_nodes']

    size = universe.size
    default_rate = 0.20  # Visualizable exclusion

    events = []
    involved = []

    # 1. Background honest activity (Sub-linear scaling)
    for _ in range(2 * math.isqrt(size)):
        b, s = universe.rng.sample(honest_nodes, 2)
        universe.propose_transaction(b, s, universe.rng.uniform(10, 50))

    # --- Behavioral schedule ---
    cycle = 20
    sell_epochs_per_cycle = round((1.0 - default_rate) * cycle)  # 16
    phase_index = epoch % cycle
    is_sell_epoch = phase_index < sell_epochs_per_cycle

    if is_sell_epoch:
        # Sell phase: attacker actively sells, transferring debt (builds S)
        events.append("Build: Selling to transfer debt (S)")
        for _ in range(10):
            buyer = universe.rng.choice(honest_nodes)
            universe.propose_transaction(buyer, attacker, universe.rng.uniform(50, 150))
    else:
        # Idle phase: attacker does NOT sell; debt expires at maturity (builds F)
        events.append(f"Idle: Letting debt expire (target r={default_rate})")
        involved.append(attacker)

    # Attacker always buys (accumulates debt) regardless of phase
    for _ in range(3):
        seller = universe.rng.choice(honest_nodes)
        universe.propose_transaction(attacker, seller, universe.rng.uniform(50, 150))

    # Re-assert self-support so randomize_support doesn't interfere
    breakdown = {k: 0.0 for k in universe.support_breakdown[attacker]}
    breakdown[attacker] = 1.0
    universe.set_support_breakdown(attacker, breakdown)

    return events, involved


def run(universe):
    """Run telemetry for oscillation suite."""
    print("--- Running Oscillation Telemetry ---")
    from sim.config import RESULTS_DIR
    out_dir = universe.result_dir if universe.result_dir else RESULTS_DIR
    os.makedirs(out_dir, exist_ok=True)
    csv_path = os.path.join(out_dir, f"oscillation_test_{universe.seed}.csv")

    with open(csv_path, 'w', newline='') as file:
        writer = csv.writer(file)
        writer.writerow(["epoch", "attacker_trust", "attacker_capacity"])

        for epoch in range(200):
            step(universe, epoch)
            universe.tick()

            roles = universe.suite_state.get('oscillation_roles', {'attacker': 0})
            atk = roles['attacker']
            writer.writerow([epoch, float(universe.global_trust[atk]), float(universe.credit_capacity[atk])])

    print(f"Telemetry saved to {csv_path}")


def run_sweep(size, seed, gpu_threshold, rates=None, progress=None, sub_task=None, use_disk=True, result_dir=None, task_id=None, load_path=None):
    if rates is None:
        rates = [0.0, 0.05, 0.10, 0.14, 0.15, 0.16, 0.20, 0.25, 0.30]

    from sim.config import get_production_params
    from sim.universe import Universe
    params = get_production_params(size)
    
    from sim.utils import resolve_load_path
    # 1. Initial shared bootstrap (Honest activity)
    lpath = resolve_load_path(load_path, task_id)
    if lpath:
        univ = Universe.load_state(lpath, result_dir=result_dir, task_id=task_id)
    else:
        univ = Universe(size, gpu_threshold, params=params, seed=seed, use_disk=use_disk, result_dir=result_dir, task_id=task_id)
        # Vouch all genesis nodes
        amount = params.base_capacity
        for node in range(size):
            candidates = [j for j in range(size) if j != node]
            if candidates:
                sponsor = univ.rng.choice(candidates)
                univ.staked_capacity[node] += amount
                univ.vouchers[node][sponsor] = univ.vouchers[node].get(sponsor, 0.0) + amount
        univ.update_credit_capacity()
    
    # Pre-run 50 epochs of honest activity to stabilize trust
    # If loading, we might already have stable trust, but it's safe to run more.
    for epoch in range(univ.epoch, 50):
        for _ in range(2 * math.isqrt(size)):
            b, s = univ.rng.sample(range(size), 2)
            univ.propose_transaction(b, s, univ.rng.uniform(10, 50))
        univ.tick()

        # Periodic checkpointing (Fix 5: Robust Resumption)
        if (epoch + 1) % 25 == 0 and univ.result_dir and task_id:
            checkpoint_path = os.path.join(univ.result_dir, f"checkpoint_{task_id}_interrupted")
            univ.save_state(checkpoint_path)

        if progress and sub_task is not None:
            progress.advance(sub_task, 1)

    results = []
    # 2. Independent testing of each rate (Testing Theorem 5.3 exclusion/recovery)
    #
    # FIX: Each rate is tested on a FRESH copy of the bootstrapped universe so that
    # accumulated S/F history from prior rates does not bleed into later ones.
    # Previously, all rates ran sequentially on the same `univ` object, meaning the
    # attacker's honest history from rate r[i-1] inflated their cumulative S counters
    # for rate r[i], making it harder to reach the target failure rate and biasing
    # the results toward under-stating the exclusion effect.
    #
    # We save the post-bootstrap state once and restore a clean copy per rate.
    bootstrap_tmp = tempfile.mkdtemp(prefix="edet_osc_bootstrap_")
    try:
        univ.save_state(bootstrap_tmp)
        # Copy suite_state so the attacker role is preserved across all rate runs.
        bootstrap_suite_state = dict(univ.suite_state)

        for r in rates:
            # Restore a fresh universe from the bootstrap snapshot.
            rate_univ = Universe.load_state(
                bootstrap_tmp,
                result_dir=result_dir,
                task_id=task_id,
            )
            # Re-apply the attacker/honest role assignment from bootstrap.
            rate_univ.suite_state = {k: v for k, v in bootstrap_suite_state.items()}
            # Each rate gets 100 epochs from epoch 50 (independent window).
            target = 50 + 100
            res = execute_oscillation_test(
                size, seed, gpu_threshold, r,
                target_epoch=target,
                progress=progress,
                sub_task=sub_task,
                univ=rate_univ,
                use_disk=use_disk,
                result_dir=result_dir,
                task_id=task_id,
            )
            res['default_rate'] = r
            results.append(res)
    finally:
        shutil.rmtree(bootstrap_tmp, ignore_errors=True)

    return results
