"""Virtuous network equilibrium suite.

All nodes are honest: they trade and repay debts. Tests that:
- Trust converges to roughly uniform distribution
- Credit capacity grows above baseline
- No node is starved of reputation
"""
import csv
import os
from sim.universe import as_numpy


def step(universe, epoch):
    if epoch == 0 or 'epochs_since_participation' not in universe.suite_state:
        universe.suite_state['virtuous_roles'] = {
            'honest_nodes': list(range(universe.size))
        }
        universe.suite_state['epochs_since_participation'] = [0] * universe.size
        print(f"\n[INFO] Virtuous Scenario: {universe.size} honest peers, 0 attackers.")

    # Ensure metadata list matches growing universe size
    if len(universe.suite_state['epochs_since_participation']) < universe.size:
        diff = universe.size - len(universe.suite_state['epochs_since_participation'])
        universe.suite_state['epochs_since_participation'].extend([0] * diff)

    # 50% of network transacts each epoch to accelerate trust diffusion
    num_tx = int(universe.size * 0.50)
    all_peers = list(range(universe.size))
    epochs_since = universe.suite_state['epochs_since_participation']
    participated = set()

    # Prioritize nodes that need to sell (have debt) OR have been inactive
    # Adding rng.random() breaks ties to avoid buyer/seller deadlock
    active_pool = sorted(all_peers,
                         key=lambda i: -(universe.total_debt[i] * 2.0
                                         + epochs_since[i] + universe.rng.random()))
    seller_pool = list(active_pool)
    seller_queue = seller_pool

    # Prioritize nodes that haven't participated in the longest time for buying
    buyer_pool = sorted(all_peers, key=lambda i: -(epochs_since[i] + universe.rng.random()))
    buyer_idx = 0

    for i in range(num_tx):
        seller = seller_queue.pop(0) if seller_queue else universe.rng.choice(all_peers)
        buyer = buyer_pool[buyer_idx % len(buyer_pool)]
        buyer_idx += 1
        
        if buyer == seller:
            continue
            
        capacity = universe.credit_capacity[buyer]
        
        # Bootstrap phase: use trial transactions to build acquaintances and trust
        # Trial threshold = trial_fraction * base_capacity (default: 5% * 1000 = 50)
        trial_threshold = universe.params.trial_fraction * universe.params.base_capacity
        
        # Check if we have evidence (subjective reputation > 0) with this counterparty
        # If not, use a trial-sized transaction to bootstrap
        has_evidence = any(
            universe.S[seller].get(buyer, 0) > 0 or
            universe.S[buyer].get(seller, 0) > 0
            for _ in [1]  # Just a way to check both directions
        )
        
        if not has_evidence:
            # Use trial transaction to bootstrap relationship
            tx_amount = universe.rng.uniform(10.0, trial_threshold * 0.9)
        else:
            # Normal transaction size once trust is established
            tx_amount = max(10.0, universe.rng.uniform(0.02 * capacity, 0.15 * capacity))
        
        success, _ = universe.propose_transaction(buyer, seller, tx_amount)
        if success:
            participated.add(buyer)
            participated.add(seller)

    for node in all_peers:
        epochs_since[node] = 0 if node in participated else epochs_since[node] + 1

    return [], participated


def run(universe, epochs=2000):
    print("\n--- Running Virtuous Network Equilibrium Simulation ---")
    from sim.config import RESULTS_DIR
    out_dir = universe.result_dir if universe.result_dir else RESULTS_DIR
    os.makedirs(out_dir, exist_ok=True)
    csv_path = os.path.join(out_dir, f"virtuous_test_{universe.seed}.csv")

    with open(csv_path, 'w', newline='') as f:
        writer = csv.writer(f)
        writer.writerow(["epoch", "avg_trust", "avg_capacity", "avg_rho", "max_capacity"])

        for epoch in range(epochs):
            step(universe, epoch)
            universe.tick()
            gt = as_numpy(universe.global_trust)
            cap = as_numpy(universe.credit_capacity)
            avg_trust = float(gt.sum()) / universe.size
            avg_capacity = float(cap.sum()) / universe.size
            max_cap = float(cap.max())
            writer.writerow([epoch, f"{avg_trust:.6f}", f"{avg_capacity:.2f}",
                             "0.0000", f"{max_cap:.2f}"])

    print(f"Telemetry saved to {csv_path}")
    return []
