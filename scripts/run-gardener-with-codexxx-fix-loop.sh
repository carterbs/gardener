#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

GARDENER_BINARY=${GARDENER_BINARY:-scripts/brad-gardener}
CODEXXX_BINARY=${CODEXXX_BINARY:-codexxx}
TARGET_BRANCH=${GARDENER_TARGET_BRANCH:-main}
MAX_FAILURES=${GARDENER_MAX_FAILURES:-5}
LOG_PATH=${GARDENER_LOG_PATH:-.gardener/otel-logs.jsonl}
STREAM_OTEL_LOGS=${GARDENER_STREAM_OTEL_LOGS:-1}
OTEL_LOG_TAIL_LINES=${GARDENER_OTEL_LOG_TAIL_LINES:-30}
ZSHRC_PATH=${ZSHRC_PATH:-"$HOME/.zshrc"}
USE_ATTEMPT_DB=${GARDENER_USE_ATTEMPT_DB:-1}
DB_ATTEMPT_DIR=.cache/gardener/agent-db-attempts
OTEL_STREAM_PID=""

if [[ -n "${GARDENER_DB_PATH:-}" ]]; then
  BASE_DB_PATH=$GARDENER_DB_PATH
elif [[ -n "${HOME:-}" ]]; then
  BASE_DB_PATH="$HOME/.gardener/backlog.sqlite"
else
  BASE_DB_PATH=".cache/gardener/backlog.sqlite"
fi

is_true() {
  case "${1:-}" in
    1 | y | yes | true | on | enabled)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

resolve_cmd_with_zshrc() {
  local cmd="$1"
  local found=""

  found=$(type -P "$cmd" 2>/dev/null || true)
  if [[ -n "$found" ]]; then
    echo "$found"
    return 0
  fi

  if [[ -z "${ZSHRC_PATH:-}" || ! -f "$ZSHRC_PATH" ]]; then
    return 1
  fi

  if ! command -v zsh >/dev/null 2>&1; then
    return 1
  fi

  found=$(zsh -lc "source \"$ZSHRC_PATH\"; whence -p \"$cmd\"")
  if [[ -n "$found" ]]; then
    echo "$found"
    return 0
  fi

  return 1
}

run_with_env() {
  local -a cmd=("$@")
  if [[ -n "$ZSHRC_PATH" && -f "$ZSHRC_PATH" ]]; then
    if ! command -v zsh >/dev/null 2>&1; then
      echo "error: zsh required to source zshrc at $ZSHRC_PATH but not found" >&2
      return 1
    fi
    local quoted
    quoted=$(printf "%q " "${cmd[@]}")
    quoted=${quoted% }
    zsh -lc "source \"$ZSHRC_PATH\"; ${quoted}"
    return $?
  fi
  "${cmd[@]}"
}

if [[ ! -x "$GARDENER_BINARY" ]]; then
  echo "error: gardener binary not executable: $GARDENER_BINARY" >&2
  exit 1
fi
if ! CODEXXX_BINARY=$(resolve_cmd_with_zshrc "$CODEXXX_BINARY"); then
  echo "error: codexxx binary not found in PATH: $CODEXXX_BINARY" >&2
  exit 1
fi
read -r CODEXXX_BINARY <<<"$CODEXXX_BINARY"

if [[ $# -gt 0 ]]; then
  GARDENER_ARGS=("$@")
else
  GARDENER_ARGS=(--quit-after 1 --config gardener.toml)
fi

if [[ "$(git rev-parse --abbrev-ref HEAD)" != "$TARGET_BRANCH" ]]; then
  echo "warning: switching working tree from $(git rev-parse --abbrev-ref HEAD) to $TARGET_BRANCH"
  git checkout "$TARGET_BRANCH"
fi

if ! is_true "${GARDENER_ALLOW_DIRTY_WORKTREE:-0}"; then
  if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "error: working tree must be clean before running agent-retry loop" >&2
    echo "set GARDENER_ALLOW_DIRTY_WORKTREE=1 to bypass (not recommended)" >&2
    exit 1
  fi
fi

run_with_agent() {
  local prompt_file=$1
  local before_sha
  before_sha=$(git rev-parse HEAD)

  # shellcheck disable=SC2206
  local codexxx_args=(${CODEXXX_ARGS:-})
  if ! run_with_env "$CODEXXX_BINARY" "${codexxx_args[@]}" <"$prompt_file"; then
    echo "error: codexxx command failed" >&2
    return 1
  fi

  local after_sha
  after_sha=$(git rev-parse HEAD)
  if [[ "$after_sha" == "$before_sha" ]]; then
    if git diff --quiet "${before_sha}" -- >/dev/null 2>&1; then
      echo "error: codexxx returned without making a commit" >&2
    else
      echo "error: codexxx changed files but did not commit" >&2
    fi
    return 1
  fi

  echo "info: codexxx produced commit $after_sha"
}

start_otel_stream() {
  if ! is_true "$STREAM_OTEL_LOGS"; then
    return 0
  fi

  if ! [[ "$OTEL_LOG_TAIL_LINES" =~ ^[0-9]+$ ]] || [[ "$OTEL_LOG_TAIL_LINES" -le 0 ]]; then
    echo "warning: invalid GARDENER_OTEL_LOG_TAIL_LINES=$OTEL_LOG_TAIL_LINES, defaulting to 30" >&2
    OTEL_LOG_TAIL_LINES=30
  fi

  {
    echo "info: streaming last $OTEL_LOG_TAIL_LINES lines from $LOG_PATH (live updates)"
    tail -n "$OTEL_LOG_TAIL_LINES" -F "$LOG_PATH" 2>/dev/null | sed -u 's/^/[otel] /'
  } &
  OTEL_STREAM_PID=$!
}

stop_otel_stream() {
  if [[ -n "${OTEL_STREAM_PID:-}" ]]; then
    kill "$OTEL_STREAM_PID" >/dev/null 2>&1 || true
    wait "$OTEL_STREAM_PID" >/dev/null 2>&1 || true
    OTEL_STREAM_PID=""
  fi
}

interrupt() {
  stop_otel_stream
  echo "info: interrupted; aborting gardener retry loop." >&2
  exit 130
}

is_interrupt_code() {
  case "${1:-}" in
    130 | 143)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

trap interrupt INT TERM
trap stop_otel_stream EXIT


mkdir -p "$DB_ATTEMPT_DIR"

for attempt in $(seq 1 "$MAX_FAILURES"); do
  echo "===== Gardener attempt $attempt of $MAX_FAILURES ====="

  attempt_dir="$DB_ATTEMPT_DIR/../gardener-agent-attempt-$attempt"
  mkdir -p "$attempt_dir"
  stdout_file="$attempt_dir/stdout.log"
  stderr_file="$attempt_dir/stderr.log"

  attempt_seed_db="$DB_ATTEMPT_DIR/attempt-${attempt}-seed.sqlite"
  attempt_backup_db="$DB_ATTEMPT_DIR/attempt-${attempt}-backup.sqlite"
  attempt_run_db="$DB_ATTEMPT_DIR/attempt-${attempt}-run.sqlite"
  run_db_env=()

  if is_true "$USE_ATTEMPT_DB"; then
    if [[ -f "$BASE_DB_PATH" ]]; then
      cp -p "$BASE_DB_PATH" "$attempt_seed_db"
    else
      : > "$attempt_seed_db"
    fi
    cp -p "$attempt_seed_db" "$attempt_run_db"
    run_db_env=(env "GARDENER_DB_PATH=$attempt_run_db")
    echo "info: attempt $attempt using isolated DB ${attempt_run_db}"
  else
    if [[ -f "$BASE_DB_PATH" ]]; then
      cp -p "$BASE_DB_PATH" "$attempt_backup_db"
    fi
    echo "info: attempt $attempt using production DB ${BASE_DB_PATH}"
  fi

  start_otel_stream
  if run_with_env "${run_db_env[@]}" "$GARDENER_BINARY" "${GARDENER_ARGS[@]}" >"$stdout_file" 2>"$stderr_file"; then
    stop_otel_stream
    echo "info: gardener succeeded on attempt $attempt"
    if is_true "$USE_ATTEMPT_DB"; then
      echo "info: isolated DB run preserved at $attempt_run_db"
    fi
    echo "--- gardener stdout ---"
    cat "$stdout_file"
    echo "--- gardener stderr ---"
    cat "$stderr_file" >&2
    exit 0
  fi
  gardener_rc=$?
  stop_otel_stream
  if is_interrupt_code "$gardener_rc"; then
    exit "$gardener_rc"
  fi

  echo "error: gardener failed with exit code $gardener_rc"
  echo "--- gardener stdout ---"
  cat "$stdout_file"
  echo "--- gardener stderr ---"
  cat "$stderr_file" >&2

  if is_true "$USE_ATTEMPT_DB"; then
    rm -f "$attempt_run_db"
  else
    if [[ -f "$attempt_backup_db" ]]; then
      echo "info: restoring DB before retry: $BASE_DB_PATH"
      mkdir -p "$(dirname "$BASE_DB_PATH")"
      cp -p "$attempt_backup_db" "$BASE_DB_PATH"
    else
      rm -f "$BASE_DB_PATH"
    fi
  fi

  if [[ "$attempt" -ge "$MAX_FAILURES" ]]; then
    echo "error: reached failure limit ($MAX_FAILURES). stopping."
    exit "$gardener_rc"
  fi

  prompt_file="$attempt_dir/codexxx-prompt.txt"
  {
    echo "A run of the Gardener runtime failed."
    echo "Attempt: $attempt"
    echo "Command: $GARDENER_BINARY ${GARDENER_ARGS[*]}"
    echo "Exit code: $gardener_rc"
    echo
    echo "STDOUT (full):"
    cat "$stdout_file"
    echo
    echo "STDERR (full):"
    cat "$stderr_file"
    echo
    if [[ -f "$LOG_PATH" ]]; then
      echo "Recent run log ($LOG_PATH):"
      tail -n "$OTEL_LOG_TAIL_LINES" "$LOG_PATH"
    fi
    echo
    echo "Task: Fix this failure in the repository and commit the fix directly to $TARGET_BRANCH."
    echo "You must only return a concise summary once commit is made."
    echo "If DB isolation is enabled, use an isolated DB path in this run only."
  } >"$prompt_file"

  echo "info: invoking $CODEXXX_BINARY for fix-and-commit"
  if ! run_with_agent "$prompt_file"; then
    run_with_agent_rc=$?
    if is_interrupt_code "$run_with_agent_rc"; then
      exit "$run_with_agent_rc"
    fi
    echo "error: codexxx did not produce a commit for attempt $attempt"
    exit 1
  fi
  echo "info: retrying gardener run after codexxx commit"

  # keep loop running only with clean state before next attempt
  if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "error: codexxx left local changes uncommitted; aborting"
    exit 1
  fi
done
