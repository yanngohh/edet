"""Cold start bootstrapping suite — zero pre-vouching.

Directly tests Theorem 2.3 (Fair Bootstrapping) under its strictest reading:
a network starting with ALL nodes unvouched (staked_capacity = 0, vouchers = {})
must be able to bootstrap positive economic activity through trial transactions
(PATH 0) alone. This matches the Rust production behavior at capacity.rs:11-24,
which explicitly supports "GRADUATION" from trials to positive capacity.

How it works (matching the Rust reference path):

    1. Every agent begins at V_staked = 0, no contracts, no acquaintances.
       - get_vouched_capacity = 0 and get_credit_capacity = 0 (no trust yet).
       - is_bootstrap_eligible = true because vouched_cap <= 0.

    2. Trial transactions below eta * V_base (= 50 units) are allowed to every
       bootstrap-eligible buyer regardless of capacity. Status is forced
       Pending; the capacity check is bypassed for trials.

    3. When a seller approves a trial, a DebtContract is created. The seller
       adds the buyer as an acquaintance and publishes their trust row.

    4. When the debtor (buyer) later acts as a seller in a subsequent
       transaction, the support cascade transfers the trial debt. When fully
       drained, the contract is marked Transferred, and BOTH directions of the
       trust edge gain S-mass:
         - Creditor-side: S[creditor][debtor] += amount
         - Debtor-side:   S[debtor][creditor] += amount  (Repayment Satisfaction)
       (See _transfer_debt in universe.py, mirroring Rust sf_counters.rs:267-298.)

    5. Once bilateral S-mass exists, EigenTrust produces a non-zero reputation
       for the graduated agent. Since the capacity formula has no vouching
       gate (matching Rust capacity.rs:11-24), positive trust yields positive
       capacity: Cap = beta * ln(rel_rep) * saturation, even with V_staked = 0.

    6. The agent's first transaction beyond the trial threshold can now use
       PATH 1/2 (risk-scored rather than bootstrap-eligible).

Contrast with `_setup_genesis_vouching()` in verify_theory.py, which grants
EVERY node base_capacity of initial stake. That shortcut is used by every
OTHER suite for efficiency. This suite is the one place where the shortcut is
deliberately withheld so the pure-trial bootstrap mechanism can be validated
as it would behave in the first days of a real deployment.

Whitepaper references:
    - Theorem 2.3 (Bootstrap): new entrants with bounded trial capacity build
      reputation via successful trials.
    - §2 Bootstrap: trial transactions are the protocol's cold-start mechanism.
    - Property 1.8 (Fair Bootstrapping): no privileged genesis required.

Pass criteria (see verify_cold_start in verify_theory.py):
    1. pct_graduated: fraction of nodes with n_S > 0 (successful transfers).
    2. pct_with_capacity: fraction of nodes with credit_capacity > 0 via
       the Rust "GRADUATION" mechanism.
    3. total_outstanding_debt: bounded by 3 * N * trial_ceiling (safety factor
       on the §6 Newcomer per-identity bound).
"""
import csv
import os
from typing import List, Tuple

import numpy as np

from sim.config import RESULTS_DIR
from sim.universe import as_numpy


def _ensure_zero_staked(universe) -> None:
    """Reset every node to zero staked capacity and zero vouchers.

    This undoes any staking that might have been applied by the harness's
    genesis-vouching shortcut before this suite runs. Called once at epoch 0.
    """
    for i in range(universe.size):
        universe.staked_capacity[i] = 0.0
        universe.vouchers[i] = {}
    universe.update_credit_capacity()


def step(universe, epoch: int) -> Tuple[List, List]:
    """Cold start dynamics — pure trial bootstrap.

    Every epoch, a random subset of nodes attempts to transact. We size
    transactions according to the buyer's current state:

      * No acquaintance or no successful transfers: trial-sized amount (PATH 0).
      * Graduated (n_S > 0 AND capacity > 0): normal-sized (PATH 1/2).

    Graduation happens automatically once a buyer's first trial contract is
    transferred via the support cascade — the Rust "GRADUATION" transition.
    """
    size = universe.size

    if epoch == 0:
        _ensure_zero_staked(universe)
        print(f"\n[INFO] Cold Start: {size} nodes, 0 vouched, 0 staked capacity "
              f"(pure trial bootstrap).")

    # Rate-limited activity: ~N/3 transactions per epoch. Cold-start throughput
    # is gated by the per-seller trial velocity limit (L_trial = 5) and the
    # per-buyer trial debt cap. Attempting more just fills rejection counters.
    tx_count = max(1, size // 3)

    trial_threshold = universe.params.trial_fraction * universe.params.base_capacity

    for _ in range(tx_count):
        buyer = universe.rng.randint(0, size - 1)
        seller = universe.rng.randint(0, size - 1)
        if seller == buyer:
            continue

        cap_buyer = float(universe.credit_capacity[buyer])
        graduated = (cap_buyer > 0.0
                     and universe.successful_transfers_global[buyer] > 0)

        if graduated:
            # Graduated buyer: normal-sized transactions (5-15% of capacity).
            amount = max(10.0, universe.rng.uniform(0.05 * cap_buyer,
                                                    0.15 * cap_buyer))
        else:
            # Bootstrap-eligible: trial-sized amount (< eta * V_base).
            amount = universe.rng.uniform(0.3 * trial_threshold,
                                          0.9 * trial_threshold)

        universe.propose_transaction(buyer, seller, amount)

    return [], []


def run(universe, epochs: int = 400) -> List:
    print(f"\n--- Running Cold Start Scenario (N={universe.size}, pure trial bootstrap) ---")
    out_dir = universe.result_dir if universe.result_dir else RESULTS_DIR
    os.makedirs(out_dir, exist_ok=True)
    csv_path = os.path.join(out_dir, f"cold_start_{universe.seed}.csv")

    with open(csv_path, 'w', newline='') as f:
        writer = csv.writer(f)
        writer.writerow([
            "epoch", "avg_trust", "pct_acquainted", "pct_graduated",
            "pct_with_capacity", "avg_capacity", "total_genesis_debt",
        ])

        total_genesis = 0.0
        for epoch in range(epochs):
            step(universe, epoch)
            universe.tick()
            total_genesis += float(universe.genesis_debt_this_epoch)

            trusts = as_numpy(universe.global_trust)
            caps = as_numpy(universe.credit_capacity)

            acquainted = sum(1 for i in range(universe.size)
                             if len(universe.acquaintances[i]) > 1)  # exclude self-acq
            graduated = sum(1 for i in range(universe.size)
                            if universe.successful_transfers_global[i] > 0)
            with_capacity = int(np.sum(caps > 0))

            writer.writerow([
                epoch,
                f"{float(trusts.sum()) / universe.size:.6f}",
                f"{acquainted / universe.size:.4f}",
                f"{graduated / universe.size:.4f}",
                f"{with_capacity / universe.size:.4f}",
                f"{float(caps.sum()) / universe.size:.4f}",
                f"{total_genesis:.2f}",
            ])

    print(f"Telemetry saved to {csv_path}")
    return []
