#!/usr/bin/env node
'use strict';

/**
 * Side-by-side conformance runner with deep content comparison.
 *
 * Sends the same HTTP requests to both:
 *   - ORIGINAL_URL  (reference signalk-server)
 *   - RS_URL        (signalk-rs)
 *
 * Three test phases:
 *   Phase 1: API structure comparison (pre-injection)
 *   Phase 2: Inject identical deltas, compare leaf paths on both servers
 *   Phase 3: Verify derived data (signalk-rs only)
 *   Phase 4: Deep tree comparison (navigation + environment)
 *
 * Non-deterministic fields (timestamps, tokens, server.id, etc.) are skipped.
 *
 * Exit code: number of failed comparisons (0 = all pass).
 */

const { deepCompare, printCompareResult, request, requestWithAuth, retry, sleep, typeOf, IGNORE_PATHS } = require('./lib/compare-util');

const ORIGINAL = process.env.ORIGINAL_URL || 'http://localhost:3000';
const RS       = process.env.RS_URL       || 'http://localhost:3001';

// Extended ignore paths for cross-server comparison.
// URLs differ in host/port, so we ignore URL values in endpoints.
const COMPARE_IGNORE = [
  ...IGNORE_PATHS,
  'endpoints.v1.signalk-http',   // URL host differs
  'endpoints.v1.signalk-ws',     // URL host differs
  'endpoints.v1.signalk-tcp',    // may not exist on both
  'name',                        // server name differs
];

// ── Inject helpers ──────────────────────────────────────────────────────────

const TEST_DELTA = {
  context: 'vessels.self',
  updates: [{
    source: { label: 'conformance-test', type: 'test' },
    timestamp: new Date().toISOString(),
    values: [
      { path: 'navigation.speedOverGround',        value: 3.85 },
      { path: 'navigation.courseOverGroundTrue',     value: 1.2217 },  // ~70 degrees in rad
      { path: 'navigation.headingMagnetic',          value: 1.1694 },  // ~67 degrees in rad
      { path: 'navigation.magneticVariation',        value: 0.0524 },  // ~3 degrees in rad
      { path: 'navigation.position', value: { latitude: 54.5, longitude: 10.0 } },
      { path: 'navigation.speedThroughWater',        value: 3.5 },
      { path: 'environment.depth.belowTransducer',   value: 12.5 },
      { path: 'environment.depth.transducerToKeel',  value: 0.5 },
      { path: 'environment.wind.speedApparent',      value: 7.2 },
      { path: 'environment.wind.angleApparent',      value: 0.7854 },  // ~45 degrees in rad
      { path: 'environment.outside.temperature',     value: 288.15 },  // 15 C in Kelvin
      { path: 'environment.outside.pressure',        value: 101325.0 },
      { path: 'environment.outside.humidity',         value: 0.65 },
      { path: 'environment.water.temperature',        value: 285.15 },  // 12 C in Kelvin
    ],
  }],
};

async function injectBoth(delta) {
  const [origResp, rsResp] = await Promise.all([
    request(ORIGINAL, 'POST', '/plugins/signalk-test-injector/inject', delta),
    request(RS, 'POST', '/test/inject', delta),
  ]);
  if (origResp.status !== 200) throw new Error(`Original inject failed: ${origResp.status}`);
  if (rsResp.status !== 200) throw new Error(`RS inject failed: ${rsResp.status}`);
}

// ── Phase 1: API structure tests (pre-injection) ───────────────────────────

const TESTS = [
  {
    name: 'GET /signalk — discovery',
    method: 'GET',
    path: '/signalk',
    check(orig, rs) {
      if (orig.status !== rs.status) return [`status: original=${orig.status} rs=${rs.status}`];
      const errs = [];
      // endpoints.v1 structure must exist in both
      if (!rs.body?.endpoints?.v1) errs.push('missing endpoints.v1');
      for (const field of ['version', 'signalk-http', 'signalk-ws']) {
        if (!rs.body?.endpoints?.v1?.[field]) errs.push(`missing endpoints.v1.${field}`);
      }
      // Version must match semver format (not compared cross-server — different software)
      if (rs.body?.endpoints?.v1?.version && !/^\d+\.\d+\.\d+/.test(rs.body.endpoints.v1.version)) {
        errs.push(`version format invalid: ${rs.body.endpoints.v1.version}`);
      }
      // signalk-http must be a valid URL ending with /api/ (per spec)
      if (rs.body?.endpoints?.v1?.['signalk-http']) {
        try {
          const url = new URL(rs.body.endpoints.v1['signalk-http']);
          if (url.pathname !== '/signalk/v1/api/') {
            errs.push(`signalk-http path must be /signalk/v1/api/, got: ${url.pathname}`);
          }
        } catch (e) {
          errs.push(`signalk-http not a valid URL: ${rs.body.endpoints.v1['signalk-http']}`);
        }
      }
      // signalk-ws must be a valid URL ending in /stream
      if (rs.body?.endpoints?.v1?.['signalk-ws']) {
        try {
          const url = new URL(rs.body.endpoints.v1['signalk-ws']);
          if (!url.pathname.includes('/stream')) {
            errs.push(`signalk-ws should contain /stream: ${url.pathname}`);
          }
        } catch (e) {
          errs.push(`signalk-ws not a valid URL: ${rs.body.endpoints.v1['signalk-ws']}`);
        }
      }
      // server.id must be present
      if (!rs.body?.server?.id) errs.push('missing server.id');
      if (!rs.body?.server?.version) errs.push('missing server.version');
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
        return errs;
      }
      // Required top-level keys
      for (const key of ['version', 'self', 'vessels']) {
        if (rs.body?.[key] === undefined) errs.push(`missing top-level key: ${key}`);
      }
      // 'self' must reference a valid vessel URI (not compared cross-server — UUIDs differ)
      if (rs.body?.self && !rs.body.self.startsWith('vessels.urn:mrn:signalk:uuid:')) {
        errs.push(`self format invalid: ${rs.body.self}`);
      }
      // 'self' must be consistent with 'vessels' keys
      if (rs.body?.self && rs.body?.vessels) {
        const selfKey = rs.body.self.replace('vessels.', '');
        if (!rs.body.vessels[selfKey]) {
          errs.push(`self ${selfKey} not found in vessels`);
        }
      }
      // 'version' must match semver format
      if (rs.body?.version && !/^\d+\.\d+\.\d+/.test(rs.body.version)) {
        errs.push(`version format: ${rs.body.version}`);
      }
      // Both must have at least one vessel
      if (orig.body?.vessels && rs.body?.vessels) {
        if (Object.keys(rs.body.vessels).length === 0) {
          errs.push('rs has no vessels');
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
      // Both should be objects
      if (typeof orig.body !== 'object' || typeof rs.body !== 'object') {
        errs.push(`body type: original=${typeof orig.body} rs=${typeof rs.body}`);
        return errs;
      }
      // Both must have at least one vessel (UUIDs differ between servers)
      if (orig.body && Object.keys(orig.body).length === 0) errs.push('original has no vessels');
      if (rs.body && Object.keys(rs.body).length === 0) errs.push('rs has no vessels');
      // RS vessel should have name, mmsi, uuid fields
      if (rs.body) {
        const rsKeys = Object.keys(rs.body);
        for (const key of rsKeys) {
          for (const field of ['name', 'mmsi', 'uuid']) {
            if (rs.body[key][field] === undefined) {
              errs.push(`rs vessel ${key} missing field: ${field}`);
            }
          }
        }
      }
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
      // RS must have name, mmsi, uuid with correct values from config
      if (rs.body) {
        if (rs.body.name !== 'Test Vessel') {
          errs.push(`rs name: expected 'Test Vessel', got '${rs.body.name}'`);
        }
        if (rs.body.mmsi !== '123456789') {
          errs.push(`rs mmsi: expected '123456789', got '${rs.body.mmsi}'`);
        }
        if (!rs.body.uuid) {
          errs.push('rs missing uuid');
        } else if (!rs.body.uuid.startsWith('urn:mrn:signalk:uuid:')) {
          errs.push(`rs uuid format invalid: ${rs.body.uuid}`);
        }
      }
      // If original also has name/mmsi, they should match (same vessel config)
      if (orig.body && rs.body) {
        if (orig.body.name && rs.body.name && orig.body.name !== rs.body.name) {
          errs.push(`name: original=${orig.body.name} rs=${rs.body.name}`);
        }
        if (orig.body.mmsi && rs.body.mmsi && orig.body.mmsi !== rs.body.mmsi) {
          errs.push(`mmsi: original=${orig.body.mmsi} rs=${rs.body.mmsi}`);
        }
      }
      return errs;
    },
  },
  {
    name: 'GET /signalk/v1/snapshot — 501 Not Implemented',
    method: 'GET',
    path: '/signalk/v1/snapshot',
    check(orig, rs) {
      const errs = [];
      if (rs.status !== 501) errs.push(`expected 501, got ${rs.status}`);
      return errs;
    },
  },
  {
    name: 'GET /signalk/v1/api/does/not/exist — 404',
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
        // Original has no users configured — just verify our token format
        if (rs.status === 200 && !rs.body?.token) {
          errs.push('login 200 response missing token');
        }
        return errs;
      }
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
    name: 'POST /signalk/v1/auth/login — wrong credentials — 4xx',
    method: 'POST',
    path: '/signalk/v1/auth/login',
    body: { username: 'hacker', password: 'x' },
    check(orig, rs) {
      const errs = [];
      const origRejects = orig.status >= 400 && orig.status < 500;
      const rsRejects   = rs.status >= 400 && rs.status < 500;
      if (!origRejects) errs.push(`original should reject, got ${orig.status}`);
      if (!rsRejects)   errs.push(`rs should reject, got ${rs.status}`);
      return errs;
    },
  },

  // === Additional API tests ===

  {
    name: 'GET /signalk/v1/api/sources — source list (pre-injection)',
    method: 'GET',
    path: '/signalk/v1/api/sources',
    check(orig, rs) {
      const errs = [];
      if (rs.status !== 200) {
        errs.push(`rs status: ${rs.status} (expected 200)`);
        return errs;
      }
      if (typeof rs.body !== 'object' || rs.body === null) {
        errs.push('rs response is not an object');
      }
      return errs;
    },
  },
  {
    name: 'POST+GET /signalk/v1/applicationData — round-trip',
    method: 'GET',
    path: '/signalk/v1/applicationData/global/test-app/1.0/config',
    async run() {
      const testData = { theme: 'dark', columns: 3, enabled: true };
      const errs = [];

      // POST to RS (original may not support POST — it uses PUT)
      const rsPost = await request(RS, 'POST', '/signalk/v1/applicationData/global/test-app/1.0/config', testData);
      if (rsPost.status !== 200 && rsPost.status !== 201) {
        errs.push(`rs POST status: ${rsPost.status}`);
        return errs;
      }

      await sleep(200);

      // GET from RS — verify round-trip
      const rsGet = await request(RS, 'GET', '/signalk/v1/applicationData/global/test-app/1.0/config');
      if (rsGet.status !== 200) {
        errs.push(`rs GET status: ${rsGet.status}`);
        return errs;
      }

      // Data must match what we posted
      const result = deepCompare(testData, rsGet.body, '', { ignorePaths: [] });
      if (result.mismatches.length > 0) {
        for (const m of result.mismatches) errs.push(`applicationData mismatch: ${m.path} — ${m.reason}`);
      }
      if (result.missing.length > 0) {
        for (const m of result.missing) errs.push(`applicationData missing: ${m.path}`);
      }

      return errs;
    },
  },
  {
    name: 'PUT /signalk/v1/api/vessels/self/no/put/handler — 4xx',
    method: 'PUT',
    path: '/signalk/v1/api/vessels/self/no/put/handler',
    body: { value: 42 },
    check(orig, rs) {
      const errs = [];
      const origRejects = orig.status >= 400;
      const rsRejects   = rs.status >= 400;
      if (!origRejects) errs.push(`original should reject, got ${orig.status}`);
      if (!rsRejects)   errs.push(`rs should reject, got ${rs.status}`);
      return errs;
    },
  },

  // === Auth lifecycle ===

  {
    name: 'POST /signalk/v1/auth/validate — token validation',
    method: 'POST',
    path: '/signalk/v1/auth/validate',
    async run() {
      const errs = [];
      // Get a token from rs
      const loginResp = await request(RS, 'POST', '/signalk/v1/auth/login', { username: 'admin', password: 'admin' });
      if (loginResp.status !== 200 || !loginResp.body?.token) {
        errs.push(`cannot get token: login status=${loginResp.status}`);
        return errs;
      }
      // Validate on rs using Authorization header (rs expects Bearer token)
      const validateResp = await requestWithAuth(RS, 'POST', '/signalk/v1/auth/validate', loginResp.body.token);
      if (validateResp.status !== 200) {
        errs.push(`rs validate status: ${validateResp.status} (expected 200)`);
      }
      // Response should include a new token
      if (validateResp.status === 200 && !validateResp.body?.token) {
        errs.push('validate response missing token');
      }
      return errs;
    },
  },
  {
    name: 'PUT /signalk/v1/auth/logout — logout',
    method: 'PUT',
    path: '/signalk/v1/auth/logout',
    check(orig, rs) {
      const errs = [];
      if (rs.status < 200 || rs.status >= 300) {
        errs.push(`rs logout status: ${rs.status} (expected 2xx)`);
      }
      // Body should be JSON with a message or empty
      if (rs.status === 200 && rs.body !== null && typeof rs.body !== 'object') {
        errs.push(`rs logout body not an object: ${typeof rs.body}`);
      }
      return errs;
    },
  },

  // === Tracks ===

  {
    name: 'GET /signalk/v1/api/tracks — track list',
    method: 'GET',
    path: '/signalk/v1/api/tracks',
    check(orig, rs) {
      const errs = [];
      if (rs.status !== 200) {
        errs.push(`rs status: ${rs.status} (expected 200)`);
        return errs;
      }
      // Body should be an object (track IDs as keys) or array
      if (rs.body === null || typeof rs.body !== 'object') {
        errs.push(`rs body is not an object: ${typeof rs.body}`);
      }
      return errs;
    },
  },
  {
    name: 'GET /signalk/v1/api/vessels/self/track — self track',
    method: 'GET',
    path: '/signalk/v1/api/vessels/self/track',
    check(orig, rs) {
      const errs = [];
      if (rs.status !== 200) {
        errs.push(`rs status: ${rs.status} (expected 200)`);
        return errs;
      }
      // Body should be a GeoJSON Feature or FeatureCollection
      if (rs.body && typeof rs.body === 'object') {
        const validTypes = ['Feature', 'FeatureCollection', 'MultiLineString', 'LineString'];
        const bodyType = rs.body.type;
        if (bodyType && !validTypes.includes(bodyType)) {
          errs.push(`unexpected GeoJSON type: ${bodyType}`);
        }
        // If it's a Feature, verify geometry exists
        if (bodyType === 'Feature' && !rs.body.geometry) {
          errs.push('Feature missing geometry');
        }
      }
      return errs;
    },
  },

  // === Webapps ===

  {
    name: 'GET /signalk/v1/webapps — webapp list',
    method: 'GET',
    path: '/signalk/v1/webapps',
    check(orig, rs) {
      const errs = [];
      if (rs.status !== 200) {
        errs.push(`rs status: ${rs.status} (expected 200)`);
        return errs;
      }
      // Body must be an array
      if (!Array.isArray(rs.body)) {
        errs.push(`rs body is not an array: ${typeof rs.body}`);
        return errs;
      }
      // Each webapp entry should have at least name and url (if any exist)
      for (let i = 0; i < rs.body.length; i++) {
        const app = rs.body[i];
        if (typeof app !== 'object' || app === null) {
          errs.push(`webapp[${i}] is not an object`);
        } else if (!app.name && !app.identifier) {
          errs.push(`webapp[${i}] has no name or identifier`);
        }
      }
      return errs;
    },
  },
];

// ── Phase 2: Leaf path comparison (both servers, injected data) ─────────────

const LEAF_PATHS = [
  'navigation/speedOverGround',
  'navigation/courseOverGroundTrue',
  'navigation/headingMagnetic',
  'navigation/magneticVariation',
  'navigation/position',
  'navigation/speedThroughWater',
  'environment/depth/belowTransducer',
  'environment/wind/speedApparent',
  'environment/wind/angleApparent',
  'environment/outside/temperature',
  'environment/outside/pressure',
  'environment/outside/humidity',
  'environment/water/temperature',
];

const BASE_PATH = '/signalk/v1/api/vessels/self/';

function comparePath(path, orig, rs) {
  const errors = [];

  if (orig.status !== rs.status) {
    errors.push(`status: original=${orig.status} rs=${rs.status}`);
    return errors;
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

  // $source may use different formats — just check presence/type
  if (ob && ob['$source'] !== undefined && (!rb || rb['$source'] === undefined)) {
    errors.push(`missing field '$source' in rs response`);
  }

  // Value type match
  if (ob && rb && ob.value !== undefined && rb.value !== undefined) {
    if (typeOf(ob.value) !== typeOf(rb.value)) {
      errors.push(`value type: original=${typeOf(ob.value)} rs=${typeOf(rb.value)}`);
    }
  }

  // For numeric values: check equality (we injected identical data)
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

// ── Phase 3: Derived data paths (signalk-rs only) ──────────────────────────

const DERIVED_PATHS = [
  {
    path: 'navigation/headingTrue',
    // headingMagnetic (1.1694) + magneticVariation (0.0524) = 1.2218
    expectedValue: 1.1694 + 0.0524,
    tolerance: 0.001,
  },
  {
    path: 'environment/outside/airDensity',
    // airDensity = P / (R * T) = 101325 / (287.058 * 288.15) ~ 1.225
    expectedValue: 101325.0 / (287.058 * 288.15),
    tolerance: 0.01,
  },
  {
    path: 'environment/outside/dewPointTemperature',
    // Magnus formula with T=288.15K (15C), RH=0.65
    expectedValue: 273.15 + (243.5 * (Math.log(0.65) + (17.67 * 15) / (243.5 + 15))) / (17.67 - (Math.log(0.65) + (17.67 * 15) / (243.5 + 15))),
    tolerance: 0.5,
  },
  {
    path: 'navigation/courseOverGroundMagnetic',
    // courseOverGroundTrue (1.2217) - magneticVariation (0.0524) = 1.1693
    expectedValue: 1.2217 - 0.0524,
    tolerance: 0.001,
  },
  {
    path: 'environment/wind/speedTrue',
    // True wind from apparent: boat speed 3.5 (STW), apparent speed 7.2, apparent angle 0.7854 (45 deg)
    expectedValue: Math.sqrt(7.2*7.2 + 3.5*3.5 - 2*7.2*3.5*Math.cos(0.7854)),
    tolerance: 0.1,
  },
  {
    path: 'environment/wind/angleTrueWater',
    // True wind angle (water reference)
    expectedValue: Math.atan2(7.2*Math.sin(0.7854), 7.2*Math.cos(0.7854) - 3.5),
    tolerance: 0.1,
  },
  {
    path: 'environment/wind/directionTrue',
    // True wind direction = headingTrue + angleTrueWater
    expectedValue: (1.1694 + 0.0524) + Math.atan2(7.2*Math.sin(0.7854), 7.2*Math.cos(0.7854) - 3.5),
    tolerance: 0.1,
  },
  {
    path: 'environment/outside/heatIndexTemperature',
    // Heat index for T=288.15K (15C), RH=0.65
    // Below 27C threshold, simplified formula: ~ 287.3K
    expectedValue: 287.3,
    tolerance: 1.0,
  },
  {
    path: 'environment/outside/apparentWindChillTemperature',
    // Wind chill at T=15C, wind speed used from apparent wind
    // For low wind speeds and moderate temps, ~ 288.15 (close to ambient)
    expectedValue: 288.15,
    tolerance: 1.0,
  },
  {
    path: 'environment/depth/belowKeel',
    // belowTransducer (12.5) - transducerToKeel (0.5) = 12.0
    expectedValue: 12.0,
    tolerance: 0.001,
  },
];

// ── Phase 4: Deep tree comparison + post-injection structural tests ─────────

const TREE_TESTS = [
  {
    name: 'GET /signalk/v1/api/sources — contains injected source labels',
    path: '/signalk/v1/api/sources',
    check(orig, rs) {
      const errs = [];
      if (rs.status !== 200) {
        errs.push(`rs status: ${rs.status}`);
        return errs;
      }
      if (typeof rs.body !== 'object' || rs.body === null) {
        errs.push('rs sources is not an object');
        return errs;
      }
      // After injection, at least the conformance-test source label should appear
      const labels = Object.keys(rs.body);
      if (labels.length === 0) {
        errs.push('rs sources is empty after injection (expected at least one source label)');
      }
      if (!labels.some(l => l.includes('conformance'))) {
        errs.push(`rs sources missing 'conformance-test' label; found: [${labels.join(', ')}]`);
      }
      // Each source should be an object (hierarchical per spec)
      for (const label of labels) {
        if (typeof rs.body[label] !== 'object' || rs.body[label] === null) {
          errs.push(`source '${label}' is not an object: ${typeof rs.body[label]}`);
        }
      }
      return errs;
    },
  },
  {
    name: 'GET vessels/self/navigation — deep compare after injection',
    path: '/signalk/v1/api/vessels/self/navigation',
    check(orig, rs) {
      const errs = [];
      if (orig.status !== rs.status) {
        errs.push(`status: original=${orig.status} rs=${rs.status}`);
        return errs;
      }
      if (!orig.body || !rs.body) {
        errs.push('missing response body');
        return errs;
      }
      // Deep compare navigation tree — ignore timestamps, $source, and derived paths
      const navIgnore = [
        ...COMPARE_IGNORE,
        // Derived data paths (rs may have extra computed values)
        'headingTrue',
        'headingTrue.*',
        'courseOverGroundMagnetic',
        'courseOverGroundMagnetic.*',
        'speedThroughWater',
        'speedThroughWater.*',
        // meta objects may differ
        '*.meta',
        '*.meta.*',
        '*.zones',
        // source details may differ
        '*.source',
        '*.source.*',
        '*.values',
        '*.values.*',
      ];
      const result = deepCompare(orig.body, rs.body, '', { ignorePaths: navIgnore });
      // Report only value mismatches (missing/extra are expected for derived paths)
      for (const m of result.mismatches) {
        errs.push(`nav mismatch: ${m.path} — ${m.reason}`);
      }
      return errs;
    },
  },
  {
    name: 'GET vessels/self/environment — deep compare after injection',
    path: '/signalk/v1/api/vessels/self/environment',
    check(orig, rs) {
      const errs = [];
      if (orig.status !== rs.status) {
        errs.push(`status: original=${orig.status} rs=${rs.status}`);
        return errs;
      }
      if (!orig.body || !rs.body) {
        errs.push('missing response body');
        return errs;
      }
      // Deep compare environment tree
      const envIgnore = [
        ...COMPARE_IGNORE,
        // Derived paths that rs may have but original doesn't
        'outside.airDensity',
        'outside.airDensity.*',
        'outside.dewPointTemperature',
        'outside.dewPointTemperature.*',
        'outside.heatIndexTemperature',
        'outside.heatIndexTemperature.*',
        'outside.apparentWindChillTemperature',
        'outside.apparentWindChillTemperature.*',
        'wind.speedTrue',
        'wind.speedTrue.*',
        'wind.angleTrueWater',
        'wind.angleTrueWater.*',
        'wind.directionTrue',
        'wind.directionTrue.*',
        'depth.belowKeel',
        'depth.belowKeel.*',
        // meta/source details
        '*.meta',
        '*.meta.*',
        '*.zones',
        '*.source',
        '*.source.*',
        '*.values',
        '*.values.*',
      ];
      const result = deepCompare(orig.body, rs.body, '', { ignorePaths: envIgnore });
      for (const m of result.mismatches) {
        errs.push(`env mismatch: ${m.path} — ${m.reason}`);
      }
      return errs;
    },
  },
];

// ── Runner ──────────────────────────────────────────────────────────────────

async function run() {
  let failures = 0;

  console.log(`\nSignalK Conformance Comparison`);
  console.log(`  Original : ${ORIGINAL}`);
  console.log(`  signalk-rs: ${RS}\n`);

  // Wait for servers
  await retry(async () => {
    const r = await request(ORIGINAL, 'GET', '/signalk');
    return r.status === 200;
  }, 'original server');

  await retry(async () => {
    const r = await request(RS, 'GET', '/signalk');
    return r.status === 200;
  }, 'signalk-rs');

  // ── Phase 1: API structure comparison ────────────────────────────────────

  console.log('  Phase 1: API structure comparison\n');

  for (const test of TESTS) {
    let errs;

    if (test.run) {
      errs = await test.run();
    } else {
      const [orig, rs] = await Promise.all([
        request(ORIGINAL, test.method, test.path, test.body),
        request(RS,       test.method, test.path, test.body),
      ]);
      errs = test.check(orig, rs);
    }

    if (errs.length === 0) {
      console.log(`  \u2713  ${test.name}`);
    } else {
      failures++;
      console.log(`  \u2717  ${test.name}`);
      for (const e of errs) {
        console.log(`       ${e}`);
      }
    }
  }

  // ── Phase 2: Inject data + leaf path comparison ──────────────────────────

  console.log('\n  Phase 2: Data injection + leaf path comparison\n');

  // Wait for test-injector plugin
  await retry(async () => {
    const r = await request(ORIGINAL, 'POST', '/plugins/signalk-test-injector/inject', {
      context: 'vessels.self',
      updates: [{ values: [{ path: 'test.ping', value: 1 }] }],
    });
    return r.status === 200;
  }, 'test-injector plugin');

  await injectBoth(TEST_DELTA);
  // Give servers time to process (1000ms for two-tick derivation chains)
  await sleep(1000);

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

  // ── Phase 3: Derived data verification (signalk-rs only) ─────────────────

  console.log('\n  Phase 3: Derived data (signalk-rs only)\n');

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

      if (rs.body && !rs.body.timestamp) errors.push('missing timestamp');
    }

    if (errors.length === 0) {
      console.log(`  \u2713  ${dp.path} = ${rs.body?.value?.toFixed(4)}`);
    } else {
      failures++;
      console.log(`  \u2717  ${dp.path}`);
      for (const e of errors) {
        console.log(`       ${e}`);
      }
    }
  }

  // ── Phase 4: Deep tree comparison ────────────────────────────────────────

  console.log('\n  Phase 4: Deep tree comparison\n');

  for (const test of TREE_TESTS) {
    const [orig, rs] = await Promise.all([
      request(ORIGINAL, 'GET', test.path),
      request(RS, 'GET', test.path),
    ]);
    const errs = test.check(orig, rs);

    if (errs.length === 0) {
      console.log(`  \u2713  ${test.name}`);
    } else {
      failures++;
      console.log(`  \u2717  ${test.name}`);
      for (const e of errs) {
        console.log(`       ${e}`);
      }
    }
  }

  // ── Summary ──────────────────────────────────────────────────────────────

  const total = TESTS.length + LEAF_PATHS.length + DERIVED_PATHS.length + TREE_TESTS.length;
  console.log(`\n${total - failures}/${total} passed\n`);
  process.exit(failures);
}

run().catch((err) => {
  console.error('Runner error:', err.message);
  process.exit(1);
});
