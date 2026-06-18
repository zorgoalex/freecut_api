# Research Documentation Workflow

This folder stores versioned research documentation for Freecut cutting optimization.

## Files

- `cutting-optimization-research-log.md` is the canonical research log. It should contain consolidated findings, benchmark results, decisions, and current next steps.
- `drafts/` is for branch-local or hypothesis-local working notes. Use it for intermediate observations before the result is ready for the canonical log.

## Workflow

1. Create a separate branch for each hypothesis or research direction.
2. Write intermediate notes in `docs/research/drafts/<branch-or-hypothesis>.md`.
3. Store large benchmark artifacts, SVGs, PNGs, generated inputs, and raw outputs under `ai_docs/tmp/`, not in Git.
4. When a hypothesis is complete, move only the final compressed conclusions into `cutting-optimization-research-log.md`.
5. Before merging a branch, either delete its draft or keep it only if the draft has durable value as supporting documentation.

The canonical log should stay compact enough to be useful as working context. Prefer links/paths to artifacts over copying raw benchmark output.
