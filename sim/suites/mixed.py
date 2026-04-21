"""Mixed attack and recovery suite.

75% honest nodes, 25% attackers using staggered waves of gateway, sybil,
slacker, and all-out attacks. Tests overall protocol resilience.

Expected outcome: honest nodes maintain >55% of total trust mass even
under sustained mixed attacks.
"""
import csv
import os
import math
import numpy as np
from .sybil import execute_sybil_attack
from .gateway import execute_gateway_attack
from .slacker import execute_slacker_attack
from . import flash_loan, manipulation, spam, griefing
from sim.config import RESULTS_DIR
from sim.universe import as_numpy
from tqdm import tqdm


def step(universe, epoch):
    if epoch == 0:
        all_indices = list(range(universe.size))
        h_size = universe.size * 3 // 4
        honest_nodes = universe.rng.sample(all_indices, h_size)
        honest_set = set(honest_nodes)
        attacker_pool = [i for i in all_indices if i not in honest_set]

        # Ally status initialized here
        cluster = set(attacker_pool)
        universe.suite_state['attacker_nodes'] = cluster
        # Strategic defaulters are configured dynamically per-epoch, 
        # but initialized here to establish ally status (collusion)
        universe.suite_state['strategic_defaulters'] = {a: None for a in attacker_pool}

        # Generate attack schedule
        phase_types = ['gateway', 'sybil', 'slacker', 'random_wave', 
                       'flash_loan', 'manipulation', 'spam', 'griefing']
        schedule = []
        annotations = []
        curr_t = 20

        while curr_t < 1950:
            p_type = universe.rng.choice(phase_types)
            duration = universe.rng.randint(20, 50)
            end_t = curr_t + duration

            if p_type == 'all_out' or universe.rng.random() < 0.1:
                active_squad = list(attacker_pool)
                p_type = 'all_out'
                label = "ALL OUT ASSAULT"
            else:
                squad_size = universe.rng.randint(2, max(5, len(attacker_pool) // 4))
                active_squad = universe.rng.sample(attacker_pool, squad_size)
                label = p_type.capitalize()

            schedule.append({
                'type': p_type, 'start': curr_t, 'end': end_t, 'squad': active_squad
            })
            annotations.append({'x': curr_t, 'label': label})

            gap = universe.rng.randint(80, 150)
            curr_t = end_t + gap

        universe.suite_state['mixed_roles'] = {
            'honest_nodes': honest_nodes,
            'attacker_pool': attacker_pool
        }
        universe.suite_state['mixed_schedule'] = schedule
        universe.suite_state['mixed_annotations'] = annotations

        # Anchor the global EigenTrust pre-trust to the honest majority.
        # Without this, the global p vector is uniform over ALL nodes (including
        # attackers), giving attackers equal teleportation mass despite being 25%
        # of the network. This mirrors what sybil.py does for the sybil scenario.
        universe.trusted_pool = list(honest_nodes)

        print(f"\n[INFO] Mixed Suite: Honest={len(honest_nodes)}, Attackers={len(attacker_pool)}")
        print(f"  - {len(schedule)} attack phases scheduled")

    roles = universe.suite_state['mixed_roles']
    honest_nodes = roles['honest_nodes']
    attacker_pool = roles['attacker_pool']
    schedule = universe.suite_state['mixed_schedule']
    events = []

    # Calculate active attackers for this epoch based on schedule and cooldown
    current_attackers = set()
    strategic = {}
    
    for phase in schedule:
        # Enforce a 60-epoch cooldown to ensure stolen debt matures and defaults
        if phase['start'] <= epoch < phase['end'] + 60:
            squad = phase['squad']
            
            # Nodes in active attack phases or cooldown become strategic defaulters
            for a in squad:
                current_attackers.add(a)
                strategic[a] = set(attacker_pool) # Only pay other attackers

    # Apply strategic defaulter state unconditionally BEFORE baseline transactions.
    # During gap epochs (no active attack phase or cooldown), attackers behave honestly.
    # During active epochs or cooldown, the active squad members are marked.
    universe.suite_state['strategic_defaulters'] = strategic

    # Baseline honest activity (Sub-linear scaling O(sqrt(N)) for speed)
    organic_nodes = honest_nodes + attacker_pool
    num_tx = 2 * math.isqrt(len(organic_nodes))
    queue = [i for i in organic_nodes if universe.total_debt[i] > 0]
    universe.rng.shuffle(queue)

    for _ in range(num_tx):
        buyer = universe.rng.choice(organic_nodes)
        seller = queue.pop(0) if queue else universe.rng.choice(organic_nodes)
        if buyer != seller:
            # Baseline trades should be trials to allow organic bootstrapping
            limit = universe.params.trial_fraction * universe.params.base_capacity
            amount = universe.rng.uniform(0.2 * limit, 0.8 * limit)
            universe.propose_transaction(buyer, seller, amount)

    # Extra liquidity (Sub-linear scaling)
    for _ in range(math.isqrt(len(organic_nodes))):
        b, s = universe.rng.sample(organic_nodes, 2)
        limit = universe.params.trial_fraction * universe.params.base_capacity
        amount = universe.rng.uniform(0.2 * limit, 0.8 * limit)
        universe.propose_transaction(b, s, amount)

    # Dynamic attack phases
    involved_nodes = set()
    
    # Now execute the attack sub-suites with the correct state
    for phase in schedule:
        if phase['start'] <= epoch < phase['end']:
            p_type = phase['type']
            squad = phase['squad']
            attack_epoch = epoch - phase['start']

            if p_type == 'gateway' and len(squad) >= 2:
                gw = squad[0]
                accomplices = squad[1:]
                victim = honest_nodes[epoch % len(honest_nodes)]
                capacity = universe.credit_capacity[gw]
                amount = universe.rng.uniform(0.2 * capacity, 0.5 * capacity)
                evts, inv = execute_gateway_attack(universe, gw, accomplices, victim,
                                                   attack_epoch, fixed_amount=amount)
                events.extend(evts)
                involved_nodes.update(inv)

            elif p_type == 'sybil':
                evts, inv = execute_sybil_attack(universe, squad, honest_nodes)
                events.extend(evts)
                involved_nodes.update(inv)

            elif p_type == 'slacker':
                evts, inv = execute_slacker_attack(universe, squad, honest_nodes)
                events.extend(evts)
                involved_nodes.update(inv)

            elif p_type in ('all_out', 'random_wave'):
                events.append(f"Attack: ALL OUT (Squad {len(squad)})")
                involved_nodes.update(squad)
                for atk in squad:
                    target = universe.rng.choice(honest_nodes)
                    involved_nodes.add(target)
                    capacity = universe.credit_capacity[target]
                    amount = universe.rng.uniform(0.1 * capacity, 0.5 * capacity)
                    universe.propose_transaction(atk, target, amount, is_attack=True)

            elif p_type == 'flash_loan':
                # Track attackers added by flash_loan
                flash_loan.step(universe, epoch)
                events.append(f"Attack: FLASH RECYCLE (Squad {len(squad)})")
                involved_nodes.update(squad)

            elif p_type == 'manipulation':
                # Simulate a coordinated subset attack
                manipulators = squad
                honest = honest_nodes
                for _ in range(len(manipulators) * 2):
                    atk = universe.rng.choice(manipulators)
                    tgt = universe.rng.choice(honest)
                    universe.propose_transaction(atk, tgt, universe.rng.uniform(50, 150), is_attack=True)
                events.append(f"Attack: COORDINATED MANIP (Squad {len(squad)})")
                involved_nodes.update(squad)

            elif p_type == 'spam':
                orig_spam = universe.suite_state.get('spam_attackers', set())
                universe.suite_state['spam_attackers'] = set(squad)
                spam.step(universe, epoch)
                universe.suite_state['spam_attackers'] = orig_spam
                events.append(f"Attack: SPAM (Squad {len(squad)})")
                involved_nodes.update(squad)

            elif p_type == 'griefing':
                orig_griefers = universe.suite_state.get('griefers', set())
                orig_victims = universe.suite_state.get('victims', [])
                universe.suite_state['griefers'] = set(squad)
                universe.suite_state['victims'] = universe.rng.sample(honest_nodes, min(len(honest_nodes), 5))
                griefing.step(universe, epoch)
                universe.suite_state['griefers'] = orig_griefers
                universe.suite_state['victims'] = orig_victims
                events.append(f"Attack: GRIEFING (Squad {len(squad)})")
                involved_nodes.update(squad)


    return events, list(involved_nodes)


def run(universe, progress=None, sub_task=None):
    print("\n--- Running Mixed Attack & Recovery Scenario ---")
    out_dir = universe.result_dir if universe.result_dir else RESULTS_DIR
    os.makedirs(out_dir, exist_ok=True)
    csv_path = os.path.join(out_dir, f"mixed_scenario_{universe.seed}.csv")

    with open(csv_path, 'w', newline='') as file:
        writer = csv.writer(file)
        writer.writerow(["epoch", "honest_avg_trust", "attacker_trust",
                         "honest_avg_pressure", "attacker_pressure",
                         "sybil_avg_trust", "sybil_avg_pressure"])

        max_attack_end = 0
        consecutive_zero_trust = 0
        
        for epoch in range(universe.epoch, 2000):
            step(universe, epoch)
            universe.tick()

            # Periodic checkpointing (Fix 5: Robust Resumption)
            if (epoch + 1) % 50 == 0 and universe.result_dir and universe.task_id:
                checkpoint_path = os.path.join(universe.result_dir, f"checkpoint_{universe.task_id}_interrupted")
                universe.save_state(checkpoint_path)

            # Dynamic step estimation: find the end of the last phase
            if epoch == 0:
                schedule = universe.suite_state.get('mixed_schedule', [])
                if schedule:
                    max_attack_end = max(p['end'] for p in schedule)
                    # Estimate total steps: last attack end + cooldown (60) + buffer (40)
                    estimated_total = min(2000, max_attack_end + 100)
                    if progress and sub_task is not None:
                        progress.update(sub_task, total=estimated_total)

            if progress and sub_task is not None:
                progress.update(sub_task, completed=epoch+1)

            roles = universe.suite_state['mixed_roles']
            honest_nodes = roles['honest_nodes']
            attacker_pool = roles['attacker_pool']

            # Use backend-aware stats to avoid full array transfers
            h_stats = universe.get_trust_stats(honest_nodes)
            a_stats = universe.get_trust_stats(attacker_pool)
            h_cap_stats = universe.get_capacity_stats(honest_nodes)
            a_cap_stats = universe.get_capacity_stats(attacker_pool)
            
            h_trust = h_stats['mean']
            a_trust_avg = a_stats['mean']
            a_trust_max = a_stats['max']
            h_cap = h_cap_stats['mean']
            a_cap_avg = a_cap_stats['mean']

            # Throttle CSV writes for large simulations to reduce IO overhead
            should_write = True
            if universe.size >= 1000:
                should_write = (epoch % 10 == 0) or (epoch > max_attack_end + 60)
                
            if should_write:
                writer.writerow([epoch, f"{h_trust:.6f}", f"{a_trust_avg:.6f}",
                                 f"{h_cap:.2f}", f"{a_cap_avg:.2f}",
                                 f"{a_trust_max:.6f}", "0.0000"])
                file.flush() # Explicit flush for real-time monitoring

            # Early stopping check:
            # If all attacks (+ cooldown) are finished and attacker trust is effectively dead
            if epoch > max_attack_end + 60:
                if a_trust_max < 1e-7:
                    consecutive_zero_trust += 1
                else:
                    consecutive_zero_trust = 0
                
                if consecutive_zero_trust >= 5:
                    print(f"  [INFO] Mixed suite converged early at epoch {epoch}")
                    # Reflect actual converged epochs in progress
                    if progress and sub_task is not None:
                        progress.update(sub_task, completed=epoch+1, total=epoch+1)
                    break

    print(f"Telemetry saved to {csv_path}")
    return universe.suite_state.get('mixed_annotations', [])
