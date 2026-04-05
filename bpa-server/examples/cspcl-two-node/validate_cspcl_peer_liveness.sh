#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
EXAMPLE_DIR="$ROOT/bpa-server/examples/cspcl-two-node"
TMP_DIR="$(mktemp -d)"
NODE1_LOG="$TMP_DIR/node1.log"
NODE2_LOG="$TMP_DIR/node2.log"
RECV_LOG="$TMP_DIR/recv.log"

cleanup() {
  jobs -pr | xargs -r kill >/dev/null 2>&1 || true
  wait >/dev/null 2>&1 || true
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing command: $1" >&2
    exit 1
  }
}

require_cmd cargo
require_cmd ip

if [[ ! -d /sys/class/net/vcan0 ]]; then
  echo "vcan0 is missing; create it before running this script" >&2
  exit 1
fi

start_node1() {
  HARDY_BPA_SERVER_LOG_LEVEL=debug \
  CSP_REPO_DIR=/home/hugo/code/libcsp \
  cargo run --bin hardy-bpa-server --features cspcl -- \
    -c "$EXAMPLE_DIR/node1.toml" >"$NODE1_LOG" 2>&1 &
}

start_node2() {
  HARDY_BPA_SERVER_LOG_LEVEL=debug \
  CSP_REPO_DIR=/home/hugo/code/libcsp \
  cargo run --bin hardy-bpa-server --features cspcl -- \
    -c "$EXAMPLE_DIR/node2.toml" >"$NODE2_LOG" 2>&1 &
}

start_receiver() {
  cargo run -p hardy-tools --bin bpa-recv -- \
    --grpc 'http://[::1]:50062' \
    --service 9000 >"$RECV_LOG" 2>&1 &
}

send_bundle() {
  local payload="$1"
  cargo run -p hardy-tools --bin bpa-send -- \
    --grpc 'http://[::1]:50061' \
    --destination ipn:2.9000 \
    --payload "$payload"
}

wait_for_log() {
  local file="$1"
  local pattern="$2"
  local timeout="${3:-10}"
  local deadline=$((SECONDS + timeout))
  while (( SECONDS < deadline )); do
    if grep -Fq "$pattern" "$file" 2>/dev/null; then
      return 0
    fi
    sleep 0.2
  done
  echo "timed out waiting for pattern in $file: $pattern" >&2
  exit 1
}

start_node1
wait_for_log "$NODE1_LOG" "Registered new CLA: cspcl1" 20

send_bundle "stored while node 2 is down"
wait_for_log "$NODE1_LOG" "CSP peer 2:11 is not verified up" 10

start_node2
wait_for_log "$NODE2_LOG" "Registered new CLA: cspcl1" 20
wait_for_log "$NODE1_LOG" "CSP peer 2:11 is verified up; registering with BPA" 20

start_receiver
send_bundle "hello after peer up"
wait_for_log "$RECV_LOG" "payload=hello after peer up" 20

pkill -f "node2.toml" >/dev/null 2>&1 || true
wait_for_log "$NODE1_LOG" "CSP peer 2:11 is down; removing from BPA" 20

send_bundle "stored again after node 2 stops"
wait_for_log "$NODE1_LOG" "CSP peer 2:11 is not verified up" 10

echo "CSPCL peer-liveness validation passed"
