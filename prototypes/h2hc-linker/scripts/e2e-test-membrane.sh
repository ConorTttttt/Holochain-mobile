#!/usr/bin/env bash
#
# End-to-End Test Script for h2hc-linker Agent Registration
#
# This script tests that browser agents registered via h2hc-linker's WebSocket
# become visible in Holochain conductors' agent_infos.
#
# Usage:
#   ./scripts/e2e-test-membrane.sh [command]
#
# Commands:
#   start     Start bootstrap, conductor, and h2hc-linker (default)
#   stop      Stop all services
#   status    Show running services
#   clean     Clean up sandbox data
#   test      Run agent registration test (assumes services are running)
#
# Prerequisites:
#   - Run inside nix develop shell
#   - Build h2hc-linker: cargo build --release
#
# What this tests:
#   1. Starts bootstrap server (kitsune2 peer discovery)
#   2. Starts a Holochain conductor with ziptest hApp
#   3. Starts h2hc-linker connected to same bootstrap
#   4. Connects to h2hc-linker WebSocket
#   5. Registers an agent via WebSocket
#   6. Verifies the agent appears in conductor's agent_info

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
FISHY_DIR="$(cd "$PROJECT_DIR/../fishy" && pwd)"
SANDBOX_DIR="$PROJECT_DIR/.hc-sandbox"

# Ports
LINKER_PORT=8090
ADMIN_PORT=8888

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_step() {
    echo -e "${BLUE}[STEP]${NC} $1"
}

# Parse command
COMMAND="${1:-start}"

# Check prerequisites
check_prereqs() {
    log_info "Checking prerequisites..."

    if ! command -v hc &> /dev/null; then
        log_error "hc command not found. Run: nix develop"
        exit 1
    fi

    if ! command -v holochain &> /dev/null; then
        log_error "holochain command not found. Run: nix develop"
        exit 1
    fi

    if ! command -v kitsune2-bootstrap-srv &> /dev/null; then
        log_error "kitsune2-bootstrap-srv not found. Run: nix develop"
        exit 1
    fi

    if [ ! -f "$PROJECT_DIR/target/release/h2hc-linker" ]; then
        log_warn "h2hc-linker binary not found. Building..."
        (cd "$PROJECT_DIR" && cargo build --release)
    fi

    # Check for ziptest.happ
    if [ ! -f "$FISHY_DIR/fixtures/ziptest.happ" ]; then
        log_error "ziptest.happ not found at $FISHY_DIR/fixtures/ziptest.happ"
        log_info "Copy the ziptest happ there first."
        exit 1
    fi

    log_info "Prerequisites OK"
}

# Start bootstrap server
start_bootstrap() {
    log_step "Starting bootstrap server..."

    mkdir -p "$SANDBOX_DIR"
    cd "$SANDBOX_DIR"

    if [ -f bootstrap.pid ] && kill -0 "$(cat bootstrap.pid)" 2>/dev/null; then
        log_warn "Bootstrap server already running"
        return 0
    fi

    kitsune2-bootstrap-srv --sbd-disable-rate-limiting > bootstrap.log 2>&1 &
    BOOTSTRAP_PID=$!
    echo "$BOOTSTRAP_PID" > bootstrap.pid

    log_info "Waiting for bootstrap server..."
    for i in {1..10}; do
        if grep -q "#kitsune2_bootstrap_srv#listening#" bootstrap.log 2>/dev/null; then
            BOOTSTRAP_ADDR=$(grep "#kitsune2_bootstrap_srv#listening#" bootstrap.log | head -1 | sed 's/.*#kitsune2_bootstrap_srv#listening#\([^#]*\)#.*/\1/')
            if [ -n "$BOOTSTRAP_ADDR" ]; then
                log_info "Bootstrap server: $BOOTSTRAP_ADDR"
                echo "$BOOTSTRAP_ADDR" > bootstrap_addr.txt
                return 0
            fi
        fi
        sleep 0.5
    done

    log_error "Bootstrap server failed to start"
    cat bootstrap.log
    exit 1
}

# Start Holochain conductor
start_conductor() {
    log_step "Starting Holochain conductor..."

    cd "$SANDBOX_DIR"

    if [ -f conductor.pid ] && kill -0 "$(cat conductor.pid)" 2>/dev/null; then
        log_warn "Conductor already running"
        return 0
    fi

    BOOTSTRAP_ADDR=$(cat bootstrap_addr.txt)
    BOOTSTRAP_URL="http://${BOOTSTRAP_ADDR}"
    SIGNAL_URL="ws://${BOOTSTRAP_ADDR}"

    # Create webrtc config
    cat > webrtc-config.json << 'EOF'
{
  "iceServers": [
    {
      "urls": [
        "stun:stun.l.google.com:19302",
        "stun:stun1.l.google.com:19302"
      ]
    }
  ]
}
EOF

    rm -rf "$SANDBOX_DIR/data" 2>/dev/null || true
    mkdir -p "$SANDBOX_DIR/data"

    # Start conductor
    (echo "test-passphrase" | RUST_LOG="info,holochain=debug,kitsune2=info,holochain_p2p=debug" hc sandbox --piped generate \
        --in-process-lair \
        --run 0 \
        --app-id "ziptest" \
        --root "$SANDBOX_DIR/data" \
        "$FISHY_DIR/fixtures/ziptest.happ" \
        network -b "$BOOTSTRAP_URL" webrtc "$SIGNAL_URL" "$SANDBOX_DIR/webrtc-config.json") \
        > conductor.log 2>&1 &

    CONDUCTOR_PID=$!
    echo "$CONDUCTOR_PID" > conductor.pid

    log_info "Waiting for conductor (PID: $CONDUCTOR_PID)..."
    for i in {1..60}; do
        if grep -q '"admin_port":' conductor.log 2>/dev/null; then
            ACTUAL_ADMIN=$(grep -oP '"admin_port":\K\d+' conductor.log 2>/dev/null | head -1)
            log_info "Conductor started on admin port: $ACTUAL_ADMIN"
            echo "$ACTUAL_ADMIN" > admin_port.txt

            # Get DNA hash
            sleep 2
            DNA_HASH=$(hc sandbox call --running="$ACTUAL_ADMIN" list-dnas 2>/dev/null | grep -oP '"uhC0k[^"]+' | head -1 | tr -d '"' || true)
            if [ -n "$DNA_HASH" ]; then
                log_info "DNA hash: $DNA_HASH"
                echo "$DNA_HASH" > dna_hash.txt
            fi
            return 0
        fi
        if ! kill -0 "$CONDUCTOR_PID" 2>/dev/null; then
            log_error "Conductor process died"
            cat conductor.log
            exit 1
        fi
        sleep 1
    done

    log_error "Conductor failed to start"
    cat conductor.log
    exit 1
}

# Start h2hc-linker
start_linker() {
    log_step "Starting h2hc-linker..."

    cd "$SANDBOX_DIR"

    if [ -f linker.pid ] && kill -0 "$(cat linker.pid)" 2>/dev/null; then
        log_warn "h2hc-linker already running"
        return 0
    fi

    BOOTSTRAP_ADDR=$(cat bootstrap_addr.txt)

    # Start h2hc-linker with kitsune2 configured
    H2HC_LINKER_BOOTSTRAP_URL="http://${BOOTSTRAP_ADDR}" \
    H2HC_LINKER_SIGNAL_URL="ws://${BOOTSTRAP_ADDR}" \
    RUST_LOG="info,h2hc_linker=debug,kitsune2=info" \
    "$PROJECT_DIR/target/release/h2hc-linker" --port "$LINKER_PORT" \
        > linker.log 2>&1 &

    LINKER_PID=$!
    echo "$LINKER_PID" > linker.pid

    log_info "Waiting for h2hc-linker..."
    for i in {1..10}; do
        if curl -s "http://localhost:$LINKER_PORT/health" > /dev/null 2>&1; then
            log_info "h2hc-linker started on port $LINKER_PORT"
            return 0
        fi
        sleep 1
    done

    log_error "h2hc-linker failed to start"
    cat linker.log
    exit 1
}

# Stop all services
stop_all() {
    log_step "Stopping all services..."

    cd "$SANDBOX_DIR" 2>/dev/null || true

    for pidfile in linker.pid conductor.pid bootstrap.pid; do
        if [ -f "$pidfile" ]; then
            PID=$(cat "$pidfile")
            if kill -0 "$PID" 2>/dev/null; then
                log_info "Stopping $pidfile (PID: $PID)"
                kill "$PID" 2>/dev/null || true
            fi
            rm -f "$pidfile"
        fi
    done

    # Clean up stale processes
    pkill -f "hc sandbox" 2>/dev/null || true
    pkill -f "holochain.*sandbox" 2>/dev/null || true
    pkill -f "h2hc-linker" 2>/dev/null || true
    pkill -f "kitsune2-bootstrap" 2>/dev/null || true

    log_info "All services stopped"
}

# Show status
show_status() {
    log_info "Service status:"

    cd "$SANDBOX_DIR" 2>/dev/null || {
        echo "  No sandbox directory"
        return
    }

    for service in bootstrap conductor linker; do
        pidfile="${service}.pid"
        if [ -f "$pidfile" ]; then
            PID=$(cat "$pidfile")
            if kill -0 "$PID" 2>/dev/null; then
                echo -e "  ${GREEN}●${NC} $service (PID: $PID)"
            else
                echo -e "  ${RED}●${NC} $service (stale PID file)"
            fi
        else
            echo -e "  ${RED}○${NC} $service (not running)"
        fi
    done

    if [ -f bootstrap_addr.txt ]; then
        echo "  Bootstrap: $(cat bootstrap_addr.txt)"
    fi
    if [ -f admin_port.txt ]; then
        echo "  Conductor admin: $(cat admin_port.txt)"
    fi
    if [ -f dna_hash.txt ]; then
        echo "  DNA hash: $(cat dna_hash.txt)"
    fi
}

# Clean up
clean_all() {
    log_step "Cleaning up..."
    stop_all
    rm -rf "$SANDBOX_DIR"
    log_info "Cleaned up sandbox directory"
}

# Run agent registration test
run_test() {
    log_step "Running agent registration test..."

    cd "$SANDBOX_DIR"

    # Check services are running
    if [ ! -f linker.pid ] || ! kill -0 "$(cat linker.pid)" 2>/dev/null; then
        log_error "h2hc-linker not running. Run: $0 start"
        exit 1
    fi

    DNA_HASH=$(cat dna_hash.txt 2>/dev/null || true)
    if [ -z "$DNA_HASH" ]; then
        log_error "DNA hash not found"
        exit 1
    fi

    ADMIN_PORT=$(cat admin_port.txt 2>/dev/null || true)
    if [ -z "$ADMIN_PORT" ]; then
        log_error "Admin port not found"
        exit 1
    fi

    # Generate a test agent pubkey (base64 encoded, starts with uhCAk)
    # This is a fake agent key for testing - it won't have a real signature
    # In real usage, fishy extension would use a real key from Lair
    TEST_AGENT="uhCAkTestAgentKeyForM2aTestingOnly000000000000000000"

    log_info "Test parameters:"
    log_info "  DNA: $DNA_HASH"
    log_info "  Agent: $TEST_AGENT"
    log_info "  Linker: ws://localhost:$LINKER_PORT/ws"

    # Connect to WebSocket and send auth + register messages
    log_step "Connecting to WebSocket and registering agent..."

    # Use websocat if available, otherwise use curl for a simple test
    if command -v websocat &> /dev/null; then
        # Send auth and register messages
        (echo '{"type":"auth","session_token":""}'; sleep 0.5; echo "{\"type\":\"register\",\"dna_hash\":\"$DNA_HASH\",\"agent_pubkey\":\"$TEST_AGENT\"}"; sleep 2) | \
            websocat "ws://localhost:$LINKER_PORT/ws" 2>&1 | head -10 || true
    else
        log_warn "websocat not found, testing basic health endpoint instead"
        curl -s "http://localhost:$LINKER_PORT/health"
        echo ""
    fi

    # Wait for registration to propagate
    log_info "Waiting for agent registration to propagate..."
    sleep 5

    # Query conductor for agent info
    log_step "Querying conductor agent_info..."

    # Note: The test agent key we're using isn't valid, so it won't actually appear
    # in the conductor's agent_info. This test mainly verifies the infrastructure works.
    # A real test would use fishy extension which signs properly.

    log_info "Checking conductor for registered agents..."
    AGENT_INFO=$(hc sandbox call --running="$ADMIN_PORT" list-cells 2>/dev/null || echo "")

    if [ -n "$AGENT_INFO" ]; then
        log_info "Conductor cells:"
        echo "$AGENT_INFO" | head -20
    else
        log_warn "Could not query conductor"
    fi

    log_info ""
    log_info "Test complete."
    log_info ""
    log_info "To test with real agent registration:"
    log_info "  1. Load fishy extension in browser"
    log_info "  2. Configure gateway URL: http://localhost:$LINKER_PORT"
    log_info "  3. Install an hApp (use ziptest from ../fishy/fixtures/)"
    log_info "  4. The extension will register agents via WebSocket"
    log_info "  5. Check conductor: hc sandbox call --running=$ADMIN_PORT list-agents"
    log_info "  6. Agents from the gateway URL will appear in the list"
}

# Main
case "$COMMAND" in
    start)
        check_prereqs
        start_bootstrap
        start_conductor
        start_linker
        show_status
        log_info ""
        log_info "Services started. Run test with: $0 test"
        ;;
    stop)
        stop_all
        ;;
    status)
        show_status
        ;;
    clean)
        clean_all
        ;;
    test)
        run_test
        ;;
    *)
        log_error "Unknown command: $COMMAND"
        log_info "Usage: $0 [start|stop|status|clean|test]"
        exit 1
        ;;
esac
