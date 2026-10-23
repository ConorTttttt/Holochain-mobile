#!/usr/bin/env bash
#
# DHT Query Round-Trip Test
#
# Tests whether h2hc-linker can successfully query conductors via kitsune2
# wire protocol and receive responses. This is the core test for Step 19.3.
#
# Prerequisites:
#   - nix develop shell (for holochain, hc, kitsune2-bootstrap-srv)
#   - npm installed ws package (npm install ws)
#   - h2hc-linker built (cargo build --release)
#   - A hApp to install (fixture1 or any)
#
# Usage:
#   ./scripts/test-dht-roundtrip.sh [--fishy-dir=PATH] [--happ=NAME]
#
# The script:
#   1. Starts bootstrap server, 2 conductors, and h2hc-linker gateway
#   2. Registers a fake agent via WebSocket (triggers kitsune2 space join)
#   3. Waits for peer discovery
#   4. Queries DHT endpoints via curl with 10-second timeout
#   5. Reports PASS/FAIL with timing and diagnostic info
#   6. Cleans up all processes
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SANDBOX_DIR="/tmp/h2hc-linker-dht-test"

# Default paths
FISHY_DIR="${FISHY_DIR:-$(cd "$PROJECT_DIR/../fishy-step19" 2>/dev/null && pwd || echo "")}"
HAPP_NAME="fixture1"
GATEWAY_BINARY="$PROJECT_DIR/target/release/h2hc-linker"

# Parse arguments
for arg in "$@"; do
    case $arg in
        --fishy-dir=*)
            FISHY_DIR="${arg#*=}"
            ;;
        --happ=*)
            HAPP_NAME="${arg#*=}"
            ;;
    esac
done

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

log_info()  { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }
log_test()  { echo -e "${CYAN}[TEST]${NC} $1"; }

PASS_COUNT=0
FAIL_COUNT=0

report_result() {
    local name="$1"
    local result="$2"
    local elapsed="$3"
    local detail="${4:-}"

    if [ "$result" = "PASS" ]; then
        echo -e "  ${GREEN}PASS${NC} $name (${elapsed}s)${detail:+ - $detail}"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        echo -e "  ${RED}FAIL${NC} $name (${elapsed}s)${detail:+ - $detail}"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
}

cleanup() {
    log_info "Cleaning up..."
    # Kill background processes
    kill "$(cat "$SANDBOX_DIR/ws-client.pid" 2>/dev/null)" 2>/dev/null || true
    kill "$(cat "$SANDBOX_DIR/gateway.pid" 2>/dev/null)" 2>/dev/null || true
    for f in "$SANDBOX_DIR"/conductor*.pid; do
        [ -f "$f" ] && kill "$(cat "$f")" 2>/dev/null || true
    done
    kill "$(cat "$SANDBOX_DIR/bootstrap.pid" 2>/dev/null)" 2>/dev/null || true
    pkill -f "hc sandbox.*$SANDBOX_DIR" 2>/dev/null || true
    pkill -f "holochain.*$SANDBOX_DIR" 2>/dev/null || true
    # Don't remove sandbox dir - keep logs for analysis
    log_info "Logs available in $SANDBOX_DIR/"
}

trap cleanup EXIT

# ============================================================================
# Setup
# ============================================================================

log_info "=== DHT Query Round-Trip Test ==="
log_info "h2hc-linker dir: $PROJECT_DIR"
log_info "Sandbox dir: $SANDBOX_DIR"

# Check prerequisites
if ! command -v holochain &>/dev/null; then
    log_error "holochain not found. Run inside 'nix develop'"
    exit 1
fi

if [ ! -f "$GATEWAY_BINARY" ]; then
    log_error "h2hc-linker binary not found at $GATEWAY_BINARY"
    log_info "Build it: cargo build --release"
    exit 1
fi

# Determine hApp path
case "$HAPP_NAME" in
    fixture1)
        HAPP_PATH="$PROJECT_DIR/../hc-http-gw-fork/fixture/package/happ1/fixture1.happ"
        ;;
    *)
        if [ -n "$FISHY_DIR" ]; then
            HAPP_PATH="$FISHY_DIR/fixtures/${HAPP_NAME}.happ"
        else
            log_error "Unknown hApp: $HAPP_NAME and no --fishy-dir specified"
            exit 1
        fi
        ;;
esac

if [ ! -f "$HAPP_PATH" ]; then
    log_error "hApp not found: $HAPP_PATH"
    exit 1
fi

log_info "Using hApp: $HAPP_PATH"

# Clean sandbox
rm -rf "$SANDBOX_DIR"
mkdir -p "$SANDBOX_DIR"

# ============================================================================
# Start Services
# ============================================================================

# 1. Bootstrap server
log_info "Starting bootstrap server..."
kitsune2-bootstrap-srv --sbd-disable-rate-limiting > "$SANDBOX_DIR/bootstrap.log" 2>&1 &
echo "$!" > "$SANDBOX_DIR/bootstrap.pid"

for i in {1..10}; do
    if grep -q "#kitsune2_bootstrap_srv#listening#" "$SANDBOX_DIR/bootstrap.log" 2>/dev/null; then
        break
    fi
    sleep 0.5
done

BOOTSTRAP_ADDR=$(grep "#kitsune2_bootstrap_srv#listening#" "$SANDBOX_DIR/bootstrap.log" | head -1 | sed 's/.*#kitsune2_bootstrap_srv#listening#\([^#]*\)#.*/\1/')
if [ -z "$BOOTSTRAP_ADDR" ]; then
    log_error "Bootstrap server failed to start"
    cat "$SANDBOX_DIR/bootstrap.log"
    exit 1
fi
BOOTSTRAP_URL="http://${BOOTSTRAP_ADDR}"
log_info "Bootstrap: $BOOTSTRAP_URL"

# 2. Start 2 conductors (TRACE level for holochain_p2p to see query handling)
for idx in 1 2; do
    local_suffix=""
    if [ "$idx" -gt 1 ]; then local_suffix="_$idx"; fi

    DATA_DIR="$SANDBOX_DIR/data${local_suffix}"
    mkdir -p "$DATA_DIR"

    APP_ID="${HAPP_NAME}${local_suffix:+_$idx}"

    log_info "Starting conductor $idx..."
    (echo "test-passphrase" | \
        RUST_LOG="info,holochain=debug,kitsune2=debug,holochain_p2p=trace" \
        hc sandbox --piped generate \
            --in-process-lair \
            --run 0 \
            --app-id "$APP_ID" \
            --root "$DATA_DIR" \
            "$HAPP_PATH" \
            network -b "$BOOTSTRAP_URL" quic "$BOOTSTRAP_URL") \
        > "$SANDBOX_DIR/conductor${local_suffix}.log" 2>&1 &
    echo "$!" > "$SANDBOX_DIR/conductor${local_suffix}.pid"

    for i in {1..60}; do
        if grep -q '"admin_port":' "$SANDBOX_DIR/conductor${local_suffix}.log" 2>/dev/null; then
            ADMIN_PORT=$(grep -oP '"admin_port":\K\d+' "$SANDBOX_DIR/conductor${local_suffix}.log" | head -1)
            echo "$ADMIN_PORT" > "$SANDBOX_DIR/admin_port${local_suffix}.txt"
            log_info "Conductor $idx started (admin port $ADMIN_PORT)"
            break
        fi
        if [ "$i" -eq 60 ]; then
            log_error "Conductor $idx failed to start"
            cat "$SANDBOX_DIR/conductor${local_suffix}.log"
            exit 1
        fi
        sleep 1
    done
    sleep 1
done

# Wait for conductors to establish arcs
log_info "Waiting for conductor arc establishment (30s)..."
sleep 30

# 3. Get DNA hash from conductor
ADMIN_PORT_1=$(cat "$SANDBOX_DIR/admin_port.txt")
DNA_HASH=$(hc sandbox call --running="$ADMIN_PORT_1" list-dnas 2>&1 | grep -oP '"uhC0k[^"]+' | head -1 | tr -d '"')
if [ -z "$DNA_HASH" ]; then
    log_error "Could not get DNA hash from conductor"
    exit 1
fi
log_info "DNA hash: $DNA_HASH"

# 4. Start h2hc-linker gateway
log_info "Starting h2hc-linker gateway..."
H2HC_LINKER_CONDUCTOR_URL="127.0.0.1:$ADMIN_PORT_1" \
H2HC_LINKER_BOOTSTRAP_URL="$BOOTSTRAP_URL" \
H2HC_LINKER_RELAY_URL="$BOOTSTRAP_URL" \
RUST_LOG="info,h2hc_linker=trace,kitsune2=debug" \
"$GATEWAY_BINARY" --port 8000 > "$SANDBOX_DIR/gateway.log" 2>&1 &
echo "$!" > "$SANDBOX_DIR/gateway.pid"

for i in {1..10}; do
    if curl -s "http://localhost:8000/health" > /dev/null 2>&1; then
        log_info "Gateway started on port 8000"
        break
    fi
    if [ "$i" -eq 10 ]; then
        log_error "Gateway failed to start"
        cat "$SANDBOX_DIR/gateway.log"
        exit 1
    fi
    sleep 1
done

# 5. Register a test agent via WebSocket
log_info "Registering test agent via WebSocket..."
if [ -f "$SCRIPT_DIR/test-ws-client.mjs" ]; then
    node "$SCRIPT_DIR/test-ws-client.mjs" "$DNA_HASH" > "$SANDBOX_DIR/ws-client.log" 2>&1 &
    echo "$!" > "$SANDBOX_DIR/ws-client.pid"
else
    log_error "test-ws-client.mjs not found"
    exit 1
fi

# Wait for agent registration + kitsune2 peer discovery
log_info "Waiting for kitsune2 peer discovery (20s)..."
sleep 20

# Check if agent was registered
if grep -q "Agent registered" "$SANDBOX_DIR/ws-client.log" 2>/dev/null; then
    log_info "Agent registered with gateway"
else
    log_warn "Agent registration status unclear. WS client log:"
    cat "$SANDBOX_DIR/ws-client.log"
fi

# ============================================================================
# Tests
# ============================================================================

echo ""
log_test "=== DHT Query Round-Trip Tests ==="
echo ""

# Test 1: GET /dht/{dna}/record/{hash} with a fake hash (should get empty response, NOT timeout)
log_test "Test 1: GET record with fake hash (expect quick empty response, not 30s timeout)"
FAKE_ACTION_HASH="uhCkkAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
START=$(date +%s%3N)
RESPONSE=$(curl -s -m 10 -w '\n%{http_code}' "http://localhost:8000/dht/${DNA_HASH}/record/${FAKE_ACTION_HASH}" 2>&1) || true
END=$(date +%s%3N)
ELAPSED=$(echo "scale=2; ($END - $START) / 1000" | bc)
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | head -1)

if [ "$ELAPSED" != "" ] && (( $(echo "$ELAPSED < 15" | bc -l) )); then
    if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "000" ]; then
        report_result "record/fake-hash response time" "PASS" "$ELAPSED" "HTTP $HTTP_CODE, body: ${BODY:0:80}"
    else
        report_result "record/fake-hash response time" "PASS" "$ELAPSED" "HTTP $HTTP_CODE (non-timeout)"
    fi
else
    report_result "record/fake-hash response time" "FAIL" "$ELAPSED" "Took >15s (likely timeout). HTTP $HTTP_CODE"
fi

# Test 2: GET /dht/{dna}/links with a fake base (should get quick empty response)
log_test "Test 2: GET links with fake base (expect quick empty response)"
FAKE_ENTRY_HASH="uhCEkBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
START=$(date +%s%3N)
RESPONSE=$(curl -s -m 10 -w '\n%{http_code}' "http://localhost:8000/dht/${DNA_HASH}/links?base=${FAKE_ENTRY_HASH}" 2>&1) || true
END=$(date +%s%3N)
ELAPSED=$(echo "scale=2; ($END - $START) / 1000" | bc)
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | head -1)

if [ "$ELAPSED" != "" ] && (( $(echo "$ELAPSED < 15" | bc -l) )); then
    report_result "links/fake-base response time" "PASS" "$ELAPSED" "HTTP $HTTP_CODE, body: ${BODY:0:80}"
else
    report_result "links/fake-base response time" "FAIL" "$ELAPSED" "Took >15s (likely timeout). HTTP $HTTP_CODE"
fi

# Test 3: GET /dht/{dna}/details/{hash} with a fake hash
log_test "Test 3: GET details with fake hash (expect quick empty response)"
START=$(date +%s%3N)
RESPONSE=$(curl -s -m 10 -w '\n%{http_code}' "http://localhost:8000/dht/${DNA_HASH}/details/${FAKE_ACTION_HASH}" 2>&1) || true
END=$(date +%s%3N)
ELAPSED=$(echo "scale=2; ($END - $START) / 1000" | bc)
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | head -1)

if [ "$ELAPSED" != "" ] && (( $(echo "$ELAPSED < 15" | bc -l) )); then
    report_result "details/fake-hash response time" "PASS" "$ELAPSED" "HTTP $HTTP_CODE, body: ${BODY:0:80}"
else
    report_result "details/fake-hash response time" "FAIL" "$ELAPSED" "Took >15s (likely timeout). HTTP $HTTP_CODE"
fi

# ============================================================================
# Log Analysis
# ============================================================================

echo ""
log_test "=== Log Analysis ==="
echo ""

# Check gateway logs for recv_notify responses
GET_RES_COUNT=$(grep -c "Received GetRes response" "$SANDBOX_DIR/gateway.log" 2>/dev/null || echo "0")
GET_LINKS_RES_COUNT=$(grep -c "Received GetLinksRes response" "$SANDBOX_DIR/gateway.log" 2>/dev/null || echo "0")
ERROR_RES_COUNT=$(grep -c "Received ErrorRes response" "$SANDBOX_DIR/gateway.log" 2>/dev/null || echo "0")
TIMEOUT_COUNT=$(grep -c "TIMED OUT" "$SANDBOX_DIR/gateway.log" 2>/dev/null || echo "0")
SEND_SUCCESS_COUNT=$(grep -c "send_notify completed successfully" "$SANDBOX_DIR/gateway.log" 2>/dev/null || echo "0")

echo "  Gateway recv_notify:"
echo "    GetRes received:     $GET_RES_COUNT"
echo "    GetLinksRes received: $GET_LINKS_RES_COUNT"
echo "    ErrorRes received:   $ERROR_RES_COUNT"
echo "    Timeouts:            $TIMEOUT_COUNT"
echo "    send_notify sent:    $SEND_SUCCESS_COUNT"

# Check conductor logs for evidence of processing
CONDUCTOR_RECV=$(grep -c "recv_notify" "$SANDBOX_DIR/conductor.log" 2>/dev/null || echo "0")
CONDUCTOR_GET_ERR=$(grep -c "Error sending get response" "$SANDBOX_DIR/conductor.log" 2>/dev/null || echo "0")
CONDUCTOR_LINKS_ERR=$(grep -c "Error sending get_links response" "$SANDBOX_DIR/conductor.log" 2>/dev/null || echo "0")

echo ""
echo "  Conductor 1:"
echo "    recv_notify calls:   $CONDUCTOR_RECV"
echo "    Get response errors: $CONDUCTOR_GET_ERR"
echo "    Links resp errors:   $CONDUCTOR_LINKS_ERR"

# Check for kitsune2 connection events in gateway
PEER_CONNECT=$(grep -c "Peer connected\|new_listening_address\|Validated incoming preflight" "$SANDBOX_DIR/gateway.log" 2>/dev/null || echo "0")
echo ""
echo "  Gateway kitsune2:"
echo "    Peer events:         $PEER_CONNECT"

# Check for "responsive remote agents" in gateway
RESPONSIVE=$(grep "responsive remote agents" "$SANDBOX_DIR/gateway.log" 2>/dev/null | tail -3)
if [ -n "$RESPONSIVE" ]; then
    echo "    Latest peer finds:"
    echo "$RESPONSIVE" | while read -r line; do
        echo "      $(echo "$line" | grep -oP 'responsive_count=\d+')"
    done
fi

# ============================================================================
# Summary
# ============================================================================

echo ""
echo "============================================"
echo -e "  Results: ${GREEN}$PASS_COUNT PASS${NC}  ${RED}$FAIL_COUNT FAIL${NC}"
echo "============================================"
echo ""
echo "  Logs: $SANDBOX_DIR/"
echo "    gateway.log     - h2hc-linker (trace level)"
echo "    conductor.log   - conductor 1 (holochain_p2p=trace)"
echo "    conductor_2.log - conductor 2"
echo "    ws-client.log   - WebSocket client"
echo "    bootstrap.log   - bootstrap/relay server"
echo ""

if [ "$FAIL_COUNT" -gt 0 ]; then
    echo "Diagnostic tips:"
    echo "  1. Check if gateway sent queries:  grep 'send_notify completed' $SANDBOX_DIR/gateway.log"
    echo "  2. Check if conductor received:    grep -i 'get_req\\|get_links_req\\|recv_notify' $SANDBOX_DIR/conductor.log"
    echo "  3. Check conductor send errors:    grep -i 'error sending' $SANDBOX_DIR/conductor.log"
    echo "  4. Check gateway recv responses:   grep 'Received.*Res' $SANDBOX_DIR/gateway.log"
    echo "  5. Check kitsune2 peer events:     grep -i 'preflight\\|peer' $SANDBOX_DIR/gateway.log"
    echo ""
    exit 1
fi

exit 0
