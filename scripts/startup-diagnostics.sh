#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/startup-diagnostics.sh --run-id <id> [--log-path <path>] [--output <path>] [--stage <name>] [--error <message>]

Captures startup/boot/run-failed log lines into a markdown summary for quick triage.
USAGE
}

RUN_ID=""
LOG_PATH="${GARDENER_LOG_PATH:-}"
OUTPUT=""
STAGE="startup-audits"
ERROR_MESSAGE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run-id)
      RUN_ID="$2"
      shift 2
      ;;
    --log-path)
      LOG_PATH="$2"
      shift 2
      ;;
    --output)
      OUTPUT="$2"
      shift 2
      ;;
    --stage)
      STAGE="$2"
      shift 2
      ;;
    --error)
      ERROR_MESSAGE="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$RUN_ID" ]]; then
  RUN_ID="$(date +%s)"
fi

if [[ -z "$LOG_PATH" ]]; then
  if [[ -f .cache/gardener/otel-logs.jsonl ]]; then
    LOG_PATH=.cache/gardener/otel-logs.jsonl
  elif [[ -f .gardener/otel-logs.jsonl ]]; then
    LOG_PATH=.gardener/otel-logs.jsonl
  fi
fi

if [[ -z "$OUTPUT" ]]; then
  BASE_DIR="${LOG_PATH%/*}"
  if [[ -z "$BASE_DIR" || "$BASE_DIR" == "." ]]; then
    BASE_DIR=".cache/gardener"
  fi
  OUTPUT="$BASE_DIR/startup-diagnostics/${RUN_ID}-startup-failure.md"
fi

mkdir -p "$(dirname "$OUTPUT")"

{
  echo "# Startup diagnostics"
  echo
  echo "- stage: $STAGE"
  echo "- run_id: $RUN_ID"
  if [[ -n "$LOG_PATH" ]]; then
    echo "- log_path: $LOG_PATH"
  fi
  echo "- captured_at: $(date -Is 2>/dev/null || date)"
  if [[ -n "$ERROR_MESSAGE" ]]; then
    echo "- error: $ERROR_MESSAGE"
  fi
  echo
  echo "## Extracted startup timeline"
  echo

  if [[ -z "$LOG_PATH" || ! -f "$LOG_PATH" ]]; then
    echo "No log file available for this run."
  else
    if command -v jq >/dev/null 2>&1; then
      if ! jq -e "." "$LOG_PATH" >/dev/null 2>&1; then
        echo "Could not parse log file as JSONL."
      else
        jq -r --arg rid "$RUN_ID" '
          select(
            ((.event_type // "") | test("^(startup\\.|boot\\.|run\\.(failed|completed))"))
            and
            (((.logRecord.attributes // []) | map(select(.key == "run.id")) | .[0].value.stringValue | tostring) == $rid)
          )
          | "\(.event_type): \(.payload | tojson)"' "$LOG_PATH" > "$OUTPUT.timeline"
      fi
    fi

    if [[ -f "$OUTPUT.timeline" ]]; then
      cat "$OUTPUT.timeline"
      rm "$OUTPUT.timeline"
    else
      grep -E '"event_type":"(startup\.|boot\.|run\.failed|run\.completed)"' "$LOG_PATH" || true
    fi
  fi
} > "$OUTPUT"

echo "startup diagnostics saved: $OUTPUT"
