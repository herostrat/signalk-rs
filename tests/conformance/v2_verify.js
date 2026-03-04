#!/usr/bin/env node
'use strict';

/**
 * v2 API Spec Conformance Tests — signalk-rs only.
 *
 * The original Node.js signalk-server does not implement v2 APIs,
 * so these tests run against signalk-rs exclusively with exact value
 * assertions against the expected spec-defined response structures.
 *
 * Test groups:
 *   1. Features API (3 tests)
 *   2. Resources CRUD — Waypoints (11 tests)
 *   3. Resources CRUD — Routes (5 tests)
 *   4. Course — Destination (14 tests)
 *   5. Course — Active Route (7 tests)
 *   6. Autopilot API (25 tests)
 *   7. Notifications API (7 tests)
 *   8. History API (5 tests)
 *
 * Exit code: number of failed tests (0 = all pass).
 */

// TODO: When SignalK JSON Schema becomes available, add schema validation
// for every v2 response. Currently we validate structure and values manually.

const { request, retry, sleep } = require('./lib/compare-util');

const RS = process.env.RS_URL || 'http://localhost:3000';

// ── Test infrastructure ─────────────────────────────────────────────────────

let passed = 0;
let failed = 0;
let currentGroup = '';

function group(name) {
  currentGroup = name;
  console.log(`\n  ${name}\n`);
}

function ok(name) {
  passed++;
  console.log(`  \u2713  ${name}`);
}

function fail(name, ...reasons) {
  failed++;
  console.log(`  \u2717  ${name}`);
  for (const r of reasons) {
    console.log(`       ${r}`);
  }
}

function assert(cond, name, reason) {
  if (cond) { ok(name); return true; }
  fail(name, reason);
  return false;
}

function assertStatus(resp, expected, name) {
  return assert(resp.status === expected, name, `expected ${expected}, got ${resp.status}`);
}

// ── Shared state across tests ───────────────────────────────────────────────

const state = {
  waypointId: null,
  waypoint2Id: null,
  routeId: null,
  notificationId: null,
};

// ── 1. Features API ─────────────────────────────────────────────────────────

async function testFeatures() {
  group('1. Features API');

  // 1.1: GET /signalk/v2/features returns 200
  const resp = await request(RS, 'GET', '/signalk/v2/features');
  if (!assertStatus(resp, 200, 'GET /signalk/v2/features — 200')) return;

  // 1.2: Response has apis and plugins arrays
  const body = resp.body;
  assert(
    Array.isArray(body?.apis) && Array.isArray(body?.plugins),
    'features response has apis[] and plugins[]',
    `apis=${typeof body?.apis}, plugins=${typeof body?.plugins}`
  );

  // 1.3: Known APIs present
  const apiIds = (body?.apis || []).map(a => a.id);
  const expectedApis = ['resources', 'course'];
  const missingApis = expectedApis.filter(id => !apiIds.includes(id));
  assert(
    missingApis.length === 0,
    'features.apis contains resources + course',
    `missing: ${missingApis.join(', ')}; found: ${apiIds.join(', ')}`
  );
}

// ── 2. Resources CRUD — Waypoints ───────────────────────────────────────────

async function testWaypoints() {
  group('2. Resources CRUD — Waypoints');

  // 2.1: Create waypoint
  const createResp = await request(RS, 'POST', '/signalk/v2/api/resources/waypoints', {
    name: 'Test Waypoint Alpha',
    description: 'Conformance test waypoint',
    position: { latitude: 54.5, longitude: 10.0 },
  });
  if (!assertStatus(createResp, 200, 'POST waypoints — create')) return;

  assert(
    createResp.body?.state === 'COMPLETED' && typeof createResp.body?.id === 'string',
    'create response: state=COMPLETED, id present',
    `state=${createResp.body?.state}, id=${createResp.body?.id}`
  );
  state.waypointId = createResp.body?.id;

  // 2.2: GET created waypoint
  const getResp = await request(RS, 'GET', `/signalk/v2/api/resources/waypoints/${state.waypointId}`);
  assertStatus(getResp, 200, 'GET waypoint by ID — 200');

  // 2.3: Waypoint has correct fields
  const wp = getResp.body;
  assert(
    wp?.name === 'Test Waypoint Alpha' &&
    wp?.position?.latitude === 54.5 &&
    wp?.position?.longitude === 10.0,
    'waypoint name + position match',
    `name=${wp?.name}, lat=${wp?.position?.latitude}, lon=${wp?.position?.longitude}`
  );

  // 2.4: Update waypoint
  const updateResp = await request(RS, 'PUT', `/signalk/v2/api/resources/waypoints/${state.waypointId}`, {
    name: 'Test Waypoint Alpha Updated',
    description: 'Updated description',
    position: { latitude: 54.6, longitude: 10.1 },
  });
  assertStatus(updateResp, 200, 'PUT waypoint — update');

  // 2.5: Verify update
  const getUpdated = await request(RS, 'GET', `/signalk/v2/api/resources/waypoints/${state.waypointId}`);
  assert(
    getUpdated.body?.name === 'Test Waypoint Alpha Updated',
    'waypoint name updated',
    `name=${getUpdated.body?.name}`
  );

  // 2.6: List waypoints includes our waypoint
  const listResp = await request(RS, 'GET', '/signalk/v2/api/resources/waypoints');
  assertStatus(listResp, 200, 'GET waypoints list — 200');

  assert(
    listResp.body && typeof listResp.body === 'object' && state.waypointId in listResp.body,
    'list contains created waypoint',
    `keys: ${Object.keys(listResp.body || {}).join(', ')}`
  );

  // 2.7: Create a second waypoint (for route later)
  const create2Resp = await request(RS, 'POST', '/signalk/v2/api/resources/waypoints', {
    name: 'Test Waypoint Beta',
    position: { latitude: 55.0, longitude: 10.5 },
  });
  assertStatus(create2Resp, 200, 'POST waypoints — create second');
  state.waypoint2Id = create2Resp.body?.id;

  // 2.8: GET providers — validate body is array or object with entries
  const provResp = await request(RS, 'GET', '/signalk/v2/api/resources/waypoints/_providers');
  assertStatus(provResp, 200, 'GET waypoints providers — 200');
  assert(
    provResp.body != null && (Array.isArray(provResp.body) || typeof provResp.body === 'object'),
    'providers body is array or object',
    `body type=${typeof provResp.body}`
  );

  // 2.9: GET default provider — validate body is not null
  const defProvResp = await request(RS, 'GET', '/signalk/v2/api/resources/waypoints/_providers/_default');
  assertStatus(defProvResp, 200, 'GET waypoints default provider — 200');
  assert(
    defProvResp.body != null,
    'default provider body is not null',
    `body=${JSON.stringify(defProvResp.body)}`
  );

  // 2.10: SET default provider (use the current default to re-set it)
  if (defProvResp.body && typeof defProvResp.body === 'string') {
    const setDefResp = await request(RS, 'POST', `/signalk/v2/api/resources/waypoints/_providers/_default/${defProvResp.body}`);
    assertStatus(setDefResp, 200, 'POST waypoints set default provider — 200');
  } else if (defProvResp.body && typeof defProvResp.body === 'object' && defProvResp.body.id) {
    const setDefResp = await request(RS, 'POST', `/signalk/v2/api/resources/waypoints/_providers/_default/${defProvResp.body.id}`);
    assertStatus(setDefResp, 200, 'POST waypoints set default provider — 200');
  } else {
    // Try with a known provider name
    const provList = provResp.body;
    const firstProv = Array.isArray(provList) && provList.length > 0 ? provList[0] : null;
    const provId = typeof firstProv === 'string' ? firstProv : firstProv?.id;
    if (provId) {
      const setDefResp = await request(RS, 'POST', `/signalk/v2/api/resources/waypoints/_providers/_default/${provId}`);
      assertStatus(setDefResp, 200, 'POST waypoints set default provider — 200');
    } else {
      fail('POST waypoints set default provider — 200', `cannot determine provider id from ${JSON.stringify(defProvResp.body)}`);
    }
  }

  // 2.11: Delete waypoint
  // (we keep waypoints alive for route tests, delete later)
  // Delete a separate test waypoint
  const tmpResp = await request(RS, 'POST', '/signalk/v2/api/resources/waypoints', {
    name: 'Temp Delete Test',
    position: { latitude: 50.0, longitude: 5.0 },
  });
  if (tmpResp.status === 200 && tmpResp.body?.id) {
    const delResp = await request(RS, 'DELETE', `/signalk/v2/api/resources/waypoints/${tmpResp.body.id}`);
    assertStatus(delResp, 200, 'DELETE waypoint — 200');
  } else {
    fail('DELETE waypoint — 200', 'could not create temp waypoint');
  }
}

// ── 3. Resources CRUD — Routes ──────────────────────────────────────────────

async function testRoutes() {
  group('3. Resources CRUD — Routes');

  // 3.1: Create route with GeoJSON LineString
  const createResp = await request(RS, 'POST', '/signalk/v2/api/resources/routes', {
    name: 'Test Route Kiel-Flensburg',
    description: 'Conformance test route',
    feature: {
      type: 'Feature',
      geometry: {
        type: 'LineString',
        coordinates: [
          [10.0, 54.5],    // Kiel (lon, lat per GeoJSON)
          [10.25, 54.75],  // midpoint
          [10.5, 55.0],    // Flensburg
        ],
      },
      properties: {},
    },
  });
  if (!assertStatus(createResp, 200, 'POST routes — create')) return;

  assert(
    createResp.body?.state === 'COMPLETED' && typeof createResp.body?.id === 'string',
    'route create: state=COMPLETED, id present',
    `state=${createResp.body?.state}, id=${createResp.body?.id}`
  );
  state.routeId = createResp.body?.id;

  // 3.2: GET created route — validate body structure
  const getResp = await request(RS, 'GET', `/signalk/v2/api/resources/routes/${state.routeId}`);
  assertStatus(getResp, 200, 'GET route by ID — 200');

  // 3.3: Route has correct name and GeoJSON Feature
  const rt = getResp.body;
  assert(
    rt?.name === 'Test Route Kiel-Flensburg',
    'route name matches',
    `name=${rt?.name}`
  );

  assert(
    rt?.feature?.type === 'Feature' &&
    rt?.feature?.geometry?.type === 'LineString' &&
    Array.isArray(rt?.feature?.geometry?.coordinates) &&
    rt?.feature?.geometry?.coordinates.length === 3,
    'route has GeoJSON Feature with 3 coordinates',
    `feature=${JSON.stringify(rt?.feature?.geometry)}`
  );

  // 3.4: List routes
  const listResp = await request(RS, 'GET', '/signalk/v2/api/resources/routes');
  assertStatus(listResp, 200, 'GET routes list — 200');

  // 3.5: Route in list
  assert(
    listResp.body && typeof listResp.body === 'object' && state.routeId in listResp.body,
    'list contains created route',
    `keys: ${Object.keys(listResp.body || {}).join(', ')}`
  );
}

// ── 4. Course — Destination ─────────────────────────────────────────────────

async function testCourseDestination() {
  group('4. Course — Destination');

  // 4.1: GET initial course state (should be empty/default)
  const initResp = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course');
  assertStatus(initResp, 200, 'GET course — initial state 200');

  // 4.2: Course has full CourseState structure (never empty {})
  assert(
    initResp.body && typeof initResp.body.arrivalCircle === 'number',
    'course has arrivalCircle (number)',
    `arrivalCircle=${initResp.body?.arrivalCircle}`
  );
  assert(
    initResp.body && 'activeRoute' in initResp.body && 'targetArrivalTime' in initResp.body,
    'course has full CourseState keys (activeRoute, targetArrivalTime)',
    `keys=[${Object.keys(initResp.body || {}).join(', ')}]`
  );

  // 4.3: Set arrival circle
  const circleResp = await request(RS, 'PUT', '/signalk/v2/api/vessels/self/navigation/course/arrivalCircle', {
    value: 100,
  });
  assertStatus(circleResp, 200, 'PUT arrivalCircle — 200');

  // 4.4: Verify arrival circle
  const getCircle = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course');
  assert(
    getCircle.body?.arrivalCircle === 100,
    'arrivalCircle updated to 100',
    `arrivalCircle=${getCircle.body?.arrivalCircle}`
  );

  // 4.5: Set destination by position
  const destResp = await request(RS, 'PUT', '/signalk/v2/api/vessels/self/navigation/course/destination', {
    position: { latitude: 55.0, longitude: 10.5 },
  });
  assertStatus(destResp, 200, 'PUT destination (position) — 200');

  // 4.6: Course has nextPoint after setting destination
  await sleep(200);
  const afterDest = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course');
  assert(
    afterDest.body?.nextPoint != null &&
    afterDest.body?.nextPoint?.position?.latitude === 55.0,
    'nextPoint set to destination position',
    `nextPoint=${JSON.stringify(afterDest.body?.nextPoint)}`
  );

  // 4.7: GET calcValues
  const calcResp = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course/calcValues');
  // May return 200 with values or 200 with empty (no GPS fix)
  assert(
    calcResp.status === 200,
    'GET calcValues — 200',
    `status=${calcResp.status}`
  );

  // 4.8: GET _config — validate body is an object with apiOnly field
  const configResp = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course/_config');
  assertStatus(configResp, 200, 'GET course/_config — 200');
  assert(
    configResp.body && typeof configResp.body === 'object',
    'course/_config body is an object',
    `body=${JSON.stringify(configResp.body)}`
  );

  // 4.9: POST _config/apiOnly — enable, then verify in _config
  const apiOnlyResp = await request(RS, 'POST', '/signalk/v2/api/vessels/self/navigation/course/_config/apiOnly');
  assertStatus(apiOnlyResp, 200, 'POST course/_config/apiOnly — enable');

  const configAfterEnable = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course/_config');
  assert(
    configAfterEnable.body?.apiOnly === true,
    'apiOnly=true after enable',
    `apiOnly=${configAfterEnable.body?.apiOnly}`
  );

  // 4.10: DELETE _config/apiOnly — disable, then verify
  const apiOnlyOffResp = await request(RS, 'DELETE', '/signalk/v2/api/vessels/self/navigation/course/_config/apiOnly');
  assertStatus(apiOnlyOffResp, 200, 'DELETE course/_config/apiOnly — disable');

  const configAfterDisable = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course/_config');
  assert(
    configAfterDisable.body?.apiOnly === false || configAfterDisable.body?.apiOnly === undefined,
    'apiOnly=false after disable',
    `apiOnly=${configAfterDisable.body?.apiOnly}`
  );

  // 4.11: PUT targetArrivalTime
  const arrivalTimeResp = await request(RS, 'PUT', '/signalk/v2/api/vessels/self/navigation/course/targetArrivalTime', {
    value: new Date(Date.now() + 7200000).toISOString(),
  });
  assert(
    arrivalTimeResp.status === 200 || arrivalTimeResp.status === 400,
    'PUT targetArrivalTime — 200 or 400 (no active course)',
    `status=${arrivalTimeResp.status}`
  );

  // 4.12: Clear course
  const clearResp = await request(RS, 'DELETE', '/signalk/v2/api/vessels/self/navigation/course');
  assertStatus(clearResp, 200, 'DELETE course — clear 200');

  // 4.13: arrivalCircle persists after course clear (was set to 100 in 4.3)
  const afterClear = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course');
  assert(
    afterClear.body?.arrivalCircle === 100,
    'arrivalCircle persists after clearing course (config value 100)',
    `arrivalCircle=${afterClear.body?.arrivalCircle}`
  );

  // 4.14: nextPoint is absent after clear (no active navigation)
  assert(
    afterClear.body?.nextPoint == null,
    'nextPoint absent after course clear',
    `nextPoint=${JSON.stringify(afterClear.body?.nextPoint)}`
  );
}

// ── 5. Course — Active Route ────────────────────────────────────────────────

async function testCourseActiveRoute() {
  group('5. Course — Active Route');

  if (!state.routeId) {
    fail('route required — skipping active route tests', 'no route ID from previous test');
    failed += 5; // count skipped
    return;
  }

  // 5.1: Activate route
  const activateResp = await request(RS, 'PUT', '/signalk/v2/api/vessels/self/navigation/course/activeRoute', {
    href: `/resources/routes/${state.routeId}`,
  });
  assertStatus(activateResp, 200, 'PUT activeRoute — activate');

  // 5.2: Course state has activeRoute
  await sleep(200);
  const courseResp = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course');
  assert(
    courseResp.body?.activeRoute != null,
    'course has activeRoute after activation',
    `activeRoute=${JSON.stringify(courseResp.body?.activeRoute)}`
  );

  // 5.3: activeRoute has correct href
  assert(
    courseResp.body?.activeRoute?.href?.includes(state.routeId),
    'activeRoute.href contains route ID',
    `href=${courseResp.body?.activeRoute?.href}`
  );

  // 5.4: Advance to next point — verify pointIndex increased
  const advanceResp = await request(RS, 'PUT', '/signalk/v2/api/vessels/self/navigation/course/activeRoute/nextPoint', {
    value: 1,
  });
  assertStatus(advanceResp, 200, 'PUT nextPoint — advance');

  const afterAdvance = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course');
  assert(
    afterAdvance.body?.activeRoute?.pointIndex >= 1,
    'pointIndex >= 1 after advance',
    `pointIndex=${afterAdvance.body?.activeRoute?.pointIndex}`
  );

  // 5.5: Set point index — jump back to 0, verify
  const indexResp = await request(RS, 'PUT', '/signalk/v2/api/vessels/self/navigation/course/activeRoute/pointIndex', {
    value: 0,
  });
  assertStatus(indexResp, 200, 'PUT pointIndex — jump to 0');

  const afterIndex = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course');
  assert(
    afterIndex.body?.activeRoute?.pointIndex === 0,
    'pointIndex=0 after set',
    `pointIndex=${afterIndex.body?.activeRoute?.pointIndex}`
  );

  // 5.6: Reverse route — verify reverse flag
  const reverseResp = await request(RS, 'PUT', '/signalk/v2/api/vessels/self/navigation/course/activeRoute/reverse');
  assertStatus(reverseResp, 200, 'PUT reverse — 200');

  const afterReverse = await request(RS, 'GET', '/signalk/v2/api/vessels/self/navigation/course');
  assert(
    afterReverse.body?.activeRoute?.reverse === true,
    'reverse=true after reverse',
    `reverse=${afterReverse.body?.activeRoute?.reverse}`
  );

  // 5.7: Clear route
  const clearResp = await request(RS, 'DELETE', '/signalk/v2/api/vessels/self/navigation/course');
  assertStatus(clearResp, 200, 'DELETE course — clear active route');
}

// ── 6. Autopilot API ────────────────────────────────────────────────────────

async function testAutopilot() {
  group('6. Autopilot API');

  // 6.1: List autopilots
  const listResp = await request(RS, 'GET', '/signalk/v2/api/vessels/self/autopilots');
  if (!assertStatus(listResp, 200, 'GET autopilots — list')) return;

  // 6.2: List has at least one entry with provider + isDefault
  const pilots = listResp.body;
  const pilotIds = Object.keys(pilots || {});
  assert(
    pilotIds.length > 0,
    'at least one autopilot registered',
    `found ${pilotIds.length} autopilots`
  );

  const deviceId = pilotIds[0]; // should be "default"

  // 6.3: GET autopilot details
  const detailResp = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}`);
  assertStatus(detailResp, 200, `GET autopilot/${deviceId} — details`);

  // 6.4: Autopilot has options with state[], mode[], actions[]
  const data = detailResp.body;
  assert(
    data?.options &&
    Array.isArray(data.options.state) &&
    Array.isArray(data.options.mode) &&
    Array.isArray(data.options.actions),
    'options: state[], mode[], actions[] present',
    `options=${JSON.stringify(data?.options)}`
  );

  // 6.5: Options contain expected states
  const states = data?.options?.state || [];
  assert(
    states.includes('enabled') && states.includes('disabled'),
    'options.state includes enabled + disabled',
    `states=[${states.join(',')}]`
  );

  // 6.6: Options contain expected modes
  const modes = data?.options?.mode || [];
  assert(
    modes.includes('compass') && modes.includes('wind'),
    'options.mode includes compass + wind',
    `modes=[${modes.join(',')}]`
  );

  // 6.7: GET state — validate returns a known state string
  const stateResp = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/state`);
  assertStatus(stateResp, 200, `GET autopilot/${deviceId}/state — 200`);
  assert(
    typeof stateResp.body?.value === 'string' && states.includes(stateResp.body.value),
    `state value is valid: "${stateResp.body?.value}"`,
    `value=${JSON.stringify(stateResp.body?.value)}, expected one of [${states}]`
  );

  // 6.8: GET mode — validate returns a known mode string
  const modeResp = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/mode`);
  assertStatus(modeResp, 200, `GET autopilot/${deviceId}/mode — 200`);
  assert(
    typeof modeResp.body?.value === 'string' && modes.includes(modeResp.body.value),
    `mode value is valid: "${modeResp.body?.value}"`,
    `value=${JSON.stringify(modeResp.body?.value)}, expected one of [${modes}]`
  );

  // 6.9: GET default provider — validate returns device id
  const defPilotResp = await request(RS, 'GET', '/signalk/v2/api/vessels/self/autopilots/_providers/_default');
  assertStatus(defPilotResp, 200, 'GET autopilots default provider — 200');
  assert(
    defPilotResp.body != null,
    'default provider body is not null',
    `body=${JSON.stringify(defPilotResp.body)}`
  );

  // 6.10: SET default provider (re-set to current)
  const setDefPilotResp = await request(RS, 'POST', `/signalk/v2/api/vessels/self/autopilots/_providers/_default/${deviceId}`);
  assert(
    setDefPilotResp.status === 200 || setDefPilotResp.status === 204,
    'POST autopilots set default provider — 2xx',
    `status=${setDefPilotResp.status}`
  );

  // 6.11: PUT state=enabled, then verify via GET
  const putStateResp = await request(RS, 'PUT', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/state`, {
    value: 'enabled',
  });
  assert(
    putStateResp.status === 200 || putStateResp.status === 204 || putStateResp.status === 400,
    'PUT state=enabled — 200 or 400 (no sensor)',
    `status=${putStateResp.status}`
  );
  if (putStateResp.status === 200 || putStateResp.status === 204) {
    const verifyState = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/state`);
    assert(
      verifyState.body?.value === 'enabled',
      'GET state confirms enabled after PUT',
      `value=${JSON.stringify(verifyState.body?.value)}`
    );
  }

  // 6.12: PUT mode=compass, then verify via GET
  const putModeResp = await request(RS, 'PUT', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/mode`, {
    value: 'compass',
  });
  assert(
    putModeResp.status === 200 || putModeResp.status === 204 || putModeResp.status === 400,
    'PUT mode=compass — 200 or 400',
    `status=${putModeResp.status}`
  );
  if (putModeResp.status === 200 || putModeResp.status === 204) {
    const verifyMode = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/mode`);
    assert(
      verifyMode.body?.value === 'compass',
      'GET mode confirms compass after PUT',
      `value=${JSON.stringify(verifyMode.body?.value)}`
    );
  }

  // 6.13: POST engage — then verify state is still enabled
  const engageResp = await request(RS, 'POST', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/engage`);
  assert(
    engageResp.status === 200 || engageResp.status === 204 || engageResp.status === 400 || engageResp.status === 422,
    'POST engage — 2xx or 4xx (no sensor)',
    `status=${engageResp.status}`
  );
  if (engageResp.status === 200 || engageResp.status === 204) {
    const afterEngage = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/state`);
    assert(
      afterEngage.body?.value === 'enabled',
      'state still enabled after engage',
      `value=${JSON.stringify(afterEngage.body?.value)}`
    );
  }

  // 6.14: GET target — validate returns number or null
  const getTargetResp = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/target`);
  assertStatus(getTargetResp, 200, 'GET target — 200');
  assert(
    getTargetResp.body?.value === null || typeof getTargetResp.body?.value === 'number',
    `target value is number or null: ${getTargetResp.body?.value}`,
    `value=${JSON.stringify(getTargetResp.body?.value)}, type=${typeof getTargetResp.body?.value}`
  );

  // 6.15: PUT target=1.5708, then verify via GET
  const targetResp = await request(RS, 'PUT', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/target`, {
    value: 1.5708,
  });
  assert(
    targetResp.status === 200 || targetResp.status === 204 || targetResp.status === 400,
    'PUT target=1.5708 — 200 or 400',
    `status=${targetResp.status}`
  );
  if (targetResp.status === 200 || targetResp.status === 204) {
    const verifyTarget = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/target`);
    assert(
      typeof verifyTarget.body?.value === 'number' && Math.abs(verifyTarget.body.value - 1.5708) < 0.001,
      'GET target confirms 1.5708 after PUT',
      `value=${verifyTarget.body?.value}`
    );
  }

  // 6.16: PUT target/adjust (+10 degrees), then verify target changed
  const preAdjustTarget = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/target`);
  const adjustResp = await request(RS, 'PUT', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/target/adjust`, {
    value: 0.1745,  // +10 degrees
  });
  assert(
    adjustResp.status === 200 || adjustResp.status === 204 || adjustResp.status === 400,
    'PUT target/adjust — 200 or 400',
    `status=${adjustResp.status}`
  );
  if ((adjustResp.status === 200 || adjustResp.status === 204) && typeof preAdjustTarget.body?.value === 'number') {
    const postAdjust = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/target`);
    const expected = preAdjustTarget.body.value + 0.1745;
    assert(
      typeof postAdjust.body?.value === 'number' && Math.abs(postAdjust.body.value - expected) < 0.01,
      `target adjusted by +0.1745: ${postAdjust.body?.value?.toFixed(4)}`,
      `expected ~${expected.toFixed(4)}, got ${postAdjust.body?.value}`
    );
  }

  // 6.17: POST tack/port — may fail if not in wind mode (expected 400)
  const tackPortResp = await request(RS, 'POST', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/tack/port`);
  assert(
    tackPortResp.status === 200 || tackPortResp.status === 204 || tackPortResp.status === 400,
    'POST tack/port — 200 or 400 (wrong mode)',
    `status=${tackPortResp.status}`
  );

  // 6.18: POST tack/invalid — must be 400 (bad direction)
  const tackInvalidResp = await request(RS, 'POST', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/tack/invalid`);
  assertStatus(tackInvalidResp, 400, 'POST tack/invalid — 400');

  // 6.19: POST gybe/starboard — may fail if not in wind mode
  const gybeResp = await request(RS, 'POST', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/gybe/starboard`);
  assert(
    gybeResp.status === 200 || gybeResp.status === 204 || gybeResp.status === 400,
    'POST gybe/starboard — 200 or 400 (wrong mode)',
    `status=${gybeResp.status}`
  );

  // 6.20: POST dodge — enter dodge mode, verify mode if successful
  const dodgeEnterResp = await request(RS, 'POST', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/dodge`);
  assert(
    dodgeEnterResp.status === 200 || dodgeEnterResp.status === 204 || dodgeEnterResp.status === 400,
    'POST dodge (enter) — 200 or 400',
    `status=${dodgeEnterResp.status}`
  );

  // 6.21: PUT dodge — adjust dodge offset
  const dodgeAdjustResp = await request(RS, 'PUT', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/dodge`, {
    value: 0.1745,  // 10 degrees
  });
  assert(
    dodgeAdjustResp.status === 200 || dodgeAdjustResp.status === 204 || dodgeAdjustResp.status === 400,
    'PUT dodge (adjust) — 200 or 400',
    `status=${dodgeAdjustResp.status}`
  );

  // 6.22: DELETE dodge — exit dodge mode
  const dodgeExitResp = await request(RS, 'DELETE', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/dodge`);
  assert(
    dodgeExitResp.status === 200 || dodgeExitResp.status === 204 || dodgeExitResp.status === 400,
    'DELETE dodge (exit) — 200 or 400',
    `status=${dodgeExitResp.status}`
  );

  // 6.23: POST courseCurrentPoint — needs active route, expect 400 without one
  const ccpResp = await request(RS, 'POST', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/courseCurrentPoint`);
  assert(
    ccpResp.status === 200 || ccpResp.status === 204 || ccpResp.status === 400,
    'POST courseCurrentPoint — 200 or 400 (no active route)',
    `status=${ccpResp.status}`
  );

  // 6.24: POST courseNextPoint — needs active route, expect 400 without one
  const cnpResp = await request(RS, 'POST', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/courseNextPoint`);
  assert(
    cnpResp.status === 200 || cnpResp.status === 204 || cnpResp.status === 400,
    'POST courseNextPoint — 200 or 400 (no active route)',
    `status=${cnpResp.status}`
  );

  // 6.25: POST disengage, then verify state is disabled
  const disengageResp = await request(RS, 'POST', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/disengage`);
  assert(
    disengageResp.status === 200 || disengageResp.status === 204 || disengageResp.status === 400,
    'POST disengage — 200 or 400',
    `status=${disengageResp.status}`
  );
  if (disengageResp.status === 200 || disengageResp.status === 204) {
    const afterDisengage = await request(RS, 'GET', `/signalk/v2/api/vessels/self/autopilots/${deviceId}/state`);
    assert(
      afterDisengage.body?.value === 'disabled',
      'state is disabled after disengage',
      `value=${JSON.stringify(afterDisengage.body?.value)}`
    );
  }
}

// ── 7. Notifications API ────────────────────────────────────────────────────

async function testNotifications() {
  group('7. Notifications API');

  // 7.1: GET notifications (initially may be empty)
  const listResp = await request(RS, 'GET', '/signalk/v2/api/notifications');
  assertStatus(listResp, 200, 'GET notifications — 200');

  assert(
    typeof listResp.body === 'object' && listResp.body !== null,
    'notifications response is an object',
    `type=${typeof listResp.body}`
  );

  // 7.2: Inject a notification via delta
  const notifDelta = {
    context: 'vessels.self',
    updates: [{
      source: { label: 'conformance-test', type: 'test' },
      timestamp: new Date().toISOString(),
      values: [{
        path: 'notifications.conformance.testAlarm',
        value: {
          state: 'alarm',
          method: ['visual', 'sound'],
          message: 'Conformance test alarm',
        },
      }],
    }],
  };
  const injectResp = await request(RS, 'POST', '/test/inject', notifDelta);
  assertStatus(injectResp, 200, 'inject notification delta — 200');

  await sleep(500);

  // 7.3: GET notifications includes our alarm
  const afterInject = await request(RS, 'GET', '/signalk/v2/api/notifications');
  const notifKey = Object.keys(afterInject.body || {}).find(k => k.includes('conformance'));
  assert(
    notifKey != null,
    'notifications list includes conformance alarm',
    `keys=[${Object.keys(afterInject.body || {}).join(', ')}]`
  );

  if (notifKey) {
    const notif = afterInject.body[notifKey];

    // 7.4: Notification has enrichment fields (id, status)
    assert(
      typeof notif.id === 'string' && notif.id.length > 0,
      'notification has UUID id',
      `id=${notif.id}`
    );
    state.notificationId = notifKey;

    // 7.5: Notification has status with all capability flags
    assert(
      notif.status &&
      typeof notif.status.canAcknowledge === 'boolean' &&
      typeof notif.status.canSilence === 'boolean' &&
      typeof notif.status.canClear === 'boolean',
      'notification.status has canAcknowledge + canSilence + canClear',
      `status=${JSON.stringify(notif.status)}`
    );

    // 7.6: Silence the notification, then verify status.silenced=true
    const silenceResp = await request(RS, 'POST', `/signalk/v2/api/notifications/${notifKey}/silence`);
    assert(
      silenceResp.status === 200 || silenceResp.status === 204,
      'POST silence — 200/204',
      `status=${silenceResp.status}`
    );
    if (silenceResp.status === 200 || silenceResp.status === 204) {
      await sleep(200);
      const afterSilence = await request(RS, 'GET', '/signalk/v2/api/notifications');
      const silencedNotif = afterSilence.body?.[notifKey];
      if (silencedNotif?.status) {
        assert(
          silencedNotif.status.silenced === true,
          'notification status.silenced=true after silence',
          `silenced=${silencedNotif.status.silenced}`
        );
      }
    }

    // 7.7: Acknowledge the notification, then verify status.acknowledged=true
    const ackResp = await request(RS, 'POST', `/signalk/v2/api/notifications/${notifKey}/acknowledge`);
    assert(
      ackResp.status === 200 || ackResp.status === 204,
      'POST acknowledge — 200/204',
      `status=${ackResp.status}`
    );
    if (ackResp.status === 200 || ackResp.status === 204) {
      await sleep(200);
      const afterAck = await request(RS, 'GET', '/signalk/v2/api/notifications');
      const ackedNotif = afterAck.body?.[notifKey];
      if (ackedNotif?.status) {
        assert(
          ackedNotif.status.acknowledged === true,
          'notification status.acknowledged=true after acknowledge',
          `acknowledged=${ackedNotif.status.acknowledged}`
        );
      }
    }
  } else {
    // Skip remaining notification tests
    failed += 4;
  }
}

// ── 8. History API ──────────────────────────────────────────────────────────

async function testHistory() {
  group('8. History API');

  // First inject some data so history has something to query
  const delta = {
    context: 'vessels.self',
    updates: [{
      source: { label: 'conformance-test', type: 'test' },
      timestamp: new Date().toISOString(),
      values: [
        { path: 'navigation.speedOverGround', value: 4.2 },
      ],
    }],
  };
  await request(RS, 'POST', '/test/inject', delta);
  await sleep(500);

  // 8.1: GET /history/values without required params → 400
  const noParamsResp = await request(RS, 'GET', '/signalk/v2/api/history/values');
  assertStatus(noParamsResp, 400, 'GET history/values (no params) — 400');

  // 8.2: GET /history/values with params → 200
  const now = new Date();
  const oneHourAgo = new Date(now.getTime() - 3600000);
  const valuesResp = await request(RS, 'GET',
    `/signalk/v2/api/history/values?paths=navigation.speedOverGround&from=${oneHourAgo.toISOString()}&to=${now.toISOString()}`
  );
  assertStatus(valuesResp, 200, 'GET history/values (with params) — 200');

  // 8.3: Values response has expected structure (context, range, values, data)
  assert(
    valuesResp.body?.context != null && valuesResp.body?.range != null,
    'history/values response has context + range',
    `keys=${Object.keys(valuesResp.body || {}).join(',')}`
  );
  assert(
    typeof valuesResp.body?.range?.from === 'string' && typeof valuesResp.body?.range?.to === 'string',
    'range has from + to strings',
    `range=${JSON.stringify(valuesResp.body?.range)}`
  );
  assert(
    Array.isArray(valuesResp.body?.values),
    'values is an array',
    `values type=${typeof valuesResp.body?.values}`
  );
  assert(
    Array.isArray(valuesResp.body?.data),
    'data is an array',
    `data type=${typeof valuesResp.body?.data}`
  );

  // 8.4: GET /history/contexts — validate returns array
  const ctxResp = await request(RS, 'GET', '/signalk/v2/api/history/contexts');
  assertStatus(ctxResp, 200, 'GET history/contexts — 200');
  assert(
    Array.isArray(ctxResp.body),
    'history/contexts returns an array',
    `body type=${typeof ctxResp.body}, value=${JSON.stringify(ctxResp.body)}`
  );

  // 8.5: GET /history/paths — validate returns array
  const pathsResp = await request(RS, 'GET', '/signalk/v2/api/history/paths');
  assertStatus(pathsResp, 200, 'GET history/paths — 200');
  assert(
    Array.isArray(pathsResp.body),
    'history/paths returns an array',
    `body type=${typeof pathsResp.body}, value=${JSON.stringify(pathsResp.body)}`
  );

  // 8.6: GET /history/_providers — validate body is object with keys
  const histProvResp = await request(RS, 'GET', '/signalk/v2/api/history/_providers');
  assertStatus(histProvResp, 200, 'GET history/_providers — 200');
  assert(
    histProvResp.body != null && typeof histProvResp.body === 'object',
    'history providers body is an object',
    `body type=${typeof histProvResp.body}`
  );
  assert(
    Object.keys(histProvResp.body).length > 0,
    'history providers has at least one entry',
    `keys=${JSON.stringify(Object.keys(histProvResp.body))}`
  );

  // 8.7: GET /history/_providers/_default — validate body has id field
  const histDefResp = await request(RS, 'GET', '/signalk/v2/api/history/_providers/_default');
  assertStatus(histDefResp, 200, 'GET history/_providers/_default — 200');
  assert(
    histDefResp.body != null && histDefResp.body.id,
    'history default provider has id field',
    `body=${JSON.stringify(histDefResp.body)}`
  );

  // 8.8: POST set default history provider — use current default id
  if (histDefResp.body && histDefResp.body.id) {
    const setHistDefResp = await request(RS, 'POST', `/signalk/v2/api/history/_providers/_default/${histDefResp.body.id}`);
    assertStatus(setHistDefResp, 200, 'POST history set default provider — 200');
  }
}

// ── Cleanup ─────────────────────────────────────────────────────────────────

async function cleanup() {
  // Delete created resources to leave server clean
  if (state.waypointId) {
    await request(RS, 'DELETE', `/signalk/v2/api/resources/waypoints/${state.waypointId}`);
  }
  if (state.waypoint2Id) {
    await request(RS, 'DELETE', `/signalk/v2/api/resources/waypoints/${state.waypoint2Id}`);
  }
  if (state.routeId) {
    await request(RS, 'DELETE', `/signalk/v2/api/resources/routes/${state.routeId}`);
  }
}

// ── Runner ──────────────────────────────────────────────────────────────────

async function run() {
  console.log(`\nSignalK v2 API Conformance`);
  console.log(`  Server: ${RS}\n`);

  // Wait for server
  await retry(async () => {
    const r = await request(RS, 'GET', '/signalk');
    return r.status === 200;
  }, 'signalk-rs');

  console.log('  Server ready.\n');

  await testFeatures();
  await testWaypoints();
  await testRoutes();
  await testCourseDestination();
  await testCourseActiveRoute();
  await testAutopilot();
  await testNotifications();
  await testHistory();

  await cleanup();

  const total = passed + failed;
  console.log(`\n${passed}/${total} passed\n`);
  process.exit(failed);
}

run().catch((err) => {
  console.error('Runner error:', err.message);
  process.exit(1);
});
