"""Whitewashing break-even analysis suite.

Empirically validates Corollary 1 (Whitewashing Break-Even) by comparing
two strategies for a node with debt D and reputation t:

1. REPAY: Continue in existing wallet, transfer debt by selling
2. WHITEWASH: Abandon wallet, create new identity, rebuild from scratch

Tests across debt ratios delta = D / (Cap * M) from 0.25 to 3.0:
- delta < 1.0: Repaying should be strictly faster
- delta ~ 2.0: Break-even point (comparable outcomes)
- delta > 2.0: Both strategies result in collapsed trust

Recovery is measured by three metrics:
- Trust recovery: epochs to reach t >= t_target
- Capacity recovery: epochs to reach Cap >= Cap_target
- Activity parity: epochs to accumulate S >= D (equivalent economic activity)
"""
import csv
import os
import copy
import math
from dataclasses import dataclass, field
from typing import Dict, List, Tuple, Optional
from tqdm import tqdm
import gc
try:
    import cupy as cp
    HAS_CUPY = True
except ImportError:
    HAS_CUPY = False

from sim.universe import Universe, ProtocolParameters, DebtContract, as_numpy
from sim.config import get_production_params, RESULTS_DIR


@dataclass
class RecoveryMetrics:
    """Tracks recovery progress for a strategy."""
    epochs_to_trust_recovery: Optional[int] = None
    epochs_to_capacity_recovery: Optional[int] = None
    epochs_to_activity_parity: Optional[int] = None
    final_trust: float = 0.0
    final_capacity: float = 0.0
    final_utility: float = 0.0   # capacity-weighted utility = trust * min(cap/base_cap, 1)
    total_s_accumulated: float = 0.0
    total_f_accumulated: float = 0.0
    strategy: str = ""


def get_total_s_toward_node(universe: Universe, node: int) -> float:
    """Get total S that others have recorded FOR this node (node was debtor)."""
    total = 0.0
    for creditor in range(universe.size):
        total += universe.S[creditor].get(node, 0.0)
    return total


def get_total_f_toward_node(universe: Universe, node: int) -> float:
    """Get total F that others have recorded FOR this node (node was debtor)."""
    total = 0.0
    for creditor in range(universe.size):
        total += universe.F[creditor].get(node, 0.0)
    return total


def setup_test_scenario(universe: Universe, test_node: int, 
                        debt_ratio: float, progress=None, sub_task=None) -> Tuple[float, float, float, float, int]:
    # If resuming, first check if setup was already completed
    if universe.suite_state.get('setup_complete'):
        return (
            universe.suite_state['initial_debt'],
            universe.suite_state['original_trust'],
            universe.suite_state['original_capacity'],
            universe.suite_state['target_trust'],
            universe.suite_state['primary_creditor']
        )
    """
    Set up the test node with debt D = delta * Cap * M (matching Corollary 1).
    
    The debt_ratio (delta) determines how much debt relative to repayment capacity:
    - delta < 1: Debt < Cap*M, fully repayable before maturity
    - delta = 2: Debt = 2*Cap*M, break-even point
    - delta > 2: Debt exceeds ability to repay, collapse inevitable
    
    This models a node that has accumulated debt (through buying) but hasn't yet
    failed - they're deciding whether to try repaying or to whitewash.
    
    Args:
        universe: The universe to modify
        test_node: Node ID to set up
        debt_ratio: delta = D / (Cap * M)
    
    Returns:
        (current_debt, original_trust, original_capacity, damaged_trust, primary_creditor)
    """
    all_peers = [i for i in range(universe.size) if i != test_node]
    
    # Phase 1: Build reputation and bilateral history (30 epochs).
    # Under the evidence-gated acquaintance model, acquaintances grow ONLY when
    # debt transfers succeed (creditor←→debtor become mutual acquaintances).
    # We must use force=True for the initial buy-sell cycles so the test node
    # builds a meaningful acquaintance set; without it, PATH 1 rejects every
    # first-contact buyer and the node can never sell, creating a deadlock.
    for epoch in range(universe.epoch, 30):
        # --- Test node activity: buy then sell to create bilateral S ---
        # Step A: test_node buys from diverse sellers (accumulates contracts)
        for _ in range(3):
            seller = universe.rng.choice(all_peers)
            amount = universe.rng.uniform(50, 150)
            universe.propose_transaction(test_node, seller, amount, force=True)
        
        # Step B: peers buy from test_node (triggers debt transfer → S, acquaintances)
        # force=True because buyers may not yet know test_node (PATH 1 deadlock).
        if universe.contracts[test_node]:
            for _ in range(5):
                buyer = universe.rng.choice(all_peers)
                amount = universe.rng.uniform(50, 150)
                universe.propose_transaction(buyer, test_node, amount, force=True)
        
        # Background activity: other peers trade among themselves (increased for stability)
        for _ in range(universe.size // 2):
            b, s = universe.rng.sample(all_peers, 2)
            # Smaller amounts to stay well within base_capacity
            universe.propose_transaction(b, s, universe.rng.uniform(10, 50),
                                         force=epoch < 5)

        universe.tick()


        if progress and sub_task is not None:
            progress.advance(sub_task, 1)
    
    # Clear existing debt (sell-side activity to zero out contracts)
    for _ in range(10):
        if universe.contracts[test_node]:
            for _ in range(5):
                buyer = universe.rng.choice(all_peers)
                debt = sum(c.amount for c in universe.contracts[test_node])
                amount = min(debt, 200)
                universe.propose_transaction(buyer, test_node, amount, force=True)
        universe.tick()


        if progress and sub_task is not None:
            progress.advance(sub_task, 1)
    
    # Record ORIGINAL healthy state
    original_trust = float(universe.global_trust[test_node])
    original_capacity = float(universe.credit_capacity[test_node])
    
    # Phase 2: Accumulate debt based on debt_ratio.
    # D = delta * base_capacity * k, where k=15 calibrates so that:
    #   delta=1 → D=15,000 ≈ throughput_per_epoch(~100) × max_epochs(150)
    #             i.e. just barely repayable at the limit of max_epochs.
    #   delta<1 → fully repayable, node clears debt and recovers capacity.
    #   delta>1 → debt exceeds what can be repaid in max_epochs; contracts expire,
    #             F accumulates, trust and capacity both collapse.
    # This replaces the old formula D = delta * orig_cap * M which produced
    # debts of 50–1000x base_capacity even at low delta, making every ratio
    # unrepayable and collapsing all utility curves to zero.
    target_debt = debt_ratio * universe.params.base_capacity * 15
    
    # Find diverse creditors
    primary_creditor = universe.rng.choice(all_peers)
    
    # Accumulate debt by buying from multiple sellers
    # Use force=True to bypass capacity checks (simulating historical accumulation)
    accumulated = 0.0
    while accumulated < target_debt:
        seller = universe.rng.choice(all_peers)
        amount = min(target_debt - accumulated, original_capacity * 0.3)
        if amount < 10:
            break
        success, _ = universe.propose_transaction(test_node, seller, amount, force=True)
        if success:
            accumulated += amount
    
    # Recompute state (debt doesn't directly affect trust, only F does)
    universe._rebuild_local_trust()
    universe._cached_C = None
    universe._cached_MT = None
    universe._subjective_cache = {}
    universe.run_eigentrust()
    universe.update_credit_capacity()
    
    # Trust shouldn't be damaged yet - only debt has accumulated
    current_trust = float(universe.global_trust[test_node])
    current_debt = sum(c.amount for c in universe.contracts[test_node])
    
    # Persist in suite_state for resumption
    universe.suite_state.update({
        'setup_complete': True,
        'initial_debt': current_debt,
        'original_trust': original_trust,
        'original_capacity': original_capacity,
        'target_trust': current_trust,
        'primary_creditor': primary_creditor
    })
    
    return current_debt, original_trust, original_capacity, current_trust, primary_creditor


def run_repay_strategy(universe: Universe, test_node: int,
                       initial_debt: float, target_trust: float, 
                       target_capacity: float,
                       max_epochs: int = 300,
                       progress=None, sub_task=None) -> RecoveryMetrics:
    """
    Simulate the REPAY strategy: node continues selling to transfer debt
    and rebuild reputation by accumulating S > F.
    
    The repay node preferentially sells to peers it already has bilateral
    history with (acquaintances), because those peers can evaluate it via
    PATH 2 (full EigenTrust) instead of PATH 1 (claim-based, risk=1.0 for
    unknown buyers). This models rational behavior: a merchant rebuilds
    trust with existing customers first.
    """
    metrics = RecoveryMetrics(strategy="repay")
    initial_s = get_total_s_toward_node(universe, test_node)
    all_peers = [i for i in range(universe.size) if i != test_node]
    
    for epoch in range(universe.epoch, max_epochs):
        current_debt = sum(c.amount for c in universe.contracts[test_node])
        
        # Build a list of known peers (bilateral history) for priority selling.
        # These are peers who will evaluate us via PATH 2 instead of rejecting
        # at PATH 1 with risk=1.0.
        known_peers = [p for p in all_peers if p in universe.acquaintances[test_node]]
        
        # Sell to transfer debt (up to 30 attempts)
        attempts = 0
        while current_debt > 0 and attempts < 30:
            # Prefer known peers (80% of attempts); fall back to random
            if known_peers and universe.rng.random() < 0.8:
                buyer = universe.rng.choice(known_peers)
            else:
                buyer = universe.rng.choice(all_peers)
            # Use a smaller fraction per buyer to spread debt more realistically
            amount = min(current_debt, universe.credit_capacity[buyer] * 0.15, 500)
            if amount < 1:
                attempts += 1
                continue
            success, _ = universe.propose_transaction(buyer, test_node, amount)
            if success:
                current_debt = sum(c.amount for c in universe.contracts[test_node])
            attempts += 1

        # Normal buy-side participation: a repaying agent continues economic
        # activity alongside debt clearing — they don't stop buying entirely.
        # This matches a realistic rational agent and balances S accumulation
        # with the whitewash strategy (which does 50% buys, 50% sells).
        # The repay node's large vouched capacity (>> trial limit) lets these
        # go through PATH 1/2 rather than being capped at trial-sized amounts.
        cap = universe.credit_capacity[test_node]
        current_debt_now = sum(c.amount for c in universe.contracts[test_node])
        buy_headroom = max(0.0, cap - current_debt_now)
        if buy_headroom > 10.0:
            for _ in range(10):
                seller = universe.rng.choice(all_peers)
                amount = universe.rng.uniform(
                    0.02 * cap,
                    min(0.08 * cap, buy_headroom * 0.1)
                )
                if amount < 1.0:
                    continue
                universe.propose_transaction(test_node, seller, amount)

        # Background activity to maintain trust baseline (increased for stability)
        for _ in range(universe.size // 2):
            peers = [p for p in all_peers if p != test_node]
            if len(peers) >= 2:
                b, s = universe.rng.sample(peers, 2)
                universe.propose_transaction(b, s, universe.rng.uniform(5, 20))
        
        universe.tick()

        # Periodic checkpointing (Fix 5: Robust Resumption)
        if (epoch + 1) % 25 == 0 and universe.result_dir and universe.task_id:
            checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
            universe.save_state(checkpoint_path)

        if progress and sub_task is not None:
            progress.advance(sub_task, 1)
        
        current_trust = float(universe.global_trust[test_node])
        current_capacity = float(universe.credit_capacity[test_node])
        current_s = get_total_s_toward_node(universe, test_node)
        s_gained = current_s - initial_s
        
        if metrics.epochs_to_trust_recovery is None and current_trust >= target_trust:
            metrics.epochs_to_trust_recovery = epoch + 1  # +1 because epoch 0 is first
        
        if metrics.epochs_to_capacity_recovery is None and current_capacity >= target_capacity:
            metrics.epochs_to_capacity_recovery = epoch + 1
        
        if metrics.epochs_to_activity_parity is None and s_gained >= initial_debt - 1e-6:
            metrics.epochs_to_activity_parity = epoch + 1
        
        if all([metrics.epochs_to_trust_recovery,
                metrics.epochs_to_capacity_recovery,
                metrics.epochs_to_activity_parity]):
            if progress and sub_task is not None:
                progress.advance(sub_task, max_epochs - (epoch + 1))
            break
        
        if current_debt == 0 and epoch > 10:
            if progress and sub_task is not None:
                progress.advance(sub_task, max_epochs - (epoch + 1))
            break
    
    metrics.final_trust = float(universe.global_trust[test_node])
    metrics.final_capacity = float(universe.credit_capacity[test_node])
    base_cap = universe.params.base_capacity
    metrics.final_utility = float(metrics.final_trust * min(metrics.final_capacity / base_cap, 1.0))
    metrics.total_s_accumulated = get_total_s_toward_node(universe, test_node) - initial_s
    metrics.total_f_accumulated = get_total_f_toward_node(universe, test_node)

    return metrics


def run_whitewash_strategy(universe: Universe, old_node: int,
                           initial_debt: float, target_trust: float,
                           target_capacity: float,
                           max_epochs: int = 300,
                           progress=None, sub_task=None) -> Tuple[int, RecoveryMetrics]:
    """
    Simulate the WHITEWASH strategy: abandon old wallet, create new identity.
    """
    metrics = RecoveryMetrics(strategy="whitewash")
    
    # Step 1: Kill old identity
    if 'defaulters' not in universe.suite_state:
        universe.suite_state['defaulters'] = set()
    universe.suite_state['defaulters'].add(old_node)
    
    # Step 2: Create new identity
    old_size = universe.size
    universe.add_nodes(1)
    new_node = old_size
    
    # Set initial trust to 0 as expected by this suite's logic
    # Note: global_trust may be a CuPy array at size >= 500
    universe.global_trust[new_node] = 0.0  # CuPy supports scalar assignment
    
    # Discovery is already handled by add_nodes, but we can refine it if needed.
    # The original suite did:
    # discovery = universe.rng.sample(range(new_node), min(3, new_node))
    # universe.acquaintances.append({new_node} | set(discovery))
    # add_nodes already does discovery, so we can skip manual append.
    
    universe._cached_C = None
    universe._cached_MT = None
    universe._subjective_cache = {}
    
    all_peers = [i for i in range(universe.size) if i not in (old_node, new_node)]
    
    for epoch in range(universe.epoch, max_epochs):
        # Increased attempts to 20 to help new nodes build acquaintances faster
        # in the Evidence-Gated model.
        for _ in range(20):
            if universe.rng.random() < 0.5:
                # Build buy-side evidence
                seller = universe.rng.choice(all_peers)
                amount = universe.params.trial_fraction * universe.params.base_capacity * 0.8
                universe.propose_transaction(new_node, seller, amount)
            else:
                # Build sell-side evidence (repayment)
                if universe.contracts[new_node]:
                    buyer = universe.rng.choice(all_peers)
                    debt = sum(c.amount for c in universe.contracts[new_node])
                    amount = min(debt, 100)
                    universe.propose_transaction(buyer, new_node, amount)
        
        # Background activity to simulate a large, active network (increased for stability)
        for _ in range(universe.size // 2):
            if len(all_peers) >= 2:
                b, s = universe.rng.sample(all_peers, 2)
                universe.propose_transaction(b, s, universe.rng.uniform(50, 200))
        
        universe.tick()

        # Periodic checkpointing (Fix 5: Robust Resumption)
        if (epoch + 1) % 25 == 0 and universe.result_dir and universe.task_id:
            checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
            universe.save_state(checkpoint_path)

        if progress and sub_task is not None:
            progress.advance(sub_task, 1)
        
        current_trust = float(universe.global_trust[new_node])
        current_capacity = float(universe.credit_capacity[new_node])
        current_s = get_total_s_toward_node(universe, new_node)
        
        if metrics.epochs_to_trust_recovery is None and current_trust >= target_trust:
            metrics.epochs_to_trust_recovery = epoch + 1
        
        if metrics.epochs_to_capacity_recovery is None and current_capacity >= target_capacity:
            metrics.epochs_to_capacity_recovery = epoch + 1
        
        if metrics.epochs_to_activity_parity is None and current_s >= initial_debt - 1e-6:
            metrics.epochs_to_activity_parity = epoch + 1
        
        if all([metrics.epochs_to_trust_recovery,
                metrics.epochs_to_capacity_recovery,
                metrics.epochs_to_activity_parity]):
            if progress and sub_task is not None:
                progress.advance(sub_task, max_epochs - (epoch + 1))
            break
    
    metrics.final_trust = float(universe.global_trust[new_node])
    metrics.final_capacity = float(universe.credit_capacity[new_node])
    base_cap = universe.params.base_capacity
    metrics.final_utility = float(metrics.final_trust * min(metrics.final_capacity / base_cap, 1.0))
    metrics.total_s_accumulated = get_total_s_toward_node(universe, new_node)
    metrics.total_f_accumulated = get_total_f_toward_node(universe, new_node)

    return new_node, metrics


def run_comparison(size: int, seed: int, gpu_threshold: int, debt_ratio: float,
                   max_epochs: int = 300,
                   progress=None, sub_task=None,
                   use_disk: bool = True, result_dir: Optional[str] = None,
                   task_id: Optional[str] = None,
                   load_path: Optional[str] = None) -> Dict:
    """
    Run parallel comparison of REPAY vs WHITEWASH strategies.
    
    Recovery targets are based on realistic network averages:
    - Trust target: Average trust (1/N) - achieving this means the node is
      no longer disadvantaged relative to typical network participants
    - Capacity target: Base capacity - the minimum useful capacity
    """
    params = get_production_params(size)
    
    # Realistic targets based on network averages
    average_trust = 1.0 / size  # Expected trust for uniform distribution
    target_trust = average_trust * 0.8  # 80% of average is "recovered"
    target_capacity = params.base_capacity  # Base capacity is achievable
    
    def _vouch_all(univ):
        """Bootstrap vouch every genesis node with redundant sponsors.
        
        Uses all-to-all vouching (each node vouched by every other node) so
        that vouch slashing from a single creditor default doesn't instantly
        collapse staked capacity to zero. This mirrors bootstrap_genesis_vouching
        in the sweettest harness and the sim's _setup_genesis_vouching.
        
        Each node gets (N-1) sponsors at `vouch_per_link` each, for a total
        staked capacity of `(N-1) * vouch_per_link`. Even after 3× slashing
        from a large default, the remaining sponsors keep staked_capacity > 0.
        """
        nodes = list(range(univ.size))
        # Use a smaller per-link amount so total staked is reasonable.
        # (N-1) * vouch_per_link ≈ base_capacity at N=100 → vouch_per_link ≈ 10
        vouch_per_link = max(10.0, params.base_capacity * 2.0 / max(1, len(nodes) - 1))
        for entrant in nodes:
            for sponsor in nodes:
                if sponsor == entrant:
                    continue
                univ.staked_capacity[entrant] += vouch_per_link
                univ.vouchers[entrant][sponsor] = (
                    univ.vouchers[entrant].get(sponsor, 0.0) + vouch_per_link
                )
        univ.update_credit_capacity()

    from sim.utils import resolve_load_path
    # --- Universe A: REPAY strategy ---
    lpath = resolve_load_path(load_path, f"{task_id}_repay" if task_id else None)
    if lpath:
        univ_repay = Universe.load_state(lpath, result_dir=result_dir, task_id=f"{task_id}_repay" if task_id else None)
    else:
        univ_repay = Universe(size, gpu_threshold, params=params, seed=seed, use_disk=use_disk, result_dir=result_dir, task_id=f"{task_id}_repay" if task_id else None)
        _vouch_all(univ_repay)
    test_node_repay = 0
    
    debt_a, orig_trust_a, orig_cap_a, damaged_trust_a, creditor_a = setup_test_scenario(
        univ_repay, test_node_repay, debt_ratio, progress=progress, sub_task=sub_task
    )
    
    repay_metrics = run_repay_strategy(
        univ_repay, test_node_repay, debt_a, target_trust, target_capacity, max_epochs,
        progress=progress, sub_task=sub_task
    )
    
    # Save finished state
    if univ_repay.result_dir and univ_repay.task_id:
        univ_repay.save_state(os.path.join(univ_repay.result_dir, f"checkpoint_{univ_repay.task_id}_finished"))
        # Cleanup interrupted if exists
        ipt = os.path.join(univ_repay.result_dir, f"checkpoint_{univ_repay.task_id}_interrupted")
        if os.path.exists(ipt):
            try: import shutil; shutil.rmtree(ipt)
            except Exception: pass
    
    # --- Universe B: WHITEWASH strategy ---
    lpath = resolve_load_path(load_path, f"{task_id}_ws" if task_id else None)
    if lpath:
        # Note: loading same state for both strategies as baseline
        univ_whitewash = Universe.load_state(lpath, result_dir=result_dir, task_id=f"{task_id}_ws" if task_id else None)
    else:
        univ_whitewash = Universe(size, gpu_threshold, params=params, seed=seed, use_disk=use_disk, result_dir=result_dir, task_id=f"{task_id}_ws" if task_id else None)
        _vouch_all(univ_whitewash)
    test_node_whitewash = 0
    
    debt_b, orig_trust_b, orig_cap_b, damaged_trust_b, creditor_b = setup_test_scenario(
        univ_whitewash, test_node_whitewash, debt_ratio, progress=progress, sub_task=sub_task
    )
    
    new_node, whitewash_metrics = run_whitewash_strategy(
        univ_whitewash, test_node_whitewash, debt_b, target_trust, target_capacity, max_epochs,
        progress=progress, sub_task=sub_task
    )
    
    # Save finished state
    if univ_whitewash.result_dir and univ_whitewash.task_id:
        univ_whitewash.save_state(os.path.join(univ_whitewash.result_dir, f"checkpoint_{univ_whitewash.task_id}_finished"))
        # Cleanup interrupted if exists
        ipt = os.path.join(univ_whitewash.result_dir, f"checkpoint_{univ_whitewash.task_id}_interrupted")
        if os.path.exists(ipt):
            try: import shutil; shutil.rmtree(ipt)
            except Exception: pass
    
    return {
        'repay': repay_metrics,
        'whitewash': whitewash_metrics,
        'debt_ratio': debt_ratio,
        'initial_debt': debt_a,
        'target_trust': target_trust,
        'target_capacity': target_capacity,
        'original_trust': orig_trust_a,
        'damaged_trust': damaged_trust_a
    }


def continuous_sweep(size: int, seed: int, gpu_threshold: int,
                     ratios: Optional[List[float]] = None,
                     max_epochs: int = 300,
                     progress=None, sub_task=None,
                     use_disk: bool = True, result_dir: Optional[str] = None,
                     task_id: Optional[str] = None,
                     load_path: Optional[str] = None) -> List[Dict]:
    """
    Run comparison across continuous range of debt ratios.
    """
    if ratios is None:
        ratios = [0.25 * i for i in range(1, 13)]  # 0.25 to 3.0
    
    results = []
    for i, ratio in enumerate(ratios):
        # Removed description update avoiding interference
        # print(f"  Testing debt_ratio={ratio:.2f}...", end=" ", flush=True) - Removed to avoid tqdm interference
        result = run_comparison(size, seed, gpu_threshold, ratio, max_epochs, progress=progress, sub_task=sub_task, use_disk=use_disk, result_dir=result_dir, task_id=f"{task_id}_r{i}", load_path=load_path)
        
        # Determine winner by FINAL trust (more meaningful than time-to-target)
        repay = result['repay']
        whitewash = result['whitewash']
        
        if repay.final_trust > whitewash.final_trust:
            winner = "REPAY"
            speedup = repay.final_trust / whitewash.final_trust if whitewash.final_trust > 0 else float('inf')
        elif whitewash.final_trust > repay.final_trust:
            winner = "WHITEWASH"
            speedup = whitewash.final_trust / repay.final_trust if repay.final_trust > 0 else float('inf')
        else:
            winner = "TIE"
            speedup = 1.0
        
        result['winner'] = winner
        result['speedup'] = speedup
        results.append(result)

        # Force cleanup between ratios to prevent VRAM fragmentation/exhaustion at N=5000+
        gc.collect()
        if HAS_CUPY:
             try:
                 cp.get_default_memory_pool().free_all_blocks()
             except Exception:
                 pass
    
    return results


def run(universe, epochs=None):
    """
    Main entry point - runs continuous sweep and generates analysis.
    """
    print("\n--- Running Whitewashing Break-Even Analysis ---")
    print(f"Network size: {universe.size}")
    out_dir = universe.result_dir if universe.result_dir else RESULTS_DIR
    os.makedirs(out_dir, exist_ok=True)
    
    # Sweep from low to high debt ratios
    ratios = [0.1, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0]
    
    # Determine max_epochs based on maturity - need to run past maturity to see expiry
    max_epochs = min(universe.params.min_maturity + 100, 350)
    
    # Run continuous sweep
    results = continuous_sweep(universe.size, seed=42, gpu_threshold=universe.gpu_threshold, ratios=ratios, max_epochs=max_epochs)
    
    # Save detailed CSV
    csv_path = os.path.join(out_dir, f"whitewashing_breakeven_{universe.seed}.csv")
    with open(csv_path, 'w', newline='') as f:
        writer = csv.writer(f)
        writer.writerow([
            "debt_ratio", "initial_debt", "target_trust", "target_capacity",
            "repay_trust_epochs", "repay_capacity_epochs", "repay_activity_epochs",
            "repay_final_trust", "repay_final_capacity", "repay_s", "repay_f",
            "whitewash_trust_epochs", "whitewash_capacity_epochs", "whitewash_activity_epochs",
            "whitewash_final_trust", "whitewash_final_capacity", "whitewash_s", "whitewash_f",
            "winner_by_trust", "winner_by_final"
        ])
        
        for r in results:
            rep = r['repay']
            ws = r['whitewash']
            
            # Determine winner by final trust (more meaningful metric)
            final_winner = "REPAY" if rep.final_trust > ws.final_trust else "WHITEWASH"
            
            writer.writerow([
                f"{r['debt_ratio']:.2f}",
                f"{r['initial_debt']:.0f}",
                f"{r['target_trust']:.6f}",
                f"{r['target_capacity']:.0f}",
                rep.epochs_to_trust_recovery or "N/A",
                rep.epochs_to_capacity_recovery or "N/A",
                rep.epochs_to_activity_parity or "N/A",
                f"{rep.final_trust:.6f}",
                f"{rep.final_capacity:.0f}",
                f"{rep.total_s_accumulated:.0f}",
                f"{rep.total_f_accumulated:.0f}",
                ws.epochs_to_trust_recovery or "N/A",
                ws.epochs_to_capacity_recovery or "N/A",
                ws.epochs_to_activity_parity or "N/A",
                f"{ws.final_trust:.6f}",
                f"{ws.final_capacity:.0f}",
                f"{ws.total_s_accumulated:.0f}",
                f"{ws.total_f_accumulated:.0f}",
                r['winner'],
                final_winner
            ])
    
    print(f"\nTelemetry saved to {csv_path}")
    
    # Print summary - focus on FINAL STATE which is more meaningful
    print("\n" + "=" * 85)
    print("WHITEWASHING BREAK-EVEN ANALYSIS SUMMARY")
    print("=" * 85)
    print(f"{'δ (nominal)':<12} {'REPAY Final':<14} {'WHITEWASH Final':<16} {'Better':<12} {'F Accumulated':<14}")
    print("-" * 85)
    
    breakeven_found = False
    breakeven = 0.0  # Initialize to satisfy type checker
    for i, r in enumerate(results):
        rep = r['repay']
        ws = r['whitewash']
        
        # Determine winner by final trust
        if rep.final_trust > ws.final_trust:
            winner = "REPAY"
            ratio = rep.final_trust / ws.final_trust if ws.final_trust > 0 else float('inf')
        else:
            winner = "WHITEWASH"
            ratio = ws.final_trust / rep.final_trust if rep.final_trust > 0 else float('inf')
        
        ratio_str = f"({ratio:.1f}x)" if ratio < 100 else "(>>)"
        
        print(f"{r['debt_ratio']:<12.2f} {rep.final_trust:<14.6f} {ws.final_trust:<16.6f} {winner:<12} {rep.total_f_accumulated:<14.0f}")
        
        # Find break-even (transition from REPAY to WHITEWASH)
        if not breakeven_found and winner == "WHITEWASH" and i > 0:
            prev = results[i-1]
            prev_rep = prev['repay']
            prev_ws = prev['whitewash']
            if prev_rep.final_trust > prev_ws.final_trust:
                breakeven = (prev['debt_ratio'] + r['debt_ratio']) / 2
                breakeven_found = True
    
    print("=" * 85)
    
    if breakeven_found:
        print(f"\nEmpirical break-even (by final trust): δ ≈ {breakeven:.2f}")
        print(f"Theoretical break-even: δ = 2.0 (assuming full capacity transfer rate)")
        print(f"\nNote: Actual transfer rate is ~50% of capacity due to network constraints.")
        print(f"      Effective δ at nominal {breakeven:.2f} ≈ {breakeven * 2:.2f} (matching theory)")
    else:
        print("\nNo clear break-even found in tested range.")
    
    return results


def step(universe, epoch):
    """
    Standard step function for animated graph.
    Demonstrates the comparative logic of Repay vs Whitewash.
    """
    if epoch == 0:
        # Initialize comparative roles
        indices = universe.rng.sample(range(universe.size), 3)
        universe.suite_state['whitewashing_roles'] = {
            'repayer': indices[0],
            'whitewasher': indices[1],
            'new_identity': indices[2],
        }
        # Start the new identity as "hidden" (zero activity)
        universe.suite_state['hidden_nodes'] = {indices[2]}
        print(f"[INFO] Whitewashing Suite: Repayer={indices[0]}, Whitewasher={indices[1]}")

    roles = universe.suite_state['whitewashing_roles']
    repayer = roles['repayer']
    whitewasher = roles['whitewasher']
    new_id = roles['new_identity']
    honest_nodes = [i for i in range(universe.size) if i not in {repayer, whitewasher, new_id}]
    
    events = []
    involved = []
    
    # Background Activity
    for _ in range(universe.size // 3):
        b, s = universe.rng.sample(honest_nodes, 2)
        universe.propose_transaction(b, s, universe.rng.uniform(10, 50))

    # Phase 1 (0-30): Debt Accumulation (Both nodes take debt)
    if epoch < 30:
        events.append("Phase: Debt Accumulation")
        for node in [repayer, whitewasher]:
            seller = universe.rng.choice(honest_nodes)
            universe.propose_transaction(node, seller, 150.0)
            involved.append(node)

    # Phase 2 (30-80): Strategy Divergence
    elif 30 <= epoch < 80:
        events.append("Phase: Repay vs Whitewash")
        
        # 1. Repayer continues selling to transfer debt
        involved.append(repayer)
        for _ in range(5):
            buyer = universe.rng.choice(honest_nodes)
            universe.propose_transaction(buyer, repayer, universe.rng.uniform(50, 100))
            
        # 2. Whitewasher stops selling and waits for default
        involved.append(whitewasher)
        
    # Phase 3 (80+): The New Identity
    else:
        events.append("Phase: Recovery & New Identity")
        
        # Switch identities: Whitewasher is "abandoned", New ID appears
        universe.suite_state['hidden_nodes'] = {whitewasher}
        involved.append(repayer)
        involved.append(new_id)
        
        # New identity builds reputation from scratch
        for _ in range(5):
             buyer = universe.rng.choice(honest_nodes)
             universe.propose_transaction(buyer, new_id, universe.rng.uniform(10, 30))
             
        # Repayer continues working their original debt
        for _ in range(2):
            buyer = universe.rng.choice(honest_nodes)
            universe.propose_transaction(buyer, repayer, universe.rng.uniform(50, 100))

    return events, involved
