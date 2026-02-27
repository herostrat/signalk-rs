#!/usr/bin/env node
'use strict';

/**
 * Data-level conformance comparison.
 *
 * Injects identical SignalK deltas into both servers via:
 *   - Original:   POST /plugins/signalk-test-injector/inject  (plugin endpoint)
 *   - signalk-rs: POST /test/inject                           (feature-gated endpoint)
 *
 * Then GETs leaf paths on both and compares:
 *   - HTTP status code
 *   - Presence of `value`, `$source`, `timestamp` fields
 *   - Value type match (number, string, object)
 *   - Value range plausibility
 *
 * Exit code: number of failed comparisons (0 = all pass).
 */

const http = require('http');

const ORIGINAL = process.env.ORIGINAL_URL || 'http://localhost:3000';
const RS       = process.env.RS_URL       || 'http://localhost:3001';

// ── HTTP helpers ─────────────────────────────────────────────────────────────

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

async function sleep(ms) {
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

// ── Inject helpers ───────────────────────────────────────────────────────────

async function injectOriginal(delta) {
  const resp = await request(ORIGINAL, 'POST', '/plugins/signalk-test-injector/inject', delta);
  if (resp.status !== 200) {
    throw new Error(`Original inject failed: ${resp.status} ${resp.raw}`);
  }
}

async function injectRs(delta) {
  const resp = await request(RS, 'POST', '/test/inject', delta);
  if (resp.status !== 200) {
    throw new Error(`RS inject failed: ${resp.status} ${resp.raw}`);
  }
}

async function injectBoth(delta) {
  await Promise.all([injectOriginal(delta), injectRs(delta)]);
}

// ── Test delta ───────────────────────────────────────────────────────────────

const TEST_DELTA = {
  context: 'vessels.self',
  updates: [{
    source: { label: 'conformance-test', type: 'test' },
    timestamp: new Date().toISOString(),
    values: [
      { path: 'navigation.speedOverGround',      value: 3.85 },
      { path: 'navigation.courseOverGroundTrue',   value: 1.2217 },  // ~70 degrees in rad
      { path: 'navigation.headingMagnetic',        value: 1.1694 },  // ~67 degrees in rad
      { path: 'navigation.magneticVariation',      value: 0.0524 },  // ~3 degrees in rad
      { path: 'navigation.position', value: { latitude: 54.5, longitude: 10.0 } },
      { path: 'environment.depth.belowTransducer', value: 12.5 },
      { path: 'environment.wind.speedApparent',    value: 7.2 },
      { path: 'environment.wind.angleApparent',    value: 0.7854 },  // ~45 degrees in rad
      { path: 'environment.outside.temperature',   value: 288.15 },  // 15 C in Kelvin
      { path: 'environment.outside.pressure',      value: 101325.0 },
      { path: 'environment.outside.humidity',       value: 0.65 },
      { path: 'environment.water.temperature',      value: 285.15 },  // 12 C in Kelvin
    ],
  }],
};

// ── Paths to compare ─────────────────────────────────────────────────────────

const LEAF_PATHS = [
  'navigation/speedOverGround',
  'navigation/courseOverGroundTrue',
  'navigation/headingMagnetic',
  'navigation/magneticVariation',
  'navigation/position',
  'environment/depth/belowTransducer',
  'environment/wind/speedApparent',
  'environment/wind/angleApparent',
  'environment/outside/temperature',
  'environment/outside/pressure',
  'environment/outside/humidity',
  'environment/water/temperature',
];

const BASE_PATH = '/signalk/v1/api/vessels/self/';

// Derived paths — computed by signalk-rs derived-data plugin.
// These are only checked on signalk-rs (the original may not have derived-data installed).
const DERIVED_PATHS = [
  {
    path: 'navigation/headingTrue',
    // headingMagnetic (1.1694) + magneticVariation (0.0524) ≈ 1.2218
    expectedValue: 1.1694 + 0.0524,
    tolerance: 0.001,
  },
  {
    path: 'environment/outside/density',
    // airDensity = P / (R * T) = 101325 / (287.058 * 288.15) ≈ 1.225
    expectedValue: 101325.0 / (287.058 * 288.15),
    tolerance: 0.01,
  },
  {
    path: 'environment/outside/dewPointTemperature',
    // Magnus formula with T=288.15K (15C), RH=0.65
    // b=17.67, c=243.5, T_c = 288.15-273.15 = 15
    // gamma = ln(0.65) + (17.67*15)/(243.5+15)
    // gamma = -0.4308 + 1.0255 = 0.5947
    // Td_c = (243.5 * 0.5947) / (17.67 - 0.5947) ≈ 8.48 C → 281.63 K
    expectedValue: 273.15 + (243.5 * (Math.log(0.65) + (17.67 * 15) / (243.5 + 15))) / (17.67 - (Math.log(0.65) + (17.67 * 15) / (243.5 + 15))),
    tolerance: 0.5,
  },
];

// ── Comparison logic ─────────────────────────────────────────────────────────

function typeOf(v) {
  if (v === null || v === undefined) return 'null';
  if (Array.isArray(v)) return 'array';
  return typeof v;
}

function comparePath(path, orig, rs) {
  const errors = [];

  // Status code
  if (orig.status !== rs.status) {
    errors.push(`status: original=${orig.status} rs=${rs.status}`);
    return errors; // no point comparing body if status differs
  }

  if (orig.status !== 200) {
    errors.push(`both returned ${orig.status} (expected 200)`);
    return errors;
  }

  const ob = orig.body;
  const rb = rs.body;

  // Required SignalK leaf fields
  for (const field of ['value', 'timestamp']) {
    const origHas = ob && ob[field] !== undefined;
    const rsHas   = rb && rb[field] !== undefined;

    if (origHas && !rsHas) {
      errors.push(`missing field '${field}' in rs response`);
    }
  }

  // $source may use different formats — just check presence/type match
  if (ob && ob['$source'] !== undefined && (!rb || rb['$source'] === undefined)) {
    errors.push(`missing field '$source' in rs response`);
  }

  // Value type match
  if (ob && rb && ob.value !== undefined && rb.value !== undefined) {
    if (typeOf(ob.value) !== typeOf(rb.value)) {
      errors.push(`value type: original=${typeOf(ob.value)} rs=${typeOf(rb.value)}`);
    }
  }

  // For numeric values: check they're equal (we injected identical data)
  if (ob && rb && typeof ob.value === 'number' && typeof rb.value === 'number') {
    if (Math.abs(ob.value - rb.value) > 0.0001) {
      errors.push(`value mismatch: original=${ob.value} rs=${rb.value}`);
    }
  }

  // For position objects: check lat/lon match
  if (ob && rb && typeOf(ob.value) === 'object' && typeOf(rb.value) === 'object') {
    if (ob.value.latitude !== undefined && rb.value.latitude !== undefined) {
      if (Math.abs(ob.value.latitude - rb.value.latitude) > 0.0001) {
        errors.push(`latitude mismatch: original=${ob.value.latitude} rs=${rb.value.latitude}`);
      }
    }
    if (ob.value.longitude !== undefined && rb.value.longitude !== undefined) {
      if (Math.abs(ob.value.longitude - rb.value.longitude) > 0.0001) {
        errors.push(`longitude mismatch: original=${ob.value.longitude} rs=${rb.value.longitude}`);
      }
    }
  }

  // Timestamp format (ISO 8601)
  if (rb && rb.timestamp) {
    if (isNaN(Date.parse(rb.timestamp))) {
      errors.push(`timestamp not ISO 8601: ${rb.timestamp}`);
    }
  }

  return errors;
}

// ── Runner ───────────────────────────────────────────────────────────────────

async function run() {
  console.log('\nSignalK Data Conformance Comparison');
  console.log(`  Original : ${ORIGINAL}`);
  console.log(`  signalk-rs: ${RS}\n`);

  // Wait for servers to be ready
  await retry(async () => {
    const r = await request(ORIGINAL, 'GET', '/signalk');
    return r.status === 200;
  }, 'original server');

  await retry(async () => {
    const r = await request(RS, 'GET', '/signalk');
    return r.status === 200;
  }, 'signalk-rs');

  // Wait for the test-injector plugin to be available on the original
  await retry(async () => {
    const r = await request(ORIGINAL, 'POST', '/plugins/signalk-test-injector/inject', {
      context: 'vessels.self',
      updates: [{ values: [{ path: 'test.ping', value: 1 }] }],
    });
    return r.status === 200;
  }, 'test-injector plugin');

  console.log('  Servers ready, injecting test data...\n');

  // Inject the same delta into both servers
  await injectBoth(TEST_DELTA);

  // Give servers time to process
  await sleep(500);

  // Compare leaf paths
  let failures = 0;

  for (const leafPath of LEAF_PATHS) {
    const url = BASE_PATH + leafPath;
    const [orig, rs] = await Promise.all([
      request(ORIGINAL, 'GET', url),
      request(RS, 'GET', url),
    ]);

    const errors = comparePath(leafPath, orig, rs);

    if (errors.length === 0) {
      console.log(`  \u2713  ${leafPath}`);
    } else {
      failures++;
      console.log(`  \u2717  ${leafPath}`);
      for (const e of errors) {
        console.log(`       ${e}`);
      }
    }
  }

  console.log(`\n${LEAF_PATHS.length - failures}/${LEAF_PATHS.length} paths match\n`);

  // ── Derived path verification (signalk-rs only) ──────────────────────────
  console.log('  Derived data (signalk-rs only):\n');
  let derivedFailures = 0;

  for (const dp of DERIVED_PATHS) {
    const url = BASE_PATH + dp.path;
    const rs = await request(RS, 'GET', url);
    const errors = [];

    if (rs.status !== 200) {
      errors.push(`status ${rs.status} (expected 200 — derived-data plugin may not have produced this path)`);
    } else {
      if (!rs.body || rs.body.value === undefined) {
        errors.push('missing "value" field');
      } else if (typeof rs.body.value !== 'number') {
        errors.push(`expected number, got ${typeOf(rs.body.value)}`);
      } else if (Math.abs(rs.body.value - dp.expectedValue) > dp.tolerance) {
        errors.push(`value=${rs.body.value}, expected ~${dp.expectedValue.toFixed(4)} (tolerance ${dp.tolerance})`);
      }

      // Check standard leaf fields
      if (!rs.body.timestamp) errors.push('missing timestamp');
    }

    if (errors.length === 0) {
      console.log(`  \u2713  ${dp.path} = ${rs.body?.value?.toFixed(4)}`);
    } else {
      derivedFailures++;
      console.log(`  \u2717  ${dp.path}`);
      for (const e of errors) {
        console.log(`       ${e}`);
      }
    }
  }

  const totalTests = LEAF_PATHS.length + DERIVED_PATHS.length;
  const totalFailures = failures + derivedFailures;
  console.log(`\n${totalTests - totalFailures}/${totalTests} total checks passed\n`);
  process.exit(totalFailures);
}

run().catch((err) => {
  console.error('Runner error:', err.message);
  process.exit(1);
});
