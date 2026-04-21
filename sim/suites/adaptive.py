"""Adaptive adversary simulation suite.

An adaptive attacker observes their own trust score each epoch and switches
strategy accordingly:
- HIGH trust (above 1.5 / N): exploit aggressively -- buy from honest sellers
  to accumulate debt, skip selling so contracts expire (F increments).
- LOW trust (below 0.5 / N): behave honestly -- sell to transfer debt, build
  reputation (S increments).

The key insight being tested: the rate-based attenuation phi(r) captures the
CUMULATIVE default rate F/(S+F) regardless of when the defaults occurred.
An adaptive timing strategy should NOT outperform a fixed-rate strategy.
"""
import csv
import os

from sim.universe import Universe, as_numpy


def setup(universe):
    """Pick one attacker node and initialize tracking state."""
    attacker = universe.rng.randint(0, universe.size - 1)
    honest_nodes = [i for i in range(universe.size) if i != attacker]

    universe.suite_state['adaptive_roles'] = {
        'attacker': attacker,
        'honest_nodes': honest_nodes,
    }
    # Mark as strategic defaulter (initially None = pays everyone)
    universe.suite_state['attacker_nodes'] = {attacker}
    universe.suite_state['strategic_defaulters'] = {attacker: None}

    # Telemetry accumulators
    universe.suite_state['trust_history'] = []
    universe.suite_state['mode_history'] = []
    universe.suite_state['cumulative_S'] = 0.0
    universe.suite_state['cumulative_F'] = 0.0
    universe.suite_state['cumulative_rate_history'] = []

    print(f"\n[INFO] Adaptive Adversary: attacker={attacker}, "
          f"network size={universe.size}")


def step(universe, epoch):
    """Execute one epoch of the adaptive adversary strategy."""
    if epoch == 0:
        setup(universe)

    roles = universe.suite_state['adaptive_roles']
    attacker = roles['attacker']
    honest_nodes = roles['honest_nodes']
    N = universe.size

    # --- Threshold decision ---
    attacker_trust = float(universe.global_trust[attacker])
    high_threshold = 1.5 / N
    low_threshold = 0.5 / N

    if attacker_trust > high_threshold:
        mode = 'attack'
    elif attacker_trust < low_threshold:
        mode = 'honest'
    else:
        # In the middle band, continue whatever we were doing last epoch
        prev = universe.suite_state['mode_history']
        mode = prev[-1] if prev else 'honest'

    universe.suite_state['trust_history'].append(attacker_trust)
    universe.suite_state['mode_history'].append(mode)

    events = []
    involved = set()

    # --- Background honest activity (keeps the economy moving) ---
    num_bg = max(10, N // 3)
    for _ in range(num_bg):
        b, s = universe.rng.sample(honest_nodes, 2)
        cap = universe.credit_capacity[b]
        universe.propose_transaction(b, s, max(10.0, universe.rng.uniform(0.05 * cap, 0.15 * cap)))

    if mode == 'honest':
        # --- Honest mode: sell to honest buyers (transfer debt, build S) ---
        # Also do small buys to maintain bilateral relationships
        universe.suite_state['strategic_defaulters'] = {attacker: None}

        for _ in range(8):
            buyer = universe.rng.choice(honest_nodes)
            amount = universe.rng.uniform(20.0, 80.0)
            success, _ = universe.propose_transaction(buyer, attacker, amount)
            if success:
                involved.add(buyer)
                involved.add(attacker)

        # Small buys to look like a normal participant
        for _ in range(2):
            seller = universe.rng.choice(honest_nodes)
            amount = universe.rng.uniform(10.0, 30.0)
            success, _ = universe.propose_transaction(attacker, seller, amount)
            if success:
                involved.add(attacker)
                involved.add(seller)

        events.append(f"Adaptive attacker {attacker}: HONEST mode "
                      f"(trust={attacker_trust:.6f})")

    else:
        # --- Attack mode: buy aggressively, DON'T sell (let contracts expire) ---
        # Block debt transfer by restricting pay set to empty (only pay self)
        universe.suite_state['strategic_defaulters'] = {attacker: {attacker}}

        for _ in range(5):
            seller = universe.rng.choice(honest_nodes)
            cap = universe.credit_capacity[attacker]
            amount = universe.rng.uniform(0.10 * cap, 0.25 * cap)
            success, _ = universe.propose_transaction(attacker, seller, amount)
            if success:
                involved.add(attacker)
                involved.add(seller)
                events.append(f"[ATTACK] Adaptive attacker {attacker} "
                              f"taking debt from {seller}")

        events.append(f"Adaptive attacker {attacker}: ATTACK mode "
                      f"(trust={attacker_trust:.6f})")

    # --- Update cumulative rate tracking ---
    total_S = 0.0
    total_F = 0.0
    for creditor in range(N):
        total_S += universe.S[creditor].get(attacker, 0.0)
        total_F += universe.F[creditor].get(attacker, 0.0)

    universe.suite_state['cumulative_S'] = total_S
    universe.suite_state['cumulative_F'] = total_F
    cumulative_rate = total_F / (total_S + total_F) if (total_S + total_F) > 0 else 0.0
    universe.suite_state['cumulative_rate_history'].append(cumulative_rate)

    return events, involved


def run(universe, epochs=200, seed=None, progress=None, sub_task=None):
    """Standard run loop with CSV telemetry."""
    print("\n--- Running Adaptive Adversary Simulation ---")
    from sim.config import RESULTS_DIR
    out_dir = universe.result_dir if universe.result_dir else RESULTS_DIR
    os.makedirs(out_dir, exist_ok=True)
    csv_path = os.path.join(out_dir, f"adaptive_adversary_{universe.seed}.csv")

    with open(csv_path, 'w', newline='') as f:
        writer = csv.writer(f)
        writer.writerow(["epoch", "attacker_trust", "attacker_capacity",
                          "mode", "cumulative_rate", "attacker_debt"])

        for epoch in range(universe.epoch, epochs):
            if progress and sub_task is not None:
                progress.advance(sub_task, 1)
            step(universe, epoch)
            universe.tick()

            # Periodic checkpointing (Fix 5: Robust Resumption)
            if (epoch + 1) % 25 == 0 and universe.result_dir and universe.task_id:
                checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
                universe.save_state(checkpoint_path)

            roles = universe.suite_state['adaptive_roles']
            atk = roles['attacker']
            atk_debt = sum(c.amount for c in universe.contracts[atk])
            mode_hist = universe.suite_state['mode_history']
            rate_hist = universe.suite_state['cumulative_rate_history']

            gt = as_numpy(universe.global_trust)
            cap = as_numpy(universe.credit_capacity)
            writer.writerow([
                epoch,
                f"{float(gt[atk]):.8f}",
                f"{float(cap[atk]):.2f}",
                mode_hist[-1] if mode_hist else 'unknown',
                f"{rate_hist[-1]:.6f}" if rate_hist else "0.000000",
                f"{atk_debt:.2f}",
            ])

    print(f"Telemetry saved to {csv_path}")
