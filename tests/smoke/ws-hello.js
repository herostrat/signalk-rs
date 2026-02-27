#!/usr/bin/env node
// WebSocket hello check: connect to the stream endpoint, verify the hello message.
// Usage: node ws-hello.js ws://127.0.0.1:13579/signalk/v1/stream
'use strict';

const WebSocket = require('ws');
const url = process.argv[2];

if (!url) {
  console.error('Usage: node ws-hello.js <ws-url>');
  process.exit(1);
}

const TIMEOUT_MS = 10000;
let pass = 0;
let fail = 0;

function check(name, ok) {
  if (ok) {
    console.log(`  PASS  ${name}`);
    pass++;
  } else {
    console.log(`  FAIL  ${name}`);
    fail++;
  }
}

const timer = setTimeout(() => {
  console.log('  FAIL  Timed out waiting for WebSocket hello');
  process.exit(1);
}, TIMEOUT_MS);

const ws = new WebSocket(url);

ws.on('error', (err) => {
  clearTimeout(timer);
  console.log(`  FAIL  WebSocket connection error: ${err.message}`);
  process.exit(1);
});

ws.on('message', (data) => {
  clearTimeout(timer);

  let msg;
  try {
    msg = JSON.parse(data.toString());
  } catch (e) {
    console.log(`  FAIL  Could not parse hello message as JSON`);
    ws.close();
    process.exit(1);
  }

  check('Hello has name field', typeof msg.name === 'string' && msg.name.length > 0);
  check('Hello has version field', typeof msg.version === 'string');
  check('Hello has self field', typeof msg.self === 'string' && msg.self.length > 0);
  check('Hello has roles array', Array.isArray(msg.roles));
  check('Hello has timestamp', typeof msg.timestamp === 'string');

  ws.close();

  console.log('');
  console.log(`Results: ${pass} passed, ${fail} failed`);
  process.exit(fail > 0 ? 1 : 0);
});
