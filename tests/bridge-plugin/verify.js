'use strict';

/**
 * Bridge plugin verification — connects to the NMEA TCP output and verifies
 * that signalk-to-nmea0183 is producing valid NMEA sentences from simulator data.
 *
 * Steps:
 *   1. Wait for signalk-rs to be healthy (REST API responds)
 *   2. Wait for bridge NMEA TCP server to accept connections
 *   3. Collect NMEA sentences for a few seconds
 *   4. Verify: at least some valid NMEA sentences arrived
 *
 * Exit 0 on success, 1 on failure.
 */

const http = require('http');
const net = require('net');

const RS_URL = process.env.RS_URL || 'http://signalk-rs:3000';
const BRIDGE_HOST = process.env.BRIDGE_HOST || 'bridge';
const NMEA_PORT = parseInt(process.env.NMEA_PORT, 10) || 10110;
const COLLECT_SECONDS = parseInt(process.env.COLLECT_SECONDS, 10) || 10;

// ─── Helpers ─────────────────────────────────────────────────────────────────

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

function httpGet(url) {
  return new Promise((resolve, reject) => {
    const u = new URL(url);
    http.get({ hostname: u.hostname, port: u.port, path: u.pathname }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => resolve({ status: res.statusCode, body: Buffer.concat(chunks).toString() }));
    }).on('error', reject);
  });
}

async function retry(fn, { times = 60, delay = 2000, label = '' } = {}) {
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

/**
 * Validate an NMEA sentence:
 *   - Starts with $ or !
 *   - Contains a * followed by two hex checksum chars
 *   - Checksum matches XOR of characters between $ and *
 */
function isValidNmea(line) {
  if (!line.startsWith('$') && !line.startsWith('!')) return false;
  const starIdx = line.lastIndexOf('*');
  if (starIdx === -1 || starIdx + 3 > line.length) return false;

  const body = line.slice(1, starIdx);
  const stated = line.slice(starIdx + 1, starIdx + 3).toUpperCase();

  let computed = 0;
  for (let i = 0; i < body.length; i++) {
    computed ^= body.charCodeAt(i);
  }
  const expected = computed.toString(16).toUpperCase().padStart(2, '0');
  return stated === expected;
}

// ─── Assertions ──────────────────────────────────────────────────────────────

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

// ─── Main ────────────────────────────────────────────────────────────────────

async function main() {
  // 1. Wait for signalk-rs
  process.stdout.write('Waiting for signalk-rs');
  await retry(
    () => httpGet(`${RS_URL}/signalk`).then((r) => {
      if (r.status !== 200) throw new Error(`HTTP ${r.status}`);
    }),
    { label: 'signalk-rs startup' }
  );
  console.log(' ready');

  // 2. Wait for bridge NMEA TCP server
  process.stdout.write('Waiting for bridge NMEA TCP');
  await retry(
    () => new Promise((resolve, reject) => {
      const sock = net.connect({ host: BRIDGE_HOST, port: NMEA_PORT }, () => {
        sock.destroy();
        resolve();
      });
      sock.on('error', reject);
      sock.setTimeout(2000, () => { sock.destroy(); reject(new Error('timeout')); });
    }),
    { label: 'NMEA TCP connect' }
  );
  console.log(' ready\n');

  // Give the simulator a few seconds to generate data and the plugin to process it
  console.log(`Collecting NMEA sentences for ${COLLECT_SECONDS}s...`);
  await wait(3000);

  // 3. Connect and collect NMEA sentences
  const sentences = await new Promise((resolve, reject) => {
    const collected = [];
    let buffer = '';

    const sock = net.connect({ host: BRIDGE_HOST, port: NMEA_PORT }, () => {
      console.log(`Connected to ${BRIDGE_HOST}:${NMEA_PORT}`);
    });

    sock.on('data', (data) => {
      buffer += data.toString();
      const lines = buffer.split('\r\n');
      // Keep the last partial line in the buffer
      buffer = lines.pop();
      for (const line of lines) {
        if (line.length > 0) collected.push(line);
      }
    });

    sock.on('error', (e) => {
      console.error('TCP error:', e.message);
      reject(e);
    });

    setTimeout(() => {
      sock.destroy();
      resolve(collected);
    }, COLLECT_SECONDS * 1000);
  });

  // 4. Validate
  console.log(`\nCollected ${sentences.length} sentence(s)\n`);

  if (sentences.length > 0) {
    console.log('Sample sentences:');
    sentences.slice(0, 5).forEach((s) => console.log(`  ${s}`));
    console.log('');
  }

  console.log('Test 1: NMEA sentence count');
  assert(sentences.length >= 3, `received >= 3 sentences (got ${sentences.length})`);

  console.log('\nTest 2: NMEA sentence format');
  const validCount = sentences.filter(isValidNmea).length;
  assert(validCount > 0, `at least one valid NMEA sentence (${validCount}/${sentences.length} valid)`);

  if (sentences.length > 0) {
    const validRatio = validCount / sentences.length;
    assert(validRatio >= 0.8, `>= 80% valid checksums (${(validRatio * 100).toFixed(0)}%)`);
  }

  console.log('\nTest 3: Sentence types');
  const types = new Set(sentences.map((s) => {
    // Extract sentence type: $GPRMC,... → RMC
    const m = s.match(/^\$..(\w{3})/);
    return m ? m[1] : null;
  }).filter(Boolean));
  console.log(`  Found types: ${[...types].join(', ')}`);
  assert(types.size >= 1, `at least 1 sentence type (got ${types.size})`);

  // ── Summary ────────────────────────────────────────────────────────────────
  console.log(`\n${'─'.repeat(50)}`);
  console.log(`Results: ${passed} passed, ${failed} failed`);

  if (failed > 0) process.exit(1);
}

main().catch((err) => {
  console.error('\nFatal error:', err.message);
  process.exit(1);
});
