#!/usr/bin/env bash
# Install the Gardener backlog watchdog as a launchd KeepAlive user agent.
#
# The watchdog runs continuously in the background, watching
# ~/.gardener/backlog.sqlite via kqueue. On any modification, truncation,
# or deletion it immediately captures stat + lsof and writes to
# ~/.gardener/audit.log — independent of whether gardener is running.
#
# Usage:
#   ./scripts/install-watchdog.sh          # install
#   ./scripts/install-watchdog.sh uninstall # remove

set -euo pipefail

LABEL="com.gardener.watchdog"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WATCHDOG_SRC="$SCRIPT_DIR/watchdog.py"
INSTALL_DIR="$HOME/.local/share/gardener"
WATCHDOG_DEST="$INSTALL_DIR/watchdog.py"
PLIST_DIR="$HOME/Library/LaunchAgents"
PLIST_PATH="$PLIST_DIR/$LABEL.plist"
LOG_DIR="$HOME/.gardener"
STDOUT_LOG="$LOG_DIR/watchdog-stdout.log"
STDERR_LOG="$LOG_DIR/watchdog-stderr.log"

if [[ "${1:-}" == "uninstall" ]]; then
  echo "Uninstalling gardener watchdog..."
  launchctl unload "$PLIST_PATH" 2>/dev/null || true
  rm -f "$PLIST_PATH" "$WATCHDOG_DEST"
  echo "Done. audit.log is preserved at $LOG_DIR/audit.log"
  exit 0
fi

echo "Installing gardener backlog watchdog..."

mkdir -p "$INSTALL_DIR" "$PLIST_DIR" "$LOG_DIR"
cp "$WATCHDOG_SRC" "$WATCHDOG_DEST"
chmod +x "$WATCHDOG_DEST"

cat > "$PLIST_PATH" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$LABEL</string>

  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/python3</string>
    <string>$WATCHDOG_DEST</string>
  </array>

  <key>KeepAlive</key>
  <true/>

  <key>RunAtLoad</key>
  <true/>

  <key>StandardOutPath</key>
  <string>$STDOUT_LOG</string>

  <key>StandardErrorPath</key>
  <string>$STDERR_LOG</string>

  <key>ThrottleInterval</key>
  <integer>5</integer>
</dict>
</plist>
PLIST

# Reload if already loaded.
launchctl unload "$PLIST_PATH" 2>/dev/null || true
launchctl load "$PLIST_PATH"

echo ""
echo "Watchdog installed and running."
echo "  DB watched:  ~/.gardener/backlog.sqlite"
echo "  Audit log:   ~/.gardener/audit.log"
echo "  Agent log:   $STDOUT_LOG"
echo ""
echo "To uninstall: ./scripts/install-watchdog.sh uninstall"
echo "To check:     launchctl list | grep gardener"
