#!/usr/bin/env bash
set -euo pipefail

INSTANCE_NAME="${LIMA_INSTANCE_NAME:-gardener-isolated}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TEMPLATE_PATH="$REPO_ROOT/tools/lima/gardener-isolated.yaml.tmpl"
GENERATED_TEMPLATE="/tmp/${INSTANCE_NAME}.yaml"
HOST_MOUNT_PATH="/workspace/host-gardener"
GUEST_REPO_PATH="${GARDENER_GUEST_REPO_PATH:-/workspace/gardener}"

usage() {
  cat <<USAGE
Usage: $(basename "$0") <command>

Commands:
  up            Start or provision Lima instance with repo-only mount
  status        Show instance status
  shell         Open interactive shell in instance
  auth          Run GitHub auth flow in instance (required for push/PR)
  run [args...] Run Gardener in the instance (default: --)
  stop          Stop instance
  delete        Delete instance and VM disk
USAGE
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command not found: $1" >&2
    exit 1
  fi
}

generate_template() {
  if [[ ! -f "$TEMPLATE_PATH" ]]; then
    echo "error: missing template at $TEMPLATE_PATH" >&2
    exit 1
  fi
  sed "s|__REPO_PATH__|$REPO_ROOT|g" "$TEMPLATE_PATH" > "$GENERATED_TEMPLATE"
}

lima_shell() {
  limactl shell --workdir / "$INSTANCE_NAME" -- "$@"
}

ensure_guest_repo() {
  local bootstrap_cmd
  bootstrap_cmd="$(cat <<EOF
set -euo pipefail
if [[ -d '$GUEST_REPO_PATH/.git' ]]; then
  exit 0
fi
if [[ ! -d '$HOST_MOUNT_PATH/.git' ]]; then
  echo "error: host-mounted repo missing at $HOST_MOUNT_PATH" >&2
  exit 1
fi
mkdir -p "$(dirname '$GUEST_REPO_PATH')"
rm -rf '$GUEST_REPO_PATH'
git clone '$HOST_MOUNT_PATH' '$GUEST_REPO_PATH'
EOF
)"
  lima_shell bash -lc "$bootstrap_cmd"
}

start_instance() {
  require_cmd limactl
  generate_template
  if limactl list | awk '{print $1}' | grep -Fxq "$INSTANCE_NAME"; then
    limactl start "$INSTANCE_NAME"
  else
    limactl start --name "$INSTANCE_NAME" "$GENERATED_TEMPLATE"
  fi
}

run_gardener() {
  local args
  if [[ "$#" -eq 0 ]]; then
    args=(--)
  else
    args=("$@")
  fi

  local quoted_args
  quoted_args="$(printf ' %q' "${args[@]}")"
  lima_shell bash -lc "set -euo pipefail; cd '$GUEST_REPO_PATH'; . \"\$HOME/.cargo/env\"; cargo run -p gardener --bin gardener --${quoted_args}"
}

cmd="${1:-}"
shift || true

case "$cmd" in
  up)
    start_instance
    ensure_guest_repo
    ;;
  status)
    require_cmd limactl
    limactl list
    ;;
  shell)
    require_cmd limactl
    ensure_guest_repo
    limactl shell --workdir "$GUEST_REPO_PATH" "$INSTANCE_NAME"
    ;;
  auth)
    require_cmd limactl
    ensure_guest_repo
    lima_shell bash -lc "cd '$GUEST_REPO_PATH'; gh auth login"
    ;;
  run)
    require_cmd limactl
    ensure_guest_repo
    run_gardener "$@"
    ;;
  stop)
    require_cmd limactl
    limactl stop "$INSTANCE_NAME"
    ;;
  delete)
    require_cmd limactl
    limactl delete "$INSTANCE_NAME"
    ;;
  *)
    usage
    exit 1
    ;;
esac
