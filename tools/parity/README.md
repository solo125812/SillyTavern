# Parity Harness

Tools for verifying behavioral parity between the Node/Express backend and the Rust (Axum) sidecar during the strangler-pattern migration.

## Overview

The parity harness captures baseline "golden" fixtures from the Node backend and then replays the same requests against the Rust sidecar, comparing status codes, response headers, body shapes, and file-system side effects.

## Directory Structure

```
tools/parity/
├── README.md          # This file
├── capture.js         # Records baseline responses from Node
├── compare.js         # Replays against Rust and diffs results
└── fixtures/          # Auto-generated golden fixtures (gitignored)
    ├── <endpoint>/
    │   ├── request.json      # Method, path, headers, body
    │   ├── response.json     # Status, headers, body
    │   └── side-effects/     # File system snapshots (if any)
    └── ...
```

## Prerequisites

- Node.js 18+
- A running SillyTavern Node server (default: `http://127.0.0.1:8000`)
- A running Rust sidecar (default: `http://127.0.0.1:5050`)
- Valid auth credentials (session cookie or basic auth)
- Data root path (default: `./data`, override with `PARITY_DATA_ROOT` or `--data-root`)
  - Safety guard: `capture.js` and `compare.js` refuse to run against the live `./data` root unless you explicitly opt in.
    Use `--allow-live-data-root` or `PARITY_ALLOW_LIVE_DATA_ROOT=1` to override.
  - Additional guard: live writes are blocked unless you pass `--allow-live-writes` or `PARITY_ALLOW_LIVE_WRITES=1`.

## Usage

### 0. One-Command Runner

The runner performs a full end-to-end check with a hermetic data root:

```bash
# Full run (build + test + capture + compare + fallback)
bash tools/parity/run.sh

# Custom ports
NODE_PORT=8011 RUST_PORT=5051 PARITY_FIXTURE_PORT=9125 bash tools/parity/run.sh

# Skip Rust build for faster re-runs
bash tools/parity/run.sh --skip-build

# Verbose compare output
bash tools/parity/run.sh --verbose
```

The runner:
1. Creates a temp data root from `tools/parity/data-root-template/`.
2. Builds/tests the Rust sidecar.
3. Starts Rust on `:5050`.
4. Starts Node on `:8001` with proxy disabled and captures baselines.
5. Compares Rust against the captured fixtures.
6. Starts Node with proxy enabled and verifies fallback when Rust is stopped.

Configuration used:
- `config.test.yaml` (Node, port 8001, CSRF disabled, proxy enabled)
- `tools/parity/data-root-template/` (minimal user settings)

Optional debug header:
- Set `rustProxy.backendHeader: true` (in `config.yaml`) or `ST_BACKEND_HEADER=1` for the Rust sidecar to emit `x-st-backend: rust` on `/version` and `/api/ping`.

### 0b. Live Parity Sequence (Non-Destructive)

If you already have Node + Rust running and want a quick, **read-only** parity check against your live data root:

```bash
# Default (safe-only endpoints, uses ./data)
bash tools/parity/live.sh

# Filter by group
bash tools/parity/live.sh --group characters

# Include side-effect endpoints (unsafe)
bash tools/parity/live.sh --unsafe

# Keep fixtures for debugging
bash tools/parity/live.sh --keep-fixtures
```

The live runner:
1. Verifies Node/Rust are reachable.
2. Captures fixtures from Node (safe-only by default).
3. Compares those fixtures against Rust.

### Safe Endpoint Report

To see which endpoints are considered safe for live parity:

```bash
node tools/parity/list_safe.js
node tools/parity/list_safe.js --group characters
node tools/parity/list_safe.js --summary
```

### Safety
`live.sh` uses `--safe-only` by default (endpoints with `hasSideEffects: false` **and no `seed` data**). Pass `--unsafe` only if you are okay with write/delete operations on the data root.
If you are running against the live `./data` root, writes are additionally blocked unless you pass `--allow-live-writes`
or set `PARITY_ALLOW_LIVE_WRITES=1`.

### 1. Capture Baseline Fixtures (Node)

Record golden responses from the Node backend:

```bash
# Capture all configured endpoints
node tools/parity/capture.js

# Capture specific endpoint group
node tools/parity/capture.js --group characters

# Custom Node server URL
node tools/parity/capture.js --node-url http://127.0.0.1:8000

# Custom fixtures output directory
node tools/parity/capture.js --fixtures-dir ./tools/parity/fixtures

# Custom data root (for side-effect snapshots)
node tools/parity/capture.js --data-root ./data

# Allow using the live data root (not recommended)
PARITY_ALLOW_LIVE_DATA_ROOT=1 node tools/parity/capture.js --data-root ./data

# Allow writes when using the live data root (dangerous)
PARITY_ALLOW_LIVE_WRITES=1 node tools/parity/capture.js --data-root ./data --allow-live-data-root --allow-live-writes

# Provide user context headers (for Rust auth)
node tools/parity/capture.js --user-handle default-user --user-name "User" --user-admin true

# Allow using the live data root (not recommended)
PARITY_ALLOW_LIVE_DATA_ROOT=1 node tools/parity/capture.js --data-root ./data
```

The capture script will:
1. Iterate through a predefined list of test endpoints.
2. Send requests to the Node server.
3. Record the status code, response headers, and body.
4. Snapshot any file-system side effects (for write operations).
5. Store everything under `tools/parity/fixtures/<endpoint>/`.

For endpoints that require external downloads (e.g. `/api/assets/download`), the harness auto-starts a local fixture server at `http://127.0.0.1:9123` and replaces `{{FIXTURE_URL}}` in request bodies.

### 2. Compare Against Rust

Replay the same requests against the Rust sidecar and diff the results:

```bash
# Compare all captured fixtures
node tools/parity/compare.js

# Compare specific endpoint group
node tools/parity/compare.js --group characters

# Custom Rust sidecar URL
node tools/parity/compare.js --rust-url http://127.0.0.1:5050

# Show detailed diffs
node tools/parity/compare.js --verbose

# Custom data root (for side-effect comparisons)
node tools/parity/compare.js --data-root ./data

# Allow using the live data root (not recommended)
PARITY_ALLOW_LIVE_DATA_ROOT=1 node tools/parity/compare.js --data-root ./data

# Allow writes when using the live data root (dangerous)
PARITY_ALLOW_LIVE_WRITES=1 node tools/parity/compare.js --data-root ./data --allow-live-data-root --allow-live-writes

# Provide user context headers (required for Rust auth)
node tools/parity/compare.js --user-handle default-user --user-name "User" --user-admin true

# Allow using the live data root (not recommended)
PARITY_ALLOW_LIVE_DATA_ROOT=1 node tools/parity/compare.js --data-root ./data
```

The compare script will:
1. Load each captured fixture from `tools/parity/fixtures/`.
2. Send the same request to the Rust sidecar.
3. Compare status codes (must match exactly).
4. Compare response headers (configurable subset).
5. Compare body shapes (JSON structure, not exact values for timestamps etc.).
6. Compare file-system outputs for write operations.
7. Report PASS/FAIL per endpoint with detailed diffs on failure.

### 3. Fixture Storage

Fixtures are stored in `tools/parity/fixtures/` and are **gitignored** by default since they may contain user-specific data. To share fixtures across environments, copy the directory or use the `--fixtures-dir` flag.

Each fixture contains:
- `request.json` — The exact request sent (method, path, headers, body)
- `response.json` — The captured response (status, headers, body)
- `side-effects/` — Directory containing file-system snapshots before/after write operations
  
`request.json` may also include:
- `query` — query parameters
- `requestType` — `json` or `multipart`
- `multipart` — fields/files for multipart requests (base64-encoded payloads)
- `seed` — files/dirs to seed before the request (`files`, `dirs`, `remove`, `clear`)
- `compareBody` — `shape` (default) or `exact`
- `sideEffects` — comparison mode/ignore patterns

## Adding New Endpoints

To add a new endpoint to the parity harness, add an entry to the `ENDPOINTS` array in `capture.js`:

```js
{
    group: 'characters',
    method: 'POST',
    path: '/api/characters/all',
    headers: { 'content-type': 'application/json' },
    body: {},
    hasSideEffects: false,
}
```

Multipart example:
```js
{
    group: 'avatars',
    method: 'POST',
    path: '/api/avatars/upload',
    requestType: 'multipart',
    multipart: {
        fields: { overwrite_name: 'avatar.png' },
        files: [
            {
                field: 'avatar',
                filename: 'avatar.png',
                contentType: 'image/png',
                dataBase64: '...'
            }
        ]
    },
    hasSideEffects: true,
    sideEffects: { mode: 'pathsOnly' },
    seed: {
        files: [{ path: '{{USER_HANDLE}}/User Avatars/avatar.png', contentsBase64: '...' }]
    }
}
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PARITY_NODE_URL` | `http://127.0.0.1:8000` | Node server base URL |
| `NODE_PORT` | `8001` | (run.sh) Node port for the one-command runner |
| `RUST_PORT` | `5050` | (run.sh) Rust sidecar port for the one-command runner |
| `PARITY_FIXTURE_PORT` | `9123` | Local fixture server port (capture/compare) |
| `PARITY_RUST_URL` | `http://127.0.0.1:5050` | Rust sidecar base URL |
| `PARITY_FIXTURES_DIR` | `./tools/parity/fixtures` | Fixtures storage path |
| `PARITY_DATA_ROOT` | `./data` | Data root used for side-effect snapshots |
| `PARITY_USER_HANDLE` | _(none)_ | User handle header for Rust auth |
| `PARITY_USER_NAME` | _(none)_ | User display name header for Rust auth |
| `PARITY_USER_ADMIN` | _(none)_ | User admin flag header (`true`/`false`) |
| `PARITY_CSRF_TOKEN` | _(none)_ | CSRF token for authenticated requests |
| `PARITY_SESSION_COOKIE` | _(none)_ | Session cookie for authenticated requests |
# Safe-only mode (skip endpoints with side effects)
node tools/parity/capture.js --safe-only

# Safe-only mode (skip fixtures that have side effects)
node tools/parity/compare.js --safe-only
