"""Coordinated Market Manipulation simulation.

Simulates multiple Sybil clusters building internal trust and attempting to 
infiltrate honest clusters to default simultaneously at scale.
"""
import csv
import os
from sim.config import RESULTS_DIR
from sim.universe import as_numpy
# from tqdm import tqdm

def step(universe, epoch):
    if 'honest_cluster' not in universe.suite_state:
        # Partition nodes into 3 clusters
        all_nodes = list(range(universe.size))
        c1_size = universe.size // 3
        c2_size = universe.size // 3
        
        universe.suite_state['honest_cluster'] = set(all_nodes[:c1_size])
        universe.suite_state['sybil_cluster_a'] = set(all_nodes[c1_size:c1_size+c2_size])
        universe.suite_state['sybil_cluster_b'] = set(all_nodes[c1_size+c2_size:])
        
        if 'strategic_defaulters' not in universe.suite_state:
            universe.suite_state['strategic_defaulters'] = {}
        
        # Sybils collude internally
        for a in universe.suite_state['sybil_cluster_a']:
            universe.suite_state['strategic_defaulters'][a] = universe.suite_state['sybil_cluster_a']
        for b in universe.suite_state['sybil_cluster_b']:
            universe.suite_state['strategic_defaulters'][b] = universe.suite_state['sybil_cluster_b']

    # Phases
    # 1. Warmup (0-50): Internal cluster trading
    if epoch < 50:
        for cluster_key in ['honest_cluster', 'sybil_cluster_a', 'sybil_cluster_b']:
            cluster = list(universe.suite_state[cluster_key])
            for _ in range(len(cluster)):
                b, s = universe.rng.sample(cluster, 2)
                universe.propose_transaction(b, s, universe.rng.uniform(10, 50))
                
    # 2. Infiltration (50-100): Sybils try to trade honestly with honest cluster
    elif 50 <= epoch < 100:
        honest = list(universe.suite_state['honest_cluster'])
        for cluster_key in ['sybil_cluster_a', 'sybil_cluster_b']:
            sybils = list(universe.suite_state[cluster_key])
            for s_node in universe.rng.sample(sybils, 5):
                h_node = universe.rng.choice(honest)
                # Trade SMALL to build acquaintance link without triggering Risk
                universe.propose_transaction(s_node, h_node, universe.rng.uniform(5, 15))
                # Also trade honest cluster -> sybil (sybil sells to honest)
                universe.propose_transaction(h_node, s_node, universe.rng.uniform(5, 15))

    # 3. Coordinated Attack (100-150): Sybils default on honest nodes
    elif 100 <= epoch < 150:
        honest = list(universe.suite_state['honest_cluster'])
        for cluster_key in ['sybil_cluster_a', 'sybil_cluster_b']:
            sybils = list(universe.suite_state[cluster_key])
            for s_node in sybils:
                h_node = universe.rng.choice(honest)
                # Maximum extraction
                cap = universe.credit_capacity[h_node]
                universe.propose_transaction(s_node, h_node, 0.5 * cap, is_attack=True)

def run(universe, progress=None, sub_task=None):
    from sim.config import RESULTS_DIR
    print("\n--- Running Coordinated Market Manipulation Scenario ---")
    out_dir = universe.result_dir or RESULTS_DIR
    csv_path = os.path.join(out_dir, f"manipulation_test_{universe.seed}.csv")
    
    with open(csv_path, 'w', newline='') as file:
        writer = csv.writer(file)
        writer.writerow(["epoch", "honest_trust", "sybil_a_trust", "sybil_b_trust"])
        
        for epoch in range(universe.epoch, 150):
            if progress and sub_task is not None:
                progress.advance(sub_task, 1)
            step(universe, epoch)
            universe.tick()

            # Periodic checkpointing (Fix 5: Robust Resumption)
            if (epoch + 1) % 25 == 0 and universe.result_dir and universe.task_id:
                checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
                universe.save_state(checkpoint_path)
            
            h = universe.suite_state['honest_cluster']
            sa = universe.suite_state['sybil_cluster_a']
            sb = universe.suite_state['sybil_cluster_b']
            
            gt = as_numpy(universe.global_trust)
            h_trust = float(gt[list(h)].sum()) / len(h)
            sa_trust = float(gt[list(sa)].sum()) / len(sa)
            sb_trust = float(gt[list(sb)].sum()) / len(sb)
            
            writer.writerow([epoch, f"{h_trust:.6f}", f"{sa_trust:.6f}", f"{sb_trust:.6f}"])
            
    print(f"Telemetry saved to {csv_path}")
