import os
import pickle
from sim.config import RESULTS_DIR

def _checkpoint_is_complete(checkpoint_dir, expected_size=None):
    """Return True if checkpoint_dir contains a usable, complete checkpoint.
    
    Checks for:
    1. Presence and non-zero size of metadata.pkl.
    2. Network size consistency (if expected_size is provided).
    3. Presence of universe_state.db if use_disk was enabled.
    """
    meta_path = os.path.join(checkpoint_dir, "metadata.pkl")
    if not os.path.exists(meta_path) or os.path.getsize(meta_path) == 0:
        return False
    try:
        with open(meta_path, "rb") as f:
            meta = pickle.load(f)
    except Exception:
        return False
    
    # Check size consistency to prevent loading mismatched states
    if expected_size is not None and meta.get("size") != expected_size:
        print(
            f"  [INFO] Ignoring checkpoint '{checkpoint_dir}': "
            f"size mismatch (found {meta.get('size')}, expected {expected_size})."
        )
        return False

    if meta.get("use_disk", False):
        db_path = os.path.join(checkpoint_dir, "universe_state.db")
        if not os.path.exists(db_path):
            print(
                f"  [WARN] Skipping incomplete checkpoint '{checkpoint_dir}': "
                f"metadata.pkl is present but universe_state.db is missing "
                f"(process was interrupted mid-save).  Starting from scratch."
            )
            return False
    return True


def resolve_load_path(load_path, task_id=None, finished_only=False, expected_size=None):
    """Helper to find the correct checkpoint directory for a specific task.
    If load_path is a session directory, looks for a suite-specific checkpoint subdirectory.
    Also handles relative paths by checking inside the results directory.

    Args:
        load_path:     Session directory or direct checkpoint path.
        task_id:       Suite-specific task key (e.g. "run1_spam").
        finished_only: When True only consider _finished checkpoints.
        expected_size: If provided, only accept checkpoints with this network size.
    """
    if not load_path:
        return None
    
    # 1. Handle relative paths by checking RESULTS_DIR
    p = os.path.abspath(load_path)
    if not os.path.exists(p):
        p_alt = os.path.join(RESULTS_DIR, load_path)
        if os.path.exists(p_alt):
            p = p_alt
    
    # 2. If it is already a direct checkpoint directory (contains metadata.pkl)
    meta_path = os.path.join(p, "metadata.pkl")
    if os.path.exists(meta_path) and os.path.getsize(meta_path) > 0:
        if not _checkpoint_is_complete(p, expected_size=expected_size):
            return None
        # If finished_only, only accept it when the directory name ends with _finished
        if finished_only and not p.endswith("_finished"):
            return None
        return p
    
    # 3. If it is a session directory, look for a suite-specific checkpoint
    if task_id:
        suffixes = ["finished"] if finished_only else ["interrupted", "finished"]
        for suffix in suffixes:
            checkpoint_name = f"checkpoint_{task_id}_{suffix}"
            subdir = os.path.join(p, checkpoint_name)
            sub_meta = os.path.join(subdir, "metadata.pkl")
            if os.path.exists(sub_meta) and os.path.getsize(sub_meta) > 0:
                if _checkpoint_is_complete(subdir, expected_size=expected_size):
                    return subdir
                # else: incomplete — keep looking (or fall through to None)
        
        # If task_id was provided but no valid checkpoint found, don't return the session dir
        # as a state path, because that would cause Universe.load_state to fail.
        return None
            
    if p and not os.path.exists(p):
        return None
            
    return p
