# AI Agents Workflow (Freecut)

## Goal
Speed up implementation of the quick optimization zone by running parallel subagents for isolated tasks, then merging via one orchestrator. Keep final review quality high with the strongest model.

## Model Policy
- `Subagents`: use a small model (`$SUBAGENT_MODEL`) for draft solutions, tests, and focused code edits.
- `Critical Review`: always use `gpt-5` (`$REVIEW_MODEL`) before merge/apply.

## Task Split for Current Plan
- `Agent A`: Stage 1 (optimize concurrency limit + overload behavior + tests).
- `Agent B`: Stage 2 (time-budget loop + timeout cleanup + telemetry fields).
- `Agent C`: Stage 5 (stock identity collision fix + qty_limit correctness + tests).
- `Orchestrator`: compares outputs, applies best parts, resolves conflicts, runs full validation.

## Branch/Worktree Strategy
- Create separate worktrees to avoid file conflicts.
- Example:
```bash
git worktree add ../freecut_a -b feat/stage1-concurrency
git worktree add ../freecut_b -b feat/stage2-budget
git worktree add ../freecut_c -b feat/stage5-stock-identity
```

## Parallel Subagent Runs
Use the orchestration script:

```bash
chmod +x scripts/multi_agent_workflow.sh

# run subagents (Stage 1/2/5) in parallel
SUBAGENT_MODEL=o4-mini ./scripts/multi_agent_workflow.sh run

# after merge, run critical review on strongest model
REVIEW_MODEL=gpt-5 ./scripts/multi_agent_workflow.sh review
```

Manual equivalent (if needed):

```bash
export SUBAGENT_MODEL="${SUBAGENT_MODEL:-o4-mini}"

codex exec -m "$SUBAGENT_MODEL" "Implement Stage 1 from ai_docs/Quick_Refact_Freecut.md. Add tests. Run cargo test." > /tmp/agent_a.log &
codex exec -m "$SUBAGENT_MODEL" "Implement Stage 2 from ai_docs/Quick_Refact_Freecut.md. Add tests. Run cargo test." > /tmp/agent_b.log &
codex exec -m "$SUBAGENT_MODEL" "Implement Stage 5 from ai_docs/Quick_Refact_Freecut.md. Add tests for stock identity collision." > /tmp/agent_c.log &

wait
```

## Integration Flow (Orchestrator)
1. Review each branch diff and test output.
2. Cherry-pick or manually merge best changes into main working branch.
3. Run mandatory checks:
```bash
cargo test
```
4. Run API smoke checks after all stages are merged (not after every micro-step):
```bash
./scripts/docker_smoke.sh
```
5. For Docker-service API validation, use `curlimages` requests (external-like path). For mass load checks, use Python scripts.

## Critical Review Gate (Mandatory)
Before final commit:

```bash
export REVIEW_MODEL="${REVIEW_MODEL:-gpt-5}"
codex exec review --uncommitted -m "$REVIEW_MODEL" \
  "Critical review: find regressions in optimizer behavior, timeout handling, stock identity mapping, and test coverage."
```

## Done Criteria
- Stage checkboxes updated in `ai_docs/Quick_Refact_Freecut.md`.
- `cargo test` is green after each completed stage merge.
- Final critical review has no unresolved high-severity findings.
