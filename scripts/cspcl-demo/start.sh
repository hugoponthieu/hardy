#!/bin/bash
# Launch a four-node Hardy BPA demo over built-in CSPCL.
#
# Usage:
#   ./scripts/cspcl-demo/start.sh
#
# The script:
# - Builds hardy-bpa-server with the cspcl feature
# - Starts node A (ipn:1), node B (ipn:2), node C (ipn:3), node D (ipn:4)
# - Streams prefixed node logs directly to this terminal
# - Prints a multi-hop app-send validation command
#
# Stop the demo with Ctrl+C in this terminal, or:
#   ./scripts/cspcl-demo/stop.sh

set -e

PROJECT_ROOT="$(git rev-parse --show-toplevel)"
DEMO_DIR="$PROJECT_ROOT/scripts/cspcl-demo"
PID_FILE="$DEMO_DIR/pids"

NODE_A_CONFIG="$DEMO_DIR/node-a.yaml"
NODE_B_CONFIG="$DEMO_DIR/node-b.yaml"
NODE_C_CONFIG="$DEMO_DIR/node-c.yaml"
NODE_D_CONFIG="$DEMO_DIR/node-d.yaml"

find_libcsp_build_dir() {
    local repo="$1"
    if [ -f "$repo/build/libcsp.a" ] || [ -f "$repo/build/libcsp.so" ]; then
        echo "$repo/build"
        return 0
    fi
    if [ -f "$repo/build/src/libcsp.a" ] || [ -f "$repo/build/src/libcsp.so" ]; then
        echo "$repo/build/src"
        return 0
    fi
    return 1
}

libcsp_archive_path() {
    local build_dir="$1"
    if [ -f "$build_dir/libcsp.a" ]; then
        echo "$build_dir/libcsp.a"
        return 0
    fi
    if [ -f "$build_dir/src/libcsp.a" ]; then
        echo "$build_dir/src/libcsp.a"
        return 0
    fi
    return 1
}

libcsp_has_zmq_symbol() {
    local archive="$1"
    nm -g "$archive" 2>/dev/null | grep -qE '(^| )T csp_zmqhub_init$'
}

libcsp_has_socketcan_symbol() {
    local archive="$1"
    nm -g "$archive" 2>/dev/null | grep -qE '(^| )T csp_can_socketcan_open_and_add_interface$'
}

rebuild_libcsp_with_features() {
    local repo="$1"

    if [ ! -x "$repo/waf" ]; then
        echo "Cannot auto-rebuild libcsp: missing $repo/waf"
        return 1
    fi
    if ! command -v pkg-config >/dev/null 2>&1; then
        echo "Cannot auto-rebuild libcsp with required features: pkg-config not found."
        return 1
    fi
    if ! pkg-config --exists libzmq; then
        echo "Cannot auto-rebuild libcsp with ZMQ support: libzmq pkg-config entry not found."
        echo "Install libzmq dev package (e.g. libzmq3-dev), then retry."
        return 1
    fi
    if ! pkg-config --exists libsocketcan; then
        echo "Cannot auto-rebuild libcsp with CAN support: libsocketcan pkg-config entry not found."
        echo "Install libsocketcan dev package (e.g. libsocketcan-dev), then retry."
        return 1
    fi

    echo "Rebuilding libcsp with ZMQ + SocketCAN support ..."
    (
        cd "$repo"
        ./waf distclean >/dev/null 2>&1 || true
        ./waf configure --with-os=posix --enable-if-zmqhub --enable-can-socketcan >/dev/null
        ./waf build >/dev/null
    )
}

discover_libcsp() {
    local candidates=()
    [ -n "${CSP_REPO_DIR:-}" ] && candidates+=("$CSP_REPO_DIR")
    [ -n "${LIBCSP_DIR:-}" ] && candidates+=("$LIBCSP_DIR")
    candidates+=(
        "/tmp/libcsp"
        "$HOME/libcsp"
        "$HOME/code/libcsp"
        "$PROJECT_ROOT/../libcsp"
    )

    local repo
    for repo in "${candidates[@]}"; do
        [ -d "$repo" ] || continue
        [ -f "$repo/include/csp/csp_types.h" ] || continue

        if [ -n "${CSP_BUILD_DIR:-}" ]; then
            if [ -f "$CSP_BUILD_DIR/libcsp.a" ] || [ -f "$CSP_BUILD_DIR/libcsp.so" ]; then
                echo "$repo|$CSP_BUILD_DIR"
                return 0
            fi
            if [ -f "$CSP_BUILD_DIR/src/libcsp.a" ] || [ -f "$CSP_BUILD_DIR/src/libcsp.so" ]; then
                echo "$repo|$CSP_BUILD_DIR/src"
                return 0
            fi
        fi

        local build_dir
        if build_dir="$(find_libcsp_build_dir "$repo")"; then
            echo "$repo|$build_dir"
            return 0
        fi
    done

    return 1
}

# Preflight: discover usable libcsp paths automatically.
if resolved="$(discover_libcsp)"; then
    CSP_REPO_DIR="${resolved%%|*}"
    CSP_BUILD_DIR="${resolved##*|}"
    # Force cspcl-sys to use the discovered real libcsp tree.
    # Important: do NOT set CSP_INCLUDE_DIR here. In cspcl-sys/build.rs, setting
    # CSP_INCLUDE_DIR bypasses CSP_REPO_DIR/CSP_BUILD_DIR-derived lib detection.
    unset CSP_INCLUDE_DIR
    CSP_USE_STUBS=0
    export CSP_REPO_DIR CSP_BUILD_DIR CSP_USE_STUBS
    echo "Using libcsp from:"
    echo "  CSP_REPO_DIR=$CSP_REPO_DIR"
    echo "  CSP_BUILD_DIR=$CSP_BUILD_DIR"
    echo "  CSP_INCLUDE_DIR=<auto via CSP_REPO_DIR>"
else
    echo "Could not locate a usable libcsp build."
    echo "Expected:"
    echo "  - repo with include/csp/csp_types.h"
    echo "  - build dir with libcsp.a or libcsp.so"
    echo ""
    echo "Fix by setting either:"
    echo "  export LIBCSP_DIR=/path/to/libcsp"
    echo "or both:"
    echo "  export CSP_REPO_DIR=/path/to/libcsp"
    echo "  export CSP_BUILD_DIR=/path/to/libcsp/build-or-build/src"
    exit 1
fi

LIBCSP_ARCHIVE="$(libcsp_archive_path "$CSP_BUILD_DIR" || true)"
if [ -n "$LIBCSP_ARCHIVE" ] && { ! libcsp_has_zmq_symbol "$LIBCSP_ARCHIVE" || ! libcsp_has_socketcan_symbol "$LIBCSP_ARCHIVE"; }; then
    if [ "${AUTO_REBUILD_LIBCSP:-1}" = "1" ]; then
        if rebuild_libcsp_with_features "$CSP_REPO_DIR"; then
            if resolved="$(discover_libcsp)"; then
                CSP_REPO_DIR="${resolved%%|*}"
                CSP_BUILD_DIR="${resolved##*|}"
                export CSP_REPO_DIR CSP_BUILD_DIR
                LIBCSP_ARCHIVE="$(libcsp_archive_path "$CSP_BUILD_DIR" || true)"
            fi
        fi
    fi
fi

if [ -z "${LIBCSP_ARCHIVE:-}" ] || ! libcsp_has_zmq_symbol "$LIBCSP_ARCHIVE" || ! libcsp_has_socketcan_symbol "$LIBCSP_ARCHIVE"; then
    echo "libcsp was found but is missing required symbols for this demo."
    echo "Required: csp_zmqhub_init and csp_can_socketcan_open_and_add_interface."
    echo ""
    echo "Fix by rebuilding libcsp with ZMQ + SocketCAN support:"
    echo "  cd \"$CSP_REPO_DIR\""
    echo "  ./waf distclean"
    echo "  ./waf configure --with-os=posix --enable-if-zmqhub --enable-can-socketcan"
    echo "  ./waf build"
    echo ""
    echo "If dependencies are missing, install libzmq and libsocketcan dev packages first."
    exit 1
fi

if [ -f "$PID_FILE" ]; then
    echo "Existing demo PID file found at $PID_FILE"
    echo "Run ./scripts/cspcl-demo/stop.sh first, then retry."
    exit 1
fi

if ss -ltn | grep -qE '(\[::1\]:51051|\[::1\]:52051|\[::1\]:53051|\[::1\]:54051)'; then
    echo "Ports [::1]:51051, [::1]:52051, [::1]:53051, or [::1]:54051 are already in use."
    echo "Stop existing BPA servers, then retry."
    echo "Hint: ./scripts/cspcl-demo/stop.sh"
    exit 1
fi

if ! ip link show vcan0 >/dev/null 2>&1; then
    echo "vcan0 is not present."
    echo "Bring it up with:"
    echo "  sudo modprobe vcan && sudo ip link add dev vcan0 type vcan 2>/dev/null || true; sudo ip link set up vcan0"
    echo "Or follow docs/user-docs/operations/vcan-on-arch.md for persistent setup."
    exit 1
fi

echo "Building hardy-bpa-server with cspcl feature..."
(
    cd "$PROJECT_ROOT"
    cargo build -p hardy-bpa-server --features cspcl
)

echo "Starting node A (ipn:1.0 / grpc [::1]:51051)..."
(
    cd "$PROJECT_ROOT"
    cargo run -p hardy-bpa-server --features cspcl -- --config "$NODE_A_CONFIG" 2>&1 | sed -u 's/^/[node-a] /'
) &
PID_A=$!

echo "Starting node B (ipn:2.0 / grpc [::1]:52051)..."
(
    cd "$PROJECT_ROOT"
    cargo run -p hardy-bpa-server --features cspcl -- --config "$NODE_B_CONFIG" 2>&1 | sed -u 's/^/[node-b] /'
) &
PID_B=$!

echo "Starting node C (ipn:3.0 / grpc [::1]:53051)..."
(
    cd "$PROJECT_ROOT"
    cargo run -p hardy-bpa-server --features cspcl -- --config "$NODE_C_CONFIG" 2>&1 | sed -u 's/^/[node-c] /'
) &
PID_C=$!

echo "Starting node D (ipn:4.0 / grpc [::1]:54051)..."
(
    cd "$PROJECT_ROOT"
    cargo run -p hardy-bpa-server --features cspcl -- --config "$NODE_D_CONFIG" 2>&1 | sed -u 's/^/[node-d] /'
) &
PID_D=$!

echo "$PID_A $PID_B $PID_C $PID_D" >"$PID_FILE"

cleanup() {
    if [ -f "$PID_FILE" ]; then
        read -r PA PB PC PD <"$PID_FILE" || true
        if [ -n "$PA" ]; then kill "$PA" 2>/dev/null || true; fi
        if [ -n "$PB" ]; then kill "$PB" 2>/dev/null || true; fi
        if [ -n "$PC" ]; then kill "$PC" 2>/dev/null || true; fi
        if [ -n "$PD" ]; then kill "$PD" 2>/dev/null || true; fi
        rm -f "$PID_FILE"
    fi
}

trap cleanup EXIT INT TERM

echo ""
echo "Demo started. Topology (linear chain over vcan0, routed by A-SABR):"
echo "  A(ipn:1) <-> B(ipn:2) <-> C(ipn:3) <-> D(ipn:4)"
echo "Each node loads scripts/cspcl-demo/contact-plan.cp and computes next hops live."
echo ""
echo "Send a bundle from A to D (3-hop store-demo, service 4242 is unregistered on D):"
echo "  cargo run -p hardy-tools --bin bp -- app-send \\"
echo "    --bpa http://[::1]:51051 \\"
echo "    --source-service 4242 \\"
echo "    --payload \"hello D from A\" \\"
echo "    ipn:4.4242"
echo ""
echo "Watch [node-b] and [node-c] for 'Queuing bundle for forwarding' lines,"
echo "and [node-d] for 'Storing bundle until a forwarding opportunity arises'."
echo ""
echo "Running nodes (Ctrl+C to stop all nodes)..."
wait "$PID_A" "$PID_B" "$PID_C" "$PID_D"
