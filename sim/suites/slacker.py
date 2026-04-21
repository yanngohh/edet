"""Slacker attack suite.

A slacker accumulates debt but never sells (defaults on all obligations).

Expected outcome (Theorem 5): slacker's trust converges to 0,
credit capacity collapses to V_base, further non-trial transactions rejected.
"""
import csv
import os
from sim.config import RESULTS_DIR
from sim.universe import as_numpy


def step(universe, epoch):
    if epoch == 0:
        indices = universe.rng.sample(range(universe.size), 2)
        universe.suite_state['slacker_roles'] = {
            'slacker': indices[0],
            'honest': indices[1]
        }
        slacker_node = indices[0]
        universe.suite_state['attacker_nodes'] = {slacker_node}
        print(f"\n[INFO] Slacker Attack: slacker={indices[0]}, control={indices[1]}")

    roles = universe.suite_state['slacker_roles']
    slacker = roles['slacker']
    honest = roles['honest']

    # Baseline: High activity to ensure mass doesn't "blur" through too many idle nodes
    num_tx = universe.size * 2
    if epoch < 20:
        # Honest phase: slacker participates normally
        candidates = list(range(universe.size))
    else:
        # Slacking phase: exclude slacker from being picked as seller
        candidates = [i for i in range(universe.size) if i != slacker]

    for _ in range(num_tx):
        b = universe.rng.choice(candidates)
        s = universe.rng.choice(candidates)
        if b == s:
            continue
        cap = universe.credit_capacity[b]
        universe.propose_transaction(b, s, max(10.0, universe.rng.uniform(0.05 * cap, 0.15 * cap)))

    # Activate slacking after epoch 20
    if epoch < 20:
        return ["Build: Normal Economic Activity"], []
    
    # Block all debt transfers for the slacker (empty pay_set = pays nobody)
    universe.suite_state['strategic_defaulters'] = {slacker: set()}
    # Target only virtuous peers (non-attackers)
    attackers = universe.suite_state.get('attacker_nodes', {slacker})
    virtuous_peers = [i for i in range(universe.size) if i not in attackers]
    
    return execute_slacker_attack(universe, [slacker], virtuous_peers)


def execute_slacker_attack(universe, slackers, honest_pool):
    """Slackers try to buy (accumulate debt) without reciprocating."""
    events = []
    involved = set(slackers) | set(honest_pool[:5])

    for slacker in slackers:
        # Increase exposure: buy from multiple peers to ensure more people see the slacking
        for _ in range(5):
            target = universe.rng.choice(honest_pool)
            if target == slacker:
                continue
            capacity = universe.volume[slacker]
            amount = universe.rng.uniform(0.1 * capacity, 0.3 * capacity)
            success, reason = universe.propose_transaction(slacker, target, amount,
                                                           is_attack=True, force=True)
            if success:
                events.append(f"[ATTACK] Slacker {slacker} taking {amount:.2f} from {target}")
            else:
                events.append(f"[REJECTED] Slacker {slacker}: {reason}")

    return events, involved


def run(universe, progress=None, sub_task=None):
    print("\n--- Running Slacking Attack Simulation ---")
    out_dir = universe.result_dir if universe.result_dir else RESULTS_DIR
    os.makedirs(out_dir, exist_ok=True)
    csv_path = os.path.join(out_dir, f"slacker_attack_{universe.seed}.csv")

    with open(csv_path, 'w', newline='') as file:
        writer = csv.writer(file)
        writer.writerow(["epoch", "slacker_trust", "honest_trust", "slacker_subjective_trust"])

        for epoch in range(universe.epoch, 200): # Weight in verify_theory.py says 150 but let's just advance it unconditionally.
            if progress and sub_task is not None:
                progress.advance(sub_task, 1)

            step(universe, epoch)
            universe.tick()

            # Periodic checkpointing (Fix 5: Robust Resumption)
            if (epoch + 1) % 25 == 0 and universe.result_dir and universe.task_id:
                checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
                universe.save_state(checkpoint_path)
            roles = universe.suite_state['slacker_roles']
            creditor = roles['honest']
            gt = as_numpy(universe.global_trust)
            writer.writerow([epoch,
                             f"{float(gt[roles['slacker']]):.6f}",
                             f"{float(gt[roles['honest']]):.6f}",
                             f"{universe.get_subjective_reputation(creditor, roles['slacker']):.6f}"])

    print(f"Telemetry saved to {csv_path}")
