#!/usr/bin/env bash
set -euo pipefail

LOG_PATH=${GARDENER_LOG_PATH:-.cache/gardener/otel-logs.jsonl}
if [[ -z "${GARDENER_LOG_PATH:-}" ]]; then
  if [[ -f .cache/gardener/otel-logs.jsonl ]]; then
    LOG_PATH=.cache/gardener/otel-logs.jsonl
  elif [[ -f .gardener/otel-logs.jsonl ]]; then
    LOG_PATH=.gardener/otel-logs.jsonl
  fi
fi

OTEL_LOG_TAIL_LINES=${GARDENER_OTEL_LOG_TAIL_LINES:-30}
OTEL_LOG_INTERVAL=${GARDENER_OTEL_LOG_INTERVAL_SECONDS:-60}
OTEL_LOG_PRETTY=${GARDENER_OTEL_PRETTY:-1}

if ! [[ "$OTEL_LOG_INTERVAL" =~ ^[0-9]+$ ]] || [[ "$OTEL_LOG_INTERVAL" -le 0 ]]; then
  echo "warn: invalid GARDENER_OTEL_LOG_INTERVAL_SECONDS=$OTEL_LOG_INTERVAL, defaulting to 60" >&2
  OTEL_LOG_INTERVAL=60
fi

if ! [[ "$OTEL_LOG_TAIL_LINES" =~ ^[0-9]+$ ]] || [[ "$OTEL_LOG_TAIL_LINES" -le 0 ]]; then
  echo "warn: invalid GARDENER_OTEL_LOG_TAIL_LINES=$OTEL_LOG_TAIL_LINES, defaulting to 30" >&2
  OTEL_LOG_TAIL_LINES=30
fi

pretty_print_line() {
  local line="$1"
  if is_true "${OTEL_LOG_PRETTY:-1}"; then
    if command -v jq >/dev/null 2>&1; then
      if printf '%s' "$line" | jq -e . >/dev/null 2>&1; then
        printf '%s\n' "$line" | jq -C .
      else
        echo "$line"
      fi
    else
      echo "$line"
    fi
  else
    echo "$line"
  fi
  echo "----------------------------------------"
}

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

while true; do
  clear
  echo "Watching: $LOG_PATH"
  echo "Last $OTEL_LOG_TAIL_LINES lines (refresh $OTEL_LOG_INTERVAL s)"
  echo "Time: $(date)"
  echo "----------------------------------------"

  if [[ -f "$LOG_PATH" ]]; then
    if is_true "$OTEL_LOG_PRETTY" && command -v jq >/dev/null 2>&1; then
      while IFS= read -r line; do
        pretty_print_line "$line"
      done < <(tail -n "$OTEL_LOG_TAIL_LINES" "$LOG_PATH")
    else
      tail -n "$OTEL_LOG_TAIL_LINES" "$LOG_PATH"
    fi
  else
    echo "log file not found: $LOG_PATH"
  fi

  echo
  echo "next refresh in ${OTEL_LOG_INTERVAL}s (Ctrl+C to stop)"
  sleep "$OTEL_LOG_INTERVAL"
done
