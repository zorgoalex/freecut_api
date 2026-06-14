#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "${SCRIPT_DIR}/.." && pwd)

MODE="${1:-run}"

WORKTREE_ROOT="${WORKTREE_ROOT:-${REPO_ROOT}/../freecut_agents}"
BASE_BRANCH="${BASE_BRANCH:-$(git -C "${REPO_ROOT}" rev-parse --abbrev-ref HEAD)}"
SUBAGENT_MODEL="${SUBAGENT_MODEL:-o4-mini}"
REVIEW_MODEL="${REVIEW_MODEL:-gpt-5}"
SUBAGENT_TIMEOUT="${SUBAGENT_TIMEOUT:-45m}"
DRY_RUN="${DRY_RUN:-0}"

LOG_DIR="${REPO_ROOT}/tmp/agent_logs"
mkdir -p "${LOG_DIR}"

PROMPT_A="scripts/multi_agent/prompts/stage1.txt"
PROMPT_B="scripts/multi_agent/prompts/stage2.txt"
PROMPT_C="scripts/multi_agent/prompts/stage5.txt"

A_DIR="${WORKTREE_ROOT}/stage1"
B_DIR="${WORKTREE_ROOT}/stage2"
C_DIR="${WORKTREE_ROOT}/stage5"

A_BRANCH="feat/stage1-concurrency"
B_BRANCH="feat/stage2-time-budget"
C_BRANCH="feat/stage5-stock-identity"

run_cmd() {
  local cmd="$1"
  echo "+ ${cmd}"
  if [[ "${DRY_RUN}" == "1" ]]; then
    return 0
  fi
  bash -lc "${cmd}"
}

ensure_worktree() {
  local dir="$1"
  local branch="$2"

  if [[ -d "${dir}" ]]; then
    echo "Worktree exists: ${dir}"
    return 0
  fi

  if git -C "${REPO_ROOT}" show-ref --verify --quiet "refs/heads/${branch}"; then
    run_cmd "git -C '${REPO_ROOT}' worktree add '${dir}' '${branch}'"
  else
    run_cmd "git -C '${REPO_ROOT}' worktree add '${dir}' -b '${branch}' '${BASE_BRANCH}'"
  fi
}

compose_prompt() {
  local prompt_file="$1"
  cat <<EOF
$(cat "${REPO_ROOT}/${prompt_file}")

Обязательные условия:
- Следовать правилам из ai_docs/Rules.md.
- После правок обновить ai_docs/last_state.md с конкретными изменениями и результатами тестов.
- Перед завершением выполнить cargo test и явно указать результат.
EOF
}

run_subagent() {
  local _label="$1"
  local worktree="$2"
  local prompt_file_rel="$3"
  local log_file="$4"

  local prompt
  prompt="$(compose_prompt "${prompt_file_rel}")"

  (
    cd "${worktree}"
    timeout "${SUBAGENT_TIMEOUT}" codex exec -m "${SUBAGENT_MODEL}" "${prompt}" > "${log_file}" 2>&1
  ) &
  SUBAGENT_PID=$!
}

run_parallel_agents() {
  ensure_worktree "${A_DIR}" "${A_BRANCH}"
  ensure_worktree "${B_DIR}" "${B_BRANCH}"
  ensure_worktree "${C_DIR}" "${C_BRANCH}"

  if [[ "${DRY_RUN}" == "1" ]]; then
    echo "DRY_RUN=1: skip subagent execution."
    return 0
  fi

  local pid_a pid_b pid_c
  run_subagent "A" "${A_DIR}" "${PROMPT_A}" "${LOG_DIR}/agent_a.log"; pid_a="${SUBAGENT_PID}"
  run_subagent "B" "${B_DIR}" "${PROMPT_B}" "${LOG_DIR}/agent_b.log"; pid_b="${SUBAGENT_PID}"
  run_subagent "C" "${C_DIR}" "${PROMPT_C}" "${LOG_DIR}/agent_c.log"; pid_c="${SUBAGENT_PID}"

  local status_a status_b status_c
  set +e
  wait "${pid_a}"; status_a=$?
  wait "${pid_b}"; status_b=$?
  wait "${pid_c}"; status_c=$?
  set -e

  echo "Subagent status:"
  echo "A(stage1): ${status_a} -> ${LOG_DIR}/agent_a.log"
  echo "B(stage2): ${status_b} -> ${LOG_DIR}/agent_b.log"
  echo "C(stage5): ${status_c} -> ${LOG_DIR}/agent_c.log"

  if [[ "${status_a}" -ne 0 || "${status_b}" -ne 0 || "${status_c}" -ne 0 ]]; then
    echo "At least one subagent failed. Inspect logs before merge." >&2
    return 1
  fi
}

run_critical_review() {
  local review_prompt
  review_prompt="Critical review: find regressions in optimizer behavior, timeout handling, stock identity mapping, API compatibility, and missing tests."
  run_cmd "cd '${REPO_ROOT}' && codex exec review --uncommitted -m '${REVIEW_MODEL}' \"${review_prompt}\""
}

usage() {
  cat <<EOF
Usage:
  $(basename "$0") [run|review|all]

Modes:
  run      Create worktrees and run subagents in parallel for Stage 1/2/5.
  review   Run critical review on current branch with REVIEW_MODEL.
  all      Run subagents first, then critical review.

Environment overrides:
  SUBAGENT_MODEL=o4-mini
  REVIEW_MODEL=gpt-5
  SUBAGENT_TIMEOUT=45m
  BASE_BRANCH=$(git -C "${REPO_ROOT}" rev-parse --abbrev-ref HEAD)
  WORKTREE_ROOT=${REPO_ROOT}/../freecut_agents
  DRY_RUN=0
EOF
}

case "${MODE}" in
  run)
    run_parallel_agents
    ;;
  review)
    run_critical_review
    ;;
  all)
    run_parallel_agents
    run_critical_review
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    echo "Unknown mode: ${MODE}" >&2
    usage
    exit 2
    ;;
esac
