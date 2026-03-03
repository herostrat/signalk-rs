'use strict';

/**
 * Shared conformance test utilities.
 *
 * - deepCompare()   — recursive JSON comparison with ignore list
 * - request()       — HTTP helper (Node.js built-in http)
 * - retry()         — retry with backoff
 * - IGNORE_PATHS    — default non-deterministic field patterns
 */

// TODO: Add JSON Schema validation (SignalK spec schema not yet available
// in machine-readable form). When available, validate both responses against
// the official schema before comparing them to each other.
// See: https://github.com/SignalK/specification PR #671

const http = require('http');

// ── Non-deterministic fields to skip during deep comparison ─────────────────

const IGNORE_PATHS = [
  '*.timestamp',
  '*.token',
  'server.id',
  'server.version',
  '$source',
  '*.$source',
  '*.pgn',
  '*.sentence',
];

// ── HTTP helper ─────────────────────────────────────────────────────────────

function request(baseUrl, method, path, body) {
  return new Promise((resolve, reject) => {
    const url = new URL(path, baseUrl);
    const opts = {
      hostname: url.hostname,
      port:     url.port || 80,
      path:     url.pathname + url.search,
      method,
      headers:  body ? { 'Content-Type': 'application/json' } : {},
    };
    const req = http.request(opts, (res) => {
      let data = '';
      res.on('data', (chunk) => { data += chunk; });
      res.on('end', () => {
        let json = null;
        try { json = JSON.parse(data); } catch (_) {}
        resolve({ status: res.statusCode, body: json, raw: data });
      });
    });
    req.on('error', reject);
    if (body) req.write(JSON.stringify(body));
    req.end();
  });
}

/**
 * HTTP request with Bearer token in Authorization header.
 */
function requestWithAuth(baseUrl, method, path, token) {
  return new Promise((resolve, reject) => {
    const url = new URL(path, baseUrl);
    const opts = {
      hostname: url.hostname,
      port:     url.port || 80,
      path:     url.pathname + url.search,
      method,
      headers:  { 'Authorization': `Bearer ${token}` },
    };
    const req = http.request(opts, (res) => {
      let data = '';
      res.on('data', (chunk) => { data += chunk; });
      res.on('end', () => {
        let json = null;
        try { json = JSON.parse(data); } catch (_) {}
        resolve({ status: res.statusCode, body: json, raw: data });
      });
    });
    req.on('error', reject);
    req.end();
  });
}

// ── Retry helper ────────────────────────────────────────────────────────────

function sleep(ms) {
  return new Promise(r => setTimeout(r, ms));
}

async function retry(fn, label, attempts = 30, delay = 1000) {
  for (let i = 0; i < attempts; i++) {
    try {
      const result = await fn();
      if (result) return result;
    } catch (_) {}
    if (i < attempts - 1) {
      process.stdout.write(`  waiting for ${label} (${i + 1}/${attempts})...\r`);
      await sleep(delay);
    }
  }
  throw new Error(`${label}: timed out after ${attempts} attempts`);
}

// ── Deep recursive comparison ───────────────────────────────────────────────

/**
 * Check if a dotted path matches an ignore pattern.
 * Patterns:
 *   'foo.bar'       — exact match
 *   '*.bar'         — any single segment prefix + .bar
 *   '*.$source'     — any single segment + .$source
 *   '$source'       — path ends with $source
 *   'server.id'     — exact match
 */
function matchesIgnore(path, patterns) {
  for (const pat of patterns) {
    // Exact match
    if (path === pat) return true;

    // Ends-with match for bare field names (e.g. '$source' matches 'a.b.$source')
    if (!pat.includes('.') && path.endsWith('.' + pat)) return true;
    if (!pat.includes('.') && path === pat) return true;

    // Wildcard patterns like '*.timestamp'
    if (pat.startsWith('*.')) {
      const suffix = pat.slice(1); // '.timestamp'
      if (path.endsWith(suffix)) return true;
    }
  }
  return false;
}

/**
 * Deep recursive comparison of two JSON values.
 *
 * @param {*} a           - "original" value
 * @param {*} b           - "rs" value
 * @param {string} path   - current dotted path (for reporting)
 * @param {object} opts   - options
 * @param {string[]} opts.ignorePaths - patterns to skip
 * @param {number}   opts.numTolerance - numeric tolerance (default 0.0001)
 * @returns {{ matches: number, mismatches: Array, missing: Array, extra: Array }}
 */
function deepCompare(a, b, path = '', opts = {}) {
  const ignorePaths = opts.ignorePaths || IGNORE_PATHS;
  const numTolerance = opts.numTolerance !== undefined ? opts.numTolerance : 0.0001;
  const result = { matches: 0, mismatches: [], missing: [], extra: [] };

  if (matchesIgnore(path, ignorePaths)) {
    result.matches++;
    return result;
  }

  const typeA = typeOf(a);
  const typeB = typeOf(b);

  // Both null/undefined
  if (typeA === 'null' && typeB === 'null') {
    result.matches++;
    return result;
  }

  // Type mismatch
  if (typeA !== typeB) {
    result.mismatches.push({ path: path || '(root)', original: a, rs: b, reason: `type: ${typeA} vs ${typeB}` });
    return result;
  }

  // Arrays — compare element by element
  if (typeA === 'array') {
    const len = Math.max(a.length, b.length);
    for (let i = 0; i < len; i++) {
      const childPath = path ? `${path}[${i}]` : `[${i}]`;
      if (i >= a.length) {
        result.extra.push({ path: childPath, rs: b[i] });
      } else if (i >= b.length) {
        result.missing.push({ path: childPath, original: a[i] });
      } else {
        const sub = deepCompare(a[i], b[i], childPath, opts);
        merge(result, sub);
      }
    }
    return result;
  }

  // Objects — compare all keys
  if (typeA === 'object') {
    const allKeys = new Set([...Object.keys(a), ...Object.keys(b)]);
    for (const key of allKeys) {
      const childPath = path ? `${path}.${key}` : key;

      if (matchesIgnore(childPath, ignorePaths)) {
        result.matches++;
        continue;
      }

      const inA = key in a;
      const inB = key in b;

      if (inA && !inB) {
        result.missing.push({ path: childPath, original: a[key] });
      } else if (!inA && inB) {
        result.extra.push({ path: childPath, rs: b[key] });
      } else {
        const sub = deepCompare(a[key], b[key], childPath, opts);
        merge(result, sub);
      }
    }
    return result;
  }

  // Numbers — tolerance comparison
  if (typeA === 'number') {
    if (Math.abs(a - b) <= numTolerance) {
      result.matches++;
    } else {
      result.mismatches.push({ path: path || '(root)', original: a, rs: b, reason: `number: ${a} vs ${b}` });
    }
    return result;
  }

  // Strings, booleans — exact
  if (a === b) {
    result.matches++;
  } else {
    result.mismatches.push({ path: path || '(root)', original: a, rs: b, reason: `value: ${JSON.stringify(a)} vs ${JSON.stringify(b)}` });
  }

  return result;
}

function typeOf(v) {
  if (v === null || v === undefined) return 'null';
  if (Array.isArray(v)) return 'array';
  return typeof v;
}

function merge(target, source) {
  target.matches += source.matches;
  target.mismatches.push(...source.mismatches);
  target.missing.push(...source.missing);
  target.extra.push(...source.extra);
}

/**
 * Print a deepCompare result and return the number of issues.
 */
function printCompareResult(label, result, { verbose = false } = {}) {
  const issues = result.mismatches.length + result.missing.length + result.extra.length;
  if (issues === 0) {
    console.log(`  \u2713  ${label} (${result.matches} fields match)`);
    return 0;
  }

  console.log(`  \u2717  ${label} (${result.matches} match, ${issues} issues)`);
  for (const m of result.mismatches) {
    console.log(`       MISMATCH ${m.path}: ${m.reason}`);
  }
  for (const m of result.missing.slice(0, 10)) {
    console.log(`       MISSING in rs: ${m.path}`);
  }
  if (result.missing.length > 10) {
    console.log(`       ... and ${result.missing.length - 10} more missing`);
  }
  for (const m of result.extra.slice(0, 10)) {
    console.log(`       EXTRA in rs: ${m.path}`);
  }
  if (result.extra.length > 10) {
    console.log(`       ... and ${result.extra.length - 10} more extra`);
  }
  return 1;
}

// ── Exports ─────────────────────────────────────────────────────────────────

module.exports = {
  deepCompare,
  printCompareResult,
  request,
  requestWithAuth,
  retry,
  sleep,
  typeOf,
  matchesIgnore,
  IGNORE_PATHS,
};
