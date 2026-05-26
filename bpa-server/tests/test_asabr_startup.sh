#!/bin/bash
# Test: bpa-server A-SABR routing startup
#
# Smoke test for the A-SABR live-routing plugin.
#   1. Server starts with a valid asabr config + contact plan.
#   2. Server fails to start with an invalid asabr config
#      (unsupported local-node-id scheme).
#
# Usage:
#   ./bpa-server/tests/test_asabr_startup.sh [--skip-build]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TEST_DIR"' EXIT

SKIP_BUILD=false
for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=true ;;
    esac
done

if [ "$SKIP_BUILD" = false ]; then
    cargo build --release -p hardy-bpa-server
fi

BINARY="$WORKSPACE_DIR/target/release/hardy-bpa-server"
if [ ! -x "$BINARY" ]; then
    echo "expected binary at $BINARY (build failed?)"
    exit 1
fi

# Write a contact plan whose contacts are currently active.
NOW=$(date -u +%s)
START=$((NOW - 60))
END=$((NOW + 3600))
cat > "$TEST_DIR/contact_plan.cp" <<EOF
node 0 n0
node 1 n1
node 2 n2
contact 1 2 ${START} ${END} 10000 1
EOF

cat > "$TEST_DIR/bpa.yaml" <<EOF
node-ids: "ipn:1.0"
log-level: warn
asabr:
  protocol-id: "asabr"
  router: "SpsnHybridParenting"
  contact-plan-path: "$TEST_DIR/contact_plan.cp"
  local-node-id: "ipn:1.0"
storage:
  metadata:
    type: memory
  bundle:
    type: memory
EOF

echo "Starting bpa-server with valid asabr config..."
"$BINARY" --config "$TEST_DIR/bpa" &
PID=$!
sleep 2

if ! kill -0 "$PID" 2>/dev/null; then
    echo "bpa-server exited unexpectedly with the valid asabr config"
    wait "$PID" || true
    exit 1
fi

kill "$PID"
wait "$PID" 2>/dev/null || true
echo "  ok"

cat > "$TEST_DIR/bad.yaml" <<EOF
node-ids: "ipn:1.0"
log-level: warn
asabr:
  protocol-id: "asabr"
  router: "SpsnHybridParenting"
  contact-plan-path: "$TEST_DIR/contact_plan.cp"
  local-node-id: "dtn://mars/"
storage:
  metadata:
    type: memory
  bundle:
    type: memory
EOF

echo "Starting bpa-server with invalid asabr config (dtn local-node-id)..."
if "$BINARY" --config "$TEST_DIR/bad" >/dev/null 2>&1; then
    echo "expected the invalid asabr config to fail startup, but it succeeded"
    exit 1
fi
echo "  ok"

echo "PASS"
