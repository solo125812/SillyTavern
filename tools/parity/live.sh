#!/usr/bin/env bash
# ============================================================================
# Parity Harness — Live Sequence (Non-Destructive)
# ============================================================================
#
# Runs a live parity sequence against already-running Node + Rust servers.
# By default, it only exercises endpoints marked as "safe" (no side effects).
#
# Usage:
#   bash tools/parity/live.sh
#   bash tools/parity/live.sh --group characters
#   bash tools/parity/live.sh --unsafe         # include side-effect endpoints
#   bash tools/parity/live.sh --keep-fixtures  # retain temp fixtures for debugging
#   bash tools/parity/live.sh --verbose        # verbose compare output
#
# Environment (optional):
#   PARITY_NODE_URL         (default http://127.0.0.1:8001)
#   PARITY_RUST_URL         (default http://127.0.0.1:5050)
#   PARITY_DATA_ROOT        (default ./data)
#   PARITY_USER_HANDLE      (default-user)
#   PARITY_USER_NAME        (User)
#   PARITY_USER_ADMIN       (true)
#   PARITY_SESSION_COOKIE   (if auth needed)
#   PARITY_CSRF_TOKEN       (if auth needed)
#
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

NODE_URL="${PARITY_NODE_URL:-http://127.0.0.1:8001}"
RUST_URL="${PARITY_RUST_URL:-http://127.0.0.1:5050}"
DATA_ROOT="${PARITY_DATA_ROOT:-$PROJECT_ROOT/data}"
FIXTURES_DIR="$(mktemp -d "${TMPDIR:-/tmp}/st-parity-live-XXXXXXXX")"

GROUP=""
SAFE_ONLY=true
KEEP_FIXTURES=false
VERBOSE=""
ALLOW_LIVE_WRITES=false

while [ $# -gt 0 ]; do
    case "$1" in
        --group)
            GROUP="${2:-}"
            shift 2
            ;;
        --unsafe)
            SAFE_ONLY=false
            shift
            ;;
        --allow-live-writes)
            ALLOW_LIVE_WRITES=true
            shift
            ;;
        --keep-fixtures)
            KEEP_FIXTURES=true
            shift
            ;;
        --verbose)
            VERBOSE="--verbose"
            shift
            ;;
        *)
            echo "Unknown argument: $1"
            exit 1
            ;;
    esac
done

cleanup() {
    if [ "$KEEP_FIXTURES" = true ]; then
        echo "Keeping fixtures directory: $FIXTURES_DIR"
    else
        rm -rf "$FIXTURES_DIR"
    fi
}
trap cleanup EXIT INT TERM

echo "═══════════════════════════════════════════════════"
echo "  Live Parity Sequence"
echo "═══════════════════════════════════════════════════"
echo "  Node URL:   $NODE_URL"
echo "  Rust URL:   $RUST_URL"
echo "  Data root:  $DATA_ROOT"
echo "  Fixtures:   $FIXTURES_DIR"
echo "═══════════════════════════════════════════════════"

echo ""
echo "Checking Node..."
curl -sf "$NODE_URL/version" >/dev/null || {
    echo "✗ Node not reachable at $NODE_URL"
    exit 1
}

echo "Checking Rust..."
curl -sf "$RUST_URL/health" >/dev/null || {
    echo "✗ Rust sidecar not reachable at $RUST_URL"
    exit 1
}

SAFE_FLAG=""
if [ "$SAFE_ONLY" = true ]; then
    SAFE_FLAG="--safe-only"
fi

GROUP_FLAG=""
if [ -n "$GROUP" ]; then
    GROUP_FLAG="--group $GROUP"
fi

LIVE_WRITES_FLAG=""
if [ "$ALLOW_LIVE_WRITES" = true ]; then
    LIVE_WRITES_FLAG="--allow-live-writes"
fi

if [ "$SAFE_ONLY" = true ]; then
    echo ""
    echo "Safe-only coverage:"
    node "$SCRIPT_DIR/list_safe.js" $GROUP_FLAG --summary
    if [ -n "$GROUP" ]; then
        echo "Full list: node tools/parity/list_safe.js --group $GROUP"
    else
        echo "Full list: node tools/parity/list_safe.js"
    fi
fi

echo ""
echo "Step 1: Capture baselines from Node"
PARITY_ALLOW_LIVE_DATA_ROOT=1 \
    node "$SCRIPT_DIR/capture.js" \
        --node-url "$NODE_URL" \
        --fixtures-dir "$FIXTURES_DIR" \
        --data-root "$DATA_ROOT" \
        $SAFE_FLAG \
        $LIVE_WRITES_FLAG \
        $GROUP_FLAG

echo ""
echo "Step 2: Compare against Rust"
PARITY_ALLOW_LIVE_DATA_ROOT=1 \
    node "$SCRIPT_DIR/compare.js" \
        --rust-url "$RUST_URL" \
        --fixtures-dir "$FIXTURES_DIR" \
        --data-root "$DATA_ROOT" \
        $SAFE_FLAG \
        $LIVE_WRITES_FLAG \
        $GROUP_FLAG \
        $VERBOSE
