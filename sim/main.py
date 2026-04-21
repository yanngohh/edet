from .universe import Universe, ProtocolParameters, HAS_GPU
from .config import RESULTS_DIR
from .suites import gateway, sybil, slacker, whitewashing, mixed, virtuous, cold_start, oscillation, flash_loan, manipulation, spam, griefing
from .plotter import plot_scenario, plot_mixed_scenario, plot_gateway_scenario
from .graph_viz import start_animation
import os
from datetime import datetime
import signal
import sys

# Global reference for signal handler
ACTIVE_UNIVERSE = None

def get_result_dir(size, seed):
    date_str = datetime.now().strftime("%Y%m%d_%H%M%S")
    path = os.path.join(RESULTS_DIR, f"{date_str}_{size}_{seed}")
    os.makedirs(path, exist_ok=True)
    return path

# signal.signal(signal.SIGINT, signal_handler) # Deprecated in favor of periodic checkpoints

from .utils import resolve_load_path

def run_all(size, params, seed=None, display=True, use_disk=True, load_path=None):
    global ACTIVE_UNIVERSE
    
    lpath = resolve_load_path(load_path)
    if lpath:
        u = Universe.load_state(lpath)
        res = u.result_dir
        seed = u.seed
        print(f"=== Resuming edet Python Simulation [GPU: {HAS_GPU}] (Scenario #{seed}) ===")
        print(f"=== Results are in: {res} ===")
    else:
        if seed is None:
            import random as r
            seed = r.randint(0, 1000000)
        res = get_result_dir(size, seed)
        gpu_str = "[GPU Active]" if HAS_GPU else "[CPU Mode]"
        print(f"=== Starting edet Python Simulation {gpu_str} (Scenario #{seed}) ===")
        print(f"=== Results will be saved to: {res} ===")
        u = Universe(size, gpu_threshold=params.gpu_threshold, params=params, seed=seed, use_disk=use_disk, result_dir=res)

    ACTIVE_UNIVERSE = u

    u.task_id = "gateway"; gateway.run(u)
    u.task_id = "sybil"; sybil.run(u)
    u.task_id = "slacker"; slacker.run(u)
    u.task_id = "whitewashing"; whitewashing.run(u)
    u.task_id = "mixed"; mixed_annotations = mixed.run(u)
    u.task_id = "virtuous"; virtuous.run(u)
    u.task_id = "cold_start"; cold_start.run(u)
    u.task_id = "oscillation"; oscillation.run(u)
    u.task_id = "flash_loan"; flash_loan.run(u)
    u.task_id = "manipulation"; manipulation.run(u)
    u.task_id = "spam"; spam.run(u)
    u.task_id = "griefing"; griefing.run(u)

    print(f"\n=== Generating Plots in {res} ===")

    # Use seed-suffixed filenames to match suite output
    plot_gateway_scenario(res + f"/gateway_attack_{seed}.csv", show=display,
                          save_path=res + "/gateway_plot.svg", seed=seed)

    plot_scenario(res + f"/sybil_attack_{seed}.csv", "Sybil Ring Attack", "epoch",
                  [{'col': 'avg_sybil_trust', 'label': 'Avg Sybil Trust (t)', 'color': 'blue'},
                   {'col': 'avg_sybil_capacity', 'label': 'Avg Sybil Capacity', 'color': 'orange'}],
                  show=display, save_path=res + "/sybil_plot.svg",
                  y_label="Global Trust / Capacity", seed=seed)

    plot_scenario(res + f"/slacker_attack_{seed}.csv", "Slacking Attack", "epoch",
                  [{'col': 'slacker_trust', 'label': 'Slacker Trust', 'color': 'red'},
                   {'col': 'honest_trust', 'label': 'Honest Trust', 'color': 'green'}],
                  show=display, save_path=res + "/slacker_plot.svg",
                  y_label="Global Trust (t)", seed=seed)

    plot_scenario(res + f"/whitewashing_test_{seed}.csv", "Whitewashing Test", "epoch",
                  [{'col': 'new_user_trust', 'label': 'New User Trust', 'color': 'blue'},
                   {'col': 'new_user_capacity', 'label': 'New User Capacity', 'color': 'orange'}],
                  show=display, save_path=res + "/whitewashing_plot.svg",
                  y_label="Trust / Capacity", seed=seed)

    plot_mixed_scenario(res + f"/mixed_scenario_{seed}.csv", show=display,
                        save_path=res + "/mixed_plot.svg", seed=seed,
                        annotations=mixed_annotations)

    plot_scenario(res + f"/virtuous_test_{seed}.csv", "Virtuous Network Equilibrium", "epoch",
                  [{'col': 'avg_trust', 'label': 'Avg Global Trust', 'color': 'green'},
                   {'col': 'avg_capacity', 'label': 'Avg Credit Capacity', 'color': 'blue'}],
                  show=display, save_path=res + "/virtuous_plot.svg",
                  y_label="Trust / Capacity", seed=seed)

    plot_scenario(res + f"/cold_start_{seed}.csv", "Cold Start Bootstrapping", "epoch",
                  [{'col': 'seed_trust', 'label': 'Top 10% Trust Mass', 'color': 'red'},
                   {'col': 'others_trust', 'label': 'Bottom 90% Trust Mass', 'color': 'blue'}],
                  show=display, save_path=res + "/cold_start_plot.svg",
                  y_label="Trust Mass", seed=seed)

    plot_scenario(res + f"/oscillation_test_{seed}.csv", "Strategic Oscillation (r=0.20)", "epoch",
                  [{'col': 'attacker_trust', 'label': 'Attacker Trust', 'color': 'red'},
                   {'col': 'attacker_capacity', 'label': 'Attacker Capacity', 'color': 'orange'}],
                  show=display, save_path=res + "/oscillation_plot.svg",
                  y_label="Global Trust / Capacity", seed=seed)

    print(f"\nSimulation complete. All results saved to {res}")


def run_suite(suite_name, size, params, seed=None, use_disk=True, load_path=None):
    global ACTIVE_UNIVERSE
    if seed is None and load_path is None:
        import random as r
        seed = r.randint(0, 1000000)

    lpath = resolve_load_path(load_path)
    if lpath:
        universe = Universe.load_state(lpath)
        seed = universe.seed
        res = universe.result_dir
    else:
        res = get_result_dir(size, seed)
        universe = Universe(size, gpu_threshold=params.gpu_threshold, params=params, seed=seed, use_disk=use_disk, result_dir=res)

    ACTIVE_UNIVERSE = universe
    print(f"=== Running edet Single Suite: {suite_name} (Scenario #{seed}) ===")
    print(f"=== Results will be saved to: {res} ===")

    suites = {
        "gateway": gateway, "sybil": sybil, "slacker": slacker,
        "whitewashing": whitewashing, "mixed": mixed,
        "virtuous": virtuous, "cold_start": cold_start,
        "oscillation": oscillation, "flash_loan": flash_loan,
        "manipulation": manipulation, "spam": spam, "griefing": griefing
    }

    if suite_name not in suites:
        print(f"Error: Unknown suite '{suite_name}'. Available: {list(suites.keys())}")
        return

    universe.task_id = suite_name
    suites[suite_name].run(universe)


def run_graph(suite_name, size, params, seed=None, save_video=False, max_frames=None, use_disk=True):
    global ACTIVE_UNIVERSE
    if seed is None:
        import random as r
        seed = r.randint(0, 1000000)

    res = get_result_dir(size, seed)
    print(f"=== Starting edet Animated Graph: {suite_name} (Scenario #{seed}) ===")
    print(f"=== Results will be saved to: {res} ===")

    suites = {
        "gateway": gateway, "sybil": sybil, "slacker": slacker,
        "whitewashing": whitewashing, "mixed": mixed,
        "virtuous": virtuous, "cold_start": cold_start,
        "oscillation": oscillation, "flash_loan": flash_loan,
        "manipulation": manipulation, "spam": spam, "griefing": griefing
    }

    if suite_name not in suites:
        print(f"Error: Unknown suite '{suite_name}'. Available: {list(suites.keys())}")
        return

    suite = suites[suite_name]
    universe = Universe(size, gpu_threshold=params.gpu_threshold, params=params, seed=seed, use_disk=use_disk, result_dir=res)
    ACTIVE_UNIVERSE = universe

    if max_frames:
        frames = max_frames
    else:
        frames = 2000 if suite_name == "mixed" else (300 if suite_name == "cold_start" else 50)

    save_path = None
    if save_video:
        save_path = f"{res}/graph_{suite_name}_{seed}.mp4"

    start_animation(universe, suite.step, frames=frames, interval=300,
                    seed=seed, save_path=save_path)


if __name__ == "__main__":
    import argparse
    from .config import get_production_params
    from . import universe as _umod

    parser = argparse.ArgumentParser(description='edet Simulation')
    parser.add_argument('--graph', type=str, help='Run animated graph for specific suite')
    parser.add_argument('--suite', type=str, help='Run specific suite telemetry')
    parser.add_argument('--seed', type=int, help='PRNG seed for reproducibility')
    parser.add_argument('--no-display', action='store_true', help='Headless mode')
    parser.add_argument('--load', type=str, help='Load simulation state from path')
    parser.add_argument('--no-disk', action='store_true', help='Disable disk-backed storage (RAM only)')

    # Protocol parameter overrides
    parser.add_argument('--eigentrust-alpha', dest='eigentrust_alpha', type=float, default=None)
    parser.add_argument('--eigentrust-epsilon', dest='eigentrust_epsilon', type=float, default=None)
    parser.add_argument('--eigentrust-iterations', dest='eigentrust_iterations', type=int, default=None)
    parser.add_argument('--gpu-threshold', type=int, default=1000,
                        help='Min network size for GPU acceleration (default: 1000)')
    parser.add_argument('--base_capacity', type=float, default=None)
    parser.add_argument('--capacity_beta', type=float, default=None)

    parser.add_argument('--frames', type=int, help='Limit animation frames')
    parser.add_argument('--size', type=int, help='Override network size')

    args = parser.parse_args()

    sim_size = args.size if args.size else (150 if args.graph else 1000)
    params = get_production_params(sim_size)
    params.gpu_threshold = args.gpu_threshold

    # Apply CLI overrides
    if args.eigentrust_alpha is not None:
        params.eigentrust_alpha = args.eigentrust_alpha
    if args.eigentrust_epsilon is not None:
        params.eigentrust_epsilon = args.eigentrust_epsilon
    if args.eigentrust_iterations is not None:
        params.eigentrust_iterations = args.eigentrust_iterations
    if args.base_capacity is not None:
        params.base_capacity = args.base_capacity
    if args.capacity_beta is not None:
        params.capacity_beta = args.capacity_beta

    use_disk = (sim_size > 1000) if not args.no_disk else False

    if args.graph:
        is_headless = os.environ.get('DISPLAY') is None
        save_video = args.no_display or is_headless
        run_graph(args.graph, sim_size, params, seed=args.seed,
                  save_video=save_video, max_frames=args.frames, use_disk=use_disk)
    elif args.suite:
        run_suite(args.suite, sim_size, params, seed=args.seed, use_disk=use_disk, load_path=args.load)
    else:
        run_all(sim_size, params, seed=args.seed, display=not args.no_display, use_disk=use_disk, load_path=args.load)
