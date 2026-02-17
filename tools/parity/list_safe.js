/**
 * Parity Harness — Safe Endpoint Report
 *
 * Lists endpoints that are safe for live parity checks
 * (no side effects and no seed data).
 *
 * Usage:
 *   node tools/parity/list_safe.js
 *   node tools/parity/list_safe.js --group characters
 *   node tools/parity/list_safe.js --summary
 */

import { parseArgs } from 'node:util';
import { ENDPOINTS, isSafeEndpoint } from './capture.js';

const { values: args } = parseArgs({
    options: {
        'group': { type: 'string', default: '' },
        'summary': { type: 'boolean', default: false },
    },
});

const FILTER_GROUP = args['group'];
const SUMMARY_ONLY = args['summary'];

const filtered = FILTER_GROUP
    ? ENDPOINTS.filter(e => e.group === FILTER_GROUP)
    : ENDPOINTS;

const safe = filtered.filter(isSafeEndpoint);

if (safe.length === 0) {
    if (FILTER_GROUP) {
        console.log(`No safe endpoints found for group: "${FILTER_GROUP}"`);
    } else {
        console.log('No safe endpoints found.');
    }
    process.exit(0);
}

if (SUMMARY_ONLY) {
    console.log(`Safe endpoints: ${safe.length}`);
    const byGroup = new Map();
    for (const endpoint of safe) {
        const key = endpoint.group || 'unknown';
        byGroup.set(key, (byGroup.get(key) || 0) + 1);
    }
    for (const [group, count] of [...byGroup.entries()].sort()) {
        console.log(`  ${group}: ${count}`);
    }
    process.exit(0);
}

console.log(`Safe endpoints (${safe.length}):`);
for (const endpoint of safe) {
    const group = endpoint.group || 'unknown';
    console.log(`- ${endpoint.method} ${endpoint.path} (${group})`);
}
