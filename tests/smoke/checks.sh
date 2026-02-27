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

# --- Summary ---
echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
