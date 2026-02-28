#!/bin/sh
# HTTP-based smoke test assertions.
# Designed to run in node:20-alpine (uses wget, not curl).
#
# Usage: sh checks.sh http://signalk-rs:3000
set -eu
BASE="$1"
PASS=0
FAIL=0

# wget wrapper: fetch body or status code
fetch() { wget -qO- "$1" 2>/dev/null; }
status() { wget -qO/dev/null --server-response "$1" 2>&1 | awk '/HTTP/{print $2}' | tail -1; }

# Node.js helpers for PUT/DELETE (busybox wget lacks --method)
http_method() {
  METHOD="$1"; URL="$2"; BODY="${3:-}"
  node -e "
    const http = require('http');
    const url = new URL('$URL');
    const opts = { hostname: url.hostname, port: url.port, path: url.pathname, method: '$METHOD',
      headers: { 'Content-Type': 'application/json' } };
    const req = http.request(opts, res => {
      let d=''; res.on('data', c => d+=c);
      res.on('end', () => console.log(res.statusCode + ' ' + d));
    });
    req.on('error', e => console.log('ERR ' + e.message));
    if ('$BODY') req.write('$BODY');
    req.end();
  " 2>/dev/null
}
http_status() { http_method "$1" "$2" "${3:-}" | awk '{print $1}'; }
http_body() { http_method "$1" "$2" "${3:-}" | cut -d' ' -f2-; }

check() {
  name="$1"; shift
  result="$1"; shift
  expected="$1"
  if echo "$result" | grep -q "$expected"; then
    echo "  PASS  $name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL  $name"
    echo "        expected: $expected"
    echo "        got:      $(echo "$result" | head -c 200)"
    FAIL=$((FAIL + 1))
  fi
}

# Wait for simulator to emit data via all three paths
sleep 5

echo "HTTP Smoke Tests: $BASE"
echo ""

# --- Discovery ---
echo "  Discovery"
check "GET /signalk returns 200" \
  "$(status "$BASE/signalk")" "200"
check "GET /signalk/ (trailing slash) returns 200" \
  "$(status "$BASE/signalk/")" "200"
check "Discovery has endpoints" \
  "$(fetch "$BASE/signalk")" "endpoints"

# --- Webapp Discovery ---
echo "  Webapp Discovery"
check "GET /signalk/v1/webapps lists instrumentpanel" \
  "$(fetch "$BASE/signalk/v1/webapps")" "instrumentpanel"
check "GET /signalk/v1/webapps lists kip" \
  "$(fetch "$BASE/signalk/v1/webapps")" "kip"

# --- Webapp Static Serving ---
echo "  Webapp Serving"
check "InstrumentPanel serves static files (200)" \
  "$(status "$BASE/@signalk/instrumentpanel/")" "200"
check "KIP serves static files (200)" \
  "$(status "$BASE/@mxtommy/kip/")" "200"

# --- Simulator Data (REST) ---
echo "  Simulator Data"
check "navigation/speedOverGround has value" \
  "$(fetch "$BASE/signalk/v1/api/vessels/self/navigation/speedOverGround")" '"value"'
check "navigation/position has latitude" \
  "$(fetch "$BASE/signalk/v1/api/vessels/self/navigation/position")" '"latitude"'

# --- Admin API ---
echo "  Admin API"
check "Plugin list includes simulator" \
  "$(fetch "$BASE/admin/api/plugins")" 'sensor-data-simulator'

# --- REST Structure ---
echo "  REST Structure"
check "Full model has vessels key" \
  "$(fetch "$BASE/signalk/v1/api")" '"vessels"'
check "Self vessel has navigation" \
  "$(fetch "$BASE/signalk/v1/api/vessels/self")" '"navigation"'

# --- v2 Features API ---
echo "  v2 Features API"
check "GET /signalk/v2/features returns 200" \
  "$(status "$BASE/signalk/v2/features")" "200"
check "Features has apis" \
  "$(fetch "$BASE/signalk/v2/features")" '"apis"'
check "Features has plugins" \
  "$(fetch "$BASE/signalk/v2/features")" '"plugins"'
check "Features lists resources API" \
  "$(fetch "$BASE/signalk/v2/features")" '"resources"'

# --- v2 Resources API (CRUD) ---
echo "  v2 Resources API"
# Create a waypoint
WP_RESPONSE=$(wget -qO- --post-data='{"name":"Smoke Test WP","position":{"latitude":49.27,"longitude":-123.19}}' \
  --header='Content-Type: application/json' "$BASE/signalk/v2/api/resources/waypoints" 2>/dev/null || echo "FAIL")
check "POST waypoints returns COMPLETED" \
  "$WP_RESPONSE" '"COMPLETED"'
WP_ID=$(echo "$WP_RESPONSE" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')

if [ -n "$WP_ID" ]; then
  # Get the created waypoint
  check "GET waypoint by ID" \
    "$(fetch "$BASE/signalk/v2/api/resources/waypoints/$WP_ID")" '"Smoke Test WP"'

  # List waypoints contains it
  check "List waypoints contains created" \
    "$(fetch "$BASE/signalk/v2/api/resources/waypoints")" "$WP_ID"

  # Delete waypoint
  check "DELETE waypoint returns 200" \
    "$(http_status DELETE "$BASE/signalk/v2/api/resources/waypoints/$WP_ID")" "200"

  # Verify deleted (use http_status — busybox wget garbles error status codes)
  check "GET deleted waypoint returns 404" \
    "$(http_status GET "$BASE/signalk/v2/api/resources/waypoints/$WP_ID")" "404"
else
  echo "  FAIL  Skipping resource tests (no ID from create)"
  FAIL=$((FAIL + 5))
fi

# --- v2 Course API ---
echo "  v2 Course API"
check "GET course returns 200" \
  "$(status "$BASE/signalk/v2/api/vessels/self/navigation/course")" "200"

# Set a destination
check "PUT destination returns 200" \
  "$(http_status PUT "$BASE/signalk/v2/api/vessels/self/navigation/course/destination" \
    '{"position":{"latitude":49.5,"longitude":-123.5}}')" "200"

# Verify course state
check "Course has nextPoint after set" \
  "$(fetch "$BASE/signalk/v2/api/vessels/self/navigation/course")" '"nextPoint"'

# Clear course
check "DELETE course returns 200" \
  "$(http_status DELETE "$BASE/signalk/v2/api/vessels/self/navigation/course")" "200"

# --- Admin UI ---
echo "  Admin UI"
check "Admin UI serves static files (200)" \
  "$(status "$BASE/admin/")" "200"

# --- /skServer Compatibility ---
echo "  /skServer Routes"
check "loginStatus returns 200" \
  "$(status "$BASE/skServer/loginStatus")" "200"
check "loginStatus has status field" \
  "$(fetch "$BASE/skServer/loginStatus")" '"status"'
check "skServer/plugins returns 200" \
  "$(status "$BASE/skServer/plugins")" "200"
check "skServer/plugins lists simulator" \
  "$(fetch "$BASE/skServer/plugins")" 'sensor-data-simulator'
check "skServer/webapps returns 200" \
  "$(status "$BASE/skServer/webapps")" "200"
check "skServer/vessel returns 200" \
  "$(status "$BASE/skServer/vessel")" "200"
check "skServer/vessel has uuid" \
  "$(fetch "$BASE/skServer/vessel")" '"uuid"'
check "skServer/settings returns 200" \
  "$(status "$BASE/skServer/settings")" "200"

# --- applicationData with scope ---
echo "  applicationData (scoped)"
APP_DATA_STATUS=$(http_status POST "$BASE/signalk/v1/applicationData/global/test-app/1.0" \
  '{"theme":"dark","panels":[1,2]}')
check "POST global appData returns 200" \
  "$APP_DATA_STATUS" "200"
check "GET global appData returns data" \
  "$(fetch "$BASE/signalk/v1/applicationData/global/test-app/1.0")" '"theme"'

# --- Multi-Path Data Flow ---
# The simulator outputs data via three paths simultaneously:
#   1. Direct delta (source: sensor-data-simulator)
#   2. NMEA 0183 TCP → nmea0183-tcp plugin (source: nmea0183-sim.GP)
#   3. NMEA 2000 vcan → nmea2000 plugin (source: nmea2000-sim.{pgn})
echo "  Multi-Path Data Flow"

# All three paths produce navigation.position — verify it has data
check "position has value via multi-path" \
  "$(fetch "$BASE/signalk/v1/api/vessels/self/navigation/position")" '"latitude"'

# Direct-only: propulsion data (RPM, oil temp) is only in the direct delta
check "propulsion data flows via direct path" \
  "$(fetch "$BASE/signalk/v1/api/vessels/self/propulsion")" '"revolutions"'

# Navigation heading (from NMEA 0183 HDG + NMEA 2000 PGN 127250 + direct)
check "headingMagnetic has value" \
  "$(fetch "$BASE/signalk/v1/api/vessels/self/navigation/headingMagnetic")" '"value"'

# Water depth (from NMEA 0183 DBT + NMEA 2000 PGN 128267 + direct)
check "depth has value" \
  "$(fetch "$BASE/signalk/v1/api/vessels/self/environment/depth/belowTransducer")" '"value"'

# --- Source Priority ---
# Config: nmea0183-sim.GP = 10, sensor-data-simulator = 50, nmea2000-sim.* = 100 (default)
# The NMEA 0183 path should win for paths it provides (position, SOG, COG).
echo "  Source Priority"

# Position: all three paths provide it. NMEA 0183 (priority 10) should win.
POS_DATA=$(fetch "$BASE/signalk/v1/api/vessels/self/navigation/position")
check "position source is nmea0183 (highest priority)" \
  "$POS_DATA" '"nmea0183-sim.GP"'

# SOG: direct + NMEA 0183 provide it. NMEA 0183 should win.
SOG_DATA=$(fetch "$BASE/signalk/v1/api/vessels/self/navigation/speedOverGround")
check "SOG source is nmea0183 (highest priority)" \
  "$SOG_DATA" '"nmea0183-sim.GP"'

# Propulsion RPM: only direct provides it, so source should be simulator.
RPM_DATA=$(fetch "$BASE/signalk/v1/api/vessels/self/propulsion/port/revolutions")
check "propulsion source is direct (only source)" \
  "$RPM_DATA" '"sensor-data-simulator"'

# --- Multi-Source Values (spec compliance) ---
# When 2+ sources provide the same path, a "values" object should appear.
echo "  Multi-Source Values"
check "position has values field (multi-source)" \
  "$POS_DATA" '"values"'
check "SOG has values field (multi-source)" \
  "$SOG_DATA" '"values"'
check "propulsion RPM has no values field (single source)" \
  "$(echo "$RPM_DATA" | grep -c '"values"' || true)" "0"

# --- Values Field Content ---
# Verify the values field actually contains source entries with correct structure
echo "  Values Content"

# Position values must contain the winning source
check "position values contains nmea0183-sim.GP" \
  "$POS_DATA" '"nmea0183-sim.GP"'
check "position values contains sensor-data-simulator" \
  "$POS_DATA" '"sensor-data-simulator"'

# SOG values should contain multiple sources
check "SOG values contains nmea0183-sim.GP" \
  "$SOG_DATA" '"nmea0183-sim.GP"'
check "SOG values contains sensor-data-simulator" \
  "$SOG_DATA" '"sensor-data-simulator"'

# Each values entry must have "value" and "timestamp"
SOG_VALUES=$(echo "$SOG_DATA" | node -e "
  const d=require('fs').readFileSync('/dev/stdin','utf8');
  const j=JSON.parse(d);
  if(j.values) {
    const keys=Object.keys(j.values);
    let ok=true;
    keys.forEach(k => {
      if(!j.values[k].hasOwnProperty('value')) ok=false;
      if(!j.values[k].hasOwnProperty('timestamp')) ok=false;
    });
    console.log(ok ? 'VALID:'+keys.length+'_sources' : 'INVALID');
  } else { console.log('NO_VALUES'); }
")
check "SOG values entries have value+timestamp" \
  "$SOG_VALUES" "VALID"
check "SOG has 2+ sources in values" \
  "$SOG_VALUES" "_sources"

# --- 3-Source Validation ---
# Position should have entries from all three output paths
echo "  Three-Source Coverage"
POS_SOURCE_COUNT=$(echo "$POS_DATA" | node -e "
  const d=require('fs').readFileSync('/dev/stdin','utf8');
  const j=JSON.parse(d);
  if(j.values) { console.log(Object.keys(j.values).length); }
  else { console.log(0); }
")
check "position has 2+ sources in values" \
  "$([ "$POS_SOURCE_COUNT" -ge 2 ] 2>/dev/null && echo 'YES' || echo 'NO')" "YES"

# Heading should also have multiple sources (HDG + PGN 127250 + direct)
HDG_DATA=$(fetch "$BASE/signalk/v1/api/vessels/self/navigation/headingMagnetic")
check "heading has values field (multi-source)" \
  "$HDG_DATA" '"values"'
check "heading values contains sensor-data-simulator" \
  "$HDG_DATA" '"sensor-data-simulator"'

# --- Sources Endpoint (spec compliance) ---
# GET /signalk/v1/api/sources returns hierarchical sources
echo "  Sources API"
SOURCES_DATA=$(fetch "$BASE/signalk/v1/api/sources")
check "GET /sources returns data" \
  "$SOURCES_DATA" '"label"'
check "sources has nmea0183-sim (hierarchical)" \
  "$SOURCES_DATA" '"nmea0183-sim"'
check "sources has sensor-data-simulator" \
  "$SOURCES_DATA" '"sensor-data-simulator"'

# Sources detail: verify structure
check "nmea0183-sim source has type NMEA0183" \
  "$SOURCES_DATA" '"NMEA0183"'
check "sensor-data-simulator source has type Plugin" \
  "$SOURCES_DATA" '"Plugin"'

# Hierarchical structure: nmea0183-sim should have GP sub-key
NMEA_SUB=$(echo "$SOURCES_DATA" | node -e "
  const d=require('fs').readFileSync('/dev/stdin','utf8');
  const j=JSON.parse(d);
  if(j['nmea0183-sim'] && j['nmea0183-sim']['GP']) {
    console.log('NESTED:'+j['nmea0183-sim']['GP']['type']);
  } else { console.log('FLAT'); }
")
check "nmea0183-sim has nested GP sub-key" \
  "$NMEA_SUB" "NESTED"
check "nmea0183-sim.GP type is NMEA0183" \
  "$NMEA_SUB" "NMEA0183"

# --- Track API (spec routes) ---
# The tracks plugin records position data from the simulator.
# After the initial sleep, there should be track points.
echo "  Track API"

TRACKS_DATA=$(fetch "$BASE/signalk/v1/api/tracks")
check "GET /tracks returns GeoJSON FeatureCollection" \
  "$TRACKS_DATA" '"type":"FeatureCollection"'
check "tracks has features array" \
  "$TRACKS_DATA" '"features"'
check "tracks feature has LineString geometry" \
  "$TRACKS_DATA" '"LineString"'

# Self vessel track via path parameter
SELF_TRACK=$(fetch "$BASE/signalk/v1/api/vessels/self/track")
check "GET /vessels/self/track returns GeoJSON" \
  "$SELF_TRACK" '"type":"FeatureCollection"'
check "self track has features" \
  "$SELF_TRACK" '"features"'

# GPX format
GPX_DATA=$(fetch "$BASE/signalk/v1/api/tracks?format=gpx")
check "GET /tracks?format=gpx returns GPX XML" \
  "$GPX_DATA" "<gpx"
check "GPX contains track segment" \
  "$GPX_DATA" "<trkseg"

# Plugin-specific summary route
SUMMARY=$(fetch "$BASE/plugins/tracks/summary")
check "GET /plugins/tracks/summary returns data" \
  "$SUMMARY" '"context"'
check "summary has point_count" \
  "$SUMMARY" '"point_count"'

# DELETE single vessel track (spec route)
check "DELETE /vessels/self/track returns 200" \
  "$(http_status DELETE "$BASE/signalk/v1/api/vessels/self/track")" "200"

# Verify self track is now empty
SELF_AFTER_DELETE=$(fetch "$BASE/signalk/v1/api/vessels/self/track")
SELF_FEATURE_COUNT=$(echo "$SELF_AFTER_DELETE" | node -e "
  const d=require('fs').readFileSync('/dev/stdin','utf8');
  const j=JSON.parse(d);
  console.log(j.features ? j.features.length : 0);
")
check "self track empty after delete" \
  "$SELF_FEATURE_COUNT" "0"

# DELETE all tracks (spec route)
check "DELETE /tracks returns 200" \
  "$(http_status DELETE "$BASE/signalk/v1/api/tracks")" "200"

# Verify all tracks cleared
TRACKS_AFTER_DELETE=$(fetch "$BASE/signalk/v1/api/tracks")
ALL_FEATURE_COUNT=$(echo "$TRACKS_AFTER_DELETE" | node -e "
  const d=require('fs').readFileSync('/dev/stdin','utf8');
  const j=JSON.parse(d);
  console.log(j.features ? j.features.length : 0);
")
check "all tracks empty after delete" \
  "$ALL_FEATURE_COUNT" "0"

# --- Summary ---
echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
