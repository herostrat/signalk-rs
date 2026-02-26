#!/usr/bin/env node
'use strict';

/**
 * Side-by-side conformance runner.
 *
 * Sends the same HTTP requests to both:
 *   - ORIGINAL_URL  (reference signalk-server)
 *   - RS_URL        (signalk-rs)
 *
 * Compares:
 *   1. HTTP status codes  — must be identical
 *   2. JSON structure     — required fields must be present in both
 *   3. Value types        — field types must match (values may differ)
 *
 * Non-deterministic fields (timestamps, tokens, UUIDs) are compared by type only.
 *
 * Exit code: number of failed comparisons (0 = all pass).
 */

const http = require('http');

const ORIGINAL = process.env.ORIGINAL_URL || 'http://localhost:3000';
const RS       = process.env.RS_URL        || 'http://localhost:3001';

// ── HTTP helper ──────────────────────────────────────────────────────────────

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

// ── Structural comparator ─────────────────────────────────────────────────────

/** Return a set of dotted key paths present in `obj`. */
function keyPaths(obj, prefix = '') {
  if (obj === null || typeof obj !== 'object') return [prefix];
  const paths = [];
  for (const [k, v] of Object.entries(obj)) {
    const p = prefix ? `${prefix}.${k}` : k;
    if (typeof v === 'object' && v !== null && !Array.isArray(v)) {
      paths.push(...keyPaths(v, p));
    } else {
      paths.push(p);
    }
  }
  return paths;
}

function typeOf(v) {
  if (v === null) return 'null';
  if (Array.isArray(v)) return 'array';
  return typeof v;
}

// ── Test cases ────────────────────────────────────────────────────────────────

const TESTS = [
  {
    name: 'GET /signalk — discovery',
    method: 'GET',
    path: '/signalk',
    check(orig, rs) {
      const errs = [];
      if (orig.status !== rs.status) {
        errs.push(`status: original=${orig.status} rs=${rs.status}`);
      }
      for (const key of ['endpoints', 'server']) {
        if (!rs.body?.[key]) errs.push(`missing key: ${key}`);
      }
      if (!rs.body?.endpoints?.v1) errs.push('missing endpoints.v1');
      for (const field of ['version', 'signalk-http', 'signalk-ws']) {
        if (!rs.body?.endpoints?.v1?.[field]) errs.push(`missing endpoints.v1.${field}`);
      }
      return errs;
    },
  },
  {
    name: 'GET /signalk/v1/api/ — full model',
    method: 'GET',
    path: '/signalk/v1/api/',
    check(orig, rs) {
      const errs = [];
      if (orig.status !== rs.status) {
        errs.push(`status: original=${orig.status} rs=${rs.status}`);
      }
      // Required top-level fields
      for (const key of ['version', 'self', 'vessels']) {
        if (rs.body?.[key] === undefined) {
          errs.push(`missing top-level key: ${key}`);
        }
        if (orig.body?.[key] !== undefined && typeOf(orig.body[key]) !== typeOf(rs.body?.[key])) {
          errs.push(`type mismatch for ${key}: original=${typeOf(orig.body[key])} rs=${typeOf(rs.body?.[key])}`);
        }
      }
      return errs;
    },
  },
  {
    name: 'GET /signalk/v1/api/vessels — vessel list',
    method: 'GET',
    path: '/signalk/v1/api/vessels',
    check(orig, rs) {
      const errs = [];
      if (orig.status !== rs.status) errs.push(`status: original=${orig.status} rs=${rs.status}`);
      if (typeOf(orig.body) !== typeOf(rs.body)) errs.push(`body type: original=${typeOf(orig.body)} rs=${typeOf(rs.body)}`);
      return errs;
    },
  },
  {
    name: 'GET /signalk/v1/api/vessels/self — self vessel',
    method: 'GET',
    path: '/signalk/v1/api/vessels/self',
    check(orig, rs) {
      const errs = [];
      if (orig.status !== rs.status) errs.push(`status: original=${orig.status} rs=${rs.status}`);
      return errs;
    },
  },
  {
    name: 'GET /signalk/v1/snapshot — 501 Not Implemented',
    method: 'GET',
    path: '/signalk/v1/snapshot',
    check(orig, rs) {
      const errs = [];
      // Both should return 501
      if (rs.status !== 501) errs.push(`expected 501, got ${rs.status}`);
      return errs;
    },
  },
  {
    name: 'GET /no/such/path — 404',
    method: 'GET',
    path: '/signalk/v1/api/does/not/exist',
    check(orig, rs) {
      const errs = [];
      if (orig.status !== rs.status) errs.push(`status: original=${orig.status} rs=${rs.status}`);
      return errs;
    },
  },
  {
    name: 'POST /signalk/v1/auth/login — valid credentials',
    method: 'POST',
    path: '/signalk/v1/auth/login',
    body: { username: 'admin', password: 'admin' },
    check(orig, rs) {
      const errs = [];
      if (orig.status !== 200) {
        // Original has no users configured — just verify our token format when we return 200
        if (rs.status === 200 && !rs.body?.token) {
          errs.push('login 200 response missing token');
        }
        return errs;
      }
      // Both should succeed
      if (rs.status !== 200) {
        errs.push(`login success mismatch: original=${orig.status} rs=${rs.status}`);
      }
      if (rs.status === 200 && !rs.body?.token) {
        errs.push('login 200 response missing token');
      }
      return errs;
    },
  },
  {
    name: 'POST /signalk/v1/auth/login — wrong username → 401',
    method: 'POST',
    path: '/signalk/v1/auth/login',
    body: { username: 'hacker', password: 'x' },
    check(orig, rs) {
      const errs = [];
      if (orig.status !== rs.status) errs.push(`status: original=${orig.status} rs=${rs.status}`);
      return errs;
    },
  },
];

// ── Runner ────────────────────────────────────────────────────────────────────

async function run() {
  let failures = 0;

  console.log(`\nSignalK Conformance Comparison`);
  console.log(`  Original : ${ORIGINAL}`);
  console.log(`  signalk-rs: ${RS}\n`);

  for (const test of TESTS) {
    const [orig, rs] = await Promise.all([
      request(ORIGINAL, test.method, test.path, test.body),
      request(RS,       test.method, test.path, test.body),
    ]);

    const errs = test.check(orig, rs);

    if (errs.length === 0) {
      console.log(`  ✓  ${test.name}`);
    } else {
      failures++;
      console.log(`  ✗  ${test.name}`);
      for (const e of errs) {
        console.log(`       ${e}`);
      }
    }
  }

  console.log(`\n${TESTS.length - failures}/${TESTS.length} passed\n`);
  process.exit(failures);
}

run().catch((err) => {
  console.error('Runner error:', err.message);
  process.exit(1);
});
