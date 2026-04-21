import math
import random
import time
import numpy as np
import scipy.sparse as sp
from dataclasses import dataclass, field
from typing import List, Dict, Set, Tuple, Optional, Any
import collections as _col

import sqlite3
import pickle
import os
import shutil

class DiskDict:
    """A disk-backed dictionary-like object using SQLite for memory efficiency."""
    def __init__(self, db_path, table_name, connection=None):
        self.db_path = db_path
        self.table_name = table_name
        self._external_conn = connection is not None
        if connection:
            self._conn = connection
        else:
            self._conn = sqlite3.connect(db_path)
            self._conn.execute("PRAGMA journal_mode=WAL") # Faster writes
            self._conn.execute("PRAGMA synchronous=NORMAL")
            
        # Use a more efficient schema: node_id, peer_id, value
        self._conn.execute(f"CREATE TABLE IF NOT EXISTS {table_name} (node_id INTEGER, peer_id INTEGER, value BLOB, PRIMARY KEY (node_id, peer_id))")
        self._conn.execute(f"CREATE INDEX IF NOT EXISTS idx_{table_name}_node ON {table_name} (node_id)")
        self._conn.commit()

    def __getitem__(self, node_id):
        return DiskNodeProxy(self, node_id)

    def set_value(self, node_id, peer_id, value):
        self._require_conn()
        data = pickle.dumps(value)
        self._conn.execute(f"INSERT OR REPLACE INTO {self.table_name} (node_id, peer_id, value) VALUES (?, ?, ?)", (node_id, peer_id, data))
        self._conn.commit()

    def get_value(self, node_id, peer_id, default=None):
        self._require_conn()
        cursor = self._conn.execute(f"SELECT value FROM {self.table_name} WHERE node_id = ? AND peer_id = ?", (node_id, peer_id))
        row = cursor.fetchone()
        if row:
            return pickle.loads(row[0])
        return default

    def get_node_dict(self, node_id):
        self._require_conn()
        cursor = self._conn.execute(f"SELECT peer_id, value FROM {self.table_name} WHERE node_id = ?", (node_id,))
        return {row[0]: pickle.loads(row[1]) for row in cursor.fetchall()}

    def remove_peer(self, node_id, peer_id):
        self._require_conn()
        self._conn.execute(f"DELETE FROM {self.table_name} WHERE node_id = ? AND peer_id = ?", (node_id, peer_id))
        self._conn.commit()

    def clear_node(self, node_id):
        self._require_conn()
        self._conn.execute(f"DELETE FROM {self.table_name} WHERE node_id = ?", (node_id,))
        self._conn.commit()
    
    def replace_node_dict(self, node_id, new_dict):
        """Replace an entire node's dictionary in one transaction."""
        self._require_conn()
        self._conn.execute(f"DELETE FROM {self.table_name} WHERE node_id = ?", (node_id,))
        if new_dict:
            data = [(node_id, peer_id, pickle.dumps(val)) for peer_id, val in new_dict.items()]
            self._conn.executemany(f"INSERT INTO {self.table_name} (node_id, peer_id, value) VALUES (?, ?, ?)", data)
        self._conn.commit()
    
    def extend(self, items):
        """Satisfy list-like extend API for add_nodes."""
        # DiskDict doesn't need explicit allocation for new nodes
        pass

    def close(self):
        if hasattr(self, "_conn") and not self._external_conn:
            self._conn.close()

    def __getstate__(self):
        state = self.__dict__.copy()
        if "_conn" in state:
            del state["_conn"]
        return state

    def __setstate__(self, state):
        self.__dict__.update(state)
        # Reconnect to DB only if we owned the connection originally
        if not self._external_conn:
            self._conn = sqlite3.connect(self.db_path)
            self._conn.execute("PRAGMA journal_mode=WAL")
            self._conn.execute("PRAGMA synchronous=NORMAL")
        # If _external_conn=True, _conn is intentionally absent here.
        # The owning Universe is responsible for re-attaching it via
        # load_state().  Any attempt to use this DiskDict before re-attachment
        # will raise a clear RuntimeError (see _require_conn).

    def _require_conn(self):
        """Raise a descriptive error if this DiskDict was unpickled without
        its external connection being re-attached by the owning Universe."""
        if not hasattr(self, "_conn"):
            raise RuntimeError(
                f"DiskDict(table='{self.table_name}', db='{self.db_path}') was "
                f"deserialized without its SQLite connection being re-attached. "
                f"This means the owning Universe was loaded from an incomplete or "
                f"corrupted checkpoint (universe_state.db missing or load_state() "
                f"did not finish). Discard this checkpoint and restart from scratch."
            )

class DiskNodeProxy:
    """A proxy for a single node's dictionary in DiskDict."""
    def __init__(self, disk_dict, node_id):
        self.disk_dict = disk_dict
        self.node_id = node_id

    def __getitem__(self, peer_id):
        val = self.disk_dict.get_value(self.node_id, peer_id)
        if val is None:
            raise KeyError(peer_id)
        return val

    def __setitem__(self, peer_id, value):
        self.disk_dict.set_value(self.node_id, peer_id, value)

    def get(self, peer_id, default=None):
        return self.disk_dict.get_value(self.node_id, peer_id, default)

    def pop(self, peer_id, default=None):
        val = self.disk_dict.get_value(self.node_id, peer_id)
        if val is not None:
            self.disk_dict.remove_peer(self.node_id, peer_id)
            return val
        return default

    def keys(self):
        self.disk_dict._require_conn()
        cursor = self.disk_dict._conn.execute(f"SELECT peer_id FROM {self.disk_dict.table_name} WHERE node_id = ?", (self.node_id,))
        return [row[0] for row in cursor.fetchall()]

    def items(self):
        return self.disk_dict.get_node_dict(self.node_id).items()

    def values(self):
        return self.disk_dict.get_node_dict(self.node_id).values()

    def __iter__(self):
        return iter(self.keys())

    def __len__(self):
        self.disk_dict._require_conn()
        cursor = self.disk_dict._conn.execute(f"SELECT COUNT(*) FROM {self.disk_dict.table_name} WHERE node_id = ?", (self.node_id,))
        return cursor.fetchone()[0]

    def update(self, other):
        for k, v in other.items():
            self[k] = v

    def __contains__(self, peer_id):
        self.disk_dict._require_conn()
        cursor = self.disk_dict._conn.execute(f"SELECT 1 FROM {self.disk_dict.table_name} WHERE node_id = ? AND peer_id = ?", (self.node_id, peer_id))
        return cursor.fetchone() is not None


GPU_DIAGNOSTIC = "Not checked"

def _check_gpu():
    """Check if CuPy is installed and CUDA is available."""
    global GPU_DIAGNOSTIC
    try:
        import importlib
        importlib.import_module('cupy')
        import cupy
        available = cupy.cuda.is_available()
        if not available:
            GPU_DIAGNOSTIC = "CuPy installed but cupy.cuda.is_available() is False (check drivers)"
        else:
            GPU_DIAGNOSTIC = f"OK (CuPy {cupy.__version__}, CUDA {cupy.cuda.runtime.getDeviceCount()} devices)"
        return available
    except ImportError:
        GPU_DIAGNOSTIC = "CuPy (cupy-cuda12x) not found in current environment"
        return False
    except Exception as e:
        GPU_DIAGNOSTIC = f"Detection crash: {e}"
        return False

HAS_GPU = _check_gpu()
cp = None        # populated lazily by _ensure_gpu()
cp_sp = None     # populated lazily by _ensure_gpu()

_gpu_allowed = True   # Process-local flag; set False by worker_wrapper when GPU semaphore not acquired

def _ensure_gpu():
    """Import CuPy modules on first real use (triggers CUDA context init)."""
    global cp, cp_sp
    if cp is None:
        import cupy as _cp
        import cupyx.scipy.sparse as _cp_sp
        cp = _cp
        cp_sp = _cp_sp

def as_numpy(arr):
    """Safely convert a CuPy or numpy array to a numpy ndarray.

    When GPU is active (``size >= gpu_threshold``), Universe
    stores ``global_trust`` as a CuPy array.  CuPy forbids implicit
    conversion via ``np.array(cupy_arr)``; this helper calls ``.get()``
    first.  ``credit_capacity`` is always CPU-pinned (plain numpy).
    """
    if hasattr(arr, 'get'):
        return np.asarray(arr.get())
    return np.asarray(arr)


@dataclass
class ProtocolParameters:
    # EigenTrust Configuration
    eigentrust_alpha: float = 0.08      # Mixing factor (1-a) * Trust + a * PreTrust (matches Rust/WP)
    eigentrust_epsilon: float = 0.001   # Convergence threshold
    eigentrust_iterations: int = 20     # Max iterations
    avg_beneficiaries: float = 15.0     # Average number of beneficiaries per node (nodes whose debt is drained)
    var_beneficiaries: float = 14.0    # Variance of beneficiary count

    # Credit Capacity (Eq. 6)
    base_capacity: float = 1000.0        # V_base
    capacity_beta: float = 5000.0        # Beta scaling factor
    acq_saturation: float = 50.0         # n0: acquaintance saturation constant

    # Trust Attenuation (Definition 5/6)
    failure_tolerance: float = 0.12      # tau: failure rate threshold for exclusion
    tau_newcomer: float = 0.05           # tau_0: tolerance for new bilateral relationships
    volume_maturation: float = 1000.0    # N_mat: bilateral volume for full tolerance
    penalty_sharpness: float = 4.0       # gamma: exponent for trust attenuation curve (matches Rust/WP)
    
    # Trust Banking Bound (Proposal 2: Trust Banking Mitigation)
    # fraction: Fraction of N_mat for S in the local trust score multiplier.
    # Bounds the maximum absolute leverage an attacker can extract from a single
    # honest relationship, preventing nodes from accumulating unlimited trust
    # to absorb massive defaults later.
    trust_banking_bound_fraction: float = 0.25
    
    # Witness-Based Contagion (Proposal 1: Selective Defaulting Mitigation)
    # k: Each independent failure witness reduces tau_eff: tau_eff' = tau_eff / (1 + k * witnesses)
    # With k=0.25, 4 witnesses reduce tau_eff by 50%.
    contagion_witness_factor: float = 0.25
    # d_w: Discount factor for the aggregate witness rate floor.
    # The median bilateral F/(S+F) across witnesses is multiplied by d_w before
    # being used as a floor for the observer's effective rate r_eff.
    # 0.5 = hearsay counts at half weight compared to direct bilateral evidence.
    witness_discount: float = 0.5
    # n_min: Minimum number of failure witnesses required before applying
    # the aggregate witness rate floor. Prevents noise from 1-2 edge cases.
    min_contagion_witnesses: int = 3

    # Behavioral
    min_maturity: int = 30               # M_min (epochs); contracts always use M = M_min
    trial_fraction: float = 0.05         # Eta: trial tx threshold as fraction of V_base

    # Risk thresholds (for auto-moderation)
    default_accept_threshold: float = 0.4   # Risk below this -> auto-accept
    default_reject_threshold: float = 0.8   # Risk above this -> auto-reject
    risk_sigmoid_k: float = 0.75            # Sigmoid steepness for risk score normalization
    k_claim_sigmoid: float = 20.0           # K_claim: sigmoid steepness for first-contact claim-based risk
                                            # hat_t_claim = n_S / (n_S + K_claim); reaches 0.5 at n_S = K_claim

    # Recent Failure Rate Window (Behavioral Switch Detection)
    recent_window_k: int = 10               # K: number of epochs in the rolling window
    recent_weight: float = 2.0              # w_r: amplification of r_recent vs r_cumul in phi
                                            # r_eff = max(r_cumul, w_r * r_recent)
                                            # w_r=2.0: 50%+ recent default rate → phi=0 even with large S

    # Dynamic Support Randomization (for Simulation)
    support_shift_prob: float = 0.1         # Probability of shifting alliances per epoch
    min_self_support: float = 0.6           # Lower bound for self-support coefficient
    max_beneficiary_fraction: float = 0.1  # Max fraction of network to pick as beneficiaries
    max_beneficiaries_cap: float = 0.01    # Hard cap on beneficiaries as % of network (prevents O(n²) cascade)
    acquaintance_prune_prob: float = 0.02   # Probability of pruning acquaintances per node per epoch (2%)

    # Mitigation Flags
    # use_vouching: controls whether vouch slash + sponsor contagion mechanics are active.
    # Regardless of this flag, the capacity formula follows the Rust production
    # behavior (capacity.rs lines 11-24): unvouched agents CAN receive positive
    # reputation-derived capacity once they graduate via trial transactions
    # (Whitepaper §2 Bootstrap, Theorem 2.3). The flag only gates sponsor-related
    # consequences of default.
    #   True  (default, matches Rust): slash + contagion on vouchee default are active.
    #   False (simulation convenience): vouching consequences disabled (e.g. for
    #          isolation tests targeting EigenTrust convergence in a frozen trust graph).
    use_vouching: bool = True

    # New Mitigations
    trial_velocity_limit: int = 5           # Max trial tx per seller per epoch
    vouch_slashing_multiplier: float = 3.0  # Asymmetric slashing X
    max_volume_per_epoch: float = 100.0     # Max S to accrue per epoch per counterparty (time-weighting)

    # Acquaintance Bound (Dunbar-style cap)
    # 0 = unbounded (original behavior); >0 = realistic cap matching Holochain impl.
    # In reality, |A_i| is bounded by social discovery (Dunbar's number ~150).
    # When >0, the capacity baseline uses |A_i| instead of N, matching trust.rs.
    max_acquaintances: int = 150

    # Subjective Trust Subgraph (matches Holochain bounded BFS)
    subgraph_max_depth: int = 4          # BFS depth for subjective trust subgraph
    max_subgraph_nodes: int = 50000      # Circuit breaker on subgraph size


class DebtContract:
    """A debt contract (delta, M, t0, creditor) per Definition 1 of the whitepaper."""
    __slots__ = ('amount', 'maturity', 'start_epoch', 'creditor', 'co_signers', 'is_trial')

    def __init__(self, amount: float, maturity: int, start_epoch: int, creditor: int,
                 co_signers: Dict[int, float] = None, is_trial: bool = False):
        self.amount = amount
        self.maturity = maturity
        self.start_epoch = start_epoch
        self.creditor = creditor  # The seller who originated this debt
        self.co_signers = co_signers or {}
        self.is_trial = is_trial  # Immutably set at creation; True iff amount < eta * V_base


class Universe:
    """
    Simulation engine implementing the edet protocol.

    - Debt transfer model: S increments when debt is transferred (via selling), not amortized
    - F increments only when contracts expire without transfer (maturity-based failure)
    - Support cascade: sellers can drain debt from beneficiaries
    - Genesis debt: transactions where seller has no debt create new debt from nothing
    
    GLOBAL VS SUBJECTIVE TRUST:
    
    This simulation maintains two types of trust/capacity metrics:
    
    1. GLOBAL (Telemetry) - Used for visualization, aggregate metrics, plotting:
       - global_trust[i]: Network-wide reputation proxy (PageRank with uniform pre-trust)
       - credit_capacity[i]: Capacity derived from global trust
       These are NOT used for protocol decisions. They represent a "God's eye view"
       that doesn't exist in the real decentralized protocol.
    
    2. SUBJECTIVE (Protocol-Accurate) - Used for actual protocol decisions:
       - get_subjective_reputation(observer, target): Observer's personalized trust
       - get_subjective_capacity(observer, target): Capacity from observer's view
       These are used in compute_risk_score() and transaction validation.
       
       Implementation: the subjective methods execute the whitepaper's Subjective
       Local Expansion algorithm (§5 Reputation Computation) via
       `compute_subjective_trust_bfs`:
         (a) BFS from observer's acquaintance set A_observer up to depth
             `subgraph_max_depth` (default 4) or `max_subgraph_nodes` (default 50000),
             whichever is reached first.
         (b) Build a sparse sub-matrix M_sub over discovered nodes only, using
             published trust rows (local_trust) restricted to the subgraph.
         (c) Run power iteration on M_sub with observer-personalized pre-trust.
         (d) Return the sub-stationary vector entry for the target node (0 if the
             target is not in the subgraph).
       
       This mirrors the production Holochain implementation (trust/mod.rs): each
       observer computes trust over a bounded local subgraph rather than the
       full network matrix. Complexity per observer is O(|A|^d * K_iter),
       bounded by `max_subgraph_nodes` for large networks.
    
    Key Principle: Every protocol decision (accept/reject transaction, assess risk)
    must use SUBJECTIVE methods. Global metrics are purely diagnostic.
    """

    def _is_cuda_oom(self, e) -> bool:
        """Check if an exception is a CUDA-related out-of-memory error."""
        error_name = type(e).__name__
        error_msg = str(e).upper()
        return ('OUTOFMEMORYERROR' in error_name or
                'ALLOC_FAILED' in error_msg or
                'STATUS_ALLOC_FAILED' in error_msg or
                'OUT OF MEMORY' in error_msg)

    def _force_gc_cuda(self):
        """Force cleanup of CUDA memory pools."""
        try:
            import cupy as _cp
            _cp.get_default_memory_pool().free_all_blocks()
            _cp.get_default_pinned_memory_pool().free_all_blocks()
        except ImportError:
            pass
        except Exception:
            pass

    def __init__(self, size: int, gpu_threshold: int,
                 params: Optional[ProtocolParameters] = None,
                 seed: Optional[int] = None,
                 use_disk: bool = True,
                 result_dir: Optional[str] = None,
                 task_id: Optional[str] = None):
        self.size = size
        self.gpu_threshold = gpu_threshold
        self.epoch = 0
        self.params = params or ProtocolParameters()
        self.use_disk = use_disk
        self.result_dir = result_dir
        self.task_id = task_id
        if result_dir:
            os.makedirs(result_dir, exist_ok=True)

        self.seed = seed
        # Initialize PRNG
        self.rng = random.Random(seed)

        # --- EigenTrust State (Definition 2: S_ij and F_ij) ---
        if self.use_disk:
            db_name = f"universe_{seed or 'none'}_{self.task_id}.db" if self.task_id else f"universe_{seed or 'none'}.db"
            db_path = os.path.join(result_dir if result_dir else "/tmp", db_name)
            self._db_path = db_path
            if result_dir and os.path.exists(db_path):
                # FRESH START: remove stale DB from previously interrupted/crashed run
                # to avoid state leakage (e.g. from suites with different sizes)
                try:
                    os.remove(db_path)
                    print(f"  [INFO] Removed stale DB: {db_path}")
                except Exception as e:
                    print(f"  [WARNING] Could not remove stale DB {db_path}: {e}")
            
            self._db_conn = sqlite3.connect(db_path)
            self._db_conn.execute("PRAGMA journal_mode=WAL")
            self._db_conn.execute("PRAGMA synchronous=NORMAL")
            
            self._disk_s = DiskDict(db_path, "S", connection=self._db_conn)
            self._disk_f = DiskDict(db_path, "F", connection=self._db_conn)
            self._disk_lt = DiskDict(db_path, "local_trust", connection=self._db_conn)
            self._disk_sb = DiskDict(db_path, "support_breakdown", connection=self._db_conn)
            self._disk_s_win = DiskDict(db_path, "S_window", connection=self._db_conn)
            self._disk_f_win = DiskDict(db_path, "F_window", connection=self._db_conn)
            self._disk_s_win_delta = DiskDict(db_path, "S_epoch_delta", connection=self._db_conn)
            self._disk_f_win_delta = DiskDict(db_path, "F_epoch_delta", connection=self._db_conn)
            self._disk_tx_count = DiskDict(db_path, "tx_count", connection=self._db_conn)
            self._disk_volume_added = DiskDict(db_path, "epoch_volume_added", connection=self._db_conn)
            self._disk_s_win_sums = DiskDict(db_path, "s_window_sums", connection=self._db_conn)
            self._disk_f_win_sums = DiskDict(db_path, "f_window_sums", connection=self._db_conn)
            self._disk_fo = DiskDict(db_path, "failure_observations", connection=self._db_conn)
            self._disk_last_win_upd = DiskDict(db_path, "last_window_update", connection=self._db_conn)
            
            self.S = [self._disk_s[i] for i in range(self.size)]
            self.F = [self._disk_f[i] for i in range(self.size)]
            self.local_trust = [self._disk_lt[i] for i in range(self.size)]
            self.support_breakdown = [self._disk_sb[i] for i in range(self.size)]
            self.S_window = [self._disk_s_win[i] for i in range(self.size)]
            self.F_window = [self._disk_f_win[i] for i in range(self.size)]
            self._S_epoch_delta = [self._disk_s_win_delta[i] for i in range(self.size)]
            self._F_epoch_delta = [self._disk_f_win_delta[i] for i in range(self.size)]
            self.tx_count = [self._disk_tx_count[i] for i in range(self.size)]
            self.epoch_volume_added = [self._disk_volume_added[i] for i in range(self.size)]
            self._s_window_sums = [self._disk_s_win_sums[i] for i in range(self.size)]
            self._f_window_sums = [self._disk_f_win_sums[i] for i in range(self.size)]
            self.failure_observations = [self._disk_fo[i] for i in range(self.size)]
            self._last_window_update = [self._disk_last_win_upd[i] for i in range(self.size)]
        else:
            self.S: List[Dict[int, float]] = [{} for _ in range(self.size)]
            self.F: List[Dict[int, float]] = [{} for _ in range(self.size)]
            self.local_trust: List[Dict[int, float]] = [{} for _ in range(self.size)]
            self.support_breakdown: List[Dict[int, float]] = [{} for _ in range(self.size)]
            self.S_window: List[Dict[int, 'collections.deque']] = [{} for _ in range(self.size)]
            self.F_window: List[Dict[int, 'collections.deque']] = [{} for _ in range(self.size)]
            self._S_epoch_delta: List[Dict[int, float]] = [{} for _ in range(self.size)]
            self._F_epoch_delta: List[Dict[int, float]] = [{} for _ in range(self.size)]
            self.tx_count: List[Dict[int, int]] = [{} for _ in range(self.size)]
            self.epoch_volume_added: List[Dict[int, float]] = [{} for _ in range(self.size)]
            self._s_window_sums: List[Dict[int, float]] = [{} for _ in range(self.size)]
            self._f_window_sums: List[Dict[int, float]] = [{} for _ in range(self.size)]
            self.failure_observations: List[Dict[int, int]] = [{} for _ in range(self.size)]
            self._last_window_update: List[Dict[int, int]] = [{} for _ in range(self.size)]

        self.acquaintances: List[Set[int]] = [set() for _ in range(self.size)]
        self._known_by: List[Set[int]] = [set() for _ in range(self.size)]
        for i in range(self.size):
            self.acquaintances[i] = {i}
            self._known_by[i] = {i}

        # --- Vouching State ---
        self.staked_capacity = np.zeros(self.size, dtype=np.float64)
        self.vouchers: List[Dict[int, float]] = [{} for _ in range(self.size)]
        self.credit_capacity = np.full(self.size, self.params.base_capacity, dtype=np.float64)
        self.volume = np.full(self.size, self.params.base_capacity, dtype=np.float64)

        # --- Reputation & Capacity Backend ---
        self._force_cpu = False
        self._gpu_allowed = True
        self._ensure_gpu()
        try:
            xp, _ = self._get_xp()
            self.global_trust = xp.full(self.size, 1.0 / self.size, dtype=xp.float64)
        except Exception as _e:
            if self._is_cuda_oom(_e):
                self._force_cpu = True
                self._force_gc_cuda()
                self.global_trust = np.full(self.size, 1.0 / self.size, dtype=np.float64)
            else:
                raise

        # --- Debt Contracts ---
        self.contracts: List[List[DebtContract]] = [[] for _ in range(self.size)]
        self._expiry_queue = _col.defaultdict(list)  # expiry_epoch -> [(debtor_id, contract), ...]
        self.extinguished_this_epoch: List[float] = [0.0] * self.size
        self.debt_acquired_this_epoch: List[float] = [0.0] * self.size
        
        self._witness_rate_cache: Dict[int, float] = {}
        self.is_active: List[bool] = [True] * self.size

        # --- Genesis Debt Steady-State Tracking ---
        self.genesis_debt_this_epoch: float = 0.0
        self.debt_transferred_this_epoch: float = 0.0
        self.debt_expired_this_epoch: float = 0.0
        self.total_tx_volume_this_epoch: float = 0.0

        # --- Transaction Tracking ---
        self.current_rejected: List[Tuple] = []
        self.current_tx: List[Tuple] = []
        self.last_epoch_rejected: List[Tuple] = []
        self.last_epoch_tx: List[Tuple] = []

        # --- Suite State ---
        self.suite_state: Dict = {}

        # --- Identity Aging ---
        self.join_epoch: List[int] = [0] * self.size
        self.trusted_pool: Optional[List[int]] = None

        # --- Performance Caches ---
        self._subjective_row_cache: Dict[int, np.ndarray] = {}
        self._cached_C: Optional[np.ndarray] = None
        self._cached_MT: Optional[np.ndarray] = None
        self._cached_A: Optional[np.ndarray] = None
        self._subjective_cache: Dict[int, Dict[int, float]] = {}
        self._subjective_cache_epoch: int = -1
        self._dirty_nodes_trust: Set[int] = set(range(self.size))

        # --- O(1) Debt Tracking ---
        self.total_debt: List[float] = [0.0] * self.size
        self.total_trial_debt: List[float] = [0.0] * self.size
        self._tick_subjective_cache = {}  # Per-tick cache for repeated lookups
        self._attenuated_score_cache = {} # Per-rebuild cache (i,j) -> score

        # --- Mitigations State ---
        self.trial_tx_count: List[int] = [0] * self.size
        self.blocked_trial_pairs: Set[Tuple[int, int]] = set()
        self.successful_transfers_global: List[int] = [0] * self.size
        self._total_s_as_debtor: List[float] = [0.0] * self.size
        # --- Constants ---
        _log_nmat = math.log1p(self.params.volume_maturation)
        self._inv_log_nmat: float = 1.0 / _log_nmat if _log_nmat > 0 else 1.0

        self.update_credit_capacity()

    def save_state(self, path: str):
        """Save the entire Universe state to disk."""
        os.makedirs(path, exist_ok=True)
        
        # 1. Save heavy numpy arrays atomically
        arrays = {
            "staked_capacity.npy": self.staked_capacity,
            "credit_capacity.npy": self.credit_capacity,
            "volume.npy": self.volume,
            "global_trust.npy": as_numpy(self.global_trust),
            "total_debt.npy": np.array(self.total_debt),
            "total_trial_debt.npy": np.array(self.total_trial_debt),
            "join_epoch.npy": np.array(self.join_epoch),
        }
        for name, arr in arrays.items():
            tmp_path = os.path.join(path, name + ".tmp")
            dest_path = os.path.join(path, name)
            with open(tmp_path, "wb") as f:
                np.save(f, arr)
            os.replace(tmp_path, dest_path)
            
        # 2. Save heavy structures (DB or dicts)
        if self.use_disk:
            # Safely copy the SQLite database using backup API
            db_dest = os.path.join(path, "universe_state.db")
            tmp_db_dest = db_dest + ".tmp"
            
            # Ensure we have at least one DiskDict to perform the backup
            if hasattr(self, "_db_conn"):
                # Use the backup API which is safe for WAL mode
                # First ensure source is fully checkpointed
                try:
                    self._db_conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
                except Exception:
                    pass

                dest_conn = sqlite3.connect(tmp_db_dest)
                try:
                    self._db_conn.backup(dest_conn)
                finally:
                    dest_conn.close()
                os.replace(tmp_db_dest, db_dest)
        else:
            # In-memory: save the dictionaries
            dicts = {
                "S": self.S,
                "F": self.F,
                "local_trust": self.local_trust,
                "support_breakdown": self.support_breakdown,
                "S_window": self.S_window,
                "F_window": self.F_window,
                "_S_epoch_delta": self._S_epoch_delta,
                "_F_epoch_delta": self._F_epoch_delta,
                "tx_count": self.tx_count,
                "epoch_volume_added": self.epoch_volume_added,
            }
            dicts_path = os.path.join(path, "dicts.pkl")
            tmp_dicts_path = dicts_path + ".tmp"
            with open(tmp_dicts_path, "wb") as f:
                pickle.dump(dicts, f)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp_dicts_path, dicts_path)

        # 3. Save metadata LAST (this is the "commit" signal for a complete checkpoint)
        # When use_disk=True, the per-node proxy lists (_last_window_update,
        # failure_observations, tx_count, etc.) are lists of DiskNodeProxy
        # objects that hold references to DiskDict instances.  Pickling them
        # strips _conn from the underlying DiskDict (see DiskDict.__getstate__),
        # producing stub objects that cannot be used after restore.  These
        # fields are always rebuilt from the SQLite backup during load_state, so
        # we deliberately save empty sentinel lists here to avoid ever writing
        # broken proxy objects into metadata.pkl.
        _empty_if_disk = [] if self.use_disk else None
        meta = {
            "size": self.size,
            "gpu_threshold": self.gpu_threshold,
            "epoch": self.epoch,
            "seed": self.seed,
            "use_disk": self.use_disk,
            "params": self.params,
            "suite_state": self.suite_state,
            "trusted_pool": self.trusted_pool,
            "_last_window_update": _empty_if_disk if self.use_disk else self._last_window_update,
            "failure_observations": _empty_if_disk if self.use_disk else self.failure_observations,
            "vouchers": self.vouchers,
            "acquaintances": self.acquaintances,
            "_known_by": self._known_by,
            "blocked_trial_pairs": self.blocked_trial_pairs,
            "successful_transfers_global": self.successful_transfers_global,
            "_total_s_as_debtor": self._total_s_as_debtor,
            "_expiry_queue": dict(self._expiry_queue),
            "contracts": self.contracts,
            "tx_count": _empty_if_disk if self.use_disk else getattr(self, "tx_count", []),
            "epoch_volume_added": _empty_if_disk if self.use_disk else getattr(self, "epoch_volume_added", []),
            "_s_window_sums": _empty_if_disk if self.use_disk else getattr(self, "_s_window_sums", []),
            "_f_window_sums": _empty_if_disk if self.use_disk else getattr(self, "_f_window_sums", []),
            "task_id": self.task_id,
        }
        
        meta_path = os.path.join(path, "metadata.pkl")
        tmp_meta_path = meta_path + ".tmp"
        with open(tmp_meta_path, "wb") as f:
            pickle.dump(meta, f)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp_meta_path, meta_path)
            
        print(f"  [INFO] Simulation state saved to {path} (Epoch {self.epoch})")

    def _close_disk_dicts(self):
        """Close all active SQLite connections."""
        if hasattr(self, "_db_conn"):
            try:
                self._db_conn.close()
            except Exception:
                pass
        
        # Also close any that might have had their own connections
        for name in dir(self):
            if name.startswith("_disk_"):
                d = getattr(self, name)
                if hasattr(d, "close"):
                    d.close()

    @classmethod
    def load_state(cls, path: str, result_dir: Optional[str] = None, task_id: Optional[str] = None) -> 'Universe':
        """Load a Universe state from disk."""
        meta_path = os.path.join(path, "metadata.pkl")
        if not os.path.exists(meta_path):
            raise FileNotFoundError(f"Metadata file not found: {meta_path}")
            
        try:
            with open(meta_path, "rb") as f:
                meta = pickle.load(f)
        except (EOFError, pickle.UnpicklingError) as e:
            # Check if it was a 0-byte file
            size = os.path.getsize(meta_path)
            raise RuntimeError(f"Failed to load metadata from {meta_path} (size={size} bytes). File might be corrupted: {e}")
            
        # Use provided result_dir, or default to the parent of the checkpoint dir
        # (which is usually the session directory).
        actual_result_dir = result_dir if result_dir is not None else os.path.dirname(os.path.abspath(path))

        # Use provided task_id, or fall back to meta if available
        actual_task_id = task_id if task_id is not None else meta.get("task_id")

        # Initialize Universe with basic params
        uni = cls(size=meta["size"], gpu_threshold=meta["gpu_threshold"], 
                  params=meta["params"], seed=meta["seed"], 
                  use_disk=meta["use_disk"], result_dir=actual_result_dir,
                  task_id=actual_task_id)
        
        uni.epoch = meta["epoch"]
        uni.suite_state = meta["suite_state"]
        uni.trusted_pool = meta["trusted_pool"]
        uni._last_window_update = meta["_last_window_update"]
        uni.failure_observations = meta["failure_observations"]
        uni.vouchers = meta["vouchers"]
        uni.acquaintances = meta["acquaintances"]
        uni._known_by = meta["_known_by"]
        uni.blocked_trial_pairs = meta["blocked_trial_pairs"]
        uni.successful_transfers_global = meta["successful_transfers_global"]
        uni._total_s_as_debtor = meta["_total_s_as_debtor"]
        uni._expiry_queue.update(meta["_expiry_queue"])
        uni.contracts = meta["contracts"]
        uni._s_window_sums = meta["_s_window_sums"]
        uni._f_window_sums = meta["_f_window_sums"]
        uni.tx_count = meta["tx_count"]
        uni.epoch_volume_added = meta["epoch_volume_added"]
        
        # Load heavy arrays
        uni.staked_capacity = np.load(os.path.join(path, "staked_capacity.npy"))
        uni.credit_capacity = np.load(os.path.join(path, "credit_capacity.npy"))
        uni.volume = np.load(os.path.join(path, "volume.npy"))
        
        gt = np.load(os.path.join(path, "global_trust.npy"))
        xp, _ = uni._get_xp()
        uni.global_trust = xp.asarray(gt)
        
        uni.total_debt = list(np.load(os.path.join(path, "total_debt.npy")))
        uni.total_trial_debt = list(np.load(os.path.join(path, "total_trial_debt.npy")))
        uni.join_epoch = list(np.load(os.path.join(path, "join_epoch.npy")))
        
        if uni.use_disk:
            # Reattach the DB
            db_src = os.path.join(path, "universe_state.db")
            if not os.path.exists(db_src):
                # The checkpoint is incomplete: metadata.pkl was written but the
                # SQLite backup was never finished (e.g. process killed mid-save).
                # Since metadata.pkl now only stores empty lists for disk-backed
                # fields (see save_state), continuing without the DB would leave
                # the Universe with empty proxy lists — almost certainly wrong.
                # Raise immediately so the caller can treat this checkpoint as
                # missing and fall back to starting fresh.
                raise FileNotFoundError(
                    f"Incomplete checkpoint at '{path}': metadata.pkl exists but "
                    f"universe_state.db is missing.  The process was likely "
                    f"interrupted before the SQLite backup completed.  "
                    f"Delete this checkpoint directory and restart from scratch."
                )
            if os.path.exists(db_src):
                # Close the fresh DB connections created in __init__
                uni._close_disk_dicts()
                
                # CRITICAL: Delete any stale WAL/SHM files that might have been created by the fresh connect
                # This prevents SQLite from trying to apply old WAL segments to the new DB image we are about to load.
                for ext in ["", "-wal", "-shm"]:
                    p = uni._db_path + ext
                    if os.path.exists(p):
                        try:
                            os.remove(p)
                        except Exception:
                            pass

                # Copy via backup API for safety
                src_conn = sqlite3.connect(db_src)
                dest_conn = sqlite3.connect(uni._db_path)
                try:
                    src_conn.backup(dest_conn)
                finally:
                    src_conn.close()
                    dest_conn.close()
                
                # RE-OPEN the shared connection for the new universe instance
                uni._db_conn = sqlite3.connect(uni._db_path)
                uni._db_conn.execute("PRAGMA journal_mode=WAL")
                uni._db_conn.execute("PRAGMA synchronous=NORMAL")
                
                # Re-initialize DiskDict objects pointed to the now-populated uni._db_path
                uni._disk_s = DiskDict(uni._db_path, "S", connection=uni._db_conn)
                uni._disk_f = DiskDict(uni._db_path, "F", connection=uni._db_conn)
                uni._disk_lt = DiskDict(uni._db_path, "local_trust", connection=uni._db_conn)
                uni._disk_sb = DiskDict(uni._db_path, "support_breakdown", connection=uni._db_conn)
                uni._disk_s_win = DiskDict(uni._db_path, "S_window", connection=uni._db_conn)
                uni._disk_f_win = DiskDict(uni._db_path, "F_window", connection=uni._db_conn)
                uni._disk_s_win_delta = DiskDict(uni._db_path, "S_epoch_delta", connection=uni._db_conn)
                uni._disk_f_win_delta = DiskDict(uni._db_path, "F_epoch_delta", connection=uni._db_conn)
                uni._disk_tx_count = DiskDict(uni._db_path, "tx_count", connection=uni._db_conn)
                uni._disk_volume_added = DiskDict(uni._db_path, "epoch_volume_added", connection=uni._db_conn)
                uni._disk_s_win_sums = DiskDict(uni._db_path, "s_window_sums", connection=uni._db_conn)
                uni._disk_f_win_sums = DiskDict(uni._db_path, "f_window_sums", connection=uni._db_conn)
                uni._disk_fo = DiskDict(uni._db_path, "failure_observations", connection=uni._db_conn)
                uni._disk_last_win_upd = DiskDict(uni._db_path, "last_window_update", connection=uni._db_conn)
                
                uni.S = [uni._disk_s[i] for i in range(uni.size)]
                uni.F = [uni._disk_f[i] for i in range(uni.size)]
                uni.local_trust = [uni._disk_lt[i] for i in range(uni.size)]
                uni.support_breakdown = [uni._disk_sb[i] for i in range(uni.size)]
                uni.S_window = [uni._disk_s_win[i] for i in range(uni.size)]
                uni.F_window = [uni._disk_f_win[i] for i in range(uni.size)]
                uni._S_epoch_delta = [uni._disk_s_win_delta[i] for i in range(uni.size)]
                uni._F_epoch_delta = [uni._disk_f_win_delta[i] for i in range(uni.size)]
                uni.tx_count = [uni._disk_tx_count[i] for i in range(uni.size)]
                uni.epoch_volume_added = [uni._disk_volume_added[i] for i in range(uni.size)]
                uni._s_window_sums = [uni._disk_s_win_sums[i] for i in range(uni.size)]
                uni._f_window_sums = [uni._disk_f_win_sums[i] for i in range(uni.size)]
                uni.failure_observations = [uni._disk_fo[i] for i in range(uni.size)]
                uni._last_window_update = [uni._disk_last_win_upd[i] for i in range(uni.size)]
        else:
            with open(os.path.join(path, "dicts.pkl"), "rb") as f:
                dicts = pickle.load(f)
            uni.S = dicts["S"]
            uni.F = dicts["F"]
            uni.local_trust = dicts["local_trust"]
            uni.support_breakdown = dicts["support_breakdown"]
            uni.S_window = dicts["S_window"]
            uni.F_window = dicts["F_window"]
            uni._S_epoch_delta = dicts["_S_epoch_delta"]
            uni._F_epoch_delta = dicts["_F_epoch_delta"]
            uni.tx_count = dicts["tx_count"]
            uni.epoch_volume_added = dicts["epoch_volume_added"]
            
        print(f"  [INFO] Simulation state loaded from {path}")
        return uni

    def _ensure_gpu(self):
        # 1. Hardware/Library Check
        if not HAS_GPU:
            self._gpu_allowed = False
            return
        
        # 2. Worker Semaphore Check
        if not _gpu_allowed:
            self._gpu_allowed = False
            self._force_cpu = True
            return

        # 3. Explicit Back-off Check
        if self._force_cpu:
            return

        try:
            # 4. Context Check & Memory Pool initialization
            import cupy as cp
            # Ensure context exists
            _ = cp.cuda.Device(0).mem_info
            self._gpu_allowed = True
        except Exception as _e:
            if self._is_cuda_oom(_e):
                self._force_cpu = True
                self._gpu_allowed = False
                self._force_gc_cuda()
                print(f"  [WARN] GPU Memory Pressure during init (size={self.size}), fallback to CPU.")
            else:
                self._gpu_allowed = False

    def _cap_acquaintances(self, i: int):
        """Enforce Dunbar-style acquaintance cap for node i.
        
        When max_acquaintances > 0, trims the acquaintance set to at most
        max_acquaintances entries. Keeps self always. Among the rest, prefers
        nodes with the highest bilateral satisfaction S[i][j] (strongest
        trading partners), breaking ties by node id for determinism.
        """
        cap = self.params.max_acquaintances
        if cap <= 0 or len(self.acquaintances[i]) <= cap:
            return
        
        others = [j for j in self.acquaintances[i] if j != i]
        # Sort by bilateral satisfaction (strongest partners first), then by id
        others.sort(key=lambda j: (-self.S[i].get(j, 0.0), j))
        keep = set(others[:cap - 1])  # -1 to leave room for self
        keep.add(i)
        
        # Phase 14.7: Update inverse graph and prune state dictionaries
        dropped = self.acquaintances[i] - keep
        for j in dropped:
            self._known_by[j].discard(i)
            # Long-term Memory Optimization: Prune monotonically growing dicts
            self.S[i].pop(j, None)
            self.F[i].pop(j, None)
            self.local_trust[i].pop(j, None)
            self.support_breakdown[i].pop(j, None)
            self.S_window[i].pop(j, None)
            self.F_window[i].pop(j, None)
            self._last_window_update[i].pop(j, None)
            self._s_window_sums[i].pop(j, None)
            self._f_window_sums[i].pop(j, None)
            self._S_epoch_delta[i].pop(j, None)
            self._F_epoch_delta[i].pop(j, None)
            self.epoch_volume_added[i].pop(j, None)
            self.tx_count[i].pop(j, None)
            
            # Remove node i from j's failure observations (i no longer tracks j)
            if j < len(self.failure_observations):
                self.failure_observations[j].pop(i, None)
            
        self.acquaintances[i] = keep

    def get_failure_witness_count(self, debtor: int) -> int:
        """Get the count of independent creditors who have observed this debtor default.
        
        Used for witness-based contagion: more witnesses = stricter tau_eff.
        """
        return len(self.failure_observations[debtor])

    def get_aggregate_witness_rate(self, debtor: int) -> float:
        """Compute the median bilateral F/(S+F) across failure witnesses of debtor.
        
        Returns 0.0 if fewer than min_contagion_witnesses witnesses exist.
        Cached per debtor per trust rebuild cycle (cleared in _rebuild_local_trust).
        
        This addresses the selective defaulting gap: when an attacker cooperates
        with observer i (bilateral r_ij = 0) but defaults on others, the median
        of the witnesses' bilateral rates provides a nonzero floor for r_eff.
        """
        if debtor in self._witness_rate_cache:
            return self._witness_rate_cache[debtor]
        
        try:
            witnesses = self.failure_observations[debtor]  # {creditor: epoch}
        except IndexError:
            print(f"  [ERROR] IndexError in get_aggregate_witness_rate: debtor={debtor}, len(failure_observations)={len(self.failure_observations)}, size={self.size}, task_id={self.task_id}")
            raise

        if len(witnesses) < self.params.min_contagion_witnesses:
            self._witness_rate_cache[debtor] = 0.0
            return 0.0
        
        rates = []
        for w in witnesses:
            s_w = self.S[w].get(debtor, 0.0)
            f_w = self.F[w].get(debtor, 0.0)
            total = s_w + f_w
            if total > 0:
                rates.append(f_w / total)
        
        if len(rates) < self.params.min_contagion_witnesses:
            self._witness_rate_cache[debtor] = 0.0
            return 0.0
        
        rates.sort()
        mid = len(rates) // 2
        median_rate = rates[mid] if len(rates) % 2 == 1 else (rates[mid - 1] + rates[mid]) / 2.0
        
        self._witness_rate_cache[debtor] = median_rate
        return median_rate

    def add_nodes(self, n: int):
        """
        Dynamically adds n new nodes to the network.
        Maintains trust conservation by scaling existing trust and initializing 
        new nodes with PageRank-neutral mass (1/N_new).
        """
        old_size = self.size
        new_size = old_size + n
        
        # 1. Scale existing trust (Trust Conservation)
        scale = old_size / new_size
        # credit_capacity, volume, and staked_capacity stay on CPU (numpy)
        self.credit_capacity = np.asarray(as_numpy(self.credit_capacity))
        self.volume = np.asarray(as_numpy(self.volume))
        self.staked_capacity = np.asarray(as_numpy(self.staked_capacity))

        try:
            xp, _ = self._get_xp()
            self.global_trust = xp.asarray(as_numpy(self.global_trust))
            self.global_trust = xp.concatenate([
                self.global_trust * scale,
                xp.full(n, 1.0 / new_size, dtype=xp.float64)
            ])
        except Exception as _e:
            if self._is_cuda_oom(_e):
                self._force_cpu = True
                self._force_gc_cuda()
                gt_cpu = as_numpy(self.global_trust)
                self.global_trust = np.concatenate([
                    gt_cpu * scale,
                    np.full(n, 1.0 / new_size, dtype=np.float64)
                ])
            else:
                raise
        
        # 2. Extend all state lists
        self.S.extend([{} for _ in range(n)])
        self.F.extend([{} for _ in range(n)])
        self.local_trust.extend([{} for _ in range(n)])
        self.support_breakdown.extend([{} for _ in range(n)])
        self.acquaintances.extend([set() for _ in range(n)])
        self.contracts.extend([[] for _ in range(n)])
        self.failure_observations.extend([{} for _ in range(n)])
        self.vouchers.extend([{} for _ in range(n)])
        
        self.credit_capacity = np.concatenate([
            self.credit_capacity,
            np.full(n, self.params.base_capacity, dtype=np.float64)
        ])
        self.volume = np.concatenate([
            self.volume,
            np.full(n, self.params.base_capacity, dtype=np.float64)
        ])
        self.staked_capacity = np.concatenate([
            self.staked_capacity,
            np.zeros(n, dtype=np.float64)
        ])
        
        self.extinguished_this_epoch.extend([0.0] * n)
        self.debt_acquired_this_epoch.extend([0.0] * n)
        self.join_epoch.extend([self.epoch] * n)
        self._witness_rate_cache = {} # Clear cache
        self.is_active.extend([True] * n)
        
        self.trial_tx_count.extend([0] * n)
        self.epoch_volume_added.extend([{} for _ in range(n)])
        self.tx_count.extend([{} for _ in range(n)])
        self.successful_transfers_global.extend([0] * n)
        self.total_debt.extend([0.0] * n)
        self.total_trial_debt.extend([0.0] * n)
        
        # Extend window structures
        self.S_window.extend([{} for _ in range(n)])
        self.F_window.extend([{} for _ in range(n)])
        self._s_window_sums.extend([{} for _ in range(n)])
        self._f_window_sums.extend([{} for _ in range(n)])
        self._S_epoch_delta.extend([{} for _ in range(n)])
        self._F_epoch_delta.extend([{} for _ in range(n)])
        self._last_window_update.extend([{} for _ in range(n)])
        self._total_s_as_debtor.extend([0.0] * n)
        
        # 3. Initialize acquaintances for new nodes (Evidence-Gated Model)
        # New nodes start with only themselves - acquaintances grow through transactions
        self._known_by.extend([set() for _ in range(n)])
        for i in range(old_size, new_size):
            self.acquaintances[i] = {i}  # Only self at birth
            self._known_by[i] = {i}
            
        # 4. Clear caches and integrate
        self.size = new_size
        self._cached_C = None        # Sparse transit matrix
        self._cached_MT = None       # Transposed for MVM
        self._cached_A = None        # Binary adjacency matrix
        self._reputation_cache = {}
        self._subjective_cache = {}
        self._subjective_row_cache = {}
        self._subjective_cache_epoch = -1
        self._dirty_nodes_trust.update(range(old_size, new_size))
        
        self._rebuild_local_trust()
        self.run_eigentrust()
        self.update_credit_capacity()

    # =========================================================================
    #  Backend Helpers
    # =========================================================================
    def _get_xp(self):
        """Unified backend selector: returns (numpy/cupy, scipy.sparse/cupyx.scipy.sparse)"""
        if HAS_GPU and _gpu_allowed and not self._force_cpu and self.size >= self.gpu_threshold:
            _ensure_gpu()
            return cp, cp_sp
        return np, sp

    # =========================================================================
    #  Pre-Trust (Definition 3)
    # =========================================================================

    def _catch_up_window(self, i: int, j: int, to_epoch: Optional[int] = None):
        """Lazy fill the S/F window with zeros for epochs where node i,j were silent."""
        if to_epoch is None:
            to_epoch = self.epoch
            
        last = self._last_window_update[i].get(j, -1)
        if last < 0:
            # First time ever seeing this pair
            return
            
        missed = to_epoch - last
        if missed > 0:
            k = self.params.recent_window_k
            if j not in self.S_window[i]:
                self.S_window[i][j] = _col.deque(maxlen=k)
                self.F_window[i][j] = _col.deque(maxlen=k)
                self._s_window_sums[i][j] = 0.0
                self._f_window_sums[i][j] = 0.0
                
            # Cap missed at K to avoid huge loops/memory if a pair was silent for 1M epochs
            fill = min(missed, k)
            for _ in range(fill):
                # When popping from full deque, update sums
                if len(self.S_window[i][j]) == k:
                    self._s_window_sums[i][j] -= self.S_window[i][j][0]
                    self._f_window_sums[i][j] -= self.F_window[i][j][0]
                
                win_s = self.S_window[i][j]
                win_f = self.F_window[i][j]
                win_s.append(0.0)
                win_f.append(0.0)
                self.S_window[i][j] = win_s
                self.F_window[i][j] = win_f
            
            self._last_window_update[i][j] = to_epoch

    def get_attenuated_score(self, i: int, j: int, s_i_dict=None, f_i_dict=None) -> float:
        """Calculate attenuated trust score s_ij * phi(r_eff)."""
        # 1. Check rebuild-level cache
        cache_key = (i, j)
        if cache_key in self._attenuated_score_cache:
            return self._attenuated_score_cache[cache_key]

        if s_i_dict is not None:
            s_val = s_i_dict.get(j, 0.0)
        else:
            s_val = self.S[i].get(j, 0.0)
            
        if f_i_dict is not None:
            f_val = f_i_dict.get(j, 0.0)
        else:
            f_val = self.F[i].get(j, 0.0)
            
        total_interactions = s_val + f_val
        if total_interactions <= 0 or s_val <= 0:
            self._attenuated_score_cache[cache_key] = 0.0
            return 0.0

        # --- Hot-path: hoist param lookups ---
        params = self.params
        _log1p = math.log1p

        # Cumulative failure rate
        r_cumul = f_val / total_interactions

        # Recent-window failure rate (Lazy Catch-up + use O(1) tracking sums)
        self._catch_up_window(i, j, to_epoch=self.epoch)
        s_win = self._s_window_sums[i].get(j, 0.0)
        f_win = self._f_window_sums[i].get(j, 0.0)
        win_total = s_win + f_win
        r_recent = f_win / win_total if win_total > 0 else 0.0
        r = max(r_cumul, params.recent_weight * r_recent)

        # Aggregate witness contagion
        r_witness = self.get_aggregate_witness_rate(j)
        if r_witness > 0:
            r = max(r, params.witness_discount * r_witness)

        n_ij = total_interactions
        age = max(1, self.epoch - self.join_epoch[j])
        n_mat_eff = min(n_ij, params.max_volume_per_epoch * age)
        
        n_mat = params.volume_maturation
        # Use precomputed log1p(n_mat) if available
        inv_log_nmat = self._inv_log_nmat
        vol_ratio = min(_log1p(n_mat_eff) * inv_log_nmat, 1.0)
        tau_eff = params.tau_newcomer + (params.failure_tolerance - params.tau_newcomer) * vol_ratio
        
        witness_count = self.get_failure_witness_count(j)
        contagion_factor = 1.0 + params.contagion_witness_factor * witness_count
        tau_eff_contagion = tau_eff / contagion_factor
        
        gamma = params.penalty_sharpness
        phi = max(0.0, 1.0 - (r / tau_eff_contagion) ** gamma) if tau_eff_contagion > 0 else (0.0 if f_val > 0 else 1.0)
        
        s_eff = min(s_val, n_mat * params.trust_banking_bound_fraction)
        score = s_eff * phi
        self._attenuated_score_cache[cache_key] = score
        return score

    def get_pre_trust_vector(self, observer_id: Optional[int] = None) -> List[float]:
        """Returns the pre-trust vector p.

        - If observer_id is provided: personalized p^(i) over EVIDENCED acquaintances
          (volume-weighted, evidence-gated: only acquaintances with S_ij > 0 receive mass).
          This is the "Pure Subjectivity" base: every node acts as its own centre.
          Peers with only F evidence (defaulters) are NOT included, preventing the
          "Sybil-at-birth" problem.
        - If None: uniform vector over all nodes in the network (telemetry only).
        """
        if observer_id is not None:
            # VOLUME-WEIGHTED PRE-TRUST
            # Hot-path: hoist attribute lookups; use inline cache for attenuated score
            _cache = self._attenuated_score_cache
            vol_mat = self.params.volume_maturation
            inv_vol_mat = 1.0 / vol_mat if vol_mat > 0 else 1.0
            evidenced_weights = {}
            total_assigned_mass = 0.0
            
            for j in self.acquaintances[observer_id]:
                if j == observer_id:
                    continue
                
                # Inline cache lookup to avoid function-call overhead (scores were
                # already computed in compute_local_trust's peer loop above).
                cache_key = (observer_id, j)
                attenuated_score = _cache.get(cache_key)
                if attenuated_score is None:
                    attenuated_score = self.get_attenuated_score(observer_id, j)
                if attenuated_score > 0:
                    w_ij = min(1.0, attenuated_score * inv_vol_mat)
                    evidenced_weights[j] = w_ij
                    total_assigned_mass += w_ij

            p = {}
            self_mass = max(0.0, 1.0 - total_assigned_mass)
            if self_mass > 0:
                p[observer_id] = self_mass
            
            for j, weight in evidenced_weights.items():
                p[j] = weight
            
            total_p = sum(p.values())
            if total_p > 0:
                inv_total_p = 1.0 / total_p
                for j in p:
                    p[j] *= inv_total_p
            
            return p
        else:
            # Global uniform distribution over TRUSTED nodes only
            # In a normal network, everyone is trusted conceptually by the "system".
            # In an attack scenario, the "Global Proxy" should represent the
            # aggregate view of the honest majority, not God's view of sybils.
            if self.trusted_pool is not None and len(self.trusted_pool) > 0:
                p = [0.0] * self.size
                val = 1.0 / len(self.trusted_pool)
                for idx in self.trusted_pool:
                    if idx < self.size:
                        p[idx] = val
                return p
            
            # Default: Strictly uniform distribution over all nodes
            return [1.0 / self.size] * self.size

    # =========================================================================
    #  Local Trust (Definition 4)
    # =========================================================================

    def compute_local_trust(self, i: int):
        """
        Compute normalized local trust c_ij for node i.
        
        Rate-based trust attenuation (Definition 5/6 of whitepaper):
          r_ij = F_ij / (S_ij + F_ij)               -- failure rate
          tau_eff = maturity curve (N_mat)          -- tolerance
          phi(r) = max(0, 1 - (r / tau_eff)^gamma)  -- attenuation function
          S_eff = min(S_ij, N_mat * f_bank)         -- score multiplier cap
          s_ij = S_eff * phi(r_ij)                   -- raw local trust
        
        The S cap in the score bounds the absolute "leverage" a node can extract
        from a single honest edge, mitigating "trust banking" exit scams without
        breaking the unbounded S in the rate denominator, which correctly enables
        long-term honest nodes to dilute arbitrary historical failures.
        
        Then normalize: c_ij = s_ij / sum_k(s_ik).
        Dangling nodes (no history) are left empty -> handled in power iteration.
        """
        # --- Hot-path optimization: fetch full dicts once for DiskDict efficiency ---
        # If using disk-backed SQLite, this avoids 3,000,000+ SELECT calls per tick.
        if hasattr(self.S[i], 'get_node_dict'): # DiskNodeProxy
            s_i_dict = self.S[i].disk_dict.get_node_dict(i)
            f_i_dict = self.F[i].disk_dict.get_node_dict(i)
        else:
            s_i_dict = self.S[i]
            f_i_dict = self.F[i]
            
        _cache = self._attenuated_score_cache
        raw_trust: Dict[int, float] = {}
        total_mass = 0.0
        
        peers = set(s_i_dict.keys())
        peers.update(f_i_dict.keys())
        for j in peers:
            # Inline cache check to avoid function-call overhead for cache hits
            cache_key = (i, j)
            cached = _cache.get(cache_key)
            if cached is not None:
                if cached > 0:
                    raw_trust[j] = cached
                    total_mass += cached
                continue
            
            # Pass the pre-fetched dicts into get_attenuated_score
            score = self.get_attenuated_score(i, j, s_i_dict=s_i_dict, f_i_dict=f_i_dict)
            if score > 0:
                raw_trust[j] = score
                total_mass += score

        n_mat = self.params.volume_maturation
        lt_i: Dict[int, float] = {}
        # Fix: If using disk-backed storage, we must update the proxy instead of
        # replacing it with a plain dict, otherwise it will never be written to DB.
        proxy = self.local_trust[i]
        
        # Point: Confidence-Weighted Trust
        # Instead of binary row stochasticity, we blend the row with pre-trust 
        # based on total bilateral evidence (Sigma_i).
        # This prevents tiny evidence from hijacking 100% of a node's reputation mass.
        w_i = min(1.0, total_mass / n_mat) if n_mat > 0 else 1.0
        
        # 1. Add weighted local evidence (only if total_mass > 0)
        if total_mass > 0:
            inv_total = w_i / total_mass
            for j, score in raw_trust.items():
                lt_i[j] = score * inv_total
        
        # 2. Add weighted pre-trust baseline (1 - w_i)
        # This fills the "confidence gap" by returning mass to the safe teleportation baseline.
        # This also handles dangling nodes naturally (w_i = 0).
        # Skip entirely when w_i >= 1.0 (saturated evidence — no pre-trust needed).
        pre_trust_weight = 1.0 - w_i
        if pre_trust_weight > 0:
            p_i = self.get_pre_trust_vector(observer_id=i)
            if isinstance(p_i, dict):
                for j, p_val in p_i.items():
                    if p_val > 0:
                        lt_i[j] = lt_i.get(j, 0.0) + pre_trust_weight * p_val
            else:
                for j, p_val in enumerate(p_i):
                    if p_val > 0:
                        lt_i[j] = lt_i.get(j, 0.0) + pre_trust_weight * p_val

        if hasattr(proxy, 'disk_dict'):  # Is a DiskNodeProxy
            proxy.disk_dict.replace_node_dict(i, lt_i)
        else:
            self.local_trust[i] = lt_i

    def _rebuild_local_trust(self):
        """Recompute local trust only for nodes with changed S or F metrics."""
        self._attenuated_score_cache = {} # Clear per-rebuild cache
        self._witness_rate_cache = {}    # Clear per-rebuild cache
        
        for i in self._dirty_nodes_trust:
            self.compute_local_trust(i)
        
        if self._dirty_nodes_trust:
            self._cached_C = None
            self._cached_MT = None
            self._cached_A = None
            self._subjective_cache = {}
            self._subjective_row_cache = {}
            self._dirty_nodes_trust.clear()

    # =========================================================================
    #  Telemetry Helpers (On-Device Stats)
    # =========================================================================

    def get_trust_stats(self, indices: Optional[List[int]] = None) -> Dict[str, float]:
        """Compute trust statistics directly on the active backend (efficient)."""
        xp, _ = self._get_xp()
        data = self.global_trust
        if isinstance(data, list):
            data = xp.array(data, dtype=xp.float64)

        if indices is not None:
            data = data[indices]
            
        if len(data) == 0:
            return {'mean': 0.0, 'max': 0.0, 'sum': 0.0}
            
        return {
            'mean': float(data.mean()),
            'max': float(data.max()),
            'sum': float(data.sum())
        }

    def get_capacity_stats(self, indices: Optional[List[int]] = None) -> Dict[str, float]:
        """Compute capacity statistics (CPU-pinned arrays)."""
        data = self.credit_capacity
        if indices is not None:
            data = data[indices]
            
        if len(data) == 0:
            return {'mean': 0.0, 'max': 0.0, 'sum': 0.0}
            
        return {
            'mean': float(data.mean()),
            'max': float(data.max()),
            'sum': float(data.sum())
        }

    # =========================================================================
    #  EigenTrust Power Iteration (Theorem 1)
    # =========================================================================

    def run_eigentrust(self):
        """Compute global trust vector for telemetry purposes."""
        # Ensure local trust is recomputed before building the matrix
        self._rebuild_local_trust()
        p = self.get_pre_trust_vector()
        self.global_trust = self._power_iteration(p)

    def validate_convergence(self, reference_iterations: int = 84) -> Dict[str, float]:
        """
        Compare the simulation's truncated EigenTrust (20 iterations) against
        the whitepaper reference (84 iterations) to verify that they converge
        to sufficiently close stationary distributions.
        
        The simulation uses `params.eigentrust_iterations = 20` for speed. The
        whitepaper and Rust production use `EIGENTRUST_MAX_ITERATIONS = 84`,
        which guarantees convergence even on adversarial/sparse graphs via
        (1-alpha)^84 ~ 9.5e-4 < epsilon.
        
        This validator runs BOTH iteration counts on the current local trust
        state and returns the L1 distance between the two stationary vectors.
        For well-connected graphs (typical simulation topology) the early-exit
        condition fires well before iteration 20, so both runs produce
        identical results.
        
        Returns dict with keys: 'l1_diff', 'max_diff', 'sim_converged_at_iter',
        'ref_converged_at_iter', 'within_epsilon'.
        """
        self._rebuild_local_trust()
        p = self.get_pre_trust_vector()
        
        # Temporarily swap iterations
        orig_iters = self.params.eigentrust_iterations
        
        try:
            # Sim run
            self.params.eigentrust_iterations = orig_iters
            t_sim = self._power_iteration(p)
            t_sim_np = as_numpy(t_sim)
            
            # Reference run
            self.params.eigentrust_iterations = reference_iterations
            t_ref = self._power_iteration(p)
            t_ref_np = as_numpy(t_ref)
        finally:
            self.params.eigentrust_iterations = orig_iters
        
        l1_diff = float(np.sum(np.abs(t_sim_np - t_ref_np)))
        max_diff = float(np.max(np.abs(t_sim_np - t_ref_np)))
        within_epsilon = l1_diff < 10.0 * self.params.eigentrust_epsilon
        
        return {
            'l1_diff': l1_diff,
            'max_diff': max_diff,
            'sim_iterations': orig_iters,
            'ref_iterations': reference_iterations,
            'within_epsilon': within_epsilon,
            'epsilon': self.params.eigentrust_epsilon,
        }

    def _power_iteration(self, p: List[float]) -> Any:
        """
        Compute reputation vector via Eq. 4:
          t^(k+1) = (1 - alpha) * C^T * t^(k) + alpha * p
        """
        xp, xp_sp = self._get_xp()
        alpha = self.params.eigentrust_alpha

        # Check if cached matrix matches current backend (CuPy sparse lacks .device)
        is_xp_gpu = (xp is not np)
        cached_is_gpu = (self._cached_MT is not None and 
                         self._cached_MT.__class__.__module__.startswith('cupyx.scipy.sparse'))
        
        if self._cached_C is None or (is_xp_gpu != cached_is_gpu):
            self._build_sparse_matrix(xp)
            # Re-fetch backend in case _build_sparse_matrix triggered an OOM fallback
            xp, xp_sp = self._get_xp()

        try:
            MT = self._cached_MT
            p_arr = xp.array(p, dtype=xp.float64)
            t = p_arr.copy()

            for _ in range(self.params.eigentrust_iterations):
                network_part = MT @ t

                # Dangling node mass redistribution
                mass_lost = 1.0 - xp.sum(network_part)

                new_t = ((1.0 - alpha) * network_part
                         + (alpha + (1.0 - alpha) * mass_lost) * p_arr)

                diff = xp.sum(xp.abs(new_t - t))
                t = new_t
                if diff < self.params.eigentrust_epsilon:
                    break

            # Ensure non-negative and normalized
            t = xp.maximum(t, 0.0)
            total = xp.sum(t)
            if total > 0:
                t /= total

            return t
        except Exception as e:
            if self._is_cuda_oom(e):
                # GPU OOM — fall back to CPU permanently for this instance
                self._force_cpu = True
                print(f"  [WARN] GPU OOM in eigentrust (size={self.size}), falling back to CPU")
                self._force_gc_cuda()
                # Rebuild matrix on CPU
                self._build_sparse_matrix(xp=np)
                MT = self._cached_MT
                p_arr = np.array(p, dtype=np.float64)
                t = p_arr.copy()
                for _ in range(self.params.eigentrust_iterations):
                    network_part = MT @ t
                    mass_lost = 1.0 - np.sum(network_part)
                    new_t = ((1.0 - alpha) * network_part
                             + (alpha + (1.0 - alpha) * mass_lost) * p_arr)
                    diff = np.sum(np.abs(new_t - t))
                    t = new_t
                    if diff < self.params.eigentrust_epsilon:
                        break
                t = np.maximum(t, 0.0)
                total = np.sum(t)
                if total > 0:
                    t /= total
                return t
            else:
                raise

    def _build_sparse_matrix(self, xp=None):
        """Internal helper to build the sparse transition matrix."""
        if xp is None:
            xp, _ = self._get_xp()
            
        nnz = sum(len(row) for row in self.local_trust)
        rows = np.empty(nnz, dtype=np.int32)
        cols = np.empty(nnz, dtype=np.int32)
        data = np.empty(nnz, dtype=np.float64)
        
        pos = 0
        for i in range(self.size):
            row = self.local_trust[i]
            if not row: continue
            r_targets = list(row.keys())
            r_vals = list(row.values())
            r_size = len(row)
            rows[pos:pos+r_size] = i
            cols[pos:pos+r_size] = r_targets
            data[pos:pos+r_size] = r_vals
            pos += r_size

        # 2. Build CSR matrix on CPU first (SciPy's C-based constructor is faster for assembly)
        C_cpu = sp.csr_matrix((data, (rows, cols)), shape=(self.size, self.size), dtype=np.float64)
        
        if xp is not np and HAS_GPU:
            try:
                # Transfer the finalized CSR structure to GPU.
                # Building a CuPy CSR from existing arrays is near-instant, vs 1s+ for assembly.
                C = cp_sp.csr_matrix((cp.asarray(C_cpu.data), 
                                      cp.asarray(C_cpu.indices), 
                                      cp.asarray(C_cpu.indptr)), 
                                     shape=C_cpu.shape, dtype=np.float64)
                self._cached_A = (C > 0).astype(cp.float64)
            except Exception as e:
                if self._is_cuda_oom(e):
                    self._force_cpu = True
                    print(f"  [WARN] GPU OOM in _build_sparse_matrix (size={self.size}), falling back to CPU")
                    self._force_gc_cuda()
                    C = C_cpu
                    self._cached_A = (C > 0).astype(np.float64)
                else:
                    raise
        else:
            C = C_cpu
            self._cached_A = (C > 0).astype(np.float64)
        
        self._cached_C = C
        self._cached_MT = C.transpose().tocsr()



    def _bfs_subgraph(self, observer_id: int) -> List[int]:
        """
        Bounded BFS expansion from observer's acquaintance set.
        
        Implements Whitepaper §5 Subjective Local Expansion step 3:
            Start from A_observer and expand up to `subgraph_max_depth` hops or
            until `max_subgraph_nodes` total vertices are discovered.
        
        Returns the list of node ids in the subgraph (always includes observer).
        """
        max_depth = self.params.subgraph_max_depth
        max_nodes = self.params.max_subgraph_nodes
        
        visited = {observer_id}
        # Seed frontier with observer's direct acquaintances
        frontier = set()
        for j in self.acquaintances[observer_id]:
            if j != observer_id:
                frontier.add(j)
                visited.add(j)
        
        if len(visited) >= max_nodes:
            return list(visited)[:max_nodes]
        
        # BFS expansion: at each depth level, add trust-linked neighbors
        # of the current frontier. Edges come from local_trust[i] (outgoing
        # trust rows published to the "DHT" — i.e. readable by any observer).
        for depth in range(1, max_depth):
            next_frontier = set()
            for node in frontier:
                for j in self.local_trust[node]:
                    if j not in visited:
                        next_frontier.add(j)
                        visited.add(j)
                        if len(visited) >= max_nodes:
                            return list(visited)
            if not next_frontier:
                break
            frontier = next_frontier
        
        return list(visited)

    def compute_subjective_trust_bfs(self, observer_id: int) -> Dict[int, float]:
        """
        Bounded-BFS subjective reputation (Whitepaper §5 Subjective Local Expansion).
        
        Algorithm (matches production Rust trust/mod.rs):
            1. BFS from observer's acquaintance set to depth `subgraph_max_depth`
               or up to `max_subgraph_nodes`, whichever is reached first.
            2. Build sparse sub-matrix M_sub over the discovered nodes, using
               published trust rows (local_trust) restricted to the subgraph.
            3. Run power iteration on M_sub with observer-personalized pre-trust
               (volume-weighted evidenced mass; matches get_pre_trust_vector).
            4. Return stationary subgraph vector as {node_id: trust}.
        
        Unlike the truncated-PageRank fallback, this computation:
            - Actually restricts to a bounded subgraph (O(|A|^d) vertices)
            - Uses real convergence (up to eigentrust_iterations) on the submatrix
            - Matches the production Holochain implementation's complexity
        
        Returns a dict mapping {node_id: trust_value} for nodes in the subgraph
        (nodes outside the subgraph have trust = 0 from this observer's view).
        """
        # Step 1: BFS expansion
        sub_nodes = self._bfs_subgraph(observer_id)
        n_sub = len(sub_nodes)
        
        if n_sub < 2:
            # Lone observer — matches Rust trust/mod.rs:104-114 (size < 2 returns 0)
            return {}
        
        idx_map = {node: i for i, node in enumerate(sub_nodes)}
        
        # Step 2: Build sparse sub-matrix C_sub (row-stochastic trust matrix
        # restricted to subgraph nodes). Keep only edges whose target is also
        # in the subgraph.
        rows_list = []
        cols_list = []
        data_list = []
        for i_global in sub_nodes:
            i_sub = idx_map[i_global]
            for j_global, weight in self.local_trust[i_global].items():
                if j_global in idx_map:
                    rows_list.append(i_sub)
                    cols_list.append(idx_map[j_global])
                    data_list.append(weight)
        
        if not data_list:
            # No edges in subgraph — return observer-only pre-trust
            return {observer_id: 1.0}
        
        # Step 2-4: Build and Solve (GPU-capable)
        xp, xp_sp = self._get_xp()
        
        while True:
            try:
                # Step 2: Build sparse sub-matrix C_sub (row-stochastic trust matrix
                # restricted to subgraph nodes). Keep only edges whose target is also
                # in the subgraph.
                rows = xp.array(rows_list, dtype=xp.int32)
                cols = xp.array(cols_list, dtype=xp.int32)
                data = xp.array(data_list, dtype=xp.float64)
                
                C_sub = xp_sp.csr_matrix((data, (rows, cols)), shape=(n_sub, n_sub),
                                       dtype=xp.float64)
                # Re-normalize rows (weights may no longer sum to 1 after truncation)
                row_sums = xp.asarray(C_sub.sum(axis=1)).flatten()
                row_sums_nz = xp.where(row_sums == 0, 1.0, row_sums)
                D_inv = xp_sp.diags(1.0 / row_sums_nz)
                C_sub = D_inv @ C_sub
                
                # Step 3: Build observer-personalized pre-trust over subgraph nodes
                p_dict = self.get_pre_trust_vector(observer_id=observer_id)
                p_sub = xp.zeros(n_sub, dtype=xp.float64)
                for node, weight in p_dict.items():
                    if node in idx_map:
                        p_sub[idx_map[node]] = weight
                p_sum = float(p_sub.sum())
                if p_sum > 0:
                    p_sub /= p_sum
                else:
                    # Fallback: concentrate pre-trust on observer
                    p_sub[idx_map[observer_id]] = 1.0
                
                # Step 4: Power iteration on the submatrix
                alpha = self.params.eigentrust_alpha
                MT_sub = C_sub.transpose().tocsr()
                
                t = p_sub.copy()
                for _ in range(self.params.eigentrust_iterations):
                    # Standard EigenTrust: t = (1-alpha) * C^T t + alpha * p
                    t_new = (1.0 - alpha) * (MT_sub @ t) + alpha * p_sub
                    # Handle dangling nodes: redistribute their mass via teleportation
                    mass_lost = 1.0 - float(t_new.sum())
                    if mass_lost > 1e-12:
                        t_new = t_new + mass_lost * p_sub
                    diff = float(xp.abs(t_new - t).sum())
                    t = t_new
                    if diff < self.params.eigentrust_epsilon:
                        break
                
                t = xp.maximum(t, 0.0)
                # Normalize so the sub-stationary distribution sums to 1
                s = float(t.sum())
                if s > 0:
                    t /= s
                
                t_cpu = as_numpy(t)
                return {sub_nodes[i]: float(t_cpu[i]) for i in range(n_sub)}
                
            except Exception as e:
                if xp is not np and self._is_cuda_oom(e):
                    # GPU OOM — fall back to CPU permanently for this instance
                    self._force_cpu = True
                    print(f"  [WARN] GPU OOM in subjective BFS (obs={observer_id}, size={n_sub}), falling back to CPU")
                    self._force_gc_cuda()
                    xp, xp_sp = np, sp
                    continue
                else:
                    raise

    def run_bulk_subjective_trust(self, observer_ids: Optional[List[int]] = None):
        """
        Compute subjective reputation for a batch of observers using
        bounded-BFS subgraph expansion (Whitepaper §5).
        
        This replaces the legacy truncated-PageRank path. Each observer's
        subgraph is computed independently via `compute_subjective_trust_bfs`,
        and the resulting sparse vector is stored in `_subjective_row_cache`
        as a dense length-N numpy array (zero-padded for nodes outside the
        subgraph) to preserve the existing cache protocol.
        
        Complexity per observer: O(|A|^d * K_iter) where |A| is the
        acquaintance set size and d = subgraph_max_depth. For bounded |A|
        (Dunbar cap = 150) and d = 4, this is O(150^4) ≈ 5e8 worst-case,
        but in practice the subgraph is capped at `max_subgraph_nodes`
        (default 50,000). For small simulations (N < 1000) the subgraph
        typically covers the entire network; for large simulations it is
        bounded by the cap.
        """
        if observer_ids is not None and not observer_ids:
            return
        
        # Ensure local trust rows are up to date (matches Rust publish_trust_row)
        if self._cached_MT is None:
            self.run_eigentrust()
        
        t_start = time.perf_counter()
        batch_ids = observer_ids if observer_ids is not None else list(range(self.size))
        
        for obs in batch_ids:
            # Cache eviction: keep at most 2000 observer vectors
            if len(self._subjective_row_cache) >= 2000:
                to_del = list(self._subjective_row_cache.keys())[:500]
                for k in to_del:
                    del self._subjective_row_cache[k]
            
            # Compute subgraph trust via bounded BFS
            subgraph_trust = self.compute_subjective_trust_bfs(obs)
            
            # Store as dense length-N vector (zero-padded outside subgraph)
            row = np.zeros(self.size, dtype=np.float32)
            for node_id, trust_val in subgraph_trust.items():
                row[node_id] = trust_val
            self._subjective_row_cache[obs] = row
        
        t_end = time.perf_counter()
        if self.epoch % 25 == 0 and batch_ids:
            print(f"  [PROF] Bulk Subjective BFS: size={self.size}, "
                  f"k={len(batch_ids)}, total={t_end-t_start:.4f}s")

    def update_credit_capacity(self):
        """
        Global Credit Capacity Proxy (Whitepaper Definition 2.4).
        
        Cap_i = V_staked_i + beta * ln(max(1, t_i / t_baseline)) * (1 - exp(-|A_i| / n0))
        
        where t_baseline = alpha / N in the global-proxy path (exact for the
        subjective path is alpha / |A_observer|; see `get_subjective_capacity`).
        
        Note on |A_i|: per the whitepaper, the acquaintance saturation factor is
        evaluated with each node's OWN acquaintance count, not the network average.
        This yields per-node capacity growth that tracks individual connectivity
        rather than a single network-wide saturation constant.
        
        Unvouched agents CAN still receive positive capacity once they graduate
        via trial transactions (bilateral S-mass accumulation drives rel_rep > 1
        in EigenTrust). This matches the Rust production behavior at
        capacity.rs:11-24 ("GRADUATION" log), which has no vouching gate on the
        capacity formula. See Whitepaper Theorem 2.3 (Fair Bootstrapping).
        """
        n = self.size
        
        # 1. Global Reputation Proxy (pinned to CPU for capacity logic)
        t_cpu = as_numpy(self.global_trust)
        
        # 2. Base Capacity (from Vouching)
        base = self.staked_capacity
        
        # 3. Baseline Reputation (alpha / N)
        t_baseline = self.params.eigentrust_alpha / n
        
        # 4. Boost Multiplier: beta * ln(max(1, t_i / t_baseline))
        rel_rep = t_cpu / t_baseline
        rel_rep[rel_rep < 1.0] = 1.0
        boost = self.params.capacity_beta * np.log(rel_rep)
        
        # 5. Per-node saturation: (1 - exp(-|A_i| / n0)) per Whitepaper Def 2.4
        degrees = np.array([len(self.acquaintances[i]) for i in range(n)], dtype=np.float64)
        saturations = 1.0 - np.exp(-degrees / self.params.acq_saturation)
        
        # 6. Final Global Capacity Proxy (element-wise per-node)
        # No vouching gate: agents with base=0 can still earn reputation-derived
        # capacity (matches Rust capacity.rs:11-24 — "GRADUATION" behavior).
        new_capacity = base + boost * saturations
            
        # 7. Sync back to state
        self.credit_capacity = new_capacity
        self.volume = new_capacity.copy()

    def get_subjective_reputation(self, observer_id: int, target_id: int) -> float:
        """Lazy lookup with batch-computation support."""
        if observer_id not in self._subjective_row_cache:
            # Batch compute this observer (and potentially others if we had a queue)
            self.run_bulk_subjective_trust([observer_id])
        
        # row_cache[observer_id] is a vector of length N
        # T[target_id, observer_id] is reputation of target from observer's view.
        return float(self._subjective_row_cache[observer_id][target_id])
    def get_subjective_capacity(self, observer: int, target: int) -> float:
         """
         Compute credit capacity of target from observer's subjective perspective.
         
         Protocol-accurate capacity per Whitepaper Definition 2.4, mirroring the
         Rust production implementation at capacity.rs:11-24:
           Cap_target = V_staked_target + beta * ln(max(1, t^(observer)_target / t_baseline))
                        * (1 - exp(-|A_target| / n0))
         
         Key subtlety — which |A| to use:
           * t_baseline uses the OBSERVER's acquaintance count: t_baseline = alpha / |A_observer|.
             This is the observer's noise floor — the reputation a random unknown node
             would receive under observer-local teleportation (Remark 5.2, Scale Invariance).
           * The saturation factor (1 - exp(-|A|/n0)) uses the TARGET's acquaintance count,
             since it reflects the target's embeddedness in the trust graph (how much
             genuine history backs the target's reputation). This is what the whitepaper
             Def 2.4 specifies with |A_i| where i = target.
         
         Unvouched targets (V_staked = 0) CAN receive positive capacity once they
         have built bilateral S-mass via trial transactions — this is the Rust
         "GRADUATION" behavior (capacity.rs:25) and the Whitepaper Theorem 2.3
         Bootstrap guarantee. No vouching gate.
         """
         # Get subjective trust from observer's perspective
         trust = self.get_subjective_reputation(observer, target)
         
         # Baseline: alpha / |A_observer| (observer's noise floor)
         num_obs_acquaintances = len(self.acquaintances[observer])
         if num_obs_acquaintances == 0:
             num_obs_acquaintances = 1  # Avoid division by zero
         t_baseline = self.params.eigentrust_alpha / num_obs_acquaintances
         
         # Relative reputation
         rel_rep = trust / t_baseline if t_baseline > 0 else 1.0
         
         # Acquaintance saturation uses TARGET's |A_target| per Def 2.4
         num_target_acquaintances = len(self.acquaintances[target])
         saturation = 1.0 - math.exp(-num_target_acquaintances / self.params.acq_saturation)
         
         # Capacity formula — no vouching gate (matches Rust capacity.rs:11-24)
         base = self.staked_capacity[target]
         boost = self.params.capacity_beta * math.log(max(1.0, rel_rep)) * saturation
         
         return base + boost

    def vouch(self, sponsor: int, new_node: int, amount: float = None) -> bool:
        """
        Sponsor locks capacity to vouch for new_node.
        The amount becomes the new_node's base capacity.
        """
        if amount is None:
            amount = self.params.base_capacity

        # Sponsor must have enough capacity (and reputation)
        # For simplicity in simulation: Sponsor just needs positive reputation
        if self.credit_capacity[sponsor] < amount:
            return False

        # Apply vouch
        self.staked_capacity[new_node] += amount
        self.vouchers[new_node][sponsor] = self.vouchers[new_node].get(sponsor, 0.0) + amount
        
        # Recalculate capacity immediately
        self.update_credit_capacity()
        return True

    # =========================================================================
    #  Transaction Protocol (Section 5)
    # =========================================================================

    def compute_risk_score(self, seller: int, buyer: int, amount: float) -> float:
        """
        Risk score R_s(b, delta) per the whitepaper (Definition: Transaction Risk Score):
        
          r_b = t^(s)_b / t_baseline
          rel_trust = r_b / (r_b + K)          (saturating sigmoid)
          R = 1 - rel_trust * (Cap_b - Debt_b) / Cap_b * transfer_factor
        
        The sigmoid normalization ensures the risk score is scale-invariant:
        raw trust t^(s)_b ~ 1/|A_s| is normalized to [0,1) via relative reputation,
        making R independent of network size and acquaintance count.
        
        transfer_factor ∈ [0.5, 1.0] penalizes buyers whose debt is growing faster
        than they are transferring it (buy-but-don't-sell pattern). A node with
        balanced debt flow gets factor=1.0; a pure accumulator gets factor=0.5.
        
        Uses SUBJECTIVE capacity (seller's view of buyer) for protocol accuracy.
        Returns value in [0, 1]. Lower = safer.
        """
        rep = self.get_subjective_reputation(seller, buyer)
        cap = self.get_subjective_capacity(seller, buyer)
        debt = sum(c.amount for c in self.contracts[buyer])
        remaining_ratio = max(0.0, (cap - debt) / cap) if cap > 0 else 0.0
        
        # Normalize trust via saturating sigmoid on relative reputation
        num_acq = len(self.acquaintances[seller])
        t_baseline = self.params.eigentrust_alpha / max(1, num_acq)
        rel_rep = rep / t_baseline if t_baseline > 0 else 0.0
        K = self.params.risk_sigmoid_k
        rel_trust = rel_rep / (rel_rep + K) if (rel_rep + K) > 0 else 0.0
        
        # Debt velocity: penalize buyers accumulating debt without transferring
        acquired = self.debt_acquired_this_epoch[buyer]
        transferred = self.extinguished_this_epoch[buyer]
        efficiency = min(1.0, transferred / acquired) if acquired > 0 else 1.0
        transfer_factor = 0.5 + 0.5 * efficiency  # [0.5, 1.0]
        
        return 1.0 - rel_trust * remaining_ratio * transfer_factor

    def check_bilateral_history(self, seller: int, buyer: int) -> bool:
        """
        Return True if seller has any recorded satisfaction from buyer (S[seller][buyer] > 0).

        Matches Rust's check_bilateral_history: returns True if the seller has ever
        had the buyer as a debtor who successfully transferred debt. Used to select
        PATH 1 (claim-based) vs PATH 2 (full EigenTrust) in propose_transaction.
        """
        return self.S[seller].get(buyer, 0.0) > 0.0

    def compute_risk_from_claim(self, seller: int, buyer: int, amount: float) -> float:
        """
        Claim-based risk score for first-contact transactions (PATH 1).

        Matches Rust's compute_risk_from_claim (trust.rs):
          n_S = number of successful transfers from buyer as debtor observed by seller
          hat_t_claim = n_S / (n_S + K_claim)   (sigmoid over transfer count)
          R = 1 - hat_t_claim * (Cap_b - Debt_b) / Cap_b * transfer_factor

        This is conservatively higher than the full EigenTrust score at n_S = 0 (unknown
        buyer: hat_t = 0, R = 1.0, auto-reject for non-trial). As bilateral history grows,
        the claim sigmoid converges toward the EigenTrust score.

        Uses global capacity (credit_capacity) as a conservative lower bound, matching
        Rust's use of claim.capacity_lower_bound.
        """
        n_s = float(self.successful_transfers_global[buyer])
        K = self.params.k_claim_sigmoid
        hat_t = n_s / (n_s + K) if (n_s + K) > 0 else 0.0

        cap = self.credit_capacity[buyer]
        debt = sum(c.amount for c in self.contracts[buyer])
        remaining_ratio = max(0.0, (cap - debt) / cap) if cap > 0 else 0.0

        # Debt velocity: penalize buyers accumulating debt without transferring
        acquired = self.debt_acquired_this_epoch[buyer]
        transferred = self.extinguished_this_epoch[buyer]
        efficiency = min(1.0, transferred / acquired) if acquired > 0 else 1.0
        transfer_factor = 0.5 + 0.5 * efficiency  # [0.5, 1.0]

        return 1.0 - hat_t * remaining_ratio * transfer_factor

    def check_open_trial_for_buyer(self, buyer: int, seller: int) -> bool:
        """
        Return True if buyer already has an open (Active) trial contract with seller.

        Gate semantics (Gap 2 mitigation, matching Rust check_open_trial_for_buyer):
          - Blocked while a trial DebtContract with creditor==seller exists in buyer's contracts
            (i.e. contract is Active — not yet repaid).
          - Permanently blocked if the pair is in blocked_trial_pairs (expired/defaulted trial).
          - Released ONLY when the trial is fully repaid (Transferred) — the contract is
            removed from self.contracts[buyer] by _transfer_debt.
          - Expired trials do NOT release the slot: they add to blocked_trial_pairs instead.
        """
        # Permanent block: trial expired (defaulted) for this pair
        if (buyer, seller) in self.blocked_trial_pairs:
            return True
        # Active block: live trial contract exists
        for c in self.contracts[buyer]:
            if c.is_trial and c.creditor == seller:
                return True
        return False

    def _is_bootstrap_eligible(self, buyer: int) -> bool:
        """
        Check if a buyer is bootstrap-eligible (PATH 0 candidate).

        Mirrors Rust's is_bootstrap_eligible() in risk.rs:
          A buyer is bootstrap-eligible iff they have no economic footprint:
            - effective_cap == 0  (unvouched or fully slashed), OR
            - n_S_global == 0     (no successful transfers with any seller yet).

        Once a buyer has BOTH cap > 0 AND some transfer history (n_S > 0),
        they are a graduated participant: small transactions go through PATH 1/2.

        O(1) via the _total_s_as_debtor cache (incremented in _transfer_debt).
        """
        if self.credit_capacity[buyer] <= 0.0:
            return True
        return self.successful_transfers_global[buyer] == 0

    def propose_transaction(self, buyer: int, seller: int, amount: float,
                            is_attack: bool = False, force: bool = False) -> Tuple[bool, Optional[str]]:
        """
        Validate and execute a transaction using 3-path architecture
        matching the Holochain production implementation.

        PATH 0 (Trial):       amount < eta * V_base AND buyer is bootstrap-eligible
                              (cap == 0 OR n_S_global == 0) -> accepted subject to velocity
                              limit only. Trial is a bootstrap mechanism: once a buyer has
                              both capacity and transfer history they go through PATH 1/2.
        PATH 1 (First-contact): S[seller][buyer] == 0 -> claim-based O(1) risk score:
                                  hat_t = n_S / (n_S + K_claim), R = 1 - hat_t * headroom.
                                  At n_S=0: R=1.0 (auto-reject for cold-starters).
                                  Special cases:
                                    - Graduated buyer (n_S=0 but active debt): cap to Pending.
                                    - Blind observer (trust=0 from EigenTrust): cap to Pending.
                                  Falls back to full EigenTrust if no fresh claim exists.
        PATH 2 (Repeat):      S[seller][buyer] > 0 -> full subjective EigenTrust risk score.

        Risk thresholds (matching Rust):
          risk < accept_threshold (0.4) -> auto-accept
          risk > reject_threshold (0.8) -> auto-reject
          else -> pending (50% accept probability in simulation)
        """
        if buyer == seller:
            return False, "Self-transaction"

        # === PATH 0: Trial Transaction (bootstrap only) ===
        # Must be evaluated BEFORE the capacity check: the Rust coordinator skips
        # the capacity check for trial transactions (matches mod.rs: `!is_trial`).
        is_trial_amount = amount < (self.params.trial_fraction * self.params.base_capacity)
        is_bootstrap = self._is_bootstrap_eligible(buyer)

        if is_trial_amount and is_bootstrap and not force:
            # Global trial debt cap: total outstanding trial debt per buyer cannot
            # exceed eta * V_base. An attacker who fans out across many sellers still
            # accumulates at most one trial's worth of debt, bounding flash loan
            # extraction to ~threshold regardless of network size.
            # Honest nodes are unaffected: their trial debt transfers each epoch.
            trial_cap = self.params.trial_fraction * self.params.base_capacity
            outstanding_trial_debt = self.total_trial_debt[buyer]
            if outstanding_trial_debt + amount > trial_cap:
                self.current_rejected.append((seller, buyer, "TrialDebtCap", is_attack))
                return False, "Trial Debt Cap Exceeded"

            # Open-trial gate: one trial per (buyer, seller) pair at a time.
            # Blocked while Active; released only on Transferred (full repayment).
            # Expired trials are permanently blocked (Sybil penalty for defaults).
            if self.check_open_trial_for_buyer(buyer, seller):
                self.current_rejected.append((seller, buyer, "OpenTrialExists", is_attack))
                return False, "Open Trial Exists (EC200019)"
            if self.trial_tx_count[seller] >= self.params.trial_velocity_limit:
                self.current_rejected.append((seller, buyer, "TrialVelocity", is_attack))
                return False, "Trial Velocity Limit Exceeded"
            self.trial_tx_count[seller] += 1
            # Trial transactions accepted without risk assessment (bootstrapping)
            self._apply_transaction(buyer, seller, amount, is_attack)
            return True, None

        # Capacity check for non-trial (or graduated) buyers.
        # Trial transactions bypass the capacity check (matching Rust mod.rs `!is_trial`).
        current_debt = self.total_debt[buyer]
        buyer_capacity = self.credit_capacity[buyer]
        if not force and current_debt + amount > buyer_capacity:
            self.current_rejected.append((seller, buyer, "SelfCap", is_attack))
            return False, "Capacity Exceeded"

        # === Non-trial risk assessment (PATH 1 / PATH 2) ===
        if not force:
            # === PATH 1 vs PATH 2: bilateral history check ===
            # Matches Rust's check_bilateral_history + compute_risk_from_claim.
            if self.check_bilateral_history(seller, buyer):
                # PATH 2: Full subjective EigenTrust (subgraph bounded)
                risk = self.compute_risk_score(seller, buyer, amount)
                
                # PATH 2 fallback: if EigenTrust result was 0.0 (e.g. evicted), 
                # still apply Blind Observer fallback to allow handshaking.
                # get_subjective_reputation is O(1) here as it's cached from compute_risk_score.
                if risk > self.params.default_reject_threshold:
                    if self.get_subjective_reputation(seller, buyer) <= 0.0:
                        risk = self.params.default_reject_threshold - 1e-4
            else:
                # PATH 1: Claim-based O(1) risk (no subgraph traversal needed)
                risk = self.compute_risk_from_claim(seller, buyer, amount)

                # PATH 1 special case: graduated buyer with n_S=0 and active debt.
                if risk >= 1.0 and current_debt > 0.0:
                    risk = self.params.default_reject_threshold - 1e-9

                # Lazy Blind Observer Fallback (Definition 16 / risk.rs line 94)
                # Only check if we are currently about to reject a PATH 1 transaction.
                # This makes the "Blind Observer" check O(1) for most transactions.
                if risk > self.params.default_reject_threshold:
                    if self.get_subjective_reputation(seller, buyer) <= 0.0:
                        risk = self.params.default_reject_threshold - 1e-4

            # Threshold decision (matches Rust from_risk_score_for_wallet)
            if risk < self.params.default_accept_threshold:
                pass  # Accept — fall through to _apply_transaction
            elif risk > self.params.default_reject_threshold:
                self.current_rejected.append((seller, buyer, "Risk", is_attack))
                return False, f"Risk Rejected (risk={risk:.4f})"
            else:
                # Pending zone: 50% accept probability (no manual moderation in sim)
                if self.rng.random() > 0.5:
                    self.current_rejected.append((seller, buyer, "Risk", is_attack))
                    return False, f"Risk Pending->Rejected (risk={risk:.4f})"

        self._apply_transaction(buyer, seller, amount, is_attack)
        return True, None

    def _transfer_debt(self, target: int, amount: float, nodes_to_cap: set) -> float:
        """
        Attempt to transfer up to `amount` of debt from `target`'s active debt pool.
        Returns the actual amount transferred.
        """
        strategic = self.suite_state.get('strategic_defaulters', {})
        target_pay_set = strategic.get(target, None)
        
        transferred = 0.0
        i = 0
        while i < len(self.contracts[target]):
            if amount <= 0.01:
                break
                
            c = self.contracts[target][i]
            
            # Strategic defaults might only pay specific creditors
            if target_pay_set is not None and c.creditor not in target_pay_set:
                i += 1
                continue
            
            transfer = min(c.amount, amount)
            if transfer > 0:
                # --- Mitigation: Time-Weighted Volume Limit ---
                current_epoch_vol = self.epoch_volume_added[c.creditor].get(target, 0.0)
                allowed = max(0.0, self.params.max_volume_per_epoch - current_epoch_vol)
                effective_s_increment = min(transfer, allowed)
                
                if effective_s_increment > 0.01:
                    # Creditor records S for debtor (standard direction).
                    # 
                    # Note on bilateral S: the Rust production code (sf_counters.rs:267-298)
                    # also records satisfaction in the DEBTOR→creditor direction on
                    # successful transfers ("Repayment Satisfaction"). We do NOT
                    # mirror that here because:
                    #   (1) The simulation processes partial transfers per-tick rather
                    #       than per-contract-completion, so a single Rust "add_satisfaction"
                    #       event maps to many per-tick increments, over-amplifying S.
                    #   (2) The simulation's transaction generator (virtuous.step) sizes
                    #       tx amounts proportionally to current capacity. Combined with
                    #       (1), bidirectional S drives a positive feedback loop in which
                    #       capacity inflates, then tx sizes inflate, then cascades
                    #       under-absorb, then genesis debt explodes — driving
                    #       steady-state genesis ratio from ~9% to ~31% (a 5x simulation
                    #       artifact not present in the Rust production path).
                    # Cold start (pure-trial bootstrap) is still achievable via the
                    # GRADUATION mechanism (removed vouching gate in capacity.py) —
                    # unvouched agents earn reputation through creditor-side S alone,
                    # as verified by the `cold_start` suite (~80% graduation rate).
                    self.S[c.creditor][target] = self.S[c.creditor].get(target, 0.0) + effective_s_increment
                    self.epoch_volume_added[c.creditor][target] = current_epoch_vol + effective_s_increment
                    self._dirty_nodes_trust.add(c.creditor)
                    self.tx_count[c.creditor][target] = self.tx_count[c.creditor].get(target, 0) + 1
                    self._S_epoch_delta[c.creditor][target] = self._S_epoch_delta[c.creditor].get(target, 0.0) + effective_s_increment
                    self._total_s_as_debtor[target] += effective_s_increment

                # --- ACQUAINTANCE CREATION ON DEBT TRANSFER ---
                # Acquaintances are added on any successful transfer, not gated behind
                # the volume cap. The S volume cap limits trust accumulation, not
                # evidence of economic interaction. Mutual and symmetric.
                self.acquaintances[c.creditor].add(target)
                self.acquaintances[target].add(c.creditor)
                self._known_by[target].add(c.creditor)
                self._known_by[c.creditor].add(target)
                nodes_to_cap.add(c.creditor)
                nodes_to_cap.add(target)

                # Standard accounting
                self.extinguished_this_epoch[target] += transfer
                self.total_debt[target] -= transfer
                if c.is_trial:
                    self.total_trial_debt[target] -= transfer
                
                c.amount -= transfer
                amount -= transfer
                transferred += transfer
                self.debt_transferred_this_epoch += transfer
                
                # Remove depleted contract
                if c.amount <= 0.01:
                    if c.is_trial:
                        # Success on trial = graduated (is_bootstrap_eligible will now be False)
                        pass 
                    self.successful_transfers_global[target] += 1
                    self.contracts[target].pop(i)
                    continue  # Don't increment i
            i += 1
            
        return transferred

    def _drain_debt(self, agent: int, amount: float, visited: set, nodes_to_cap: set,
                    co_signers: dict, supporter: int = -1) -> float:
        """
        Recursive Waterfill Cascade (Whitepaper Section 5.2).

        1. Drain own debt horizontally
        2. If short, allocate to supporters and recursively call this
        3. If a supporter is dry, spillover their quota horizontally to remaining supporters

        The cascade is unconditional — beneficiaries have already consented by being
        in the support breakdown. Risk assessment belongs in propose_transaction, not
        here. Gating cascades on risk scores kills debt transfer for honest nodes that
        have thin bilateral history early in the simulation, flooding the system with
        genesis debt and preventing trust (S) from accumulating.
        """
        if amount <= 0.01 or agent in visited:
            return 0.0

        visited.add(agent)

        # 1. Primary Extinguishment: Drain own debt pool
        transferred = self._transfer_debt(agent, amount, nodes_to_cap)
        if transferred > 0:
            co_signers[agent] = co_signers.get(agent, 0.0) + transferred
            

        remaining = amount - transferred
        if remaining <= 0.01:
            return transferred

        # 2. Recursive Propagation & Horizontal Spillover setup
        breakdown = self.support_breakdown[agent]
        if not breakdown:
            return transferred

        active_supporters = {target: coef for target, coef in breakdown.items()
                           if target != agent and coef > 0 and target not in visited}

        # 3. Waterfilling Loop (Spillover)
        max_iters = 100
        iters = 0
        while remaining > 0.01 and sum(active_supporters.values()) > 0:
            iters += 1
            if iters > max_iters:
                break

            total_coef = sum(active_supporters.values())
            pass_transferred = 0.0

            # Iterate statically so we can remove dry nodes from active_supporters
            for target, coef in list(active_supporters.items()):
                target_amount = remaining * (coef / total_coef)
                if target_amount <= 0.01:
                    continue

                # Deep Recursion Call (unconditional — no risk gating)
                drained = self._drain_debt(target, target_amount, visited.copy(), nodes_to_cap, co_signers, supporter=agent)
                pass_transferred += drained

                # Spillover check: target didn't absorb its full quota
                if drained < (target_amount - 0.01):
                    del active_supporters[target]

            remaining -= pass_transferred
            transferred += pass_transferred

            # Everyone in the active list is dry/cyclic; exit to prevent infinite loop
            if pass_transferred <= 0.01:
                break

        return transferred

    def _apply_transaction(self, buyer: int, seller: int, amount: float,
                           is_attack: bool = False):
        """
        Execute a transaction with debt transfer semantics (whitepaper Section 5).
        
        When buyer b purchases from seller s for amount δ:
        1. Try to clear debt from seller -> triggers recursive Support Cascade
        2. Buyer gets ONE contract (δ, M, epoch, creditor=seller)
        
        The buyer always owes the seller (not the original creditor).
        S increments track the target's fulfillment of their obligations.
        
        OPTIMIZATION: For trial transactions (amount < V_base * eta), skip the
        full support cascade and use seller-only drain. This avoids cascade complexity
        for small transactions.
        """
        # Contracts always use M = M_min (dynamic maturity removed — Change 4)
        maturity = self.params.min_maturity
        remaining = amount

        # Track total transaction volume for genesis ratio computation
        self.total_tx_volume_this_epoch += amount
        
        co_signers = {}
        nodes_to_cap = set()
        
        # --- OPTIMIZATION: Skip support cascade for trial transactions ---
        trial_threshold = self.params.base_capacity * self.params.trial_fraction
        is_trial = amount < trial_threshold
        
        if is_trial:
            # Simplified path: only drain from seller's own contracts
            seller_transferred = self._transfer_debt(seller, amount, nodes_to_cap)
            if seller_transferred > 0:
                co_signers[seller] = seller_transferred
            remaining -= seller_transferred
        else:
            # --- Deep Recursive Waterfilling Cascade ---
            total_drained = self._drain_debt(seller, amount, set(), nodes_to_cap, co_signers)
            remaining -= total_drained
        
        # --- Phase 3: Buyer contracts debt with seller as creditor ---
        # Per whitepaper: "b' contracts debt min(r, Debt(b)) with b as creditor"
        # plus genesis for the remainder. In practice, the buyer gets one contract
        # for the full amount with the seller as creditor.
        contract = DebtContract(amount, maturity, self.epoch, creditor=seller,
                                co_signers=co_signers, is_trial=is_trial)
        self.contracts[buyer].append(contract)
        # Phase 14.8: Expiry Queue optimization
        self._expiry_queue[self.epoch + maturity].append((buyer, contract))
        self.debt_acquired_this_epoch[buyer] += amount
        self.total_debt[buyer] += amount
        if is_trial:
            self.total_trial_debt[buyer] += amount
        self.current_tx.append((seller, buyer, amount, is_attack, remaining > 0))

        # Track genesis debt: the portion of the transaction amount that was NOT
        # covered by existing debt transfer (own + cascade).
        if remaining > 0.01:
            self.genesis_debt_this_epoch += remaining
        
        # Batch acquaintance capping (for nodes that gained acquaintances during debt transfer)
        for node in nodes_to_cap:
            self._cap_acquaintances(node)

    # =========================================================================
    #  Epoch Tick
    def tick(self):
        """
        Advance one epoch:
        1. Randomize support breakdowns for organic alliances
        2. Process contract expirations (maturity-based failures)
        3. Update acquaintance sets
        4. Recompute local trust, global trust, credit capacity
        """
        self.epoch += 1
        self._tick_subjective_cache = {}  # Clear per-tick cache
        # Archive current epoch transactions
        self.last_epoch_tx = list(self.current_tx)
        self.current_tx = []
        self.last_epoch_rejected = list(self.current_rejected)
        self.current_rejected = []

        # Periodically shift support alliances
        self.randomize_support(probability=self.params.support_shift_prob)

        # Invalidate caches
        self._cached_C = None
        self._cached_MT = None
        self._reputation_cache = {}
        self._subjective_cache = {}

        # Reset per-epoch counters
        self.extinguished_this_epoch = [0.0] * self.size
        self.debt_acquired_this_epoch = [0.0] * self.size
        self.trial_tx_count = [0] * self.size
        self.epoch_volume_added = [{} for _ in range(self.size)]
        self.genesis_debt_this_epoch = 0.0
        self.debt_transferred_this_epoch = 0.0
        self.debt_expired_this_epoch = 0.0
        self.total_tx_volume_this_epoch = 0.0
        
        t_init = time.perf_counter()

        # --- 1. Process Contract Expirations (Phase 14.8: Expiry Queue) ---
        failed_debtors = set()
        expiring = self._expiry_queue.pop(self.epoch, [])
        for debtor_id, c in expiring:
            # Check if contract was already repaid/drained
            if c.amount > 0.01:
                # 1a. Standard Default
                failed_debtors.add(debtor_id)
                self.F[c.creditor][debtor_id] = self.F[c.creditor].get(debtor_id, 0.0) + c.amount
                self._F_epoch_delta[c.creditor][debtor_id] = self._F_epoch_delta[c.creditor].get(debtor_id, 0.0) + c.amount
                self._dirty_nodes_trust.add(c.creditor)
                
                # Track debt destruction from expiration
                self.debt_expired_this_epoch += c.amount
                self.total_debt[debtor_id] -= c.amount
                if c.is_trial:
                    self.total_trial_debt[debtor_id] -= c.amount
                    self.blocked_trial_pairs.add((debtor_id, c.creditor))
                
                # 1b. Vouch Contagion & Slashing (when use_vouching=True)
                if self.params.use_vouching and len(self.vouchers[debtor_id]) > 0:
                    # Fix: Apportion slash and contagion among multiple sponsors by their vouch weight
                    total_vouched = sum(v for v in self.vouchers[debtor_id].values() if v > 0)
                    if total_vouched > 0:
                        for sponsor, vouched_amt in self.vouchers[debtor_id].items():
                            if vouched_amt > 0:
                                weight = vouched_amt / total_vouched
                                slash = (c.amount * weight) * self.params.vouch_slashing_multiplier
                                self.staked_capacity[sponsor] = max(0.0, self.staked_capacity[sponsor] - slash)
                                # Contagion: creditors count sponsor as partially responsible for default
                                failed_debtors.add(sponsor)
                                self.F[c.creditor][sponsor] = self.F[c.creditor].get(sponsor, 0.0) + (c.amount * weight)
                                self._dirty_nodes_trust.add(sponsor)

                    new_staked = sum(
                        min(vouched_amt, float(self.staked_capacity[s]))
                        for s, vouched_amt in self.vouchers[debtor_id].items()
                    )
                    self.staked_capacity[debtor_id] = new_staked
                    self._dirty_nodes_trust.add(debtor_id)

                # 1c. Support Escrow Contagion (Co-Signers)
                total_coef = sum(coef for cosigner, coef in c.co_signers.items()
                                 if cosigner != debtor_id and cosigner != c.creditor)
                if total_coef > 0.0:
                    for cosigner, coef in c.co_signers.items():
                        if cosigner != debtor_id and cosigner != c.creditor:
                            penalty = c.amount * (coef / total_coef)
                            if penalty > 0.01:
                                failed_debtors.add(cosigner)
                                self.F[c.creditor][cosigner] = self.F[c.creditor].get(cosigner, 0.0) + penalty
                                self._dirty_nodes_trust.add(cosigner)
                        
                # 1d. Acquaintance pruning + failure observation publishing (Contagion)
                self.failure_observations[debtor_id][c.creditor] = self.epoch
                if self.F[c.creditor].get(debtor_id, 0.0) > self.S[c.creditor].get(debtor_id, 0.0):
                    if debtor_id in self.acquaintances[c.creditor]:
                        self.acquaintances[c.creditor].remove(debtor_id)
                        self._known_by[debtor_id].discard(c.creditor)
                
                # Finally remove from main contract list
                if c in self.contracts[debtor_id]:
                    self.contracts[debtor_id].remove(c)
        
        t_exp = time.perf_counter()
        
        t_win_init = time.perf_counter()
        # --- Flush epoch deltas into rolling windows (OPTIMIZED: Lazy/Active nodes only) ---
        import collections as _col
        k = self.params.recent_window_k
        
        active_sources = set()
        for i, s_delta in enumerate(self._S_epoch_delta):
            if s_delta: active_sources.add(i)
        for i, f_delta in enumerate(self._F_epoch_delta):
            if f_delta: active_sources.add(i)

        for i in active_sources:
            # Mark the source as dirty so its local trust row is recomputed
            self._dirty_nodes_trust.add(i)
            
            s_delta = self._S_epoch_delta[i]
            f_delta = self._F_epoch_delta[i]
            
            # Combine all targets that had S or F activity
            targets = set(s_delta.keys()) | set(f_delta.keys())
            
            for j in targets:
                # 1. Catch up to last epoch (self.epoch - 1) before adding current delta
                self._catch_up_window(i, j, to_epoch=self.epoch - 1)
                
                # 2. Append current deltas
                if j not in self.S_window[i]:
                    self.S_window[i][j] = _col.deque(maxlen=k)
                    self.F_window[i][j] = _col.deque(maxlen=k)
                    self._s_window_sums[i][j] = 0.0
                    self._f_window_sums[i][j] = 0.0
                
                # Robust sum initialization if missing
                if j not in self._s_window_sums[i]:
                    self._s_window_sums[i][j] = 0.0
                if j not in self._f_window_sums[i]:
                    self._f_window_sums[i][j] = 0.0

                # Update sums (handle deque overflow)
                s_new = s_delta.get(j, 0.0)
                f_new = f_delta.get(j, 0.0)
                
                win_s = self.S_window[i][j]
                win_f = self.F_window[i][j]

                if len(win_s) == k:
                    self._s_window_sums[i][j] -= win_s[0]
                    self._f_window_sums[i][j] -= win_f[0]
                
                win_s.append(s_new)
                win_f.append(f_new)
                
                # Update back to DiskDict if needed (some proxies might need explicit set for deque mutations)
                self.S_window[i][j] = win_s
                self.F_window[i][j] = win_f

                self._s_window_sums[i][j] += s_new
                self._f_window_sums[i][j] += f_new
                
                # Also ensure _last_window_update is initialized
                if i >= len(self._last_window_update):
                    # This should not happen if add_nodes is correct but let's be safe
                    pass
                else:
                    self._last_window_update[i][j] = self.epoch

        # Reset epoch deltas
        self._S_epoch_delta = [{} for _ in range(len(self._S_epoch_delta))]
        self._F_epoch_delta = [{} for _ in range(len(self._F_epoch_delta))]
        
        # --- 1e. Propagate Contagion: Selective Dirtying (Phase 14.7) ---
        # Only dirty nodes that have bilateral history (S or F) with the
        # failed debtor.  Nodes that merely "know of" j via acquaintance
        # but have no S[i][j] or F[i][j] produce attenuated_score=0 for j,
        # so their local trust row is unaffected by j's failure.
        for j in failed_debtors:
            for i in self._known_by[j]:
                if self.S[i].get(j, 0) > 0 or self.F[i].get(j, 0) > 0:
                    self._dirty_nodes_trust.add(i)

        # --- 2. Prune acquaintance sets (probabilistic to reduce overhead) ---
        # In reality, nodes don't reorganize their social graph every epoch.
        # Pruning with probability < 1 models realistic beneficiary changes
        # while reducing O(N × acquaintances) overhead per tick.
        prune_prob = self.params.acquaintance_prune_prob
        for i in range(self.size):
            if self.rng.random() >= prune_prob:
                continue  # Skip pruning this node this epoch
            evidenced = {j for j in self.acquaintances[i] if j == i or (self.S[i].get(j, 0) > 0 or self.F[i].get(j, 0) > 0 or
                         self.S[j].get(i, 0) > 0 or self.F[j].get(i, 0) > 0)}
            
            if len(evidenced) > 1:
                # Phase 14.7: Update inverse graph
                dropped = self.acquaintances[i] - evidenced
                for j in dropped:
                    self._known_by[j].discard(i)
                self.acquaintances[i] = evidenced


        t_win = time.perf_counter()

        # --- 3. Recompute trust and capacity ---
        self._rebuild_local_trust()
        
        t_rebuild = time.perf_counter()

        # Telemetry trust/capacity is only for plotting/diagnostics.
        # Throttled to reduce O(N^2) overhead in the simulation loop.
        if self.epoch % 5 == 0:
            self.run_eigentrust()
            self.update_credit_capacity()
        
        # Reset Subjective trust matrix to trigger fresh bulk solve on next TX
        self._subjective_row_cache = {}
        
        t_alg = time.perf_counter()
        t_end = time.perf_counter()
        if self.epoch % 25 == 0:
            print(f"  [PROF] Tick {self.epoch}: total={t_end-t_init:.4f}s, exp={t_exp-t_init:.4f}s, reb={t_rebuild-t_win:.4f}s, alg={t_alg-t_rebuild:.4f}s")

    # =========================================================================
    #  Support Breakdown Management
    # =========================================================================

    def randomize_support(self, probability: Optional[float] = None):
        """
        Periodically randomize support breakdowns for nodes to model shifting
        social and economic alliances with high variability.

        Respects append-only semantics: existing beneficiaries are retained with
        coefficient 0 if not selected, rather than removed from the breakdown.
        """
        if probability is None:
            probability = self.params.support_shift_prob

        for i in range(self.size):
            if self.rng.random() < probability:
                # Highly variable self-support using parametrized range
                self_coef = self.rng.uniform(self.params.min_self_support, 1.0)
                
                # Pick 0 to N% of network size as beneficiaries (nodes whose debt will be drained)
                # Apply hard cap (max_beneficiaries_cap) to prevent O(n²) cascade growth
                discovery = list(self.acquaintances[i])
                if i in discovery:
                    discovery.remove(i)
                
                hard_cap = max(3, int(self.size * self.params.max_beneficiaries_cap))
                max_possible = min(len(discovery), hard_cap, max(3, int(self.size * self.params.max_beneficiary_fraction)))
                num_beneficiaries = self.rng.randint(0, max_possible)
                
                if num_beneficiaries > 0:
                    beneficiaries = self.rng.sample(discovery, num_beneficiaries)
                    rem_fraction = 1.0 - self_coef
                    breakdown = {i: self_coef}
                    
                    if num_beneficiaries == 1:
                        breakdown[beneficiaries[0]] = rem_fraction
                    else:
                        # Random split using Dirichlet-like normalization
                        shares = [self.rng.random() for _ in range(num_beneficiaries)]
                        total_shares = sum(shares)
                        for b_idx, b_id in enumerate(beneficiaries):
                            breakdown[b_id] = rem_fraction * (shares[b_idx] / total_shares)
                else:
                    breakdown = {i: 1.0}

                # Append-only: include all existing beneficiaries, zeroing any not selected
                existing = self.support_breakdown[i]
                for existing_beneficiary in existing:
                    if existing_beneficiary not in breakdown:
                        breakdown[existing_beneficiary] = 0.0

                self.set_support_breakdown(i, breakdown)

    def set_support_breakdown(self, node: int, beneficiaries: Dict[int, float]):
        """
        Set the support breakdown for a node (the supporter).

        The `beneficiaries` dict maps each beneficiary node ID to the fraction of the
        cascade drawn from them when `node` sells. The supporter sets this unilaterally.

        Enforces append-only semantics matching Holochain's on-chain integrity constraint:
        existing beneficiaries (non-zero coefficient) cannot be removed from the breakdown.
        They may be zeroed (coefficient = 0) but not deleted. New beneficiaries can be added.
        Coefficients can be freely redistributed.
        """
        if beneficiaries and node not in beneficiaries:
            raise ValueError(f"Node {node} must be in its own support breakdown (self-support)")
        
        total = sum(beneficiaries.values())
        if total > 1.0 + 1e-9:
            raise ValueError(f"Support coefficients sum to {total} > 1.0")
        
        for b, coef in beneficiaries.items():
            if not (0.0 <= coef <= 1.0):
                raise ValueError(f"Coefficient {coef} not in [0, 1]")
        
        # Append-only: existing beneficiaries in the current breakdown cannot be dropped.
        # They must remain (possibly with coefficient 0). New entries can be freely added.
        current = self.support_breakdown[node]
        for existing_beneficiary in current:
            if existing_beneficiary not in beneficiaries:
                raise ValueError(
                    f"Cannot remove beneficiary {existing_beneficiary} from support breakdown "
                    f"(append-only: existing beneficiaries may only be zeroed, not removed)"
                )
        
        # Fix: Overwriting the proxy with a dict breaks disk persistence
        proxy = self.support_breakdown[node]
        if hasattr(proxy, 'disk_dict'):
            proxy.disk_dict.replace_node_dict(node, beneficiaries)
        else:
            self.support_breakdown[node] = dict(beneficiaries)

    def get_total_debt(self, node: int) -> float:
        """Get total outstanding debt for a node."""
        return sum(c.amount for c in self.contracts[node])

    def get_debt_by_creditor(self, node: int) -> Dict[int, float]:
        """Get debt breakdown by creditor for a node."""
        result: Dict[int, float] = {}
        for c in self.contracts[node]:
            result[c.creditor] = result.get(c.creditor, 0.0) + c.amount
        return result

    def get_total_system_debt(self) -> float:
        """Sum of all active contract amounts across all agents."""
        return sum(c.amount for i in range(self.size) for c in self.contracts[i])
