/**
 * Parity Harness — Compare
 *
 * Replays captured golden fixtures against the Rust sidecar and compares
 * status codes, response headers, body shapes, and file-system side effects.
 *
 * Usage:
 *   node tools/parity/compare.js [--group <name>] [--rust-url <url>] [--fixtures-dir <path>] [--verbose]
 *
 * @module tools/parity/compare
 */

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import http from 'node:http';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

const { values: args } = parseArgs({
    options: {
        'group': { type: 'string', default: '' },
        'rust-url': { type: 'string', default: process.env.PARITY_RUST_URL || 'http://127.0.0.1:5050' },
        'fixtures-dir': { type: 'string', default: process.env.PARITY_FIXTURES_DIR || './tools/parity/fixtures' },
        'data-root': { type: 'string', default: process.env.PARITY_DATA_ROOT || './data' },
        'allow-live-data-root': { type: 'boolean', default: false },
        'allow-live-writes': { type: 'boolean', default: false },
        'safe-only': { type: 'boolean', default: false },
        'allow-network': { type: 'boolean', default: false },
        'user-handle': { type: 'string', default: process.env.PARITY_USER_HANDLE || '' },
        'user-name': { type: 'string', default: process.env.PARITY_USER_NAME || '' },
        'user-admin': { type: 'string', default: process.env.PARITY_USER_ADMIN || '' },
        'csrf-token': { type: 'string', default: process.env.PARITY_CSRF_TOKEN || '' },
        'session-cookie': { type: 'string', default: process.env.PARITY_SESSION_COOKIE || '' },
        'verbose': { type: 'boolean', default: false },
    },
});

const RUST_URL = args['rust-url'];
const FIXTURES_DIR = args['fixtures-dir'];
const DATA_ROOT = args['data-root'];
const ALLOW_LIVE_DATA_ROOT = args['allow-live-data-root'];
const ALLOW_LIVE_WRITES = args['allow-live-writes'];
const SAFE_ONLY = args['safe-only'];
const ALLOW_NETWORK = args['allow-network'];
const USER_HANDLE = args['user-handle'];
const USER_NAME = args['user-name'];
const USER_ADMIN = args['user-admin'];
const CSRF_TOKEN = args['csrf-token'];
const SESSION_COOKIE = args['session-cookie'];
const FILTER_GROUP = args['group'];
const VERBOSE = args['verbose'];

const ENV_ALLOW_LIVE_DATA_ROOT = (() => {
    const raw = process.env.PARITY_ALLOW_LIVE_DATA_ROOT;
    if (!raw) return false;
    return ['1', 'true', 'yes', 'on'].includes(raw.toLowerCase());
})();
const ENV_ALLOW_LIVE_WRITES = (() => {
    const raw = process.env.PARITY_ALLOW_LIVE_WRITES;
    if (!raw) return false;
    return ['1', 'true', 'yes', 'on'].includes(raw.toLowerCase());
})();
const ENV_ALLOW_NETWORK = (() => {
    const raw = process.env.PARITY_ALLOW_NETWORK;
    if (!raw) return false;
    return ['1', 'true', 'yes', 'on'].includes(raw.toLowerCase());
})();

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(SCRIPT_DIR, '../..');
const LIVE_DATA_ROOT = path.resolve(PROJECT_ROOT, 'data');
const RESOLVED_DATA_ROOT = path.resolve(DATA_ROOT);
const LIVE_DATA_ROOT_PREFIX = LIVE_DATA_ROOT + path.sep;
const IS_LIVE_DATA_ROOT =
    RESOLVED_DATA_ROOT === LIVE_DATA_ROOT || RESOLVED_DATA_ROOT.startsWith(LIVE_DATA_ROOT_PREFIX);
const LIVE_WRITES_ALLOWED = ALLOW_LIVE_WRITES || ENV_ALLOW_LIVE_WRITES;

if (IS_LIVE_DATA_ROOT && !(ALLOW_LIVE_DATA_ROOT || ENV_ALLOW_LIVE_DATA_ROOT)) {
    console.error('✗ Refusing to run parity compare against live data root.');
    console.error(`  Data root: ${RESOLVED_DATA_ROOT}`);
    console.error('  Use a temp data root (recommended), or pass --allow-live-data-root to override.');
    console.error('  Example: PARITY_DATA_ROOT=/tmp/st-parity node tools/parity/compare.js ...');
    process.exit(1);
}

const FIXTURE_SERVER_PORT = Number.parseInt(process.env.PARITY_FIXTURE_PORT || '9123', 10);
const FIXTURE_SERVER_URL = `http://127.0.0.1:${FIXTURE_SERVER_PORT}`;
const EFFECTIVE_USER_HANDLE = USER_HANDLE || 'default-user';
const UPLOADS_DIR_NAME = '_uploads';
const CONTEXT = {};

// Headers to compare (lowercase). Others are ignored for parity purposes.
const COMPARED_HEADERS = new Set([
    'content-type',
    'x-custom-content-type',
    'clear-site-data',
]);

/**
 * Normalize header values for comparison.
 * Currently normalizes content-type by stripping parameters (e.g. charset).
 * @param {string} header
 * @param {string} value
 * @returns {string}
 */
function normalizeHeaderValue(header, value) {
    if (!value) return value;
    if (header === 'content-type') {
        return value.split(';')[0].trim().toLowerCase();
    }
    return value;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Replace template tokens in strings.
 * @param {string} value
 * @returns {string}
 */
function replaceTokens(value) {
    let resolved = value
        .replaceAll('{{USER_HANDLE}}', EFFECTIVE_USER_HANDLE)
        .replaceAll('{{FIXTURE_URL}}', FIXTURE_SERVER_URL);
    for (const [key, val] of Object.entries(CONTEXT)) {
        if (val !== undefined && val !== null) {
            resolved = resolved.replaceAll(`{{${key}}}`, String(val));
        }
    }
    return resolved;
}

/**
 * Deep replace tokens in objects/arrays.
 * @param {any} value
 * @returns {any}
 */
function replaceTokensDeep(value) {
    if (typeof value === 'string') {
        return replaceTokens(value);
    }
    if (Array.isArray(value)) {
        return value.map(replaceTokensDeep);
    }
    if (value && typeof value === 'object') {
        const out = {};
        for (const [key, val] of Object.entries(value)) {
            out[key] = replaceTokensDeep(val);
        }
        return out;
    }
    return value;
}

/**
 * Override request fields with dynamic context (e.g., data-maid tokens).
 * @param {Object} request
 */
function applyContextOverrides(request) {
    if (!request || typeof request !== 'object') return;

    if (typeof request.path === 'string' && request.path.startsWith('/api/data-maid/')) {
        if (request.path.endsWith('/view')) {
            if (CONTEXT.DATA_MAID_TOKEN && CONTEXT.DATA_MAID_HASH) {
                request.query = {
                    ...(request.query || {}),
                    token: CONTEXT.DATA_MAID_TOKEN,
                    hash: CONTEXT.DATA_MAID_HASH,
                };
            }
        }
        if (request.path.endsWith('/delete')) {
            if (CONTEXT.DATA_MAID_TOKEN && CONTEXT.DATA_MAID_HASH) {
                request.body = {
                    ...(request.body || {}),
                    token: CONTEXT.DATA_MAID_TOKEN,
                    hashes: [CONTEXT.DATA_MAID_HASH],
                };
            }
        }
        if (request.path.endsWith('/finalize')) {
            if (CONTEXT.DATA_MAID_TOKEN) {
                request.body = {
                    ...(request.body || {}),
                    token: CONTEXT.DATA_MAID_TOKEN,
                };
            }
        }
    }
}

/**
 * Capture dynamic context values from a response.
 * @param {Object} request
 * @param {{status: number, headers: Record<string, string>, body: any}} actual
 */
function captureContextFromResponse(request, actual) {
    if (!request?.capture) return;

    if (request.capture.dataMaid) {
        const body = actual?.body;
        if (body && typeof body === 'object') {
            if (typeof body.token === 'string' && body.token.length > 0) {
                CONTEXT.DATA_MAID_TOKEN = body.token;
            }
            const report = body.report || {};
            const categories = [
                'images',
                'files',
                'chats',
                'groupChats',
                'avatarThumbnails',
                'backgroundThumbnails',
                'personaThumbnails',
                'chatBackups',
                'settingsBackups',
            ];
            let hash = null;
            for (const key of categories) {
                const items = report[key];
                if (Array.isArray(items) && items.length > 0) {
                    const candidate = items[0]?.hash;
                    if (typeof candidate === 'string' && candidate.length > 0) {
                        hash = candidate;
                        break;
                    }
                }
            }
            if (hash) {
                CONTEXT.DATA_MAID_HASH = hash;
            }
        }
    }
}

/**
 * Ensure the global uploads directory exists for multer.
 */
function ensureUploadsDir() {
    const uploadsPath = path.join(DATA_ROOT, UPLOADS_DIR_NAME);
    fs.mkdirSync(uploadsPath, { recursive: true });
}

/**
 * Append a file part to a FormData instance.
 * @param {FormData} form
 * @param {{field: string, filename?: string, contentType?: string, dataBase64?: string}} file
 */
function appendFormFile(form, file) {
    const bytes = Buffer.from(file.dataBase64 || '', 'base64');
    const filename = file.filename || 'file.bin';
    const contentType = file.contentType || 'application/octet-stream';

    if (typeof File === 'function') {
        const fileObj = new File([bytes], filename, { type: contentType });
        form.append(file.field, fileObj);
        return;
    }

    if (typeof Blob === 'function') {
        const blob = new Blob([bytes], { type: contentType });
        form.append(file.field, blob, filename);
        return;
    }

    form.append(file.field, bytes, filename);
}

/**
 * Apply seed files/dirs before a request.
 * @param {any} seed
 */
function applySeed(seed) {
    if (!seed) return;

    const removePath = (target) => {
        const resolved = path.join(DATA_ROOT, replaceTokens(target));
        if (fs.existsSync(resolved)) {
            fs.rmSync(resolved, { recursive: true, force: true });
        }
    };

    const clearPath = (target) => {
        const resolved = path.join(DATA_ROOT, replaceTokens(target));
        if (!fs.existsSync(resolved)) {
            fs.mkdirSync(resolved, { recursive: true });
            return;
        }
        if (!fs.statSync(resolved).isDirectory()) {
            fs.rmSync(resolved, { force: true });
            fs.mkdirSync(resolved, { recursive: true });
            return;
        }
        for (const entry of fs.readdirSync(resolved)) {
            fs.rmSync(path.join(resolved, entry), { recursive: true, force: true });
        }
    };

    if (Array.isArray(seed.remove)) {
        for (const target of seed.remove) {
            removePath(target);
        }
    }

    if (Array.isArray(seed.clear)) {
        for (const target of seed.clear) {
            clearPath(target);
        }
    }

    if (Array.isArray(seed.dirs)) {
        for (const dir of seed.dirs) {
            const resolved = path.join(DATA_ROOT, replaceTokens(dir));
            fs.mkdirSync(resolved, { recursive: true });
        }
    }

    if (Array.isArray(seed.files)) {
        for (const file of seed.files) {
            const resolved = path.join(DATA_ROOT, replaceTokens(file.path));
            fs.mkdirSync(path.dirname(resolved), { recursive: true });
            const contents = file.contentsBase64
                ? Buffer.from(file.contentsBase64, 'base64')
                : Buffer.from(file.contents ?? '', file.encoding || 'utf8');
            fs.writeFileSync(resolved, contents);
            if (file.mtimeMs) {
                const mtimeSeconds = file.mtimeMs / 1000;
                fs.utimesSync(resolved, mtimeSeconds, mtimeSeconds);
            }
        }
    }
}

/**
 * Start a tiny fixture HTTP server for download tests.
 * @returns {Promise<import('node:http').Server>}
 */
function startFixtureServer() {
    const server = http.createServer((req, res) => {
        if (req.url === '/asset.bin') {
            res.writeHead(200, { 'content-type': 'application/octet-stream' });
            res.end('fixture-asset');
            return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
    });

    return new Promise((resolve, reject) => {
        server.on('error', reject);
        server.listen(FIXTURE_SERVER_PORT, '127.0.0.1', () => resolve(server));
    });
}

/**
 * Builds request headers including auth tokens.
 * @param {Record<string, string>} [extra]
 * @param {string} [requestType]
 * @returns {Record<string, string>}
 */
function buildHeaders(extra = {}, requestType = 'json') {
    const headers = {
        'accept': 'application/json',
        ...extra,
    };

    if (requestType === 'json') {
        headers['content-type'] = 'application/json';
    }

    if (CSRF_TOKEN) {
        headers['x-csrf-token'] = CSRF_TOKEN;
    }

    if (SESSION_COOKIE) {
        headers['cookie'] = SESSION_COOKIE;
    }

    if (USER_HANDLE) {
        headers['x-st-user-handle'] = USER_HANDLE;
    }

    if (USER_NAME) {
        headers['x-st-user-name'] = USER_NAME;
    }

    if (USER_ADMIN) {
        headers['x-st-user-admin'] = USER_ADMIN;
    }

    return headers;
}

/**
 * Computes a SHA-256 hash of a file.
 * @param {string} filePath
 * @returns {Promise<string>}
 */
function hashFile(filePath) {
    return new Promise((resolve, reject) => {
        const hash = crypto.createHash('sha256');
        const stream = fs.createReadStream(filePath);
        stream.on('data', chunk => hash.update(chunk));
        stream.on('end', () => resolve(hash.digest('hex')));
        stream.on('error', reject);
    });
}

/**
 * Recursively lists all files under a root directory.
 * @param {string} root
 * @returns {string[]} Absolute file paths
 */
function listFiles(root) {
    const entries = fs.readdirSync(root, { withFileTypes: true });
    const files = [];
    for (const entry of entries) {
        const fullPath = path.join(root, entry.name);
        if (entry.isDirectory()) {
            files.push(...listFiles(fullPath));
        } else if (entry.isFile()) {
            files.push(fullPath);
        }
    }
    return files;
}

/**
 * Creates a snapshot of the data root (relative file metadata + hashes).
 * @param {string} root
 * @returns {Promise<{root: string, capturedAt: string, files: Record<string, {size: number, mtimeMs: number, sha256: string}>}>}
 */
async function snapshotDataRoot(root) {
    const absoluteRoot = path.resolve(root);
    const files = listFiles(absoluteRoot).sort();
    const snapshot = {
        root: absoluteRoot,
        capturedAt: new Date().toISOString(),
        files: {},
    };

    for (const filePath of files) {
        const relPath = path.relative(absoluteRoot, filePath);
        const stat = fs.statSync(filePath);
        const sha256 = await hashFile(filePath);
        snapshot.files[relPath] = {
            size: stat.size,
            mtimeMs: stat.mtimeMs,
            sha256,
        };
    }

    return snapshot;
}

/**
 * Extracts the "shape" of a JSON value for structural comparison.
 * Replaces leaf values with their type names so that timestamps,
 * random IDs, etc. don't cause false failures.
 *
 * @param {any} value
 * @returns {any} Shape descriptor
 */
function extractShape(value) {
    if (value === null || value === undefined) {
        return null;
    }

    if (Array.isArray(value)) {
        if (value.length === 0) return '[]';
        // Sample first element's shape for arrays
        return [extractShape(value[0]), `...${value.length} items`];
    }

    if (typeof value === 'object') {
        const shape = {};
        for (const [k, v] of Object.entries(value)) {
            shape[k] = extractShape(v);
        }
        return shape;
    }

    return typeof value;
}

/**
 * Deep equality check for JSON-compatible values.
 * @param {any} a
 * @param {any} b
 * @returns {boolean}
 */
function deepEqual(a, b) {
    if (a === b) return true;
    if (a === null || b === null) return a === b;
    if (typeof a !== typeof b) return false;
    if (Array.isArray(a)) {
        if (!Array.isArray(b) || a.length !== b.length) return false;
        for (let i = 0; i < a.length; i++) {
            if (!deepEqual(a[i], b[i])) return false;
        }
        return true;
    }
    if (typeof a === 'object') {
        const aKeys = Object.keys(a);
        const bKeys = Object.keys(b);
        if (aKeys.length !== bKeys.length) return false;
        for (const key of aKeys) {
            if (!Object.prototype.hasOwnProperty.call(b, key)) return false;
            if (!deepEqual(a[key], b[key])) return false;
        }
        return true;
    }
    return false;
}

/**
 * Deep-compares two shape descriptors.
 * @param {any} expected
 * @param {any} actual
 * @param {string} [path]
 * @returns {string[]} List of differences
 */
function diffShapes(expected, actual, currentPath = '$', options = {}) {
    const diffs = [];

    if (expected === null && actual === null) return diffs;
    if (expected === null || actual === null) {
        diffs.push(`${currentPath}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
        return diffs;
    }

    if (typeof expected !== typeof actual) {
        diffs.push(`${currentPath}: type mismatch — expected ${typeof expected}, got ${typeof actual}`);
        return diffs;
    }

    if (typeof expected === 'string') {
        if (expected !== actual) {
            diffs.push(`${currentPath}: expected "${expected}", got "${actual}"`);
        }
        return diffs;
    }

    if (Array.isArray(expected)) {
        if (!Array.isArray(actual)) {
            diffs.push(`${currentPath}: expected array, got ${typeof actual}`);
            return diffs;
        }
        // Compare first element shape
        if (expected.length > 0 && actual.length > 0) {
            diffs.push(...diffShapes(expected[0], actual[0], `${currentPath}[0]`, options));
        }
        return diffs;
    }

    if (typeof expected === 'object') {
        const expectedKeys = new Set(Object.keys(expected));
        const actualKeys = new Set(Object.keys(actual));

        for (const key of expectedKeys) {
            if (!actualKeys.has(key)) {
                diffs.push(`${currentPath}.${key}: missing in Rust response`);
            } else {
                diffs.push(...diffShapes(expected[key], actual[key], `${currentPath}.${key}`, options));
            }
        }

        if (!options.allowExtraKeys) {
            for (const key of actualKeys) {
                if (!expectedKeys.has(key)) {
                    diffs.push(`${currentPath}.${key}: extra key in Rust response`);
                }
            }
        }
    }

    return diffs;
}

/**
 * Sends a request to the Rust sidecar.
 * @param {Object} request Request definition from fixture
 * @returns {Promise<{status: number, headers: Record<string, string>, body: any}>}
 */
async function callRust(request) {
    const url = new URL(request.path, RUST_URL);
    if (request.query) {
        url.search = new URLSearchParams(request.query).toString();
    }

    /** @type {RequestInit} */
    const fetchOptions = {
        method: request.method,
        headers: buildHeaders(request.headers, request.requestType || (request.multipart ? 'multipart' : (request.body ? 'json' : 'none'))),
    };

    const requestType = request.requestType || (request.multipart ? 'multipart' : (request.body ? 'json' : 'none'));
    if (requestType === 'multipart' && request.multipart) {
        const form = new FormData();
        const fields = request.multipart.fields || {};
        for (const [key, value] of Object.entries(fields)) {
            form.append(key, String(value));
        }
        const files = request.multipart.files || [];
        for (const file of files) {
            appendFormFile(form, file);
        }
        fetchOptions.body = form;
    } else if (requestType === 'json' && request.body && request.method !== 'GET') {
        fetchOptions.body = JSON.stringify(request.body);
    }

    const response = await fetch(url.toString(), fetchOptions);

    const responseHeaders = {};
    response.headers.forEach((value, key) => {
        responseHeaders[key] = value;
    });

    let body;
    const contentType = response.headers.get('content-type') || '';
    if (contentType.includes('application/json')) {
        try {
            body = await response.json();
        } catch {
            body = await response.text();
        }
    } else {
        body = await response.text();
    }

    return { status: response.status, headers: responseHeaders, body };
}

/**
 * Compares a single fixture against the Rust response.
 * @param {Object} request The original request definition
 * @param {Object} expected The captured Node response
 * @param {Object} actual The Rust response
 * @returns {{ pass: boolean, issues: string[] }}
 */
function compareResponses(request, expected, actual) {
    const issues = [];

    // 1. Compare status codes
    if (expected.status !== actual.status) {
        issues.push(`Status: expected ${expected.status}, got ${actual.status}`);
    }

    // 2. Compare selected headers
    for (const header of COMPARED_HEADERS) {
        const expRaw = expected.headers[header];
        const actRaw = actual.headers[header];
        const exp = expRaw ? normalizeHeaderValue(header, expRaw) : expRaw;
        const act = actRaw ? normalizeHeaderValue(header, actRaw) : actRaw;
        if (exp && act && exp !== act) {
            issues.push(`Header "${header}": expected "${expRaw}", got "${actRaw}"`);
        } else if (exp && !act) {
            issues.push(`Header "${header}": missing in Rust response (expected "${expRaw}")`);
        }
    }

    const compareMode = request.compareBody || 'shape';

    // 3. Compare bodies
    if (compareMode === 'exact') {
        if (typeof expected.body === 'object' && expected.body !== null) {
            if (!deepEqual(expected.body, actual.body)) {
                issues.push(`Body: exact JSON mismatch`);
            }
        } else if (typeof expected.body === 'string' && typeof actual.body === 'string') {
            if (expected.body !== actual.body) {
                issues.push(`Body: expected "${expected.body}", got "${actual.body}"`);
            }
        } else if (expected.body !== actual.body) {
            issues.push(`Body: expected ${JSON.stringify(expected.body)}, got ${JSON.stringify(actual.body)}`);
        }
    } else if (typeof expected.body === 'object' && expected.body !== null) {
        const expectedShape = extractShape(expected.body);
        const actualShape = extractShape(actual.body);
        const shapeDiffs = diffShapes(expectedShape, actualShape, '$', {
            allowExtraKeys: request.allowExtraKeys,
        });
        issues.push(...shapeDiffs);
    } else if (typeof expected.body === 'string' && typeof actual.body === 'string') {
        // For non-JSON, just check that both are non-empty or both empty
        if ((expected.body.length === 0) !== (actual.body.length === 0)) {
            issues.push(`Body: expected ${expected.body.length === 0 ? 'empty' : 'non-empty'}, got ${actual.body.length === 0 ? 'empty' : 'non-empty'}`);
        }
    }

    return {
        pass: issues.length === 0,
        issues,
    };
}

/**
 * Computes a delta between two snapshots.
 * @param {Object} pre
 * @param {Object} post
 * @returns {{added: Record<string, any>, removed: Record<string, any>, modified: Record<string, {before: any, after: any}>}}
 */
function computeDelta(pre, post) {
    const added = {};
    const removed = {};
    const modified = {};

    const preFiles = pre?.files || {};
    const postFiles = post?.files || {};

    for (const [relPath, meta] of Object.entries(preFiles)) {
        if (!postFiles[relPath]) {
            removed[relPath] = meta;
        } else if (postFiles[relPath].sha256 !== meta.sha256 || postFiles[relPath].size !== meta.size) {
            modified[relPath] = { before: meta, after: postFiles[relPath] };
        }
    }

    for (const [relPath, meta] of Object.entries(postFiles)) {
        if (!preFiles[relPath]) {
            added[relPath] = meta;
        }
    }

    return { added, removed, modified };
}

/**
 * Convert a glob-like pattern to a RegExp.
 * Supports "*" and "**".
 * @param {string} pattern
 * @returns {RegExp}
 */
function globToRegex(pattern) {
    let regex = '^';
    for (let i = 0; i < pattern.length; i++) {
        const ch = pattern[i];
        if (ch === '*') {
            const next = pattern[i + 1];
            if (next === '*') {
                regex += '.*';
                i += 1;
            } else {
                regex += '[^/]*';
            }
            continue;
        }
        if (/[.+^${}()|[\]\\]/.test(ch)) {
            regex += `\\${ch}`;
        } else {
            regex += ch;
        }
    }
    regex += '$';
    return new RegExp(regex);
}

/**
 * Check if a path matches any ignore pattern.
 * @param {string} relPath
 * @param {string[]} patterns
 * @returns {boolean}
 */
function matchesAnyPattern(relPath, patterns) {
    return patterns.some(pattern => globToRegex(pattern).test(relPath));
}

/**
 * Filter delta entries by ignore patterns.
 * @param {{added: Record<string, any>, removed: Record<string, any>, modified: Record<string, any>}} delta
 * @param {string[]} ignore
 * @returns {{added: Record<string, any>, removed: Record<string, any>, modified: Record<string, any>}}
 */
function filterDelta(delta, ignore) {
    if (!ignore || ignore.length === 0) return delta;
    const filtered = { added: {}, removed: {}, modified: {} };
    for (const [key, value] of Object.entries(delta.added)) {
        if (!matchesAnyPattern(key, ignore)) {
            filtered.added[key] = value;
        }
    }
    for (const [key, value] of Object.entries(delta.removed)) {
        if (!matchesAnyPattern(key, ignore)) {
            filtered.removed[key] = value;
        }
    }
    for (const [key, value] of Object.entries(delta.modified)) {
        if (!matchesAnyPattern(key, ignore)) {
            filtered.modified[key] = value;
        }
    }
    return filtered;
}

/**
 * Compares file-system side effects between Node and Rust.
 * @param {string} fixtureDir Path to the fixture directory
 * @param {Object} actualPre Snapshot before Rust call
 * @param {Object} actualPost Snapshot after Rust call
 * @param {{mode?: string, ignore?: string[]}} [options]
 * @returns {{ pass: boolean, issues: string[] }}
 */
function compareSideEffects(fixtureDir, actualPre, actualPost, options = {}) {
    const sideEffectsDir = path.join(fixtureDir, 'side-effects');
    if (!fs.existsSync(sideEffectsDir)) {
        return { pass: true, issues: [] };
    }

    const prePath = path.join(sideEffectsDir, 'pre.json');
    const postPath = path.join(sideEffectsDir, 'post.json');
    const issues = [];

    if (!fs.existsSync(prePath) || !fs.existsSync(postPath)) {
        return { pass: false, issues: ['Missing pre.json or post.json side-effect snapshots'] };
    }

    const expectedPre = JSON.parse(fs.readFileSync(prePath, 'utf-8'));
    const expectedPost = JSON.parse(fs.readFileSync(postPath, 'utf-8'));

    if (options.mode === 'none') {
        return { pass: true, issues: [] };
    }

    const expectedDelta = filterDelta(computeDelta(expectedPre, expectedPost), options.ignore || []);
    const actualDelta = filterDelta(computeDelta(actualPre, actualPost), options.ignore || []);

    const compareSet = (label, expected, actual) => {
        const expectedKeys = new Set(Object.keys(expected));
        const actualKeys = new Set(Object.keys(actual));

        for (const key of expectedKeys) {
            if (!actualKeys.has(key)) {
                issues.push(`${label}: missing "${key}" in Rust side effects`);
            }
        }

        for (const key of actualKeys) {
            if (!expectedKeys.has(key)) {
                issues.push(`${label}: extra "${key}" in Rust side effects`);
            }
        }
    };

    compareSet('Added files', expectedDelta.added, actualDelta.added);
    compareSet('Removed files', expectedDelta.removed, actualDelta.removed);
    compareSet('Modified files', expectedDelta.modified, actualDelta.modified);

    if (options.mode !== 'pathsOnly') {
        // Validate content hashes for added/modified files
        for (const [relPath, meta] of Object.entries(expectedDelta.added)) {
            const actualMeta = actualDelta.added[relPath];
            if (actualMeta && actualMeta.sha256 !== meta.sha256) {
                issues.push(`Added file "${relPath}": hash mismatch (expected ${meta.sha256}, got ${actualMeta.sha256})`);
            }
        }

        for (const [relPath, meta] of Object.entries(expectedDelta.modified)) {
            const actualMeta = actualDelta.modified[relPath];
            if (actualMeta && actualMeta.after.sha256 !== meta.after.sha256) {
                issues.push(`Modified file "${relPath}": hash mismatch (expected ${meta.after.sha256}, got ${actualMeta.after.sha256})`);
            }
        }
    }

    return { pass: issues.length === 0, issues };
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
    console.log(`Parity Compare — Rust URL: ${RUST_URL}`);
    console.log(`Fixtures directory: ${FIXTURES_DIR}`);
    console.log();

    ensureUploadsDir();

    if (!fs.existsSync(FIXTURES_DIR)) {
        console.error(`Fixtures directory not found: ${FIXTURES_DIR}`);
        console.error('Run "node tools/parity/capture.js" first to generate baseline fixtures.');
        process.exit(1);
    }

    const fixtureDirs = fs.readdirSync(FIXTURES_DIR, { withFileTypes: true })
        .filter(d => d.isDirectory())
        .map(d => d.name);

    if (fixtureDirs.length === 0) {
        console.error('No fixtures found. Run capture.js first.');
        process.exit(1);
    }

    const fixtures = [];
    for (const fixtureName of fixtureDirs) {
        const fixtureDir = path.join(FIXTURES_DIR, fixtureName);
        const requestPath = path.join(fixtureDir, 'request.json');
        const responsePath = path.join(fixtureDir, 'response.json');

        if (!fs.existsSync(requestPath) || !fs.existsSync(responsePath)) {
            console.warn(`  ⚠ ${fixtureName}: missing request.json or response.json, skipping`);
            continue;
        }

        const request = JSON.parse(fs.readFileSync(requestPath, 'utf-8'));
        const expectedResponse = JSON.parse(fs.readFileSync(responsePath, 'utf-8'));
        fixtures.push({ fixtureName, fixtureDir, request, expectedResponse });
    }

    let filteredFixtures = FILTER_GROUP
        ? fixtures.filter(f => f.request.group === FILTER_GROUP)
        : fixtures;

    if (SAFE_ONLY) {
        filteredFixtures = filteredFixtures.filter(f => !f.request.hasSideEffects && !f.request.seed);
    }

    if (IS_LIVE_DATA_ROOT && !LIVE_WRITES_ALLOWED) {
        const before = filteredFixtures.length;
        filteredFixtures = filteredFixtures.filter(
            f => !f.request.hasSideEffects && !f.request.seed,
        );
        if (before !== filteredFixtures.length) {
            console.warn('⚠ Live data root detected — filtering fixtures with side effects or seeds.');
            console.warn('  To allow live writes, pass --allow-live-writes or set PARITY_ALLOW_LIVE_WRITES=1.');
        }
    }

    if (filteredFixtures.length === 0) {
        console.error('No fixtures matched the filter.');
        process.exit(1);
    }

    filteredFixtures.sort((a, b) => {
        const aSeq = a.request.sequence ?? Number.MAX_SAFE_INTEGER;
        const bSeq = b.request.sequence ?? Number.MAX_SAFE_INTEGER;
        if (aSeq !== bSeq) return aSeq - bSeq;
        return a.fixtureName.localeCompare(b.fixtureName);
    });

    let fixtureServer = null;
    if (JSON.stringify(filteredFixtures).includes('{{FIXTURE_URL}}') || JSON.stringify(filteredFixtures).includes(FIXTURE_SERVER_URL)) {
        fixtureServer = await startFixtureServer();
        console.log(`  Fixture server running at ${FIXTURE_SERVER_URL}`);
    }

    let passed = 0;
    let failed = 0;
    let skipped = 0;

    try {
        for (const entry of filteredFixtures) {
            const { fixtureDir } = entry;
            const request = replaceTokensDeep(entry.request);
            applyContextOverrides(request);
            const expectedResponse = entry.expectedResponse;

            if (request.skipCompare) {
                console.log(`  ↷ ${request.method} ${request.path} (skipped)`);
                skipped++;
                continue;
            }
            if (request.requiresNetwork && !(ALLOW_NETWORK || ENV_ALLOW_NETWORK)) {
                console.log(`  ↷ ${request.method} ${request.path} (network skipped)`);
                skipped++;
                continue;
            }

            const label = `${request.method} ${request.path}`;

            try {
                let actualPre = null;
                let actualPost = null;

                applySeed(request.seed);

                if (request.hasSideEffects) {
                    if (!fs.existsSync(DATA_ROOT)) {
                        throw new Error(`Data root not found: ${DATA_ROOT}`);
                    }
                    actualPre = await snapshotDataRoot(DATA_ROOT);
                }

                const actualResponse = await callRust(request);
                captureContextFromResponse(request, actualResponse);
                const result = compareResponses(request, expectedResponse, actualResponse);

                if (request.hasSideEffects) {
                    actualPost = await snapshotDataRoot(DATA_ROOT);
                    const sideEffectResult = compareSideEffects(
                        fixtureDir,
                        actualPre,
                        actualPost,
                        request.sideEffects || {},
                    );
                    if (!sideEffectResult.pass) {
                        result.pass = false;
                        result.issues.push(...sideEffectResult.issues);
                    }
                }

                if (result.pass) {
                    console.log(`  ✓ ${label}`);
                    passed++;
                } else {
                    console.log(`  ✗ ${label}`);
                    for (const issue of result.issues) {
                        console.log(`      ${issue}`);
                    }
                    failed++;
                }

                if (VERBOSE && result.pass) {
                    console.log(`      Status: ${actualResponse.status}`);
                    console.log(`      Body type: ${typeof actualResponse.body}`);
                }
            } catch (error) {
                console.log(`  ✗ ${label} — Network error: ${error.message}`);
                failed++;
            }
        }
    } finally {
        if (fixtureServer) {
            fixtureServer.close();
        }
    }

    console.log();
    console.log(`Results: ${passed} passed, ${failed} failed, ${skipped} skipped`);

    if (failed > 0) {
        process.exit(1);
    }
}

main().catch(err => {
    console.error('Fatal error:', err);
    process.exit(1);
});
