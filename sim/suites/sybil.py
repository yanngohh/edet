"""Sybil ring attack suite.

A cluster of Sybil nodes trades internally to boost local trust, then
attempts to scam honest nodes.

Expected outcome (Theorem 2 / Corollary):
- Sybils have zero trust from any honest observer's perspective.
- Their credit capacity remains at V_base (trial transactions only).
- Scam attempts are rejected by honest sellers.
"""
import csv
import os
from sim.config import RESULTS_DIR
from sim.universe import as_numpy


def step(universe, epoch):
    if epoch == 0:
        sybil_count = max(2, universe.size // 4)
        sybil_start = universe.rng.randint(0, universe.size - sybil_count)
        sybils = list(range(sybil_start, sybil_start + sybil_count))

        universe.suite_state['sybil_roles'] = {
            'sybil_start': sybil_start,
            'sybil_count': sybil_count
        }

        # Establish sybil alliance for collusion logic
        sybil_set = set(sybils)
        universe.suite_state['attacker_nodes'] = sybil_set
        # strategic_defaulters is armed at attack phase start (epoch 20),
        # so honest trades during the build phase complete normally.

        # Sybils that are not explicitly vouched should have 0 staked capacity.
        # Genesis vouching (run before this suite) grants all nodes capacity,
        # so we revoke it here for unvouched sybils to model the realistic scenario
        # where attackers are newcomers without sponsor backing.
        # If 'sybils_are_vouched' is set, keep their capacity (Smart Attack scenario).
        is_vouched = universe.suite_state.get('sybils_are_vouched', False)
        if is_vouched:
            print("[INFO] Sybils are VOUCHED (Smart Attack scenario). Keeping staked capacity.")
        else:
            print("[INFO] Revoking staked capacity from unvouched Sybils.")
            for s in sybils:
                universe.staked_capacity[s] = 0.0
                universe.vouchers[s] = {}
            universe.update_credit_capacity()

        print(f"\n[INFO] Sybil Ring: {sybil_count} nodes [{sybil_start}..{sybil_start + sybil_count - 1}]")

    roles = universe.suite_state['sybil_roles']
    sybil_start = roles['sybil_start']
    sybil_count = roles['sybil_count']
    sybils = list(range(sybil_start, sybil_start + sybil_count))
    sybil_set = set(sybils)
    honest_pool = [i for i in range(universe.size) if i not in sybil_set]
    
    # Ensure Global Proxy reflects honest view
    universe.trusted_pool = honest_pool

    # Honest-only baseline economy: only honest nodes trade with each other.
    # Sybils are NOT part of the legitimate economy --- they only transact
    # internally (to inflate their own local trust) and externally as attacks.
    # This matches the Theorem 2 assumption: Sybils only transact among themselves.
    num_tx = universe.size * 2
    for _ in range(num_tx):
        b, s = universe.rng.sample(honest_pool, 2)
        cap = universe.credit_capacity[b]
        amount = max(10.0, universe.rng.uniform(0.05 * cap, 0.15 * cap))
        universe.propose_transaction(b, s, amount)

    # Attack logic starts after epoch 20
    if epoch < 20:
        # BUILD PHASE: sybils trade honestly with BOTH internal peers AND honest nodes.
        # This gives them real bilateral S values with honest nodes before attacking —
        # mirroring the real-world pattern of "behave first, attack later".

        # Internal ring churn (as before)
        for i in range(len(sybils)):
            seller = sybils[i]
            buyer = sybils[(i + 1) % len(sybils)]
            amount = universe.params.trial_fraction * universe.params.base_capacity * 0.9
            universe.propose_transaction(buyer, seller, amount)

        # External honest trades: sybils buy from honest nodes legitimately
        for s in universe.rng.sample(sybils, min(5, len(sybils))):
            h = universe.rng.choice(honest_pool)
            universe.propose_transaction(s, h, universe.rng.uniform(10, 30))
            # And sell to honest nodes (builds S[honest][sybil])
            universe.propose_transaction(h, s, universe.rng.uniform(10, 30))

        return ["Build: Sybil establishing external reputation"], []
    else:
        # ATTACK PHASE: arm strategic defaulters now that build phase is over
        universe.suite_state['strategic_defaulters'] = {s: sybil_set for s in sybils}
        return execute_sybil_attack(universe, sybils, honest_pool)


def execute_sybil_attack(universe, sybils, honest_pool):
    """
    Sybil Ring attack:
    1. Internal churn: ring topology trades to build local trust among themselves.
    2. Scam attempts: Sybils try to buy from honest sellers (after warmup).
    """
    sybil_count = len(sybils)
    events = []

    # 1. Internal ring churn (Sybils pay each other -> builds internal S_ij)
    # They use trial-sized transactions since they have no external reputation
    for i in range(sybil_count):
        seller = sybils[i]
        buyer = sybils[(i + 1) % sybil_count]
        # Trial transaction size (within V_base * eta)
        amount = universe.params.trial_fraction * universe.params.base_capacity * 0.9
        universe.propose_transaction(buyer, seller, amount, is_attack=True)

    # 2. Scam attempts: Sybils try to exploit honest sellers
    if len(honest_pool) >= 1:
        for _ in range(50):
            s = universe.rng.choice(honest_pool)
            b = universe.rng.choice(sybils)
            amount = 10.0
            universe.propose_transaction(b, s, amount, is_attack=True)

    return [f"Sybil ring ({sybil_count} nodes) active"], set(sybils)


def run(universe, progress=None, sub_task=None):
    print("\n--- Running Sybil Ring Attack Simulation ---")
    out_dir = universe.result_dir if universe.result_dir else RESULTS_DIR
    os.makedirs(out_dir, exist_ok=True)
    csv_path = os.path.join(out_dir, f"sybil_attack_{universe.seed}.csv")

    with open(csv_path, 'w', newline='') as file:
        writer = csv.writer(file)
        writer.writerow(["epoch", "avg_sybil_trust", "avg_sybil_capacity"])

        for epoch in range(universe.epoch, 150):
            if progress and sub_task is not None:
                progress.advance(sub_task, 1)

            step(universe, epoch)
            universe.tick()

            # Periodic checkpointing (Fix 5: Robust Resumption)
            if (epoch + 1) % 25 == 0 and universe.result_dir and universe.task_id:
                checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
                universe.save_state(checkpoint_path)
            roles = universe.suite_state.get('sybil_roles', {})
            ss = roles.get('sybil_start', 0)
            sc = roles.get('sybil_count', 0)

            if sc > 0:
                gt = as_numpy(universe.global_trust)
                cap = as_numpy(universe.credit_capacity)
                avg_trust = float(gt[ss:ss + sc].sum()) / sc
                avg_cap = float(cap[ss:ss + sc].sum()) / sc
            else:
                avg_trust = avg_cap = 0

            writer.writerow([epoch, f"{avg_trust:.6f}", f"{avg_cap:.2f}"])

    print(f"Telemetry saved to {csv_path}")
