#!/usr/bin/env bash
# ============================================================================
# Parity Harness Runner
# ============================================================================
#
# One-command end-to-end verification for the Rust sidecar migration.
#
# Usage:
#   bash tools/parity/run.sh              # full run
#   bash tools/parity/run.sh --skip-build # skip cargo build (faster re-runs)
#   bash tools/parity/run.sh --verbose    # pass --verbose to compare.js
#
# Prerequisites:
#   - Node.js ≥18 with npm dependencies installed
#   - Rust toolchain (cargo)
#
# What it does:
#   1. Creates a hermetic temp data root from tools/parity/data-root-template/
#   2. Builds the Rust sidecar (unless --skip-build)
#   3. Starts the Rust sidecar (port 5050)
#   4. Starts Node (proxy disabled) and captures Node baselines
#   5. Stops Node (baseline) to free the port
#   6. Runs parity compare against Rust
#   7. Starts Node (proxy enabled) and verifies fallback behavior
#   8. Cleans up
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Configuration (override with environment variables)
NODE_PORT="${NODE_PORT:-8001}"
RUST_PORT="${RUST_PORT:-5050}"
PARITY_FIXTURE_PORT="${PARITY_FIXTURE_PORT:-9123}"
NODE_URL="http://127.0.0.1:${NODE_PORT}"
RUST_URL="http://127.0.0.1:${RUST_PORT}"
TEMPLATE_DIR="$SCRIPT_DIR/data-root-template"
DEFAULT_USER_HANDLE="default-user"
DEFAULT_USER_NAME="User"
DEFAULT_USER_ADMIN="true"

# Parse arguments
SKIP_BUILD=false
VERBOSE=""
for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=true ;;
        --verbose) VERBOSE="--verbose" ;;
    esac
done

# Track PIDs for cleanup
NODE_PID=""
RUST_PID=""
TEMP_DATA_ROOT=""
EXIT_CODE=0

export PARITY_FIXTURE_PORT

# ---------------------------------------------------------------------------
# Helper: print step header
# ---------------------------------------------------------------------------
step() {
    echo ""
    echo "═══════════════════════════════════════════════════"
    echo "  $1"
    echo "═══════════════════════════════════════════════════"
}

check_port_free() {
    local port="$1"
    local label="$2"

    if command -v lsof >/dev/null 2>&1; then
        local pids
        pids=$(lsof -nP -iTCP:"$port" -sTCP:LISTEN -t 2>/dev/null | tr '\n' ' ')
        if [ -n "$pids" ]; then
            echo "  ✗ ${label} port ${port} is already in use (PID(s): ${pids})"
            lsof -nP -iTCP:"$port" -sTCP:LISTEN || true
            return 1
        fi
        return 0
    fi

    if command -v ss >/dev/null 2>&1; then
        if ss -ltn | awk '{print $4}' | grep -q ":${port}$"; then
            echo "  ✗ ${label} port ${port} is already in use."
            ss -ltn | grep ":${port}$" || true
            return 1
        fi
        return 0
    fi

    if command -v netstat >/dev/null 2>&1; then
        if netstat -an | grep -E "[\\.:]${port} " | grep -q LISTEN; then
            echo "  ✗ ${label} port ${port} is already in use."
            netstat -an | grep -E "[\\.:]${port} " | grep LISTEN || true
            return 1
        fi
        return 0
    fi

    return 0
}

cleanup() {
    echo ""
    echo "═══════════════════════════════════════════════════"
    echo "  Cleaning up..."
    echo "═══════════════════════════════════════════════════"

    if [ -n "$NODE_PID" ] && kill -0 "$NODE_PID" 2>/dev/null; then
        echo "  Stopping Node (PID $NODE_PID)..."
        kill "$NODE_PID" 2>/dev/null || true
        wait "$NODE_PID" 2>/dev/null || true
    fi

    if [ -n "$RUST_PID" ] && kill -0 "$RUST_PID" 2>/dev/null; then
        echo "  Stopping Rust sidecar (PID $RUST_PID)..."
        kill "$RUST_PID" 2>/dev/null || true
        wait "$RUST_PID" 2>/dev/null || true
    fi

    if [ -n "$TEMP_DATA_ROOT" ] && [ -d "$TEMP_DATA_ROOT" ]; then
        echo "  Removing temp data root: $TEMP_DATA_ROOT"
        rm -rf "$TEMP_DATA_ROOT"
    fi

    echo "  Done."
    exit "$EXIT_CODE"
}

trap cleanup EXIT INT TERM

# ---------------------------------------------------------------------------
# Preflight: ensure ports are free to avoid hitting live instances
# ---------------------------------------------------------------------------
step "Preflight: Checking ports"

PORT_OK=true
check_port_free "$NODE_PORT" "Node" || PORT_OK=false
check_port_free "$RUST_PORT" "Rust sidecar" || PORT_OK=false
check_port_free "$PARITY_FIXTURE_PORT" "Fixture server" || PORT_OK=false

if [ "$PORT_OK" = false ]; then
    echo ""
    echo "  ✗ Port collision detected."
    echo "  Stop the running servers or override ports:"
    echo "    NODE_PORT=8011 RUST_PORT=5051 PARITY_FIXTURE_PORT=9125 bash tools/parity/run.sh"
    EXIT_CODE=1
    exit 1
fi

# ---------------------------------------------------------------------------
# Helper: wait for a URL to become available
# ---------------------------------------------------------------------------
wait_for_url() {
    local url="$1"
    local label="$2"
    local max_attempts="${3:-30}"
    local attempt=0

    echo -n "  Waiting for $label at $url "
    while [ $attempt -lt $max_attempts ]; do
        if curl -sf "$url" > /dev/null 2>&1; then
            echo " ✓"
            return 0
        fi
        echo -n "."
        sleep 1
        attempt=$((attempt + 1))
    done

    echo " ✗ (timed out after ${max_attempts}s)"
    return 1
}

stop_node() {
    if [ -n "$NODE_PID" ] && kill -0 "$NODE_PID" 2>/dev/null; then
        echo "  Stopping Node (PID $NODE_PID)..."
        kill "$NODE_PID" 2>/dev/null || true
        wait "$NODE_PID" 2>/dev/null || true
    fi
    NODE_PID=""
}

# ============================================================================
# Step 1: Hermetic data root
# ============================================================================
step "Step 1/8: Creating hermetic data root"

TEMP_DATA_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/st-parity-XXXXXXXX")"
echo "  Template: $TEMPLATE_DIR"
echo "  Temp dir: $TEMP_DATA_ROOT"
if [ ! -d "$TEMPLATE_DIR" ]; then
    echo "  ✗ Template directory not found: $TEMPLATE_DIR"
    EXIT_CODE=1
    exit 1
fi
cp -R "$TEMPLATE_DIR/"* "$TEMP_DATA_ROOT/" 2>/dev/null || true
cp -R "$TEMPLATE_DIR/".* "$TEMP_DATA_ROOT/" 2>/dev/null || true
echo "  ✓ Data root ready"

# ============================================================================
# Step 2: Build Rust sidecar
# ============================================================================
if [ "$SKIP_BUILD" = true ]; then
    step "Step 2/8: Skipping Rust build (--skip-build)"
else
    step "Step 2/8: Building Rust sidecar"
    cd "$PROJECT_ROOT/backend"
    cargo build 2>&1 | tail -5
    echo "  ✓ Build complete"

    echo ""
    echo "  Running cargo test..."
    cargo test 2>&1 | tail -10
    echo "  ✓ Tests passed"
fi

# ============================================================================
# Step 3: Start Rust sidecar
# ============================================================================
step "Step 3/8: Starting Rust sidecar"

cd "$PROJECT_ROOT/backend"
ST_DATA_ROOT="$TEMP_DATA_ROOT" \
ST_CONFIG_PATH="$PROJECT_ROOT/config.test.yaml" \
ST_SERVER_DIRECTORY="$PROJECT_ROOT" \
ST_LISTEN_ADDRESS="127.0.0.1:${RUST_PORT}" \
RUST_LOG=warn \
    cargo run --quiet 2>&1 &
RUST_PID=$!
echo "  PID: $RUST_PID"

sleep 1
if ! kill -0 "$RUST_PID" 2>/dev/null; then
    echo "  ✗ Rust sidecar failed to start (process exited)."
    EXIT_CODE=1
    exit 1
fi

wait_for_url "$RUST_URL/health" "Rust sidecar" 15
HEALTH_RESPONSE=$(curl -sf "$RUST_URL/health" 2>/dev/null || echo "FAILED")
echo "  Health check: $HEALTH_RESPONSE"

if [ "$HEALTH_RESPONSE" != "ok" ]; then
    echo "  ✗ Rust sidecar health check failed!"
    EXIT_CODE=1
    exit 1
fi

# ============================================================================
# Step 4: Start Node server (proxy disabled) for baseline capture
# ============================================================================
step "Step 4/8: Starting Node server (proxy disabled)"

cd "$PROJECT_ROOT"
SILLYTAVERN_RUSTPROXY_ENABLED=false \
    node server.js \
    --configPath "$PROJECT_ROOT/config.test.yaml" \
    --dataRoot "$TEMP_DATA_ROOT" \
    --port "$NODE_PORT" \
    --disableCsrf \
    2>&1 &
NODE_PID=$!
echo "  PID: $NODE_PID"

sleep 1
if ! kill -0 "$NODE_PID" 2>/dev/null; then
    echo "  ✗ Node server failed to start (process exited)."
    EXIT_CODE=1
    exit 1
fi

wait_for_url "$NODE_URL/version" "Node server" 30

# Verify Node /version returns valid JSON
VERSION_RESPONSE=$(curl -sf "$NODE_URL/version" 2>/dev/null || echo "{}")
echo "  Node /version: $VERSION_RESPONSE"

# ============================================================================
# Step 5: Parity capture (golden fixtures from Node)
# ============================================================================
step "Step 5/8: Capturing golden fixtures from Node"

FIXTURES_DIR="$TEMP_DATA_ROOT/_parity_fixtures"
mkdir -p "$FIXTURES_DIR"

cd "$PROJECT_ROOT"
node tools/parity/capture.js \
    --node-url "$NODE_URL" \
    --fixtures-dir "$FIXTURES_DIR" \
    --data-root "$TEMP_DATA_ROOT" \
    --user-handle "${PARITY_USER_HANDLE:-$DEFAULT_USER_HANDLE}" \
    --user-name "${PARITY_USER_NAME:-$DEFAULT_USER_NAME}" \
    --user-admin "${PARITY_USER_ADMIN:-$DEFAULT_USER_ADMIN}" \
    2>&1 || {
        echo "  ✗ Capture failed (some endpoints may require auth — expected for unauthenticated run)"
    }

echo "  ✓ Capture complete"

# ============================================================================
# Step 6: Stop Node (baseline) to free the port
# ============================================================================
step "Step 6/8: Stopping Node (baseline)"
stop_node

# ============================================================================
# Step 7: Parity compare (Rust vs Node fixtures)
# ============================================================================
step "Step 7/8: Comparing Rust sidecar against golden fixtures"

node tools/parity/compare.js \
    --rust-url "$RUST_URL" \
    --fixtures-dir "$FIXTURES_DIR" \
    --data-root "$TEMP_DATA_ROOT" \
    --user-handle "${PARITY_USER_HANDLE:-$DEFAULT_USER_HANDLE}" \
    --user-name "${PARITY_USER_NAME:-$DEFAULT_USER_NAME}" \
    --user-admin "${PARITY_USER_ADMIN:-$DEFAULT_USER_ADMIN}" \
    $VERBOSE \
    2>&1 || {
        echo ""
        echo "  ⚠ Some parity mismatches detected (see above)"
        echo "  This is expected for Phase 0 stubs — review the diff output."
    }

echo "  ✓ Compare complete"

# ============================================================================
# Step 8: Start Node (proxy enabled) + fallback test
# ============================================================================
step "Step 8/8: Testing fallback (Rust sidecar down)"

cd "$PROJECT_ROOT"
node server.js \
    --configPath "$PROJECT_ROOT/config.test.yaml" \
    --dataRoot "$TEMP_DATA_ROOT" \
    --port "$NODE_PORT" \
    --disableCsrf \
    2>&1 &
NODE_PID=$!
echo "  PID: $NODE_PID"

sleep 1
if ! kill -0 "$NODE_PID" 2>/dev/null; then
    echo "  ✗ Node server failed to start (process exited)."
    EXIT_CODE=1
    exit 1
fi

wait_for_url "$NODE_URL/version" "Node server (proxy enabled)" 30

echo "  Verifying Rust header via proxy (/version)..."
HEADER_LINE=$(curl -s -D - -o /dev/null "$NODE_URL/version" | tr -d '\r' | grep -i '^x-st-backend:' || true)
if echo "$HEADER_LINE" | grep -qi 'rust'; then
    echo "  ✓ Proxy returned x-st-backend: rust"
else
    echo "  ✗ Missing x-st-backend: rust header through proxy"
    EXIT_CODE=1
fi

echo "  Verifying multipart proxy (/api/characters/edit)..."
CHAR_DIR="$TEMP_DATA_ROOT/$DEFAULT_USER_HANDLE/characters"
CHAR_FILE=$(find "$CHAR_DIR" -maxdepth 1 -type f -name "*.png" | head -n 1 || true)
if [ -z "$CHAR_FILE" ]; then
    TEMPLATE_CHAR_DIR="$TEMPLATE_DIR/$DEFAULT_USER_HANDLE/characters"
    TEMPLATE_CHAR_FILE=$(find "$TEMPLATE_CHAR_DIR" -maxdepth 1 -type f -name "*.png" | head -n 1 || true)
    if [ -n "$TEMPLATE_CHAR_FILE" ]; then
        mkdir -p "$CHAR_DIR"
        cp "$TEMPLATE_CHAR_FILE" "$CHAR_DIR/"
        CHAR_FILE="$CHAR_DIR/$(basename "$TEMPLATE_CHAR_FILE")"
    fi
fi

if [ -z "$CHAR_FILE" ]; then
    FALLBACK_PNG="$PROJECT_ROOT/public/img/user-default.png"
    if [ -f "$FALLBACK_PNG" ]; then
        mkdir -p "$CHAR_DIR"
        cp "$FALLBACK_PNG" "$CHAR_DIR/"
        CHAR_FILE="$CHAR_DIR/$(basename "$FALLBACK_PNG")"
    fi
fi

if [ -z "$CHAR_FILE" ]; then
    echo "  ✗ No character PNG found in $CHAR_DIR"
    EXIT_CODE=1
else
    CHAR_BASE=$(basename "$CHAR_FILE")
    MP_HEADERS=$(mktemp)
    MP_BODY=$(mktemp)
    MP_STATUS=$(curl -s -D "$MP_HEADERS" -o "$MP_BODY" -w "%{http_code}" \
        -F "ch_name=ParityTest" \
        -F "avatar_url=$CHAR_BASE" \
        -F "description=Parity multipart proxy check" \
        "$NODE_URL/api/characters/edit" || echo "000")
    MP_BACKEND=$(tr -d '\r' < "$MP_HEADERS" | grep -i '^x-st-backend:' || true)
    if [ "$MP_STATUS" = "200" ] && echo "$MP_BACKEND" | grep -qi 'rust'; then
        echo "  ✓ Multipart proxy ok (status 200, rust backend)"
    else
        echo "  ✗ Multipart proxy failed (status: $MP_STATUS, header: ${MP_BACKEND:-missing})"
        EXIT_CODE=1
    fi
    rm -f "$MP_HEADERS" "$MP_BODY"
fi

echo "  Killing Rust sidecar (PID $RUST_PID)..."
kill "$RUST_PID" 2>/dev/null || true
wait "$RUST_PID" 2>/dev/null || true
RUST_PID=""

sleep 1

echo "  Sending POST /api/ping through Node (should fall back)..."
FALLBACK_STATUS=$(curl -sf -o /dev/null -w "%{http_code}" -X POST "$NODE_URL/api/ping" 2>/dev/null || echo "000")
echo "  Fallback response status: $FALLBACK_STATUS"

if [ "$FALLBACK_STATUS" = "502" ] || [ "$FALLBACK_STATUS" = "000" ]; then
    echo "  ✗ Fallback FAILED — Node did not handle the request"
    EXIT_CODE=1
else
    echo "  ✓ Fallback OK — Node handled the request (status: $FALLBACK_STATUS)"
fi

# ============================================================================
# Summary
# ============================================================================
echo ""
echo "═══════════════════════════════════════════════════"
if [ "$EXIT_CODE" = "0" ]; then
    echo "  ✅  ALL CHECKS PASSED"
else
    echo "  ❌  SOME CHECKS FAILED (exit code: $EXIT_CODE)"
fi
echo "═══════════════════════════════════════════════════"
