"""Flash Loan Recycle Attack simulation.

Simulates an attacker using many fresh identities to exploit 'trial' capacity.
Newcomers can trade small amounts regardless of reputation. 
The attacker cycles these identities to maximize extraction.
"""
import csv
import os
from sim.universe import as_numpy
# from tqdm import tqdm  # removed

def step(universe, epoch):
    if 'attacker_nodes' not in universe.suite_state:
        # Attacker setup
        universe.suite_state['attacker_nodes'] = []
    
    # Every few epochs, introduce a new set of attackers
    if 'strategic_defaulters' not in universe.suite_state:
        universe.suite_state['strategic_defaulters'] = {}

    if epoch % 5 == 0:
        n_new = 5
        start_idx = universe.size
        universe.add_nodes(n_new)
        new_attackers = list(range(start_idx, start_idx + n_new))
        
        # Handle both list and set types to support mixed suite integration
        current_attackers = universe.suite_state['attacker_nodes']
        if isinstance(current_attackers, set):
            current_attackers.update(new_attackers)
        else:
            current_attackers.extend(new_attackers)
        
        # Mark them as strategic defaulters (they never pay honest nodes)
        for a in new_attackers:
            universe.suite_state['strategic_defaulters'][a] = set(universe.suite_state['attacker_nodes'])

    # Attackers perform "flash" transactions using trial capacity
    attackers = universe.suite_state['attacker_nodes']
    honest_nodes = [i for i in range(universe.size) if i not in attackers]
    
    if honest_nodes and attackers:
        for a in attackers:
            # Only attack if the identity is fresh (to simulate recycling)
            age = epoch - universe.join_epoch[a]
            if age < 15:
                # Target an honest node
                target = universe.rng.choice(honest_nodes)
                # Use trial fraction (eta)
                limit = universe.params.trial_fraction * universe.params.base_capacity
                amount = universe.rng.uniform(0.5 * limit, 0.9 * limit)
                universe.propose_transaction(a, target, amount, is_attack=True)

def run(universe, progress=None, sub_task=None):
    from sim.config import RESULTS_DIR
    print("\n--- Running Flash Loan Recycle Attack Scenario ---")
    csv_path = os.path.join(universe.result_dir or RESULTS_DIR, f"flash_loan_test_{universe.seed}.csv")
    
    with open(csv_path, 'w', newline='') as file:
        writer = csv.writer(file)
        writer.writerow(["epoch", "attacker_count", "avg_attacker_trust", "total_extraction"])
        
        total_extracted = 0.0
        for epoch in range(universe.epoch, 100):
            if progress and sub_task is not None:
                progress.advance(sub_task, 1)

            step(universe, epoch)
            universe.tick()

            # Periodic checkpointing (Fix 5: Robust Resumption)
            if (epoch + 1) % 25 == 0 and universe.result_dir and universe.task_id:
                checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
                universe.save_state(checkpoint_path)
            
            attackers = universe.suite_state['attacker_nodes']
            honest_nodes = [i for i in range(universe.size) if i not in attackers]
            gt = as_numpy(universe.global_trust)
            a_trust = float(gt[list(attackers)].sum()) / len(attackers) if attackers else 0
            
            # Compute cumulative extraction from F matrix (debt that expired unpaid).
            # After tick(), expired contracts are removed from universe.contracts
            # and their amounts recorded in universe.F[creditor][debtor].
            cumulative = sum(
                sum(universe.F[h].get(a, 0.0) for h in honest_nodes)
                for a in attackers
            )
            total_extracted = cumulative
            writer.writerow([epoch, len(attackers), f"{a_trust:.6f}", f"{total_extracted:.2f}"])
            
    print(f"Telemetry saved to {csv_path}")
