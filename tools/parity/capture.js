/**
 * Parity Harness — Capture
 *
 * Records baseline "golden" fixtures from the Node/Express backend.
 * For each configured endpoint, this script sends a request and stores
 * the status code, response headers, body, and file-system side effects.
 *
 * Usage:
 *   node tools/parity/capture.js [--group <name>] [--node-url <url>] [--fixtures-dir <path>]
 *
 * @module tools/parity/capture
 */

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import http from 'node:http';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';

// ---------------------------------------------------------------------------
// CLI argument parsing (initialized in main)
// ---------------------------------------------------------------------------

let NODE_URL = process.env.PARITY_NODE_URL || 'http://127.0.0.1:8000';
let FIXTURES_DIR = process.env.PARITY_FIXTURES_DIR || './tools/parity/fixtures';
let DATA_ROOT = process.env.PARITY_DATA_ROOT || './data';
let ALLOW_LIVE_DATA_ROOT = false;
let ALLOW_LIVE_WRITES = false;
let SAFE_ONLY = false;
let USER_HANDLE = process.env.PARITY_USER_HANDLE || '';
let USER_NAME = process.env.PARITY_USER_NAME || '';
let USER_ADMIN = process.env.PARITY_USER_ADMIN || '';
let CSRF_TOKEN = process.env.PARITY_CSRF_TOKEN || '';
let SESSION_COOKIE = process.env.PARITY_SESSION_COOKIE || '';
let FILTER_GROUP = '';
let EFFECTIVE_USER_HANDLE = USER_HANDLE || 'default-user';
let ALLOW_NETWORK = false;

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
const LIVE_DATA_ROOT_PREFIX = LIVE_DATA_ROOT + path.sep;

function parseCliArgs() {
    const { values: args } = parseArgs({
        options: {
            'group': { type: 'string', default: '' },
            'node-url': { type: 'string', default: NODE_URL },
            'fixtures-dir': { type: 'string', default: FIXTURES_DIR },
            'data-root': { type: 'string', default: DATA_ROOT },
            'allow-live-data-root': { type: 'boolean', default: false },
            'allow-live-writes': { type: 'boolean', default: false },
            'safe-only': { type: 'boolean', default: false },
            'allow-network': { type: 'boolean', default: false },
            'user-handle': { type: 'string', default: USER_HANDLE },
            'user-name': { type: 'string', default: USER_NAME },
            'user-admin': { type: 'string', default: USER_ADMIN },
            'csrf-token': { type: 'string', default: CSRF_TOKEN },
            'session-cookie': { type: 'string', default: SESSION_COOKIE },
        },
    });

    NODE_URL = args['node-url'];
    FIXTURES_DIR = args['fixtures-dir'];
    DATA_ROOT = args['data-root'];
    ALLOW_LIVE_DATA_ROOT = args['allow-live-data-root'];
    ALLOW_LIVE_WRITES = args['allow-live-writes'];
    SAFE_ONLY = args['safe-only'];
    ALLOW_NETWORK = args['allow-network'];
    USER_HANDLE = args['user-handle'];
    USER_NAME = args['user-name'];
    USER_ADMIN = args['user-admin'];
    CSRF_TOKEN = args['csrf-token'];
    SESSION_COOKIE = args['session-cookie'];
    FILTER_GROUP = args['group'];
    EFFECTIVE_USER_HANDLE = USER_HANDLE || 'default-user';
}

function assertLiveDataRootAllowed() {
    const resolvedDataRoot = path.resolve(DATA_ROOT);
    const isLive =
        resolvedDataRoot === LIVE_DATA_ROOT ||
        resolvedDataRoot.startsWith(LIVE_DATA_ROOT_PREFIX);

    if (isLive && !(ALLOW_LIVE_DATA_ROOT || ENV_ALLOW_LIVE_DATA_ROOT)) {
        console.error('✗ Refusing to run parity capture against live data root.');
        console.error(`  Data root: ${resolvedDataRoot}`);
        console.error('  Use a temp data root (recommended), or pass --allow-live-data-root to override.');
        console.error('  Example: PARITY_DATA_ROOT=/tmp/st-parity node tools/parity/capture.js ...');
        process.exit(1);
    }

    return { isLive, resolvedDataRoot };
}

const FIXTURE_PNG_BASE64 =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==';
const FIXTURE_CHAR_PNG_BASE64 =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAA4Z0RVh0Y2hhcmEAZXlKemNHVmpJam9pWTJoaGNtRmZZMkZ5WkY5Mk1pSXNJbk53WldOZmRtVnljMmx2YmlJNklqSXVNQ0lzSW1SaGRHRWlPbnNpYm1GdFpTSTZJbEJoY21sMGVTQkJiR2xqWlNJc0ltUmxjMk55YVhCMGFXOXVJam9pVkdWemRDQmphR0Z5WVdOMFpYSWlMQ0p3WlhKemIyNWhiR2wwZVNJNklpSXNJbk5qWlc1aGNtbHZJam9pSWl3aVptbHljM1JmYldWeklqb2lTR1ZzYkc4aUxDSnRaWE5mWlhoaGJYQnNaU0k2SWlJc0ltTnlaV0YwYjNKZmJtOTBaWE1pT2lJaUxDSnplWE4wWlcxZmNISnZiWEIwSWpvaUlpd2ljRzl6ZEY5b2FYTjBiM0o1WDJsdWMzUnlkV04wYVc5dWN5STZJaUlzSW1Gc2RHVnlibUYwWlY5bmNtVmxkR2x1WjNNaU9sdGRMQ0owWVdkeklqcGJYU3dpWTNKbFlYUnZjaUk2SWlJc0ltTm9ZWEpoWTNSbGNsOTJaWEp6YVc5dUlqb2lJaXdpWlhoMFpXNXphVzl1Y3lJNmV5SjBZV3hyWVhScGRtVnVaWE56SWpvd0xqVXNJbVpoZGlJNlptRnNjMlVzSW5kdmNteGtJam9pSW4wc0ltTm9ZWEpoWTNSbGNsOWliMjlySWpwN0ltVjRkR1Z1YzJsdmJuTWlPbnQ5TENKbGJuUnlhV1Z6SWpwYlhYMTlMQ0p1WVcxbElqb2lVR0Z5YVhSNUlFRnNhV05sSWl3aVpHVnpZM0pwY0hScGIyNGlPaUpVWlhOMElHTm9ZWEpoWTNSbGNpSXNJbkJsY25OdmJtRnNhWFI1SWpvaUlpd2ljMk5sYm1GeWFXOGlPaUlpTENKbWFYSnpkRjl0WlhNaU9pSklaV3hzYnlJc0ltMWxjMTlsZUdGdGNHeGxJam9pSWl3aVkzSmxZWFJ2Y21OdmJXMWxiblFpT2lJaUxDSmhkbUYwWVhJaU9pSnViMjVsSWl3aVkyaGhkQ0k2SWxCaGNtbDBlU0JCYkdsalpTQXRJREl3TWpBdE1ERXRNREZBTURCb01EQnRNREJ6TURBd2JYTWlMQ0owWVd4cllYUnBkbVZ1WlhOeklqb3dMalVzSW1aaGRpSTZabUZzYzJVc0luUmhaM01pT2x0ZGZRPT1uNoRyAAADhXRFWHRjY3YzAGV5SnpjR1ZqSWpvaVkyaGhjbUZmWTJGeVpGOTJNeUlzSW5Od1pXTmZkbVZ5YzJsdmJpSTZJak11TUNJc0ltUmhkR0VpT25zaWJtRnRaU0k2SWxCaGNtbDBlU0JCYkdsalpTSXNJbVJsYzJOeWFYQjBhVzl1SWpvaVZHVnpkQ0JqYUdGeVlXTjBaWElpTENKd1pYSnpiMjVoYkdsMGVTSTZJaUlzSW5OalpXNWhjbWx2SWpvaUlpd2labWx5YzNSZmJXVnpJam9pU0dWc2JHOGlMQ0p0WlhOZlpYaGhiWEJzWlNJNklpSXNJbU55WldGMGIzSmZibTkwWlhNaU9pSWlMQ0p6ZVhOMFpXMWZjSEp2YlhCMElqb2lJaXdpY0c5emRGOW9hWE4wYjNKNVgybHVjM1J5ZFdOMGFXOXVjeUk2SWlJc0ltRnNkR1Z5Ym1GMFpWOW5jbVZsZEdsdVozTWlPbHRkTENKMFlXZHpJanBiWFN3aVkzSmxZWFJ2Y2lJNklpSXNJbU5vWVhKaFkzUmxjbDkyWlhKemFXOXVJam9pSWl3aVpYaDBaVzV6YVc5dWN5STZleUowWVd4cllYUnBkbVZ1WlhOeklqb3dMalVzSW1aaGRpSTZabUZzYzJVc0luZHZjbXhrSWpvaUluMHNJbU5vWVhKaFkzUmxjbDlpYjI5cklqcDdJbVY0ZEdWdWMybHZibk1pT250OUxDSmxiblJ5YVdWeklqcGJYWDE5TENKdVlXMWxJam9pVUdGeWFYUjVJRUZzYVdObElpd2laR1Z6WTNKcGNIUnBiMjRpT2lKVVpYTjBJR05vWVhKaFkzUmxjaUlzSW5CbGNuTnZibUZzYVhSNUlqb2lJaXdpYzJObGJtRnlhVzhpT2lJaUxDSm1hWEp6ZEY5dFpYTWlPaUpJWld4c2J5SXNJbTFsYzE5bGVHRnRjR3hsSWpvaUlpd2lZM0psWVhSdmNtTnZiVzFsYm5RaU9pSWlMQ0poZG1GMFlYSWlPaUp1YjI1bElpd2lZMmhoZENJNklsQmhjbWwwZVNCQmJHbGpaU0F0SURJd01qQXRNREV0TURGQU1EQm9NREJ0TURCek1EQXdiWE1pTENKMFlXeHJZWFJwZG1WdVpYTnpJam93TGpVc0ltWmhkaUk2Wm1Gc2MyVXNJblJoWjNNaU9sdGRmUT09AJWOigAAAABJRU5ErkJggg==';
const FIXTURE_SPRITE_ZIP_BASE64 =
    'UEsDBBQAAAAIAEKHUFw5YQprPwAAAEYAAAAHAAAAam95LnBuZ+sM8HPn5ZLiYmBg4PX0cAkC0owgzMEGJOVFj3SCJVwcQyrmJP84f+CDPAMrA+P/zpm2skAJBk9XP5d1TglNAFBLAQIUAxQAAAAIAEKHUFw5YQprPwAAAEYAAAAHAAAAAAAAAAAAAACAAQAAAABqb3kucG5nUEsFBgAAAAABAAEANQAAAGQAAAAAAA==';
const FIXTURE_TEXT_BASE64 = Buffer.from('hello-parity').toString('base64');
const FIXTURE_MTIME_MS = 1577836800000; // 2020-01-01T00:00:00Z
const FIXTURE_CHAT = [
    { chat_metadata: {}, user_name: 'User', character_name: 'Seraphina' },
    { name: 'User', is_user: true, send_date: '2020-01-01T00:00:00.000Z', mes: 'Hello', extra: {} },
    { name: 'Seraphina', is_user: false, send_date: '2020-01-01T00:00:01.000Z', mes: 'Hi', extra: {} },
];
const FIXTURE_CHAT_NO_MATCH = [
    { chat_metadata: {}, user_name: 'User', character_name: 'Seraphina' },
    { name: 'User', is_user: true, send_date: '2020-01-01T00:00:00.000Z', mes: 'Goodbye', extra: {} },
];
const FIXTURE_CHAT_JSONL = FIXTURE_CHAT.map(line => JSON.stringify(line)).join('\n');
const FIXTURE_CHAT_NO_MATCH_JSONL = FIXTURE_CHAT_NO_MATCH.map(line => JSON.stringify(line)).join('\n');
const FIXTURE_CHAT_JSONL_BASE64 = Buffer.from(FIXTURE_CHAT_JSONL).toString('base64');
const FIXTURE_GROUP_CHAT_JSONL = FIXTURE_CHAT_JSONL;
const FIXTURE_GROUP = {
    id: 'group-1',
    name: 'Group One',
    chats: ['group-chat-1'],
    chat_id: 'group-chat-1',
    members: [],
    allow_self_responses: false,
    activation_strategy: 0,
    generation_mode: 0,
    disabled_members: [],
    auto_mode_delay: 5,
    generation_mode_join_prefix: '',
    generation_mode_join_suffix: '',
};
const FIXTURE_GROUP_JSON = JSON.stringify(FIXTURE_GROUP, null, 4);
const FIXTURE_WORLDINFO = {
    entries: {
        '0': { uid: 1, key: ['alpha'], content: 'Alpha content' },
    },
    name: 'Fixture World',
    extensions: { depth: 2 },
};
const FIXTURE_WORLDINFO_JSON = JSON.stringify(FIXTURE_WORLDINFO, null, 4);
const FIXTURE_WORLDINFO_JSON_BASE64 = Buffer.from(FIXTURE_WORLDINFO_JSON).toString('base64');
const FIXTURE_WORLDINFO_EDIT = {
    entries: {
        '0': { uid: 2, key: ['beta'], content: 'Beta content' },
    },
    name: 'Edit World',
    extensions: {},
};
const FIXTURE_WORLDINFO_EDIT_JSON = JSON.stringify(FIXTURE_WORLDINFO_EDIT, null, 4);
const FIXTURE_IMAGE_METADATA_INDEX_JSON = JSON.stringify({
    version: 1,
    images: {
        'backgrounds/ghost.png': {
            hash: 'deadbeef',
            aspectRatio: 1,
            isAnimated: false,
            dominantColor: '#000000',
            folderIds: [],
            addedTimestamp: 0,
            thumbnailResolution: 14400,
            mtime: 0,
        },
    },
    folders: [],
}, null, 2);
const FIXTURE_VECTOR_EMBEDDINGS = {
    hello: [1, 0],
    world: [0, 1],
};
const UPLOADS_DIR_NAME = '_uploads';

// ---------------------------------------------------------------------------
// Endpoint definitions
// ---------------------------------------------------------------------------

/**
 * @typedef {Object} EndpointDef
 * @property {string} group Logical group name (e.g. "characters", "chats")
 * @property {string} method HTTP method
 * @property {string} path URL path
 * @property {Record<string, string>} [query] Query parameters
 * @property {Record<string, string>} [headers] Additional request headers
 * @property {any} [body] Request body (will be JSON-serialized)
 * @property {string} [requestType] Request type ("json" | "multipart" | "none")
 * @property {Object} [multipart] Multipart form definition
 * @property {string} [compareBody] Body comparison mode ("shape" | "exact")
 * @property {boolean} [hasSideEffects] Whether this endpoint modifies the filesystem
 * @property {{mode?: string, ignore?: string[]}} [sideEffects] Side-effect compare options
 * @property {Object} [seed] Seed data to write before request
 * @property {{dataMaid?: boolean, secretId?: boolean}} [capture] Capture dynamic values from response
 * @property {boolean} [skipCompare] Skip compare for this fixture
 * @property {boolean} [requiresNetwork] Skip unless allow-network is set
 * @property {string} [description] Human-readable description
 */

/** @type {EndpointDef[]} */
const ENDPOINTS = [
    // Phase 1 — Global
    {
        group: 'global',
        method: 'POST',
        path: '/api/ping',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Health check ping',
    },
    {
        group: 'global',
        method: 'GET',
        path: '/version',
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Server version info',
    },
    {
        group: 'global',
        method: 'GET',
        path: '/health',
        hasSideEffects: false,
        skipCompare: true,
        description: 'Sidecar health (Rust-only)',
    },
    // Phase 1 — User asset routing
    {
        group: 'user-assets',
        method: 'GET',
        path: '/backgrounds/parity-bg.txt',
        hasSideEffects: false,
        description: 'Serve background asset',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/backgrounds/parity-bg.txt', contents: 'bg' },
            ],
        },
    },
    {
        group: 'user-assets',
        method: 'GET',
        path: '/characters/parity-char.txt',
        hasSideEffects: false,
        description: 'Serve character asset',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/characters/parity-char.txt', contents: 'char' },
            ],
        },
    },
    {
        group: 'user-assets',
        method: 'GET',
        path: '/User%20Avatars/parity-avatar.txt',
        hasSideEffects: false,
        description: 'Serve user avatar asset',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/User Avatars/parity-avatar.txt', contents: 'avatar' },
            ],
        },
    },
    {
        group: 'user-assets',
        method: 'GET',
        path: '/assets/bgm/parity-bgm.txt',
        hasSideEffects: false,
        description: 'Serve user asset file',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/assets/bgm/parity-bgm.txt', contents: 'bgm' },
            ],
        },
    },
    {
        group: 'user-assets',
        method: 'GET',
        path: '/user/images/parity-image.txt',
        hasSideEffects: false,
        description: 'Serve user image file',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/user/images/parity-image.txt', contents: 'image' },
            ],
        },
    },
    {
        group: 'user-assets',
        method: 'GET',
        path: '/user/files/parity-file.txt',
        hasSideEffects: false,
        description: 'Serve user file asset',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/user/files/parity-file.txt', contents: 'file' },
            ],
        },
    },
    {
        group: 'user-assets',
        method: 'GET',
        path: '/scripts/extensions/third-party/parity-ext.js',
        hasSideEffects: false,
        description: 'Serve third-party extension asset',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/extensions/parity-ext.js', contents: 'console.log(\"parity\");' },
            ],
        },
    },
    // Phase 2 — Thumbnails
    {
        group: 'thumbnails',
        method: 'GET',
        path: '/thumbnail',
        query: { type: 'persona', file: 'avatar-thumb.png' },
        hasSideEffects: false,
        description: 'Serve persona thumbnail (falls back to original)',
        seed: {
            files: [
                {
                    path: '{{USER_HANDLE}}/User Avatars/avatar-thumb.png',
                    contentsBase64: FIXTURE_PNG_BASE64,
                },
                {
                    path: '{{USER_HANDLE}}/thumbnails/persona/avatar-thumb.png',
                    contentsBase64: FIXTURE_PNG_BASE64,
                },
            ],
        },
    },
    // Phase 2 — Read-only
    {
        group: 'avatars',
        method: 'POST',
        path: '/api/avatars/get',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'List user avatars',
        seed: {
            clear: ['{{USER_HANDLE}}/User Avatars'],
            files: [
                { path: '{{USER_HANDLE}}/User Avatars/avatar-a.png', contents: 'a' },
                { path: '{{USER_HANDLE}}/User Avatars/avatar-b.jpg', contents: 'b' },
                { path: '{{USER_HANDLE}}/User Avatars/ignore.txt', contents: 'c' },
            ],
        },
    },
    {
        group: 'backgrounds',
        method: 'POST',
        path: '/api/backgrounds/all',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'List backgrounds',
        seed: {
            clear: ['{{USER_HANDLE}}/backgrounds'],
            files: [
                { path: '{{USER_HANDLE}}/backgrounds/bg-a.png', contents: 'a' },
                { path: '{{USER_HANDLE}}/backgrounds/bg-b.jpg', contents: 'b' },
            ],
        },
    },
    {
        group: 'images',
        method: 'POST',
        path: '/api/images/list',
        body: { folder: 'folder1', sortField: 'name', sortOrder: 'asc', type: 1 },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'List images in folder1',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/user/images/folder1/img-a.png', contents: 'a' },
                { path: '{{USER_HANDLE}}/user/images/folder1/img-b.jpg', contents: 'b' },
            ],
        },
    },
    {
        group: 'images',
        method: 'POST',
        path: '/api/images/list/folder2',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List images via URL folder',
        seed: {
            files: [
                {
                    path: '{{USER_HANDLE}}/user/images/folder2/img-c.png',
                    contentsBase64: FIXTURE_PNG_BASE64,
                },
            ],
        },
    },
    {
        group: 'images',
        method: 'POST',
        path: '/api/images/folders',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List image folders',
        seed: {
            dirs: [
                '{{USER_HANDLE}}/user/images/folder1',
                '{{USER_HANDLE}}/user/images/folder2',
            ],
        },
    },
    // Phase 3 — Avatars writes
    {
        group: 'avatars',
        method: 'POST',
        path: '/api/avatars/upload',
        requestType: 'multipart',
        multipart: {
            fields: { overwrite_name: 'avatar-overwrite.png' },
            files: [
                {
                    field: 'avatar',
                    filename: 'avatar.png',
                    contentType: 'image/png',
                    dataBase64: FIXTURE_PNG_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        description: 'Upload avatar (overwrite)',
        seed: {
            dirs: ['{{USER_HANDLE}}/User Avatars'],
            remove: ['{{USER_HANDLE}}/User Avatars/avatar-overwrite.png'],
        },
    },
    {
        group: 'avatars',
        method: 'POST',
        path: '/api/avatars/delete',
        body: { avatar: 'avatar-delete.png' },
        hasSideEffects: true,
        description: 'Delete avatar',
        seed: {
            files: [
                {
                    path: '{{USER_HANDLE}}/User Avatars/avatar-delete.png',
                    contents: 'delete',
                },
            ],
        },
    },
    // Phase 3 — Backgrounds writes
    {
        group: 'backgrounds',
        method: 'POST',
        path: '/api/backgrounds/upload',
        requestType: 'multipart',
        multipart: {
            files: [
                {
                    field: 'avatar',
                    filename: 'bg-upload.png',
                    contentType: 'image/png',
                    dataBase64: FIXTURE_PNG_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        sideEffects: { ignore: ['**/image-metadata.json'] },
        description: 'Upload background',
        seed: {
            dirs: ['{{USER_HANDLE}}/backgrounds'],
            remove: ['{{USER_HANDLE}}/backgrounds/bg-upload.png'],
        },
    },
    {
        group: 'backgrounds',
        method: 'POST',
        path: '/api/backgrounds/delete',
        body: { bg: 'bg-delete.png' },
        hasSideEffects: true,
        sideEffects: { ignore: ['**/image-metadata.json'] },
        description: 'Delete background',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/backgrounds/bg-delete.png', contents: 'delete' },
            ],
        },
    },
    {
        group: 'backgrounds',
        method: 'POST',
        path: '/api/backgrounds/rename',
        body: { old_bg: 'bg-rename.png', new_bg: 'bg-renamed.png' },
        hasSideEffects: true,
        sideEffects: { ignore: ['**/image-metadata.json'] },
        description: 'Rename background',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/backgrounds/bg-rename.png', contents: 'rename' },
            ],
            remove: ['{{USER_HANDLE}}/backgrounds/bg-renamed.png'],
        },
    },
    // Phase 3 — Images writes
    {
        group: 'images',
        method: 'POST',
        path: '/api/images/upload',
        body: {
            image: FIXTURE_TEXT_BASE64,
            format: 'png',
            filename: 'upload',
            ch_name: 'CharOne',
        },
        hasSideEffects: true,
        description: 'Upload image (base64)',
        seed: {
            remove: ['{{USER_HANDLE}}/user/images/CharOne/upload.png'],
        },
    },
    {
        group: 'images',
        method: 'POST',
        path: '/api/images/delete',
        body: { path: 'user/images/delete-folder/delete.png' },
        hasSideEffects: true,
        description: 'Delete image',
        seed: {
            files: [
                {
                    path: '{{USER_HANDLE}}/user/images/delete-folder/delete.png',
                    contents: 'delete',
                },
            ],
        },
    },
    // Phase 3 — Files
    {
        group: 'files',
        method: 'POST',
        path: '/api/files/sanitize-filename',
        body: { fileName: 'bad name.png' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Sanitize filename',
    },
    {
        group: 'files',
        method: 'POST',
        path: '/api/files/upload',
        body: { name: 'upload.txt', data: FIXTURE_TEXT_BASE64 },
        hasSideEffects: true,
        description: 'Upload file (base64)',
        seed: {
            remove: ['{{USER_HANDLE}}/user/files/upload.txt'],
        },
    },
    {
        group: 'files',
        method: 'POST',
        path: '/api/files/delete',
        body: { path: 'user/files/delete.txt' },
        hasSideEffects: true,
        description: 'Delete file',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/user/files/delete.txt', contents: 'delete' },
            ],
        },
    },
    {
        group: 'files',
        method: 'POST',
        path: '/api/files/verify',
        body: { urls: ['user/files/verify.txt', 'user/files/missing.txt'] },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Verify files',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/user/files/verify.txt', contents: 'verify' },
            ],
            remove: ['{{USER_HANDLE}}/user/files/missing.txt'],
        },
    },
    // Phase 3 — Sprites
    {
        group: 'sprites',
        method: 'GET',
        path: '/api/sprites/get',
        query: { name: 'SpriteChar' },
        hasSideEffects: false,
        description: 'List sprites for character',
        seed: {
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/SpriteChar/anger.png',
                    contents: 'anger',
                    mtimeMs: FIXTURE_MTIME_MS,
                },
                {
                    path: '{{USER_HANDLE}}/characters/SpriteChar/joy.png',
                    contents: 'joy',
                    mtimeMs: FIXTURE_MTIME_MS,
                },
            ],
        },
    },
    {
        group: 'sprites',
        method: 'POST',
        path: '/api/sprites/upload',
        requestType: 'multipart',
        multipart: {
            fields: { label: 'joy', name: 'SpriteChar', spriteName: 'joy' },
            files: [
                {
                    field: 'avatar',
                    filename: 'joy.png',
                    contentType: 'image/png',
                    dataBase64: FIXTURE_PNG_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        description: 'Upload sprite',
        seed: {
            dirs: ['{{USER_HANDLE}}/characters/SpriteChar'],
            remove: ['{{USER_HANDLE}}/characters/SpriteChar/joy.png'],
        },
    },
    {
        group: 'sprites',
        method: 'POST',
        path: '/api/sprites/upload-zip',
        requestType: 'multipart',
        multipart: {
            fields: { name: 'SpriteChar' },
            files: [
                {
                    field: 'avatar',
                    filename: 'sprites.zip',
                    contentType: 'application/zip',
                    dataBase64: FIXTURE_SPRITE_ZIP_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        description: 'Upload sprites from ZIP',
        seed: {
            dirs: ['{{USER_HANDLE}}/characters/SpriteChar'],
            remove: ['{{USER_HANDLE}}/characters/SpriteChar/joy.png'],
        },
    },
    {
        group: 'sprites',
        method: 'POST',
        path: '/api/sprites/delete',
        body: { name: 'SpriteChar', label: 'joy' },
        hasSideEffects: true,
        description: 'Delete sprite',
        seed: {
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/SpriteChar/joy.png',
                    contents: 'delete',
                },
            ],
        },
    },
    // Phase 3 — Assets
    {
        group: 'assets',
        method: 'POST',
        path: '/api/assets/get',
        body: {},
        hasSideEffects: false,
        description: 'List assets',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/assets/bgm/track1.mp3', contents: 'bgm' },
                { path: '{{USER_HANDLE}}/assets/ambient/amb1.mp3', contents: 'ambient' },
                { path: '{{USER_HANDLE}}/assets/blip/blip1.mp3', contents: 'blip' },
                { path: '{{USER_HANDLE}}/assets/live2d/ModelA/model.json', contents: '{}' },
                { path: '{{USER_HANDLE}}/assets/vrm/model/model1.vrm', contents: 'vrm' },
                { path: '{{USER_HANDLE}}/assets/vrm/animation/anim1.fbx', contents: 'anim' },
            ],
        },
    },
    {
        group: 'assets',
        method: 'POST',
        path: '/api/assets/download',
        body: {
            url: '{{FIXTURE_URL}}/asset.bin',
            category: 'bgm',
            filename: 'download.bin',
        },
        hasSideEffects: true,
        description: 'Download asset',
        seed: {
            remove: ['{{USER_HANDLE}}/assets/bgm/download.bin'],
        },
    },
    {
        group: 'assets',
        method: 'POST',
        path: '/api/assets/delete',
        body: { category: 'bgm', filename: 'delete.bin' },
        hasSideEffects: true,
        description: 'Delete asset',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/assets/bgm/delete.bin', contents: 'delete' },
            ],
        },
    },
    {
        group: 'assets',
        method: 'POST',
        path: '/api/assets/character',
        query: { name: 'SpriteChar', category: 'bgm' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List character assets',
        seed: {
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/SpriteChar/bgm/chartrack.mp3',
                    contents: 'char',
                },
            ],
        },
    },
    // Phase 3 — Themes
    {
        group: 'themes',
        method: 'POST',
        path: '/api/themes/save',
        body: { name: 'theme-parity', color: '#123456' },
        hasSideEffects: true,
        description: 'Save theme',
        seed: {
            remove: ['{{USER_HANDLE}}/themes/theme-parity.json'],
        },
    },
    {
        group: 'themes',
        method: 'POST',
        path: '/api/themes/delete',
        body: { name: 'theme-delete' },
        hasSideEffects: true,
        description: 'Delete theme',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/themes/theme-delete.json', contents: '{}' },
            ],
        },
    },
    // Phase 3 — Moving UI
    {
        group: 'moving-ui',
        method: 'POST',
        path: '/api/moving-ui/save',
        body: { name: 'layout-1', x: 10, y: 20 },
        hasSideEffects: true,
        description: 'Save moving UI layout',
        seed: {
            remove: ['{{USER_HANDLE}}/movingUI/layout-1.json'],
        },
    },
    // Phase 3 — Quick Replies
    {
        group: 'quick-replies',
        method: 'POST',
        path: '/api/quick-replies/save',
        body: { name: 'quick-1', replies: ['Hello', 'Hi'] },
        hasSideEffects: true,
        description: 'Save quick replies',
        seed: {
            remove: ['{{USER_HANDLE}}/QuickReplies/quick-1.json'],
        },
    },
    {
        group: 'quick-replies',
        method: 'POST',
        path: '/api/quick-replies/delete',
        body: { name: 'quick-delete' },
        hasSideEffects: true,
        description: 'Delete quick replies',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/QuickReplies/quick-delete.json', contents: '{}' },
            ],
        },
    },
    // Phase 4/5 — Characters (reads + writes)
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/all',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List all characters',
        seed: {
            clear: ['{{USER_HANDLE}}/characters'],
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/Alice.png',
                    contentsBase64: FIXTURE_CHAR_PNG_BASE64,
                },
            ],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/get',
        body: { avatar_url: 'Alice.png' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Get character',
        seed: {
            clear: ['{{USER_HANDLE}}/characters'],
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/Alice.png',
                    contentsBase64: FIXTURE_CHAR_PNG_BASE64,
                },
            ],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/chats',
        body: { avatar_url: 'Chat_Alice.png', simple: true },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List character chats (simple)',
        seed: {
            clear: ['{{USER_HANDLE}}/chats/Chat_Alice'],
            dirs: ['{{USER_HANDLE}}/chats/Chat_Alice'],
            files: [
                {
                    path: '{{USER_HANDLE}}/chats/Chat_Alice/chat-1.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/create',
        requestType: 'multipart',
        multipart: {
            fields: {
                ch_name: 'Parity Alice',
                file_name: 'Parity_Alice',
                description: 'Test character',
                first_mes: 'Hello',
            },
            files: [
                {
                    field: 'avatar',
                    filename: 'alice.png',
                    contentType: 'image/png',
                    dataBase64: FIXTURE_PNG_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Create character',
        seed: {
            clear: ['{{USER_HANDLE}}/characters', '{{USER_HANDLE}}/chats/Parity_Alice'],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/edit',
        requestType: 'multipart',
        multipart: {
            fields: {
                ch_name: 'Edit Alice',
                avatar_url: 'Edit_Alice.png',
                description: 'Edited',
                first_mes: 'Hi',
                chat: 'Edit Alice - 2020-01-01@00h00m00s000ms',
                create_date: '2020-01-01T00:00:00.000Z',
            },
            files: [
                {
                    field: 'avatar',
                    filename: 'alice.png',
                    contentType: 'image/png',
                    dataBase64: FIXTURE_PNG_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly', ignore: ['_cache/characters/**', '**/_cache/characters/**'] },
        compareBody: 'exact',
        description: 'Edit character',
        seed: {
            clear: ['{{USER_HANDLE}}/characters', '{{USER_HANDLE}}/chats/Edit_Alice'],
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/Edit_Alice.png',
                    contentsBase64: FIXTURE_CHAR_PNG_BASE64,
                },
            ],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/edit-avatar',
        requestType: 'multipart',
        multipart: {
            fields: { avatar_url: 'Edit_Avatar.png' },
            files: [
                {
                    field: 'avatar',
                    filename: 'avatar.png',
                    contentType: 'image/png',
                    dataBase64: FIXTURE_PNG_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        sideEffects: { ignore: ['_cache/characters/**', '**/_cache/characters/**'] },
        compareBody: 'exact',
        description: 'Edit avatar image',
        seed: {
            clear: ['{{USER_HANDLE}}/characters'],
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/Edit_Avatar.png',
                    contentsBase64: FIXTURE_CHAR_PNG_BASE64,
                },
            ],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/edit-attribute',
        body: {
            avatar_url: 'Attr_Alice.png',
            ch_name: 'Attr Alice',
            field: 'description',
            value: 'New description',
        },
        hasSideEffects: true,
        sideEffects: { ignore: ['_cache/characters/**', '**/_cache/characters/**'] },
        compareBody: 'exact',
        description: 'Edit character attribute',
        seed: {
            clear: ['{{USER_HANDLE}}/characters'],
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/Attr_Alice.png',
                    contentsBase64: FIXTURE_CHAR_PNG_BASE64,
                },
            ],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/merge-attributes',
        body: {
            avatar: 'Merge_Alice.png',
            data: { name: 'Merge Alice', description: 'Merged' },
        },
        hasSideEffects: true,
        sideEffects: { ignore: ['_cache/characters/**', '**/_cache/characters/**'] },
        compareBody: 'exact',
        description: 'Merge character attributes',
        seed: {
            clear: ['{{USER_HANDLE}}/characters'],
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/Merge_Alice.png',
                    contentsBase64: FIXTURE_CHAR_PNG_BASE64,
                },
            ],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/rename',
        body: { avatar_url: 'Rename_Alice.png', new_name: 'Renamed Alice' },
        hasSideEffects: true,
        sideEffects: { ignore: ['_cache/characters/**', '**/_cache/characters/**'] },
        compareBody: 'exact',
        description: 'Rename character',
        seed: {
            clear: ['{{USER_HANDLE}}/characters', '{{USER_HANDLE}}/chats/Rename_Alice'],
            dirs: ['{{USER_HANDLE}}/chats/Rename_Alice'],
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/Rename_Alice.png',
                    contentsBase64: FIXTURE_CHAR_PNG_BASE64,
                },
                {
                    path: '{{USER_HANDLE}}/chats/Rename_Alice/old.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/duplicate',
        body: { avatar_url: 'Dup_Alice.png' },
        hasSideEffects: true,
        compareBody: 'exact',
        description: 'Duplicate character',
        seed: {
            clear: ['{{USER_HANDLE}}/characters'],
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/Dup_Alice.png',
                    contentsBase64: FIXTURE_CHAR_PNG_BASE64,
                },
            ],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/delete',
        body: { avatar_url: 'Delete_Alice.png', delete_chats: true },
        hasSideEffects: true,
        compareBody: 'exact',
        description: 'Delete character',
        seed: {
            clear: ['{{USER_HANDLE}}/characters', '{{USER_HANDLE}}/chats/Delete_Alice'],
            dirs: ['{{USER_HANDLE}}/chats/Delete_Alice'],
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/Delete_Alice.png',
                    contentsBase64: FIXTURE_CHAR_PNG_BASE64,
                },
                {
                    path: '{{USER_HANDLE}}/chats/Delete_Alice/chat.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/import',
        requestType: 'multipart',
        multipart: {
            fields: { file_type: 'png' },
            files: [
                {
                    field: 'avatar',
                    filename: 'import.png',
                    contentType: 'image/png',
                    dataBase64: FIXTURE_CHAR_PNG_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly', ignore: ['_cache/characters/**', '**/_cache/characters/**'] },
        compareBody: 'exact',
        description: 'Import character (png)',
        seed: {
            clear: ['{{USER_HANDLE}}/characters'],
        },
    },
    {
        group: 'characters',
        method: 'POST',
        path: '/api/characters/export',
        body: { avatar_url: 'Export_Alice.png', format: 'json' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Export character (json)',
        seed: {
            clear: ['{{USER_HANDLE}}/characters'],
            files: [
                {
                    path: '{{USER_HANDLE}}/characters/Export_Alice.png',
                    contentsBase64: FIXTURE_CHAR_PNG_BASE64,
                },
            ],
        },
    },
    // Phase 6 — Chats
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/save',
        body: { avatar_url: 'Seraphina.png', file_name: 'chat-save', chat: FIXTURE_CHAT },
        hasSideEffects: true,
        sideEffects: { ignore: ['**/backups/**'] },
        compareBody: 'exact',
        description: 'Save chat',
        seed: {
            clear: ['{{USER_HANDLE}}/chats', '{{USER_HANDLE}}/backups'],
            dirs: ['{{USER_HANDLE}}/chats/Seraphina', '{{USER_HANDLE}}/backups'],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/get',
        body: { avatar_url: 'Seraphina.png', file_name: 'chat-get' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Get chat',
        seed: {
            clear: ['{{USER_HANDLE}}/chats/Seraphina'],
            files: [
                {
                    path: '{{USER_HANDLE}}/chats/Seraphina/chat-get.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                    mtimeMs: FIXTURE_MTIME_MS,
                },
            ],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/rename',
        body: { avatar_url: 'Seraphina.png', original_file: 'rename-me.jsonl', renamed_file: 'renamed.jsonl' },
        hasSideEffects: true,
        compareBody: 'exact',
        description: 'Rename chat',
        seed: {
            clear: ['{{USER_HANDLE}}/chats/Seraphina'],
            files: [
                {
                    path: '{{USER_HANDLE}}/chats/Seraphina/rename-me.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/delete',
        body: { avatar_url: 'Seraphina.png', chatfile: 'delete-chat' },
        hasSideEffects: true,
        compareBody: 'exact',
        description: 'Delete chat',
        seed: {
            clear: ['{{USER_HANDLE}}/chats/Seraphina'],
            files: [
                {
                    path: '{{USER_HANDLE}}/chats/Seraphina/delete-chat.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/export',
        body: { avatar_url: 'Seraphina.png', file: 'export-chat.jsonl', format: 'jsonl', exportfilename: 'export-chat.jsonl' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Export chat (jsonl)',
        seed: {
            clear: ['{{USER_HANDLE}}/chats/Seraphina'],
            files: [
                {
                    path: '{{USER_HANDLE}}/chats/Seraphina/export-chat.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/import',
        requestType: 'multipart',
        multipart: {
            fields: {
                avatar_url: 'Seraphina.png',
                file_type: 'jsonl',
                character_name: 'Seraphina',
                user_name: 'User',
            },
            files: [
                {
                    field: 'avatar',
                    filename: 'chat.jsonl',
                    contentType: 'application/json',
                    dataBase64: FIXTURE_CHAT_JSONL_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        sideEffects: { ignore: ['**/backups/**', '**/chats/**/* imported.jsonl'] },
        compareBody: 'shape',
        description: 'Import chat (jsonl)',
        seed: {
            clear: ['{{USER_HANDLE}}/chats/Seraphina', '{{USER_HANDLE}}/backups'],
            dirs: ['{{USER_HANDLE}}/chats/Seraphina', '{{USER_HANDLE}}/backups'],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/search',
        body: { query: 'hello', avatar_url: 'Seraphina.png' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Search chats',
        seed: {
            clear: ['{{USER_HANDLE}}/chats/Seraphina'],
            files: [
                {
                    path: '{{USER_HANDLE}}/chats/Seraphina/search-match.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
                {
                    path: '{{USER_HANDLE}}/chats/Seraphina/search-nomatch.jsonl',
                    contents: FIXTURE_CHAT_NO_MATCH_JSONL,
                },
            ],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/recent',
        body: { max: 10, pinned: [] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Recent chats',
        seed: {
            clear: ['{{USER_HANDLE}}/chats', '{{USER_HANDLE}}/group chats', '{{USER_HANDLE}}/groups'],
            files: [
                { path: '{{USER_HANDLE}}/characters/Seraphina.png', contents: 'png' },
                {
                    path: '{{USER_HANDLE}}/chats/Seraphina/recent.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                    mtimeMs: FIXTURE_MTIME_MS,
                },
                {
                    path: '{{USER_HANDLE}}/groups/group-1.json',
                    contents: JSON.stringify({ id: 'group-1', chats: ['group-chat-1'] }),
                },
                {
                    path: '{{USER_HANDLE}}/group chats/group-chat-1.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                    mtimeMs: FIXTURE_MTIME_MS,
                },
            ],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/group/import',
        requestType: 'multipart',
        multipart: {
            files: [
                {
                    field: 'avatar',
                    filename: 'group-chat.jsonl',
                    contentType: 'application/json',
                    dataBase64: FIXTURE_CHAT_JSONL_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        sideEffects: { ignore: ['**/group chats/*.jsonl'] },
        compareBody: 'shape',
        description: 'Import group chat',
        seed: {
            clear: ['{{USER_HANDLE}}/group chats'],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/group/get',
        body: { id: 'group-chat-1' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Get group chat',
        seed: {
            clear: ['{{USER_HANDLE}}/group chats'],
            files: [
                {
                    path: '{{USER_HANDLE}}/group chats/group-chat-1.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/group/info',
        body: { id: 'group-chat-1' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Group chat info',
        seed: {
            clear: ['{{USER_HANDLE}}/group chats'],
            files: [
                {
                    path: '{{USER_HANDLE}}/group chats/group-chat-1.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/group/delete',
        body: { id: 'group-chat-del' },
        hasSideEffects: true,
        compareBody: 'exact',
        description: 'Delete group chat',
        seed: {
            clear: ['{{USER_HANDLE}}/group chats'],
            files: [
                {
                    path: '{{USER_HANDLE}}/group chats/group-chat-del.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'chats',
        method: 'POST',
        path: '/api/chats/group/save',
        body: { id: 'group-chat-save', chat: FIXTURE_CHAT },
        hasSideEffects: true,
        sideEffects: { ignore: ['**/backups/**'] },
        compareBody: 'exact',
        description: 'Save group chat',
        seed: {
            clear: ['{{USER_HANDLE}}/group chats', '{{USER_HANDLE}}/backups'],
            dirs: ['{{USER_HANDLE}}/group chats', '{{USER_HANDLE}}/backups'],
        },
    },
    // Phase 7 — Groups
    {
        group: 'groups',
        method: 'POST',
        path: '/api/groups/all',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        allowExtraKeys: true,
        description: 'List groups',
        seed: {
            clear: ['{{USER_HANDLE}}/groups', '{{USER_HANDLE}}/group chats'],
            files: [
                {
                    path: '{{USER_HANDLE}}/groups/group-1.json',
                    contents: FIXTURE_GROUP_JSON,
                },
                {
                    path: '{{USER_HANDLE}}/group chats/group-chat-1.jsonl',
                    contents: FIXTURE_GROUP_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'groups',
        method: 'POST',
        path: '/api/groups/create',
        body: {
            name: 'Parity Group',
            members: ['Alice'],
            allow_self_responses: 'yes',
            activation_strategy: 2,
            generation_mode: 1,
            disabled_members: [],
            chat_id: 'chat-1',
            chats: ['chat-1'],
            auto_mode_delay: 3,
            generation_mode_join_prefix: '[',
            generation_mode_join_suffix: ']',
        },
        hasSideEffects: true,
        sideEffects: { mode: 'none' },
        compareBody: 'shape',
        description: 'Create group',
        seed: {
            clear: ['{{USER_HANDLE}}/groups'],
        },
    },
    {
        group: 'groups',
        method: 'POST',
        path: '/api/groups/edit',
        body: {
            id: 'group-edit',
            name: 'Group Edit',
            chats: ['group-chat-edit'],
            chat_id: 'group-chat-edit',
            members: ['Alice'],
            allow_self_responses: false,
            activation_strategy: 1,
            generation_mode: 0,
            disabled_members: [],
            auto_mode_delay: 5,
            generation_mode_join_prefix: '',
            generation_mode_join_suffix: '',
            chat_metadata: { legacy: true },
            past_metadata: { legacy: true },
        },
        hasSideEffects: true,
        compareBody: 'exact',
        description: 'Edit group',
        seed: {
            clear: ['{{USER_HANDLE}}/groups'],
        },
    },
    {
        group: 'groups',
        method: 'POST',
        path: '/api/groups/delete',
        body: { id: 'group-delete' },
        hasSideEffects: true,
        compareBody: 'exact',
        description: 'Delete group',
        seed: {
            clear: ['{{USER_HANDLE}}/groups', '{{USER_HANDLE}}/group chats'],
            files: [
                {
                    path: '{{USER_HANDLE}}/groups/group-delete.json',
                    contents: JSON.stringify({ id: 'group-delete', chats: ['group-chat-del-a', 'group-chat-del-b'] }),
                },
                {
                    path: '{{USER_HANDLE}}/group chats/group-chat-del-a.jsonl',
                    contents: FIXTURE_GROUP_CHAT_JSONL,
                },
                {
                    path: '{{USER_HANDLE}}/group chats/group-chat-del-b.jsonl',
                    contents: FIXTURE_GROUP_CHAT_JSONL,
                },
            ],
        },
    },
    // Phase 7 — World Info
    {
        group: 'worldinfo',
        method: 'POST',
        path: '/api/worldinfo/list',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'List world info files',
        seed: {
            clear: ['{{USER_HANDLE}}/worlds'],
            files: [
                {
                    path: '{{USER_HANDLE}}/worlds/Alpha.json',
                    contents: JSON.stringify({ entries: {}, name: 'Alpha', extensions: { depth: 1 } }, null, 4),
                },
                {
                    path: '{{USER_HANDLE}}/worlds/Beta.json',
                    contents: JSON.stringify({ entries: {}, name: 'Beta', extensions: [] }, null, 4),
                },
            ],
        },
    },
    {
        group: 'worldinfo',
        method: 'POST',
        path: '/api/worldinfo/get',
        body: { name: 'GetWorld' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Get world info file',
        seed: {
            clear: ['{{USER_HANDLE}}/worlds'],
            files: [
                {
                    path: '{{USER_HANDLE}}/worlds/GetWorld.json',
                    contents: FIXTURE_WORLDINFO_JSON,
                },
            ],
        },
    },
    {
        group: 'worldinfo',
        method: 'POST',
        path: '/api/worldinfo/delete',
        body: { name: 'DeleteWorld' },
        hasSideEffects: true,
        compareBody: 'exact',
        description: 'Delete world info file',
        seed: {
            files: [
                {
                    path: '{{USER_HANDLE}}/worlds/DeleteWorld.json',
                    contents: FIXTURE_WORLDINFO_JSON,
                },
            ],
        },
    },
    {
        group: 'worldinfo',
        method: 'POST',
        path: '/api/worldinfo/import',
        requestType: 'multipart',
        multipart: {
            files: [
                {
                    field: 'avatar',
                    filename: 'World-Import.json',
                    contentType: 'application/json',
                    dataBase64: FIXTURE_WORLDINFO_JSON_BASE64,
                },
            ],
        },
        hasSideEffects: true,
        compareBody: 'exact',
        description: 'Import world info file',
        seed: {
            clear: ['{{USER_HANDLE}}/worlds'],
        },
    },
    {
        group: 'worldinfo',
        method: 'POST',
        path: '/api/worldinfo/edit',
        body: { name: 'EditWorld', data: FIXTURE_WORLDINFO_EDIT },
        hasSideEffects: true,
        compareBody: 'exact',
        description: 'Edit world info file',
        seed: {
            clear: ['{{USER_HANDLE}}/worlds'],
        },
    },
    // Phase 8 — Settings
    {
        group: 'settings',
        method: 'POST',
        path: '/api/settings/make-snapshot',
        body: {},
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Create settings snapshot',
        seed: {
            clear: ['{{USER_HANDLE}}/backups'],
        },
    },
    {
        group: 'settings',
        method: 'POST',
        path: '/api/settings/get-snapshots',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List settings snapshots',
    },
    {
        group: 'settings',
        method: 'POST',
        path: '/api/settings/load-snapshot',
        body: { name: 'invalid-snapshot' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Load settings snapshot (invalid)',
    },
    {
        group: 'settings',
        method: 'POST',
        path: '/api/settings/restore-snapshot',
        body: { name: 'invalid-snapshot' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Restore settings snapshot (invalid)',
    },
    {
        group: 'settings',
        method: 'POST',
        path: '/api/settings/get',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Get settings bundle',
    },
    {
        group: 'settings',
        method: 'POST',
        path: '/api/settings/save',
        body: { test: true },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'shape',
        description: 'Save settings',
        seed: {
            remove: ['{{USER_HANDLE}}/settings.json'],
        },
    },
    // Phase 8 — Presets
    {
        group: 'presets',
        method: 'POST',
        path: '/api/presets/save',
        body: { apiId: 'openai', name: 'preset-parity', preset: { temperature: 0.5 } },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'shape',
        description: 'Save preset',
        seed: {
            remove: ['{{USER_HANDLE}}/OpenAI Settings/preset-parity.json'],
        },
    },
    {
        group: 'presets',
        method: 'POST',
        path: '/api/presets/delete',
        body: { apiId: 'openai', name: 'preset-delete' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Delete preset',
        seed: {
            files: [
                { path: '{{USER_HANDLE}}/OpenAI Settings/preset-delete.json', contents: '{}' },
            ],
        },
    },
    {
        group: 'presets',
        method: 'POST',
        path: '/api/presets/restore',
        body: { apiId: 'openai', name: 'Nonexistent' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Restore preset (non-default)',
    },
    // Phase 8 — Secrets
    {
        group: 'secrets',
        method: 'POST',
        path: '/api/secrets/write',
        body: { key: 'libre_url', value: 'http://example.com', label: 'parity' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'shape',
        capture: { secretId: true },
        description: 'Write secret',
        seed: {
            remove: ['{{USER_HANDLE}}/secrets.json'],
        },
    },
    {
        group: 'secrets',
        method: 'POST',
        path: '/api/secrets/read',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Read secrets state',
    },
    {
        group: 'secrets',
        method: 'POST',
        path: '/api/secrets/find',
        body: { key: 'libre_url', id: '{{SECRET_ID}}' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Find secret by id',
    },
    {
        group: 'secrets',
        method: 'POST',
        path: '/api/secrets/rename',
        body: { key: 'libre_url', id: '{{SECRET_ID}}', label: 'parity-renamed' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Rename secret',
    },
    {
        group: 'secrets',
        method: 'POST',
        path: '/api/secrets/rotate',
        body: { key: 'libre_url', id: '{{SECRET_ID}}' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Rotate secret',
    },
    {
        group: 'secrets',
        method: 'POST',
        path: '/api/secrets/delete',
        body: { key: 'libre_url', id: '{{SECRET_ID}}' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Delete secret',
    },
    {
        group: 'secrets',
        method: 'POST',
        path: '/api/secrets/view',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'View secrets (forbidden by config)',
    },
    // Phase 8 — Users (public)
    {
        group: 'users-public',
        method: 'POST',
        path: '/api/users/list',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List users',
    },
    {
        group: 'users-public',
        method: 'POST',
        path: '/api/users/login',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Login (missing handle)',
    },
    {
        group: 'users-public',
        method: 'POST',
        path: '/api/users/recover-step1',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Recover step 1 (missing handle)',
    },
    {
        group: 'users-public',
        method: 'POST',
        path: '/api/users/recover-step2',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Recover step 2 (missing fields)',
    },
    // Phase 8 — Users (private)
    {
        group: 'users-private',
        method: 'GET',
        path: '/api/users/me',
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Get current user',
    },
    {
        group: 'users-private',
        method: 'POST',
        path: '/api/users/logout',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Logout',
    },
    {
        group: 'users-private',
        method: 'POST',
        path: '/api/users/change-avatar',
        body: { handle: 'default-user', avatar: '' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'shape',
        description: 'Change avatar',
    },
    {
        group: 'users-private',
        method: 'POST',
        path: '/api/users/backup',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Backup user (missing handle)',
    },
    {
        group: 'users-private',
        method: 'POST',
        path: '/api/users/reset-settings',
        body: {},
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'shape',
        description: 'Reset settings',
    },
    {
        group: 'users-private',
        method: 'POST',
        path: '/api/users/change-name',
        body: { handle: 'default-user', name: 'Parity User' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'shape',
        description: 'Change user name',
    },
    {
        group: 'users-private',
        method: 'POST',
        path: '/api/users/change-password',
        body: { handle: 'default-user', newPassword: 'parity-pass' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'shape',
        description: 'Change password',
    },
    {
        group: 'users-private',
        method: 'POST',
        path: '/api/users/reset-step1',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Reset step 1',
    },
    {
        group: 'users-private',
        method: 'POST',
        path: '/api/users/reset-step2',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Reset step 2 (missing code)',
    },
    // Phase 8 — Users (admin)
    {
        group: 'users-admin',
        method: 'POST',
        path: '/api/users/slugify',
        body: { text: 'Parity User' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Slugify handle',
    },
    {
        group: 'users-admin',
        method: 'POST',
        path: '/api/users/create',
        body: { handle: 'parity-user', name: 'Parity User', admin: false, password: '' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'shape',
        description: 'Create user',
    },
    {
        group: 'users-admin',
        method: 'POST',
        path: '/api/users/get',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List users (admin)',
    },
    {
        group: 'users-admin',
        method: 'POST',
        path: '/api/users/disable',
        body: { handle: 'parity-user' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Disable user',
    },
    {
        group: 'users-admin',
        method: 'POST',
        path: '/api/users/enable',
        body: { handle: 'parity-user' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Enable user',
    },
    {
        group: 'users-admin',
        method: 'POST',
        path: '/api/users/promote',
        body: { handle: 'parity-user' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Promote user',
    },
    {
        group: 'users-admin',
        method: 'POST',
        path: '/api/users/demote',
        body: { handle: 'parity-user' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Demote user',
    },
    {
        group: 'users-admin',
        method: 'POST',
        path: '/api/users/delete',
        body: { handle: 'parity-user', purge: true },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Delete user',
    },
    // Phase 8 — Tokenizers
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/llama/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Llama encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/nerdstash/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Nerdstash encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/nerdstash_v2/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Nerdstash v2 encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/mistral/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Mistral encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/yi/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Yi encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/gemma/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Gemma encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/jamba/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Jamba encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/gpt2/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'GPT-2 encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/claude/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Claude encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/llama3/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Llama3 encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/qwen2/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Qwen2 encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/command-r/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Command-R encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/command-a/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Command-A encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/nemo/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Nemo encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/deepseek/encode',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'DeepSeek encode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/llama/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Llama decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/nerdstash/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Nerdstash decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/nerdstash_v2/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Nerdstash v2 decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/mistral/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Mistral decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/yi/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Yi decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/gemma/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Gemma decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/jamba/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Jamba decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/gpt2/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'GPT-2 decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/claude/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Claude decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/llama3/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Llama3 decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/qwen2/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Qwen2 decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/command-r/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Command-R decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/command-a/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Command-A decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/nemo/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Nemo decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/deepseek/decode',
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'DeepSeek decode',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/openai/encode',
        query: { model: 'gpt-4o-mini' },
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'OpenAI encode (router)',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/openai/decode',
        query: { model: 'gpt-4o-mini' },
        body: { ids: [1, 2, 3] },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'OpenAI decode (router)',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/openai/count',
        query: { model: 'gpt-4o-mini' },
        body: [{ role: 'user', content: 'Hello' }],
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'OpenAI count (router)',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/remote/kobold/count',
        body: { text: 'Hello', url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Remote Kobold count (invalid URL)',
    },
    {
        group: 'tokenizers',
        method: 'POST',
        path: '/api/tokenizers/remote/textgenerationwebui/encode',
        body: { text: 'Hello', url: '', api_type: 'ooba', vllm_model: '', aphrodite_model: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Remote TextGen encode (invalid URL)',
    },
    // Phase 8 — Extensions
    {
        group: 'extensions',
        method: 'POST',
        path: '/api/extensions/install',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Install extension (missing URL)',
    },
    {
        group: 'extensions',
        method: 'POST',
        path: '/api/extensions/update',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Update extension (missing name)',
    },
    {
        group: 'extensions',
        method: 'POST',
        path: '/api/extensions/branches',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List extension branches (missing name)',
    },
    {
        group: 'extensions',
        method: 'POST',
        path: '/api/extensions/switch',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Switch extension branch (missing data)',
    },
    {
        group: 'extensions',
        method: 'POST',
        path: '/api/extensions/move',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Move extension (missing data)',
    },
    {
        group: 'extensions',
        method: 'POST',
        path: '/api/extensions/version',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Extension version (missing name)',
    },
    {
        group: 'extensions',
        method: 'POST',
        path: '/api/extensions/delete',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Delete extension (missing name)',
    },
    {
        group: 'extensions',
        method: 'GET',
        path: '/api/extensions/discover',
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Discover extensions',
    },
    // Phase 8 — Content manager
    {
        group: 'content',
        method: 'POST',
        path: '/api/content/importURL',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Import content by URL (missing url)',
    },
    {
        group: 'content',
        method: 'POST',
        path: '/api/content/importUUID',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Import content by UUID (missing url)',
    },
    // Phase 9 — Image metadata
    {
        group: 'image-metadata',
        method: 'POST',
        path: '/api/image-metadata',
        body: { path: 'backgrounds/meta-test.png', type: 'bg' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'shape',
        description: 'Get image metadata (single)',
        seed: {
            clear: ['{{USER_HANDLE}}/backgrounds'],
            remove: ['{{USER_HANDLE}}/image-metadata.json'],
            files: [
                {
                    path: '{{USER_HANDLE}}/backgrounds/meta-test.png',
                    contentsBase64: FIXTURE_PNG_BASE64,
                },
            ],
        },
    },
    {
        group: 'image-metadata',
        method: 'POST',
        path: '/api/image-metadata/all',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List all image metadata',
    },
    {
        group: 'image-metadata',
        method: 'POST',
        path: '/api/image-metadata/cleanup',
        body: {},
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Cleanup orphaned image metadata',
        seed: {
            clear: ['{{USER_HANDLE}}/backgrounds'],
            files: [
                {
                    path: '{{USER_HANDLE}}/image-metadata.json',
                    contents: FIXTURE_IMAGE_METADATA_INDEX_JSON,
                },
            ],
        },
    },
    // Phase 9 — Backups
    {
        group: 'backups',
        method: 'POST',
        path: '/api/backups/chat/get',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List chat backups',
        seed: {
            clear: ['{{USER_HANDLE}}/backups'],
            files: [
                {
                    path: '{{USER_HANDLE}}/backups/chat_Test.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'backups',
        method: 'POST',
        path: '/api/backups/chat/download',
        body: { name: 'chat_Test.jsonl' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Download chat backup',
        seed: {
            clear: ['{{USER_HANDLE}}/backups'],
            files: [
                {
                    path: '{{USER_HANDLE}}/backups/chat_Test.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    {
        group: 'backups',
        method: 'POST',
        path: '/api/backups/chat/delete',
        body: { name: 'chat_Test.jsonl' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Delete chat backup',
        seed: {
            clear: ['{{USER_HANDLE}}/backups'],
            files: [
                {
                    path: '{{USER_HANDLE}}/backups/chat_Test.jsonl',
                    contents: FIXTURE_CHAT_JSONL,
                },
            ],
        },
    },
    // Phase 9 — Stats
    {
        group: 'stats',
        method: 'POST',
        path: '/api/stats/update',
        body: { example: 'value' },
        hasSideEffects: true,
        sideEffects: { mode: 'none' },
        compareBody: 'exact',
        description: 'Update stats',
        seed: {
            remove: ['{{USER_HANDLE}}/stats.json'],
        },
    },
    {
        group: 'stats',
        method: 'POST',
        path: '/api/stats/get',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Get stats',
    },
    {
        group: 'stats',
        method: 'POST',
        path: '/api/stats/recreate',
        body: {},
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Recreate stats',
        seed: {
            clear: ['{{USER_HANDLE}}/characters', '{{USER_HANDLE}}/chats'],
            remove: ['{{USER_HANDLE}}/stats.json'],
        },
    },
    // Phase 9 — Vectors
    {
        group: 'vector',
        method: 'POST',
        path: '/api/vector/insert',
        body: {
            collectionId: 'vec-a',
            items: [
                { hash: 1, text: 'hello', index: 0 },
            ],
            source: 'webllm',
            model: 'parity',
            embeddings: FIXTURE_VECTOR_EMBEDDINGS,
        },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Insert vectors (collection A)',
        seed: {
            clear: ['{{USER_HANDLE}}/vectors'],
        },
    },
    {
        group: 'vector',
        method: 'POST',
        path: '/api/vector/insert',
        body: {
            collectionId: 'vec-b',
            items: [
                { hash: 2, text: 'world', index: 0 },
            ],
            source: 'webllm',
            model: 'parity',
            embeddings: FIXTURE_VECTOR_EMBEDDINGS,
        },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Insert vectors (collection B)',
    },
    {
        group: 'vector',
        method: 'POST',
        path: '/api/vector/list',
        body: { collectionId: 'vec-a', source: 'webllm', model: 'parity' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'List vector hashes',
    },
    {
        group: 'vector',
        method: 'POST',
        path: '/api/vector/query',
        body: {
            collectionId: 'vec-a',
            searchText: 'hello',
            topK: 1,
            threshold: 0.5,
            source: 'webllm',
            model: 'parity',
            embeddings: FIXTURE_VECTOR_EMBEDDINGS,
        },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Query vectors',
    },
    {
        group: 'vector',
        method: 'POST',
        path: '/api/vector/query-multi',
        body: {
            collectionIds: ['vec-a', 'vec-b'],
            searchText: 'hello',
            topK: 2,
            threshold: 0.5,
            source: 'webllm',
            model: 'parity',
            embeddings: FIXTURE_VECTOR_EMBEDDINGS,
        },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Query multiple collections',
    },
    {
        group: 'vector',
        method: 'POST',
        path: '/api/vector/delete',
        body: {
            collectionId: 'vec-a',
            hashes: [1],
            source: 'webllm',
            model: 'parity',
        },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Delete vectors',
    },
    {
        group: 'vector',
        method: 'POST',
        path: '/api/vector/purge',
        body: { collectionId: 'vec-b' },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Purge vector collection',
    },
    {
        group: 'vector',
        method: 'POST',
        path: '/api/vector/purge-all',
        body: {},
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Purge all vectors',
    },
    // Phase 9 — Search (validation-only, avoids external calls)
    {
        group: 'search',
        method: 'POST',
        path: '/api/search/serpapi',
        body: { query: 'test' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Search via SerpApi (missing key → 400)',
    },
    {
        group: 'search',
        method: 'POST',
        path: '/api/search/transcript',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Transcript (missing id → 400)',
    },
    {
        group: 'search',
        method: 'POST',
        path: '/api/search/searxng',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'SearXNG (missing baseUrl/query → 400)',
    },
    {
        group: 'search',
        method: 'POST',
        path: '/api/search/tavily',
        body: { query: 'test' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Tavily (missing key → 400)',
    },
    {
        group: 'search',
        method: 'POST',
        path: '/api/search/koboldcpp',
        body: { query: 'test' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'KoboldCpp (missing url → 400)',
    },
    {
        group: 'search',
        method: 'POST',
        path: '/api/search/serper',
        body: { query: 'test' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Serper (missing key → 400)',
    },
    {
        group: 'search',
        method: 'POST',
        path: '/api/search/zai',
        body: { query: 'test' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Z.AI (missing key → 400)',
    },
    {
        group: 'search',
        method: 'POST',
        path: '/api/search/visit',
        body: { url: 'ftp://example.com' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Visit (invalid protocol → 400)',
    },
    // Phase 9 — Translate (validation-only, avoids external calls)
    {
        group: 'translate',
        method: 'POST',
        path: '/api/translate/libre',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'LibreTranslate (missing url → 400)',
    },
    {
        group: 'translate',
        method: 'POST',
        path: '/api/translate/google',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Google Translate (missing text/lang → 400)',
    },
    {
        group: 'translate',
        method: 'POST',
        path: '/api/translate/yandex',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Yandex Translate (missing chunks/lang → 400)',
    },
    {
        group: 'translate',
        method: 'POST',
        path: '/api/translate/lingva',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Lingva (missing text/lang → 400)',
    },
    {
        group: 'translate',
        method: 'POST',
        path: '/api/translate/deepl',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'DeepL (missing key/text/lang → 400)',
    },
    {
        group: 'translate',
        method: 'POST',
        path: '/api/translate/onering',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'OneRing (missing text/lang → 400)',
    },
    {
        group: 'translate',
        method: 'POST',
        path: '/api/translate/deeplx',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'DeepLX (missing text/lang → 400)',
    },
    {
        group: 'translate',
        method: 'POST',
        path: '/api/translate/bing',
        body: {},
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Bing Translate (missing text/lang → 400)',
    },
    // Phase 9 — Classify (validation-only)
    {
        group: 'classify',
        method: 'POST',
        path: '/api/extra/classify/labels',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Classify labels',
    },
    {
        group: 'classify',
        method: 'POST',
        path: '/api/extra/classify',
        body: { text: 'Hello' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Classify text',
    },
    // Phase 9 — Caption (validation-only)
    {
        group: 'caption',
        method: 'POST',
        path: '/api/extra/caption',
        body: { image: '' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Caption (invalid image → 400)',
    },
    // Phase 10 — Data Maid
    {
        group: 'data-maid',
        method: 'POST',
        path: '/api/data-maid/report',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        capture: { dataMaid: true },
        description: 'Generate data maid report',
        seed: {
            clear: [
                '{{USER_HANDLE}}/user/images',
                '{{USER_HANDLE}}/user/files',
                '{{USER_HANDLE}}/chats',
                '{{USER_HANDLE}}/group chats',
                '{{USER_HANDLE}}/groups',
                '{{USER_HANDLE}}/characters',
                '{{USER_HANDLE}}/backgrounds',
                '{{USER_HANDLE}}/User Avatars',
                '{{USER_HANDLE}}/thumbnails/avatar',
                '{{USER_HANDLE}}/thumbnails/bg',
                '{{USER_HANDLE}}/thumbnails/persona',
                '{{USER_HANDLE}}/backups',
            ],
            files: [
                {
                    path: '{{USER_HANDLE}}/user/images/data-maid-orphan.png',
                    contentsBase64: FIXTURE_PNG_BASE64,
                },
            ],
        },
    },
    {
        group: 'data-maid',
        method: 'GET',
        path: '/api/data-maid/view',
        query: { token: '{{DATA_MAID_TOKEN}}', hash: '{{DATA_MAID_HASH}}' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'View data maid file by hash',
    },
    {
        group: 'data-maid',
        method: 'POST',
        path: '/api/data-maid/delete',
        body: { token: '{{DATA_MAID_TOKEN}}', hashes: ['{{DATA_MAID_HASH}}'] },
        hasSideEffects: true,
        sideEffects: { mode: 'pathsOnly' },
        compareBody: 'exact',
        description: 'Delete data maid file by hash',
    },
    {
        group: 'data-maid',
        method: 'POST',
        path: '/api/data-maid/finalize',
        body: { token: '{{DATA_MAID_TOKEN}}' },
        hasSideEffects: false,
        compareBody: 'exact',
        description: 'Finalize data maid token',
    },
    // Phase 11 — Providers and backends (validation-only unless noted)
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/caption-image',
        body: { api: 'openai' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'OpenAI caption (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/generate-voice',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'OpenAI TTS (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/custom/generate-voice',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Custom OpenAI-compatible TTS (network)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/electronhub/generate-voice',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ElectronHub TTS (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/electronhub/models',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ElectronHub models (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/chutes/generate-voice',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Chutes TTS (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/chutes/models/embedding',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Chutes embedding models (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/nanogpt/models/embedding',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'NanoGPT embedding models (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/generate-image',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'OpenAI image generation (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/generate-video',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'OpenAI video generation (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/transcribe-audio',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'OpenAI transcribe (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/groq/transcribe-audio',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Groq transcribe (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/mistral/transcribe-audio',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Mistral transcribe (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/zai/transcribe-audio',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Z.AI transcribe (missing key)',
    },
    {
        group: 'openai',
        method: 'POST',
        path: '/api/openai/chutes/transcribe-audio',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Chutes transcribe (missing key)',
    },
    {
        group: 'google',
        method: 'POST',
        path: '/api/google/list-voices',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Google TTS list voices',
    },
    {
        group: 'google',
        method: 'POST',
        path: '/api/google/list-native-voices',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Google native TTS voices',
    },
    {
        group: 'google',
        method: 'POST',
        path: '/api/google/caption-image',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Google caption image (network)',
    },
    {
        group: 'google',
        method: 'POST',
        path: '/api/google/generate-voice',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Google voice generate (network)',
    },
    {
        group: 'google',
        method: 'POST',
        path: '/api/google/generate-native-tts',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Google native TTS generate (network)',
    },
    {
        group: 'google',
        method: 'POST',
        path: '/api/google/generate-image',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Google image generate (network)',
    },
    {
        group: 'google',
        method: 'POST',
        path: '/api/google/generate-video',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Google video generate (network)',
    },
    {
        group: 'anthropic',
        method: 'POST',
        path: '/api/anthropic/caption-image',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Anthropic caption (invalid image)',
    },
    {
        group: 'openrouter',
        method: 'POST',
        path: '/api/openrouter/image/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'OpenRouter image generate (missing key)',
    },
    {
        group: 'openrouter',
        method: 'POST',
        path: '/api/openrouter/models/providers',
        body: { model: 'gpt-4o-mini' },
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'OpenRouter model providers (network)',
    },
    {
        group: 'openrouter',
        method: 'POST',
        path: '/api/openrouter/models/multimodal',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'OpenRouter multimodal models (network)',
    },
    {
        group: 'openrouter',
        method: 'POST',
        path: '/api/openrouter/models/embedding',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'OpenRouter embedding models (network)',
    },
    {
        group: 'openrouter',
        method: 'POST',
        path: '/api/openrouter/models/image',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'OpenRouter image models (network)',
    },
    {
        group: 'novelai',
        method: 'POST',
        path: '/api/novelai/status',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'NovelAI status (missing key)',
    },
    {
        group: 'novelai',
        method: 'POST',
        path: '/api/novelai/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'NovelAI generate (missing key)',
    },
    {
        group: 'novelai',
        method: 'POST',
        path: '/api/novelai/generate-image',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'NovelAI image generate (missing key)',
    },
    {
        group: 'novelai',
        method: 'POST',
        path: '/api/novelai/generate-voice',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'NovelAI voice generate (missing key)',
    },
    {
        group: 'azure',
        method: 'POST',
        path: '/api/azure/list',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Azure voices list (missing key)',
    },
    {
        group: 'azure',
        method: 'POST',
        path: '/api/azure/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Azure voice generate (missing key)',
    },
    {
        group: 'volcengine',
        method: 'POST',
        path: '/api/volcengine/generate-voice',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Volcengine voice generate (missing key)',
    },
    {
        group: 'minimax',
        method: 'POST',
        path: '/api/minimax/generate-voice',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Minimax voice generate (missing key)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/recognize',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Speech recognize (model required)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/synthesize',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Speech synthesize (model required)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/pollinations/voices',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Pollinations voices (network)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/pollinations/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Pollinations TTS generate (network)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/elevenlabs/voices',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'ElevenLabs voices (network)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/elevenlabs/voices/add',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'ElevenLabs add voice (network)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/elevenlabs/voice-settings',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'ElevenLabs voice settings (network)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/elevenlabs/synthesize',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'ElevenLabs synthesize (network)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/elevenlabs/recognize',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'ElevenLabs recognize (network)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/elevenlabs/history',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'ElevenLabs history (network)',
    },
    {
        group: 'speech',
        method: 'POST',
        path: '/api/speech/elevenlabs/history-audio',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'ElevenLabs history audio (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/text-workers',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde text workers (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/text-models',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde text models (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/status',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde status (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/cancel-task',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde cancel task (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/task-status',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde task status (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/generate-text',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde generate text (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/sd-samplers',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde samplers (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/sd-models',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde SD models (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/caption-image',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde caption image (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/user-info',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde user info (network)',
    },
    {
        group: 'horde',
        method: 'POST',
        path: '/api/horde/generate-image',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Horde generate image (network)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/ping',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'SD ping (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/upscalers',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'SD upscalers (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/vaes',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'SD VAEs (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/samplers',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'SD samplers (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/schedulers',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'SD schedulers (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/models',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'SD models (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/get-model',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'SD get model (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/set-model',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'SD set model (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/generate',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'SD generate (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/sd-next/upscalers',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'SD Next upscalers (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/ping',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI ping (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/samplers',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI samplers (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/models',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI models (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/schedulers',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI schedulers (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/vaes',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI VAEs (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/workflows',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI workflows (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/workflow',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI workflow (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/save-workflow',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI save workflow (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/delete-workflow',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI delete workflow (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/rename-workflow',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI rename workflow (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/comfy/generate',
        body: { url: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'ComfyUI generate (invalid URL)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/together/models',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Together models (network)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/together/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Together generate (network)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/stability/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Stability generate (network)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/pollinations/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Pollinations generate (network)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/huggingface/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Hugging Face generate (network)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/electronhub/models',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'ElectronHub models (network)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/electronhub/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'ElectronHub generate (network)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/chutes/models',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Chutes models (network)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/chutes/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Chutes generate (network)',
    },
    {
        group: 'sd',
        method: 'POST',
        path: '/api/sd/generic/generate',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Generic SD generate (network)',
    },
    {
        group: 'backends-text',
        method: 'POST',
        path: '/api/backends/text-completions/status',
        body: { api_server: '', api_type: 'ooba' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Text completions status (invalid URL)',
    },
    {
        group: 'backends-text',
        method: 'POST',
        path: '/api/backends/text-completions/props',
        body: { api_server: '', api_type: 'ooba' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Text completions props (invalid URL)',
    },
    {
        group: 'backends-text',
        method: 'POST',
        path: '/api/backends/text-completions/generate',
        body: { api_server: '', api_type: 'ooba' },
        hasSideEffects: false,
        compareBody: 'shape',
        description: 'Text completions generate (invalid URL)',
    },
    {
        group: 'backends-kobold',
        method: 'POST',
        path: '/api/backends/kobold/generate',
        body: { api_server: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Kobold generate (invalid URL)',
    },
    {
        group: 'backends-kobold',
        method: 'POST',
        path: '/api/backends/kobold/status',
        body: { api_server: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Kobold status (invalid URL)',
    },
    {
        group: 'backends-kobold',
        method: 'POST',
        path: '/api/backends/kobold/transcribe-audio',
        body: { api_server: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Kobold transcribe (invalid URL)',
    },
    {
        group: 'backends-kobold',
        method: 'POST',
        path: '/api/backends/kobold/embed',
        body: { api_server: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Kobold embed (invalid URL)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/status',
        body: { api_server: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Chat completions status (invalid URL)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/bias',
        body: { api_server: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Chat completions bias (invalid URL)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/generate',
        body: { api_server: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Chat completions generate (invalid URL)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/process',
        body: { api_server: '' },
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Chat completions process (invalid URL)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/multimodal-models/pollinations',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Multimodal models (Pollinations, network)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/multimodal-models/aimlapi',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Multimodal models (AIMLAPI, network)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/multimodal-models/nanogpt',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Multimodal models (NanoGPT, network)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/multimodal-models/electronhub',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Multimodal models (ElectronHub, network)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/multimodal-models/chutes',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Multimodal models (Chutes, network)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/multimodal-models/mistral',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Multimodal models (Mistral, network)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/multimodal-models/xai',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Multimodal models (xAI, network)',
    },
    {
        group: 'backends-chat',
        method: 'POST',
        path: '/api/backends/chat-completions/multimodal-models/moonshot',
        body: {},
        hasSideEffects: false,
        compareBody: 'shape',
        requiresNetwork: true,
        description: 'Multimodal models (Moonshot, network)',
    },
];

export function isSafeEndpoint(endpoint) {
    return !endpoint?.hasSideEffects && !endpoint?.seed;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Sanitizes a URL path into a safe directory name.
 * @param {string} urlPath
 * @returns {string}
 */
function pathToFixtureName(urlPath) {
    return urlPath
        .replace(/^\//, '')
        .replace(/\//g, '__')
        .replace(/[^a-zA-Z0-9_-]/g, '_');
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

const FIXTURE_SERVER_PORT = Number.parseInt(process.env.PARITY_FIXTURE_PORT || '9123', 10);
const FIXTURE_SERVER_URL = `http://127.0.0.1:${FIXTURE_SERVER_PORT}`;
const CONTEXT = {};

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
 * Capture dynamic context values from a response.
 * @param {EndpointDef} endpoint
 * @param {{status: number, headers: Record<string, string>, body: any}} captured
 */
function captureContextFromResponse(endpoint, captured) {
    if (!endpoint?.capture) return;

    if (endpoint.capture.dataMaid) {
        const body = captured?.body;
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

    if (endpoint.capture.secretId) {
        const body = captured?.body;
        if (body && typeof body === 'object' && typeof body.id === 'string' && body.id.length > 0) {
            CONTEXT.SECRET_ID = body.id;
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
 * Fetch with a small retry on network errors.
 * @param {string} url
 * @param {RequestInit} options
 * @param {number} retries
 * @param {number} delayMs
 */
async function fetchWithRetry(url, options, retries = 1, delayMs = 200) {
    let lastError = null;
    for (let attempt = 0; attempt <= retries; attempt++) {
        try {
            return await fetch(url, options);
        } catch (error) {
            lastError = error;
            if (attempt < retries) {
                await new Promise(resolve => setTimeout(resolve, delayMs));
            }
        }
    }
    throw lastError;
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
 * Sends a request to the Node server and captures the response.
 * @param {EndpointDef} endpoint
 * @returns {Promise<{status: number, headers: Record<string, string>, body: any}>}
 */
async function captureEndpoint(endpoint) {
    const url = new URL(endpoint.path, NODE_URL);
    if (endpoint.query) {
        url.search = new URLSearchParams(endpoint.query).toString();
    }

    const requestType = endpoint.requestType || (endpoint.multipart ? 'multipart' : (endpoint.body ? 'json' : 'none'));
    /** @type {RequestInit} */
    const fetchOptions = {
        method: endpoint.method,
        headers: buildHeaders(endpoint.headers, requestType),
    };

    if (requestType === 'multipart' && endpoint.multipart) {
        const form = new FormData();
        const fields = endpoint.multipart.fields || {};
        for (const [key, value] of Object.entries(fields)) {
            form.append(key, String(value));
        }
        const files = endpoint.multipart.files || [];
        for (const file of files) {
            appendFormFile(form, file);
        }
        fetchOptions.body = form;
    } else if (requestType === 'json' && endpoint.body && endpoint.method !== 'GET') {
        fetchOptions.body = JSON.stringify(endpoint.body);
    }

    const response = await fetchWithRetry(url.toString(), fetchOptions);

    // Capture response headers as plain object
    const responseHeaders = {};
    response.headers.forEach((value, key) => {
        responseHeaders[key] = value;
    });

    // Capture body
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

    return {
        status: response.status,
        headers: responseHeaders,
        body,
    };
}

/**
 * Stores a captured fixture to disk.
 * @param {EndpointDef} endpoint
 * @param {{status: number, headers: Record<string, string>, body: any}} captured
 * @param {{pre?: any, post?: any}} [sideEffects]
 */
function storeFixture(endpoint, captured, sideEffects = {}) {
    const baseName = pathToFixtureName(endpoint.path);
    const prefix = endpoint.sequence !== null && endpoint.sequence !== undefined
        ? `${String(endpoint.sequence).padStart(4, '0')}__`
        : '';
    const fixtureName = `${prefix}${baseName}`;
    const fixtureDir = path.join(FIXTURES_DIR, fixtureName);

    fs.mkdirSync(fixtureDir, { recursive: true });

    // Store the request definition
    const requestData = {
        group: endpoint.group,
        method: endpoint.method,
        path: endpoint.path,
        sequence: endpoint.sequence ?? null,
        query: endpoint.query || null,
        headers: endpoint.headers || {},
        body: endpoint.body || null,
        requestType: endpoint.requestType || null,
        multipart: endpoint.multipart || null,
        compareBody: endpoint.compareBody || null,
        allowExtraKeys: endpoint.allowExtraKeys || false,
        hasSideEffects: endpoint.hasSideEffects || false,
        sideEffects: endpoint.sideEffects || null,
        seed: endpoint.seed || null,
        capture: endpoint.capture || null,
        skipCompare: endpoint.skipCompare || false,
        requiresNetwork: endpoint.requiresNetwork || false,
        description: endpoint.description || '',
        capturedAt: new Date().toISOString(),
    };

    fs.writeFileSync(
        path.join(fixtureDir, 'request.json'),
        JSON.stringify(requestData, null, 2),
    );

    // Store the response
    fs.writeFileSync(
        path.join(fixtureDir, 'response.json'),
        JSON.stringify(captured, null, 2),
    );

    // Create side-effects directory for write operations
    if (endpoint.hasSideEffects) {
        const sideDir = path.join(fixtureDir, 'side-effects');
        fs.mkdirSync(sideDir, { recursive: true });

        if (sideEffects.pre) {
            fs.writeFileSync(
                path.join(sideDir, 'pre.json'),
                JSON.stringify(sideEffects.pre, null, 2),
            );
        }
        if (sideEffects.post) {
            fs.writeFileSync(
                path.join(sideDir, 'post.json'),
                JSON.stringify(sideEffects.post, null, 2),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
    parseCliArgs();
    const { isLive, resolvedDataRoot } = assertLiveDataRootAllowed();

    console.log(`Parity Capture — Node URL: ${NODE_URL}`);
    console.log(`Fixtures directory: ${FIXTURES_DIR}`);
    if (isLive) {
        console.log(`Data root: ${resolvedDataRoot}`);
    }
    console.log();

    ensureUploadsDir();

    // Filter endpoints by group if specified
    let endpoints = FILTER_GROUP
        ? ENDPOINTS.filter(e => e.group === FILTER_GROUP)
        : ENDPOINTS;

    if (SAFE_ONLY) {
        endpoints = endpoints.filter(e => !e.hasSideEffects && !e.seed);
    }

    if (!ALLOW_NETWORK && !ENV_ALLOW_NETWORK) {
        const before = endpoints.length;
        endpoints = endpoints.filter(e => !e.requiresNetwork);
        if (before !== endpoints.length) {
            console.warn('⚠ Networked endpoints filtered out. Use --allow-network or PARITY_ALLOW_NETWORK=1 to enable.');
        }
    }

    if (isLive && !(ALLOW_LIVE_WRITES || ENV_ALLOW_LIVE_WRITES)) {
        const before = endpoints.length;
        endpoints = endpoints.filter(e => !e.hasSideEffects && !e.seed);
        if (before !== endpoints.length) {
            console.warn('⚠ Live data root detected — filtering endpoints with side effects or seeds.');
            console.warn('  To allow live writes, pass --allow-live-writes or set PARITY_ALLOW_LIVE_WRITES=1.');
        }
    }

    if (endpoints.length === 0) {
        if (FILTER_GROUP) {
            console.error(`No endpoints found for group: "${FILTER_GROUP}"`);
        } else {
            console.error('No endpoints found for capture.');
        }
        process.exit(1);
    }

    console.log(`Capturing ${endpoints.length} endpoint(s)...`);
    console.log();

    let fixtureServer = null;
    if (JSON.stringify(endpoints).includes('{{FIXTURE_URL}}')) {
        fixtureServer = await startFixtureServer();
        console.log(`  Fixture server running at ${FIXTURE_SERVER_URL}`);
    }

    let passed = 0;
    let failed = 0;

    try {
        let sequence = 0;
        for (const rawEndpoint of endpoints) {
            const endpoint = replaceTokensDeep(rawEndpoint);
            const label = `${endpoint.method} ${endpoint.path}`;
            try {
                let preSnapshot = null;
                let postSnapshot = null;

                applySeed(endpoint.seed);

                if (endpoint.hasSideEffects) {
                    if (!fs.existsSync(DATA_ROOT)) {
                        throw new Error(`Data root not found: ${DATA_ROOT}`);
                    }
                    preSnapshot = await snapshotDataRoot(DATA_ROOT);
                }

                const captured = await captureEndpoint(endpoint);
                captureContextFromResponse(endpoint, captured);

                if (endpoint.hasSideEffects) {
                    postSnapshot = await snapshotDataRoot(DATA_ROOT);
                }

                storeFixture(
                    { ...endpoint, sequence },
                    captured,
                    { pre: preSnapshot, post: postSnapshot },
                );
                console.log(`  ✓ ${label} → ${captured.status}`);
                passed++;
                sequence += 1;
            } catch (error) {
                console.error(`  ✗ ${label} — ${error.message}`);
                failed++;
            }
        }
    } finally {
        if (fixtureServer) {
            fixtureServer.close();
        }
    }

    console.log();
    console.log(`Done: ${passed} captured, ${failed} failed.`);

    if (failed > 0) {
        process.exit(1);
    }
}

const ENTRY_PATH = process.argv[1];
if (ENTRY_PATH && import.meta.url === pathToFileURL(ENTRY_PATH).href) {
    main().catch(err => {
        console.error('Fatal error:', err);
        process.exit(1);
    });
}

export { ENDPOINTS };
