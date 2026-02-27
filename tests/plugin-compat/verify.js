'use strict';

/**
 * Plugin compatibility test runner.
 *
 * Connects to a running signalk-rs + bridge stack and verifies that the full
 * plugin API surface works end-to-end — without any real marine hardware.
 *
 * Data flow under test:
 *
 *   verify.js
 *     │  POST /plugins/signalk-test-plugin/inject  (plugin REST proxy)
 *     ▼
 *   signalk-rs (port 3000)
 *     │  uds_proxy → bridge.sock
 *     ▼
 *   bridge → app.handleMessage → POST /internal/v1/delta → rs.sock
 *     │
 *     ▼
 *   signalk-rs store (delta applied)
 *     │  WS broadcast
 *     ▼
 *   bridge WebSocket client → subscriptionmanager callback
 *     │  state.deltas.push(delta)
 *     ▼
 *   verify.js → GET /plugins/signalk-test-plugin/recorded → assert
 *
 * Tests:
 *   1. Plugin REST proxy     (registerWithRouter + uds_proxy)
 *   2. handleMessage         (delta emission from plugin → store)
 *   3. Subscription fanout   (WS delta delivered back to plugin)
 *   4. getSelfPath           (Internal API path query)
 *   5. PUT forwarding        (registerPutHandler + PUT from public API)
 *
 * No external dependencies — uses only Node.js built-in `http` module.
 */

const http = require('http');

const BASE   = process.env.SIGNALK_URL || 'http://signalk-rs:3000';
const PLUGIN = `${BASE}/plugins/signalk-test-plugin`;

// ─── HTTP helpers ─────────────────────────────────────────────────────────────

function request(method, url, body) {
  return new Promise((resolve, reject) => {
    const payload = body != null ? JSON.stringify(body) : null;
    const u = new URL(url);

    const opts = {
      hostname: u.hostname,
      port:     Number(u.port) || 80,
      path:     u.pathname + (u.search || ''),
      method,
      headers: {
        'Content-Type': 'application/json',
        ...(payload != null ? { 'Content-Length': Buffer.byteLength(payload) } : {}),
      },
    };

    const req = http.request(opts, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => {
        const text = Buffer.concat(chunks).toString();
        let parsed;
        try { parsed = text ? JSON.parse(text) : null; } catch { parsed = text; }
        resolve({ status: res.statusCode, body: parsed });
      });
    });

    req.on('error', reject);
    if (payload != null) req.write(payload);
    req.end();
  });
}

const get  = (url)        => request('GET',  url, null);
const post = (url, body)  => request('POST', url, body);
const put  = (url, body)  => request('PUT',  url, body);

// ─── Utilities ────────────────────────────────────────────────────────────────

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

async function retry(fn, { times = 30, delay = 1000, label = '' } = {}) {
  for (let i = 0; i < times; i++) {
    try {
      return await fn();
    } catch (e) {
      if (i === times - 1) throw new Error(`${label}: ${e.message}`);
      process.stdout.write('.');
      await wait(delay);
    }
  }
}

// ─── Assertions ───────────────────────────────────────────────────────────────

let passed = 0;
let failed = 0;

function assert(condition, message) {
  if (condition) {
    console.log(`  ✓  ${message}`);
    passed++;
  } else {
    console.error(`  ✗  ${message}`);
    failed++;
  }
}

// ─── Main ─────────────────────────────────────────────────────────────────────

async function main() {
  // ── Wait for signalk-rs ────────────────────────────────────────────────────
  process.stdout.write('Waiting for signalk-rs');
  await retry(
    () => get(`${BASE}/signalk`).then((r) => {
      if (r.status !== 200) throw new Error(`HTTP ${r.status}`);
    }),
    { label: 'signalk-rs startup' }
  );
  console.log(' ready');

  // ── Wait for test plugin REST endpoint ─────────────────────────────────────
  // The endpoint is only available after:
  //   bridge registered → plugin loaded → registerWithRouter → plugin-routes registered
  process.stdout.write('Waiting for test plugin');
  await retry(
    () => get(`${PLUGIN}/recorded`).then((r) => {
      if (r.status !== 200) throw new Error(`HTTP ${r.status}`);
    }),
    { label: 'plugin startup' }
  );
  console.log(' ready\n');

  // Reset to a clean state before running tests.
  await post(`${PLUGIN}/reset`, {});
  await wait(200);

  // ── Test 1: Plugin REST proxy ──────────────────────────────────────────────
  console.log('Test 1: Plugin REST proxy (registerWithRouter)');
  {
    const r = await get(`${PLUGIN}/recorded`);
    assert(r.status === 200, 'GET /recorded returns 200');
    assert(Array.isArray(r.body.deltas), 'response.deltas is an array');
    assert(Array.isArray(r.body.puts),   'response.puts is an array');
  }

  // ── Test 2: handleMessage → delta stored in signalk-rs ────────────────────
  console.log('\nTest 2: handleMessage (plugin → signalk-rs store)');
  {
    const inject = await post(`${PLUGIN}/inject`, {
      context: 'vessels.self',
      updates: [{
        source: { label: 'test', type: 'test' },
        values: [{ path: 'navigation.speedOverGround', value: 3.5 }],
      }],
    });
    assert(inject.status === 200, `POST /inject returns 200 (got ${inject.status})`);

    // Verify the value is now in the public REST API.
    await wait(300);
    const rest = await get(`${BASE}/signalk/v1/api/vessels/self/navigation/speedOverGround`);
    assert(rest.status === 200, 'path visible via public REST API');
    assert(
      Math.abs((rest.body.value ?? rest.body) - 3.5) < 0.001,
      `stored value is 3.5 (got ${JSON.stringify(rest.body)})`
    );
  }

  // ── Test 3: Subscription fanout (WS → plugin callback) ────────────────────
  console.log('\nTest 3: Subscription fanout (WS delta → plugin callback)');
  {
    await wait(500);  // allow WS message to arrive at bridge and be recorded
    const r = await get(`${PLUGIN}/recorded`);
    const sogDelta = r.body.deltas.find((d) =>
      d.updates?.some((u) =>
        u.values?.some((v) => v.path === 'navigation.speedOverGround')
      )
    );
    assert(!!sogDelta, 'plugin subscriptionmanager received speedOverGround delta');
  }

  // ── Test 4: getSelfPath ────────────────────────────────────────────────────
  console.log('\nTest 4: getSelfPath (Internal API path query)');
  {
    const r = await post(`${PLUGIN}/getSelfPath`, { path: 'navigation.speedOverGround' });
    assert(r.status === 200, `POST /getSelfPath returns 200 (got ${r.status})`);
    assert(
      Math.abs(r.body.value - 3.5) < 0.001,
      `getSelfPath returns stored value (got ${r.body.value})`
    );
  }

  // ── Test 5: PUT forwarding → plugin handler ────────────────────────────────
  console.log('\nTest 5: PUT forwarding (public API → bridge → plugin handler)');
  {
    const r = await put(
      `${BASE}/signalk/v1/api/vessels/self/steering/autopilot/target/headingTrue`,
      { value: 1.5 }
    );
    assert(r.status === 200,          `PUT returns 200 (got ${r.status})`);
    assert(r.body.state === 'COMPLETED', `PUT state is COMPLETED (got ${JSON.stringify(r.body)})`);

    await wait(300);
    const recorded = (await get(`${PLUGIN}/recorded`)).body;
    const putRecord = recorded.puts.find((p) => Math.abs(p.value - 1.5) < 0.001);
    assert(!!putRecord, 'plugin PUT handler was invoked with value 1.5');
  }

  // ── Summary ────────────────────────────────────────────────────────────────
  console.log(`\n${'─'.repeat(50)}`);
  console.log(`Results: ${passed} passed, ${failed} failed`);

  if (failed > 0) process.exit(1);
}

main().catch((err) => {
  console.error('\nFatal error:', err.message);
  process.exit(1);
});
