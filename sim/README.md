# edet Simulation Suite: Technical Documentation

This directory contains the Python-based simulation environment for the **edet** protocol. It is designed for high-performance verification of trust dynamics and pressure-based credit limits using NumPy/SciPy (CPU) or CuPy (GPU).

## Getting Started

The **edet** development environment uses a **Hybrid** approach for maximum performance and simplicity:
1.  **Nix Shell**: Manages the Holochain SDK, Rust, and Node.js.
2.  **Conda/Mamba Environment**: Manages the GPU-accelerated Python simulation (where CUDA management is native and faster).

---

### Phase 1: Setup the Python Simulation (Conda)

Recommended for GPU acceleration and straightforward dependency management.

```bash
# 1. Create the conda environment (from project root)
conda env create -f environment.yml

# 2. Activate it
conda activate edet-sim

# 3. Use Pip to install your specific CuPy version if needed
# If you have CUDA 12: pip install cupy-cuda12x
# If you have CUDA 11: pip install cupy-cuda11x
```

### Phase 2: Setup Holochain (Nix)
For building the DNAs and running the web UI.

```bash
# Enter the nix shell
nix develop
```

---

## Execution Entry Points

### 1. Main Telemetry & Animation (`sim.main`)
The primary driver for running specific scenarios and generating plots or real-time visualizations.

```bash
# Run all standard suites and generate SVG plots
python3 -m sim.main

# Run a specific suite with telemetry output
python3 -m sim.main --suite mixed --size 1000

# Run real-time animation for a specific suite
python3 -m sim.main --graph gateway --size 200
```

**CLI Arguments (`main.py`):**
- `--suite <name>`: Execute a specific suite telemetry (see [Simulation Suites](#simulation-suites)).
- `--graph <name>`: Launch interactive Matplotlib animation for a suite.
- `--size <int>`: Network size (default: 1000 for telemetry, 150 for graphs).
- `--seed <int>`: PRNG seed for reproducible single runs.
- `--no-display`: Headless mode (saves animation as MP4 if `--graph` is set).
- `--frames <int>`: Limit the number of animation frames.
- `--gpu-threshold <int>`: Min network size for GPU acceleration (default: 1000).
- `--load <path>`: Resume simulation from a checkpoint directory.
- `--use-disk`: Enable disk-based storage for P2P data (Default: True). Use `--no-disk` to disable.

**Protocol Overrides (`main.py`):**
- `--eigentrust-alpha <float>`: Mixing factor $\alpha$.
- `--eigentrust-epsilon <float>`: Convergence precision.
- `--eigentrust-iterations <int>`: Max power iterations.
- `--base_capacity <float>`: Initial capacity $V_{base}$.
- `--capacity_beta <float>`: Reputation scaling factor $\beta$.

### 2. Formal Verification (`sim.verify_theory`)
Automated suite to validate mathematical invariants and protocol theorems.

```bash
# Quick verification (single run)
python3 -m sim.verify_theory --size 200

# Comprehensive multi-run benchmark (required for performance plots)
python3 -m sim.verify_theory --size 500 --runs 5
```

**CLI Arguments (`verify_theory.py`):**
- `--size <int>`: Network size for all tests.
- `--runs <int>`: Number of iterations per suite with different seeds.
- `--seed <int>`: Base seed for multi-run reproducibility (used as `seed + run_id`).
- `--suites <list>`: Comma-separated list of suites to run (e.g., `mixed,sybil`).
- `--batch-runs <int>`: Max concurrent runs submitted to worker pool (default: 2; 0 = all).
- `--no-plot`: Skip generation of `results/verification_summary.png`.
- `--fail-fast`: Terminate immediately on any invariant failure.
- `--scale`: Run key suites at N=2000 in addition to normal tests.
- `--workers <int>`: Number of parallel worker processes (default: CPU count).
- `--gpu-workers <int>`: Max concurrent GPU slots (auto-detected based on VRAM).
- `--gpu-threshold <int>`: Min network size for GPU acceleration (default: 1000).
- `--load <path>`: Resume verification run from a session directory (must be paired with `--size`).
- `--use-disk`: Enable disk-based storage (Default: True).

---

## Simulation Suites

Available suites in `sim/suites/`:

| Suite | Description |
| :--- | :--- |
| `virtuous` | Baseline honest network (Theorem 1: Convergence). |
| `gateway` | Coordinated accomplice-based reputation inflation (Theorem 4). |
| `sybil` | Circular trading and identity clustering (Theorem 2). |
| `slacker` | Defaulting/Inactivity monitoring (Theorem 5). |
| `mixed` | Composite scenario with randomized attack schedules. |
| `whitewashing` | Identity-cycling break-even analysis (Corollary 1). |
| `oscillation` | Strategic "build-then-defect" behavior. |
| `flash_loan` | Sudden liquidity/reputation spikes. |
| `griefing` | Economic denial-of-service attempts. |
| `adaptive` | Attackers that react to observer behavior. |
| `manipulation` | Coordinated multi-cluster attacks with mass default. |
| `open_trial_gate` | Mitigation for strategic trial exploitation. |
| `genesis_equilibrium` | Equitable equilibrium in virtuous networks (Theorem 3). |

---

## Persistence & Checkpointing

The simulation suite supports robust state persistence, allowing you to resume long-running experiments or recover from interruptions.

### Checkpointing Strategy

The system uses a naming convention to distinguish between partial and complete states:

1.  **Interrupted States (`_interrupted`)**: If you interrupt a simulation with `Ctrl+C` (SIGINT), the system automatically triggers a `universe.save_state()`. These checkpoints allow resuming from the exact epoch where the simulation was stopped.
2.  **Finished States (`_finished`)**: When a simulation task completes its full epoch range, it saves a `_finished` checkpoint.

### Resuming Simulations

Use the `--load <path>` argument to resume a session.

#### `verify_theory.py`

Pass the **session directory** (`verification_YYYYMMDD_HHMMSS_N`). Each worker process searches inside it for its own `checkpoint_run{N}_{suite}_interrupted` or `checkpoint_run{N}_{suite}_finished` subdirectory and resumes from there. Suites with a `_finished` checkpoint are replayed instantly; suites with an `_interrupted` checkpoint resume from their last saved epoch; suites with no checkpoint start fresh.

```bash
python3 -m sim.verify_theory --size 1000 --load verification_20260316_183857_1000
```

#### `main.py` (single run or suite)

Each suite checkpoints independently under the result directory as `checkpoint_{suite}_interrupted`. Pass the **specific checkpoint directory** for the suite you want to resume from:

```bash
# Resume run_all from a mid-run gateway checkpoint
python3 -m sim.main --load sim/results/20260316_193655_1000_42/checkpoint_gateway_interrupted

# Resume a single-suite run
python3 -m sim.main --suite sybil --load sim/results/20260316_193655_1000_42/checkpoint_sybil_interrupted
```

### Path Resolution
The `--load` argument accepts any of the following forms and resolves them in order:
- **Directory name only**: `verification_20260316_183857_1000` — searched automatically inside `sim/results/`
- **Relative path**: `sim/results/verification_20260316_183857_1000`
- **Absolute path**: `/home/user/edet/sim/results/verification_20260316_183857_1000`

### Disk-Based Storage (Memory Optimization)
For large-scale simulations ($N \ge 10,000$), holding the entire transaction history and reputation matrices in RAM can lead to exhaustion. 
- **Default Behavior**: `--use-disk` is enabled by default, offloading large peer-to-peer data structures to an optimized SQLite backend (`universe_*.db`).
- **Isolation**: Each worker in a multi-run simulation uses a unique `task_id` for its database to prevent file locking and data corruption.

---

## Technical Architecture

### Core Components
- `universe.py`: The `Universe` class manages the state of all $N$ nodes, including $S/F$ counters, `global_trust` (EigenTrust), and `credit_capacity`.
- `config.py`: Single Source of Truth for protocol parameters.
- `graph_viz.py`: Matplotlib/NetworkX implementation of the force-directed graph animation.
- `plotter.py`: Utilities for generating SVG/PNG telemetry plots.

### Output Artifacts
All simulation outputs are consolidated in `sim/results/` using a structured directory format:
`sim/results/<date>_<size>_<seed>/`

**Contents:**
- `verification.log`: Combined trace logs for the entire session.
- `universe_*.db`: SQLite-based checkpoint files (if `--use-disk` is active).
- `verification_summary.png`: Real-time and final pass/fail benchmarks.
- `*.csv`: Raw telemetry for specific suites (Trust, Capacity, Pressure).
- `plots/`: Time-series SVG plots of key metrics.