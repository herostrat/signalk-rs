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

# Wait for simulator to emit data
sleep 3

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
  "$(fetch "$BASE/admin/api/plugins")" '"simulator"'

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

# --- Summary ---
echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
