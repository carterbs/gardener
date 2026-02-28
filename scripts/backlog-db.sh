#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/backlog-db.sh list [--db PATH]
    List backlog rows with task_id, title, priority, status, source.

  scripts/backlog-db.sh add --title "TITLE" --details "DETAILS" [options]
    --details TEXT     Required
    --title TEXT       Required
    --priority P0|P1|P2  (default: P1)
    --scope KEY        (default: runtime)
    --status ready|complete|leased|in_progress|failed (default: ready)
    --kind feature|maintenance|quality_gap|bugfix|infra|merge_conflict|pr_collision (default: feature)
    --source manual    (default: manual)
    --id TASK_ID       Optional custom task_id (default: manual:<scope>:auto-<unix_ms>)
    --db PATH          Optional DB path (default: .cache/gardener/backlog.sqlite)

  scripts/backlog-db.sh help
    Show this help text.

Environment:
  GARDENER_DB_PATH can also set the default DB path.
USAGE
}

env_db_path="${GARDENER_DB_PATH:-.cache/gardener/backlog.sqlite}"

if [[ $# -eq 0 ]]; then
  usage
  exit 1
fi

cmd="$1"
shift

db_path="$env_db_path"

default_priority="P1"
default_scope="runtime"
default_status="ready"
default_kind="feature"
default_source="manual"

title=""
details=""
priority="$default_priority"
scope_key="$default_scope"
status="$default_status"
kind="$default_kind"
source="$default_source"
task_id=""

case "$cmd" in
  help|-h|--help)
    usage
    exit 0
    ;;
  list)
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --db)
          db_path="$2"
          shift 2
          ;;
        *)
          echo "unknown arg: $1" >&2
          usage
          exit 1
          ;;
      esac
    done

    sqlite3 "$db_path" "SELECT task_id, title, priority, status, source, scope_key FROM backlog_tasks ORDER BY created_at DESC LIMIT 50;"
    ;;

  add)
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --db)
          db_path="$2"
          shift 2
          ;;
        --title)
          title="$2"
          shift 2
          ;;
        --details)
          details="$2"
          shift 2
          ;;
        --priority)
          priority="$2"
          shift 2
          ;;
        --scope)
          scope_key="$2"
          shift 2
          ;;
        --status)
          status="$2"
          shift 2
          ;;
        --kind)
          kind="$2"
          shift 2
          ;;
        --source)
          source="$2"
          shift 2
          ;;
        --id)
          task_id="$2"
          shift 2
          ;;
        *)
          echo "unknown arg: $1" >&2
          usage
          exit 1
          ;;
      esac
    done

    if [[ -z "$title" || -z "$details" ]]; then
      echo "--title and --details are required for add" >&2
      usage
      exit 1
    fi

    now=$(date +%s000)
    if [[ -z "$task_id" ]]; then
      task_id="manual:${scope_key}:auto-${now}"
    fi

    mkdir -p "$(dirname "$db_path")"

    if [[ ! -f "$db_path" ]]; then
      echo "database file not found: $db_path" >&2
      exit 1
    fi

    title_esc=$(printf "%s" "$title" | sed "s/'/''/g")
    details_esc=$(printf "%s" "$details" | sed "s/'/''/g")
    task_id_esc=$(printf "%s" "$task_id" | sed "s/'/''/g")
    scope_esc=$(printf "%s" "$scope_key" | sed "s/'/''/g")
    kind_esc=$(printf "%s" "$kind" | sed "s/'/''/g")
    status_esc=$(printf "%s" "$status" | sed "s/'/''/g")
    source_esc=$(printf "%s" "$source" | sed "s/'/''/g")
    priority_esc=$(printf "%s" "$priority" | sed "s/'/''/g")

    sqlite3 "$db_path" "INSERT INTO backlog_tasks (task_id, kind, title, details, scope_key, priority, status, last_updated, source, attempt_count, created_at) VALUES ('$task_id_esc', '$kind_esc', '$title_esc', '$details_esc', '$scope_esc', '$priority_esc', '$status_esc', $now, '$source_esc', 0, $now);"

    echo "created: $task_id_esc"
    ;;
  *)
    usage
    exit 1
    ;;
esac
