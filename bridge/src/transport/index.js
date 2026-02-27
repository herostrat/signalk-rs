'use strict';

/**
 * Transport factory — creates the appropriate transport based on config.
 *
 * Currently supported:
 *   'uds'  — HTTP over Unix Domain Sockets (default, lowest latency)
 *   'http' — plain TCP HTTP (for Docker/remote bridge)
 *
 * Future (not yet implemented):
 *   'shm'      — shared memory (zero-copy, >1000 msg/s)
 *   'io_uring' — kernel-bypass
 *
 * Transport interface (all methods return Promises):
 *   sendDelta(delta)                     → void
 *   queryPath(skPath)                    → { value, $source, timestamp } | null
 *   querySources(skPath)               → { sourceRef: value, ... } | null
 *   queryMetadata(skPath)               → Metadata | null
 *   writePath(skPath, value)             → void
 *   registerHandler(pluginId, path)      → void
 *   registerPluginRoutes(pluginId, pfx)  → void
 *   register(version)                    → void
 *   reportPlugins(plugins)              → void
 *   startServer()                        → void (sync, starts background server)
 *   stop()                               → void
 *
 * Emits:
 *   'put'       — signalk-rs forwards a PUT to a plugin handler
 *   'lifecycle' — signalk-rs sends a lifecycle event
 *   'proxy'     — signalk-rs proxies a plugin HTTP request
 */

const { UdsTransport } = require('./uds');

function createTransport(config) {
  const backend = config.transport || 'uds';
  switch (backend) {
    case 'uds':
      return new UdsTransport(config);
    case 'http':
      // TODO M5+: implement HttpTransport
      throw new Error('HTTP transport not yet implemented');
    default:
      throw new Error(`Unknown transport backend: ${backend}`);
  }
}

module.exports = { createTransport };
