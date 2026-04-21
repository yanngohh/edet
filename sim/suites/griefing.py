"""Economic Griefing simulation.

Attackers behave honestly for BUILD_EPOCHS to build genuine bilateral trust
with victims, then intentionally let contracts expire to damage victim liquidity
and force them into debt saturation.

This tests robustness against the "behave first, attack later" strategy.
"""
import csv
import os
from sim.config import RESULTS_DIR
from sim.universe import as_numpy

BUILD_EPOCHS = 30  # epochs of honest behavior before attacking


def step(universe, epoch):
    if 'victims' not in universe.suite_state:
        # Default to 10 attackers if not specified
        num_attackers = universe.suite_state.get('num_attackers', 10)
        all_nodes = list(range(universe.size))
        
        # Attackers coordinate (indices 0 to num_attackers - 1)
        griefers = set(range(num_attackers))
        universe.suite_state['griefers'] = griefers
        
        # Pick victims from remaining nodes to ensure disjoint sets
        honest_nodes = [i for i in all_nodes if i not in griefers]
        
        # Pick up to 5 victims or all remaining honest nodes
        num_victims = min(5, len(honest_nodes))
        if num_victims > 0:
            universe.suite_state['victims'] = universe.rng.sample(honest_nodes, num_victims)
        else:
            universe.suite_state['victims'] = []

        if 'strategic_defaulters' not in universe.suite_state:
            universe.suite_state['strategic_defaulters'] = {}

    griefers = universe.suite_state['griefers']
    victims = universe.suite_state['victims']

    # --- BUILD PHASE: honest trading to accumulate real S with victims ---
    # Griefers buy from victims honestly and repay — builds real bilateral history.
    # This mirrors the real-world pattern: attackers earn trust before defecting.
    if epoch < BUILD_EPOCHS:
        if victims:
            for g in griefers:
                v = universe.rng.choice(victims)
                # Griefers buy from victims to earn trust (reputation)
                amt = universe.params.base_capacity * 0.1
                universe.propose_transaction(g, v, amt)   # honest (no is_attack)
    
    # Attack phase: griefers default on transactions
    else:
        # --- ATTACK PHASE: griefers buy and NEVER repay ---
        # Arm the strategic_defaulters mechanism so contracts will expire (never transferred).
        for g in griefers:
            universe.suite_state['strategic_defaulters'][g] = universe.suite_state['griefers']

        for g in griefers:
            if victims:
                v = universe.rng.choice(victims)
                # Now Griefer buys from Victim and defaults
                amt = universe.credit_capacity[v] * 0.1
                universe.propose_transaction(g, v, amt, is_attack=True)


def run(universe, progress=None, sub_task=None):
    from sim.config import RESULTS_DIR
    print("\n--- Running Economic Griefing Scenario ---")
    total_epochs = BUILD_EPOCHS + 100
    out_dir = universe.result_dir or RESULTS_DIR
    csv_path = os.path.join(out_dir, f"griefing_test_{universe.seed}.csv")

    with open(csv_path, 'w', newline='') as file:
        writer = csv.writer(file)
        writer.writerow(["epoch", "phase", "avg_victim_capacity", "avg_griefer_trust"])

        for epoch in range(universe.epoch, total_epochs):
            if progress and sub_task is not None:
                progress.advance(sub_task, 1)
            step(universe, epoch)
            universe.tick()

            # Periodic checkpointing (Fix 5: Robust Resumption)
            if (epoch + 1) % 25 == 0 and universe.result_dir and universe.task_id:
                checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
                universe.save_state(checkpoint_path)

            v_nodes = universe.suite_state.get('victims', [])
            g_nodes = list(universe.suite_state.get('griefers', []))

            gt = as_numpy(universe.global_trust)
            cap = as_numpy(universe.credit_capacity)
            v_cap = float(cap[v_nodes].sum()) / len(v_nodes) if v_nodes else 0
            g_trust = float(gt[g_nodes].sum()) / len(g_nodes) if g_nodes else 0
            phase = "build" if epoch < BUILD_EPOCHS else "attack"

            writer.writerow([epoch, phase, f"{v_cap:.2f}", f"{g_trust:.6f}"])

    print(f"Telemetry saved to {csv_path}")
