"""Transaction Spam simulation.

Attackers behave honestly for BUILD_EPOCHS, building genuine acquaintance links
and bilateral history, then flood the network with high-volume small transactions
to bloat the trust graph (a realistic DoS pattern: earn trust, then spam).
"""
import csv
import os
from sim.config import RESULTS_DIR

BUILD_EPOCHS = 20  # honest phase before spam begins


def step(universe, epoch):
    if 'spam_attackers' not in universe.suite_state:
        all_indices = list(range(universe.size))
        n_attackers = universe.size // 10
        universe.suite_state['spam_attackers'] = set(all_indices[:n_attackers])

    attackers = universe.suite_state['spam_attackers']
    all_nodes = list(range(universe.size))

    # Normal background economy (entire network, all epochs)
    for _ in range(universe.size // 2):
        b, s = universe.rng.sample(all_nodes, 2)
        universe.propose_transaction(b, s, universe.rng.uniform(10, 100))

    if epoch < BUILD_EPOCHS:
        # BUILD PHASE: attackers participate honestly — moderate volume, normal amounts
        # This gives them real bilateral S values with many peers before spamming.
        for a in attackers:
            target = universe.rng.choice([n for n in all_nodes if n != a])
            universe.propose_transaction(a, target, universe.rng.uniform(10, 50))
    else:
        # SPAM PHASE: high-frequency tiny transactions to saturate the trust graph
        for a in attackers:
            for _ in range(5):  # 5x normal rate per attacker
                target = universe.rng.choice([n for n in all_nodes if n != a])
                universe.propose_transaction(a, target, 1.0)  # trial-sized


def run(universe, progress=None, sub_task=None):
    from sim.config import RESULTS_DIR
    print("\n--- Running Transaction Spam Scenario ---")
    total_epochs = BUILD_EPOCHS + 50
    out_dir = universe.result_dir or RESULTS_DIR
    csv_path = os.path.join(out_dir, f"spam_test_{universe.seed}.csv")

    with open(csv_path, 'w', newline='') as file:
        writer = csv.writer(file)
        writer.writerow(["epoch", "phase", "total_contracts", "avg_acquaintances"])

        for epoch in range(universe.epoch, total_epochs):
            if progress and sub_task is not None:
                progress.advance(sub_task, 1)
            step(universe, epoch)
            universe.tick()

            # Periodic checkpointing (Fix 5: Robust Resumption)
            if (epoch + 1) % 25 == 0 and universe.result_dir and universe.task_id:
                checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
                universe.save_state(checkpoint_path)

            total_c = sum(len(c_list) for c_list in universe.contracts)
            avg_acq = sum(len(a) for a in universe.acquaintances) / universe.size
            phase = "build" if epoch < BUILD_EPOCHS else "spam"

            writer.writerow([epoch, phase, total_c, f"{avg_acq:.2f}"])

    print(f"Telemetry saved to {csv_path}")
