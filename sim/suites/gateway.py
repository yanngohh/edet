"""Gateway peer attack suite.

A gateway node uses accomplices to build reputation, then exploits an honest
victim. Accomplices absorb gateway's debt but default toward the honest network.

Expected outcome (Theorem 4): gateway's trust collapses once victim records
F > S, and accomplices are isolated since they default on honest debts.
"""
import csv
import os
from sim.config import RESULTS_DIR
from sim.universe import as_numpy


def step(universe, epoch):
    if epoch == 0:
        chunk = universe.size // 5
        universe.suite_state['gateway_roles'] = {
            'gateway': universe.rng.randint(0, chunk - 1),
            'victim': universe.rng.randint(chunk, 2 * chunk - 1),
            'accomplices': universe.rng.sample(range(2 * chunk, universe.size), 
                                               min(5, universe.size - 2 * chunk))
        }
        roles = universe.suite_state['gateway_roles']
        accomplices = roles['accomplices']
        gateway = roles['gateway']

        # Ally status initialized here
        cluster = set(accomplices) | {gateway}
        universe.suite_state['attacker_nodes'] = cluster
        universe.suite_state['strategic_defaulters'] = {n: None for n in cluster}

        print(f"\n[INFO] Gateway Attack: gateway={gateway}, "
              f"victim={roles['victim']}, accomplices={accomplices}")

    roles = universe.suite_state['gateway_roles']
    gateway = roles['gateway']
    victim = roles['victim']
    accomplices = roles['accomplices']

    # Baseline: All nodes trade among themselves now (attackers build reputation)
    num_tx = max(10, universe.size // 3)
    for _ in range(num_tx):
        b, s = universe.rng.sample(range(universe.size), 2)
        cap = universe.credit_capacity[b]
        universe.propose_transaction(b, s, max(10.0, universe.rng.uniform(0.05 * cap, 0.15 * cap)))

    events, involved = execute_gateway_attack(universe, gateway, accomplices, victim, epoch)
    return events, involved


def execute_gateway_attack(universe, gateway, accomplices, victim, epoch_in_phase,
                           fixed_amount=None):
    """
    Phased Gateway Attack:
    1. Build Reputation (0-20): Accomplices trade with gateway to build S counters.
    2. Bust Out (21-40): Gateway accumulates massive debt from victim.
    3. Default (41+): Gateway and accomplices stop repaying honest network.
    """
    events = []

    # Phase 1: Build artificial reputation within cluster
    if epoch_in_phase <= 20:
        for acc in accomplices:
            for _ in range(2):
                capacity = universe.credit_capacity[gateway]
                amount = universe.rng.uniform(0.002 * capacity, 0.004 * capacity)
                ok, msg = universe.propose_transaction(acc, gateway, amount,
                                                       is_attack=True)
                if not ok:
                    events.append(f"[X] Accomplice {acc} rejected: {msg}")
        events.append(f"Gateway {gateway} building artificial reputation")

    # Phase 2: Bust out from victim
    elif 21 <= epoch_in_phase <= 40:
        for _ in range(2):
            if fixed_amount is not None:
                amount = fixed_amount
            else:
                capacity = universe.credit_capacity[gateway]
                amount = universe.rng.uniform(0.25 * capacity, 0.35 * capacity)

            # Enable strategic defaulting: Cluster only pays itself
            cluster = set(accomplices) | {gateway}
            strategic = {}
            for node in cluster:
                strategic[node] = cluster
            universe.suite_state['strategic_defaulters'] = strategic

            success, reason = universe.propose_transaction(gateway, victim, amount,
                                                           is_attack=True)
            if success:
                events.append(f"[ATTACK] Gateway {gateway} taking debt from victim {victim}")
            else:
                events.append(f"[REJECTED] {reason}")

        # Relief: accomplices absorb gateway's debt
        if sum(c.amount for c in universe.contracts[gateway]) > 1.0:
            for _ in range(3):
                acc = universe.rng.choice(accomplices)
                current_debt = sum(c.amount for c in universe.contracts[gateway])
                if current_debt < 1.0:
                    break
                amount = universe.rng.uniform(0.15 * current_debt, 0.25 * current_debt)
                success, reason = universe.propose_transaction(acc, gateway, amount,
                                                               is_attack=True)
                if success:
                    events.append(f"[RELIEF] Acc {acc} absorbed {amount:.2f}")

    # Phase 3: Cleanup / continued relief
    elif 41 <= epoch_in_phase <= 70:
        if sum(c.amount for c in universe.contracts[gateway]) > 1.0:
            acc = universe.rng.choice(accomplices)
            current_debt = sum(c.amount for c in universe.contracts[gateway])
            amount = min(current_debt, universe.rng.uniform(0.1 * current_debt,
                                                            0.3 * current_debt))
            universe.propose_transaction(acc, gateway, amount, is_attack=True)

    # Only flag as "involved" (Blue log) during the bust-out and default phases
    involved = {victim}
    if 21 <= epoch_in_phase <= 70:
        involved.add(gateway)
        involved.update(accomplices)
        
    return events, involved


def run(universe, progress=None, sub_task=None):
    print("\n--- Running Gateway Peer Attack Simulation ---")
    out_dir = universe.result_dir if universe.result_dir else RESULTS_DIR
    os.makedirs(out_dir, exist_ok=True)
    csv_path = os.path.join(out_dir, f"gateway_attack_{universe.seed}.csv")

    with open(csv_path, 'w', newline='') as file:
        writer = csv.writer(file)
        writer.writerow(["epoch", "gateway_capacity", "victim_capacity",
                         "gateway_trust", "victim_trust", "gateway_debt", "victim_debt"])

        for epoch in range(universe.epoch, 150):
            if progress and sub_task is not None:
                progress.advance(sub_task, 1)

            step(universe, epoch)
            universe.tick()

            # Periodic checkpointing (Fix 5: Robust Resumption)
            if (epoch + 1) % 25 == 0 and universe.result_dir and universe.task_id:
                checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
                universe.save_state(checkpoint_path)
            roles = universe.suite_state['gateway_roles']
            gw = roles['gateway']
            vic = roles['victim']
            gw_debt = sum(c.amount for c in universe.contracts[gw])
            vic_debt = sum(c.amount for c in universe.contracts[vic])

            gt = as_numpy(universe.global_trust)
            cap = as_numpy(universe.credit_capacity)
            writer.writerow([epoch,
                             f"{float(cap[gw]):.2f}",
                             f"{float(cap[vic]):.2f}",
                             f"{float(gt[gw]):.6f}",
                             f"{float(gt[vic]):.6f}",
                             f"{gw_debt:.2f}",
                             f"{vic_debt:.2f}"])

    print(f"Telemetry saved to {csv_path}")
