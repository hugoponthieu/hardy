#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

assert_contains() {
    local file="$1"
    local needle="$2"
    if ! grep -Fq "$needle" "$file"; then
        echo "expected '$needle' in $file" >&2
        exit 1
    fi
}

assert_contains "$SCRIPT_DIR/start.sh" 'NODE_A_CONFIG="$DEMO_DIR/node-a.yaml"'
assert_contains "$SCRIPT_DIR/start.sh" 'NODE_B_CONFIG="$DEMO_DIR/node-b.yaml"'
assert_contains "$SCRIPT_DIR/start.sh" 'NODE_C_CONFIG="$DEMO_DIR/node-c.yaml"'
assert_contains "$SCRIPT_DIR/start.sh" 'NODE_D_CONFIG="$DEMO_DIR/node-d.yaml"'
assert_contains "$SCRIPT_DIR/start.sh" 'PID_D=$!'
assert_contains "$SCRIPT_DIR/start.sh" 'echo "$PID_A $PID_B $PID_C $PID_D" >"$PID_FILE"'
assert_contains "$SCRIPT_DIR/stop.sh" 'read -r PID_A PID_B PID_C PID_D <"$PID_FILE"'
assert_contains "$SCRIPT_DIR/node-a.yaml" 'node-ids: "ipn:1.0"'
assert_contains "$SCRIPT_DIR/node-b.yaml" 'node-ids: "ipn:2.0"'
assert_contains "$SCRIPT_DIR/node-c.yaml" 'node-ids: "ipn:3.0"'
assert_contains "$SCRIPT_DIR/node-d.yaml" 'node-ids: "ipn:4.0"'

echo "cspcl demo layout looks consistent"
