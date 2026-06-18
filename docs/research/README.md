# Research Documentation Workflow

This folder stores versioned research documentation for Freecut cutting optimization.

## Files

- `cutting-optimization-research-log.md` is the index for the bilingual research log.
- `cutting-optimization-research-log.ru.md` is the Russian research log.
- `cutting-optimization-research-log.en.md` is the English research log.
- `drafts/` is for branch-local or hypothesis-local working notes. Use it for intermediate observations before the result is ready for the canonical log.

## Workflow

1. Create a separate branch for each hypothesis or research direction.
2. Write intermediate notes in `docs/research/drafts/<branch-or-hypothesis>.md`.
3. Store large benchmark artifacts, SVGs, PNGs, generated inputs, and raw outputs under `ai_docs/tmp/`, not in Git.
4. When a hypothesis is complete, move only the final compressed conclusions into both `cutting-optimization-research-log.ru.md` and `cutting-optimization-research-log.en.md`.
5. Before merging a branch, either delete its draft or keep it only if the draft has durable value as supporting documentation.
6. Run `python scripts/check_research_log_sync.py` before committing research-log changes.

The canonical logs should stay compact enough to be useful as working context. Prefer links/paths to artifacts over copying raw benchmark output. The Russian and English logs must keep the same `research-log-sync-index`.
