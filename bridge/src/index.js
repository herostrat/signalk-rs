'use strict';

/**
 * signalk-bridge — entry point
 *
 * Starts the bridge between signalk-rs and Node.js plugins.
 *
 * Environment variables:
 *   SIGNALK_BRIDGE_TOKEN   — shared secret (required, set by signalk-rs)
 *   SIGNALK_RS_SOCKET      — signalk-rs UDS socket path (default: /run/signalk/rs.sock)
 *   SIGNALK_BRIDGE_SOCKET  — bridge UDS socket path (default: /run/signalk/bridge.sock)
 *   SIGNALK_WS_URL         — public WebSocket URL for subscriptions
 *   SIGNALK_SELF_URI       — vessel UUID
 *   SIGNALK_DATA_DIR       — data directory for plugin config/data
 *   SIGNALK_PLUGINS_DIR    — node_modules directory to scan for plugins
 *   SIGNALK_TRANSPORT      — transport backend: 'uds' (default) | 'http'
 */

const path = require('path');

const { createTransport } = require('./transport');
const { SignalKApp } = require('./app');
const { PluginLoader } = require('./plugins/loader');
const { NmeaTcpServer } = require('./nmea-tcp');

const BRIDGE_VERSION = require('../package.json').version;

async function main() {
  const token = process.env.SIGNALK_BRIDGE_TOKEN;
  if (!token) {
    console.error('[bridge] SIGNALK_BRIDGE_TOKEN is required');
    process.exit(1);
  }

  const config = {
    transport: process.env.SIGNALK_TRANSPORT || 'uds',
    rsSocket: process.env.SIGNALK_RS_SOCKET || '/run/signalk/rs.sock',
    bridgeSocket: process.env.SIGNALK_BRIDGE_SOCKET || '/run/signalk/bridge.sock',
    bridgeToken: token,
  };

  const selfUri = process.env.SIGNALK_SELF_URI || 'urn:mrn:signalk:uuid:unknown';
  const dataDir = process.env.SIGNALK_DATA_DIR || path.join(process.cwd(), '.signalk');
  const pluginsDir = process.env.SIGNALK_PLUGINS_DIR || path.join(process.cwd(), 'node_modules');

  console.log(`[bridge] signalk-bridge v${BRIDGE_VERSION} starting`);
  console.log(`[bridge] Transport: ${config.transport}`);
  console.log(`[bridge] Plugins dir: ${pluginsDir}`);

  // Create transport
  const transport = createTransport(config);

  // Start bridge callback server
  transport.startServer();

  // Register with signalk-rs
  try {
    await transport.register(BRIDGE_VERSION);
    console.log('[bridge] Registered with signalk-rs');
  } catch (err) {
    console.error('[bridge] Failed to register with signalk-rs:', err.message);
    console.error('[bridge] Is signalk-rs running? Retrying in 5s...');
    await new Promise((r) => setTimeout(r, 5000));
    await transport.register(BRIDGE_VERSION);
  }

  // WebSocket URL for streambundle (reactive streams for plugins)
  const wsUrl = process.env.SIGNALK_WS_URL || 'ws://localhost:3000/signalk/v1/stream';

  // Create app object
  const app = new SignalKApp({ transport, selfUri, dataDir, config, wsUrl });

  // Start streambundle (opens WS connection for reactive data feeds)
  app.startStreams();

  // Start NMEA 0183 TCP output server (forwards plugin nmea0183out events)
  const nmeaTcp = new NmeaTcpServer(app);
  nmeaTcp.start();

  // Load plugins
  const loader = new PluginLoader(app, pluginsDir);
  await loader.loadAll();

  // Handle lifecycle events from signalk-rs
  transport.on('lifecycle', async ({ event, pluginId }) => {
    console.log(`[bridge] Lifecycle event: ${event}`, pluginId || '(all)');
    if (event === 'stop') {
      if (pluginId) {
        // TODO: stop specific plugin
      } else {
        await loader.stopAll();
      }
    }
  });

  // Graceful shutdown
  const shutdown = async (signal) => {
    console.log(`[bridge] ${signal} received, stopping plugins...`);
    await loader.stopAll();
    nmeaTcp.stop();
    app.stopStreams();
    await transport.stop();
    process.exit(0);
  };

  process.on('SIGTERM', () => shutdown('SIGTERM'));
  process.on('SIGINT', () => shutdown('SIGINT'));

  console.log(`[bridge] Running with ${loader.list().length} plugin(s)`);
}

main().catch((err) => {
  console.error('[bridge] Fatal error:', err);
  process.exit(1);
});
