#!/bin/bash
# Stop the four-node CSPCL demo started by start.sh.

set -e

PROJECT_ROOT="$(git rev-parse --show-toplevel)"
PID_FILE="$PROJECT_ROOT/scripts/cspcl-demo/pids"

if [ ! -f "$PID_FILE" ]; then
    echo "No running demo found (missing $PID_FILE)."
    exit 0
fi

read -r PID_A PID_B PID_C PID_D <"$PID_FILE"

for pid in "$PID_A" "$PID_B" "$PID_C" "$PID_D"; do
    if [ -n "$pid" ]; then
        kill "$pid" 2>/dev/null || true
    fi
done

rm -f "$PID_FILE"
echo "CSPCL demo stopped."
