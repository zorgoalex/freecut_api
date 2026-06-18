# V59/V61 productionization B: async-safe + bounded post-process

Branch: `feat/freecut-async-postprocess` (from `main` @ `2c86de6`).
Date: 2026-06-18.

## Goal

The V59 consolidation and V61 anytime-LNS post-processors ran synchronously
inside the async `run_restarts_with_budget` (`engine=heuristic` branch), unlike
the GA restarts which already use `tokio::task::spawn_blocking`. Both are
CPU-bound and `lns` runs to the `time_limit_ms` deadline, so a deep request
blocked a tokio worker thread for seconds. Under concurrent deep load this
starves the runtime — health endpoints and other requests stall. Goal: move the
post-process onto the blocking pool and confirm/limit deep-request concurrency.

## Setup

- Wrap the consolidate+lns post-process in `tokio::task::spawn_blocking`
  (`src/optimizer.rs`, heuristic branch). `PreparedInput` is `Clone`; owned
  copies + configs + seeds move into the closure, which returns
  `(Candidate, Option<ConsolidateTelemetry>)`. `Candidate`/`Solution` are `Send`
  (the GA path already moves solutions across `spawn_blocking`).
- The no-post-process path skips the spawn (avoids clone/dispatch cost).
- Concurrency decision **(a)**: the request already holds an `optimize_semaphore`
  permit (`main.rs`, size `MAX_CONCURRENT_OPTIMIZE`, default = cpu count) for
  its whole lifetime — the async fn `await`s the join — so total concurrent deep
  jobs stay bounded by that semaphore. No separate deep-only cap added; the
  measurement below shows the blocking-pool move alone fixes responsiveness, so
  (b) is unnecessary.
- **Admission queue (soft variant, follow-up):** the original behaviour rejected
  any request over the permit cap immediately with `429` (`try_acquire_owned`).
  Changed to *wait* for a permit up to `OPTIMIZE_QUEUE_WAIT_MS` (new config,
  default 60000) via `tokio::time::timeout(.., sem.acquire_owned())`, so a short
  burst of users is queued rather than failed; only an exhausted wait returns
  `429 OVERLOADED` (now with `queue_wait_ms` in the error details). `0` disables
  queueing and restores the instant-`429` path. Chosen over scaling CPU first as
  the cheap, reversible step; CPU can be added later if throughput demands it.
- Measured on the **prod profile**: dev container at `--cpus 1.5 -m 512m`,
  `MAX_CONCURRENT_OPTIMIZE=2` (NOT `erp_test-freecut-1`, which was left untouched).
- Harness: `/tmp/loadtest.py` + `/tmp/measure_taskb.py`. Deep job =
  `engine=heuristic`, `consolidate{max_window:4}`, `lns{max_window:4,max_iters:4000}`,
  N50 (524 parts, sheet 2070x2800).
- A/B done with two built binaries: `main` (inline) vs this branch
  (spawn_blocking), identical request set.

## Intermediate Observations

- `available_parallelism()` on the 1.5-cpu container reports 2, so tokio runs ~2
  async worker threads. Two inline deep jobs therefore occupy *both* workers and
  the runtime cannot service `/health/ready` until one finishes — exactly the
  stall this task removes.
- The post-process is deterministic given the seed, so moving its execution
  context cannot change results. Confirmed empirically (43 sheets both ways).

## Measurements

Deep N50, `max_iters=4000`, `time_limit_ms=60000` (deadline does not bind):

| binary         | wall    | sheets | note                    |
|----------------|---------|-------:|-------------------------|
| spawn_blocking | ~13.1s  | 43     | true 4000-iter cost     |
| inline (main)  | ~13.1s* | 43     | same (single-shot)      |

\* single-shot inline matches; the divergence appears only under concurrency.

Runtime responsiveness — 2 concurrent deep N50 jobs while polling
`/health/ready` (n=40, 50ms interval):

| binary         | health median | health p95 | health max | timeouts | deep wall |
|----------------|--------------:|-----------:|-----------:|---------:|----------:|
| inline (main)  | 2.2ms         | **3964ms** | timeout    | **2/40** | ~26.5s    |
| spawn_blocking | 2.9ms         | **7.8ms**  | 124.9ms    | **0/40** | ~12.5s    |

No-load reference: health median 2.2ms, p95 2.7ms (both binaries).

Admission queue (soft variant), live confirmation — `MAX_CONCURRENT_OPTIMIZE=1`,
`OPTIMIZE_QUEUE_WAIT_MS=60000`, two concurrent deep N30 jobs:

| request | http | wall   | sheets |
|---------|------|-------:|-------:|
| req0    | 200  | 6284ms | 26     |
| req1    | 200  | 12173ms| 26     |

req1 waited ~6s for req0 to release the single permit, then ran its own ~6s —
both 200, no `429`. Pre-change this second request would have been rejected
immediately. Integration tests: `optimize_queues_and_waits_instead_of_429`
(queue=30s, cap=1 -> second request 200) and `optimize_returns_429_when_overloaded`
(queue=0 -> instant 429 path preserved).

Reading: inline lets the two deep jobs monopolize both async workers — health
p95 blows up to ~4s with 2 hard timeouts, and the deep jobs serialize to ~26.5s.
spawn_blocking keeps the async runtime free — health p95 stays 7.8ms, zero
timeouts, and the two deep jobs run concurrently on the blocking pool (~12.5s).

## Visual Findings

N/A — no layout change; sheet counts identical (43) in every run.

## Tests

- No new tests: the existing `optimize_consolidate_never_regresses_and_preserves_parts`
  and `optimize_lns_never_regresses_and_preserves_parts` integration tests
  exercise the wired post-process path; this is a pure execution-context move.
- Gates green: `cargo build --release`, `cargo fmt --check`, `cargo test
  --release` (70 pass / 6 ignored).

## Final Conclusion Candidate

Moving the heuristic consolidate+lns post-process into `spawn_blocking` removes
the runtime stall under concurrent deep load (health p95 3964ms -> 7.8ms, 2
timeouts -> 0) with identical results (43 sheets) and no behaviour change for
`engine=ga` or the no-post-process default. Deep-request concurrency is bounded
by the existing `optimize_semaphore` (decision (a)); a separate deep cap is not
needed at the 1.5-cpu prod profile. Note deep mode costs ~13s for N50 at that
profile (vs ~2-3s on the 3-cpu dev box), so `MAX_CONCURRENT_OPTIMIZE` should be
kept small in prod. As a soft follow-up, over-cap requests now wait in an
admission queue (`OPTIMIZE_QUEUE_WAIT_MS`, default 60s) instead of an instant
`429`, so a burst of users is serialized rather than rejected — CPU scaling
stays as a later option if sustained concurrent deep load needs real throughput.
Ready to merge independently of the `cut_quality` work.
