"""V21 research: broad candidate grid for the remaining V20 seed-4 outlier."""

import os

WORKTREE_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
ARTIFACT_ROOT = os.environ.get(
    "FREECUT_ARTIFACT_ROOT",
    os.path.abspath(os.path.join(WORKTREE_ROOT, "..", "..", "..")),
)

os.environ.setdefault("FREECUT_SEED_LIST", "4")
os.environ.setdefault("FREECUT_PROFILE_POOL", "0.0,0.1,0.2,0.3,0.4,0.5,0.8")
os.environ.setdefault("FREECUT_SEED_OFFSETS", "0,1000003,2000006,3000009,4000012")
os.environ.setdefault(
    "FREECUT_OUT_DIR",
    os.path.join(ARTIFACT_ROOT, "best_layouts_v21_seed4_candidate_grid"),
)

from test_v19_outlier_candidate_grid import main


if __name__ == "__main__":
    main()
