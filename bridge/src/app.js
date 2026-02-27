'use strict';

/**
 * Plugin app object — implements the full SignalK plugin API surface.
 *
 * Plugins receive an instance of this as their first argument:
 *   module.exports = function(app) { ... }
 *
 * This implementation proxies all calls to signalk-rs via the internal API.
 * See: https://demo.signalk.org/documentation/develop/plugins/server_plugin.html
 */

const { EventEmitter } = require('events');
const path = require('path');
const fs = require('fs');
const { StreamBundle } = require('./streambundle');

class SignalKApp extends EventEmitter {
  /**
   * @param {object} options
   * @param {object} options.transport   - transport instance (UdsTransport etc.)
   * @param {string} options.selfUri     - vessel UUID
   * @param {string} options.dataDir     - base data directory for plugin data
   * @param {object} options.config      - server config
   * @param {string} [options.wsUrl]     - WebSocket URL for streambundle
   */
  constructor({ transport, selfUri, dataDir, config, wsUrl }) {
    super();
    this._transport = transport;
    this._selfUri = selfUri;
    this._dataDir = dataDir;
    this._config = config;
    this._pluginStatuses = new Map();
    this._putHandlers = new Map();  // path → handler fn

    /** Self vessel ID — full URN, used by plugins in delta context matching. */
    this.selfId = selfUri;

    // streambundle — per-path Bacon.js reactive streams
    const bundleUrl = wsUrl || process.env.SIGNALK_WS_URL || 'ws://localhost:3000/signalk/v1/stream';
    this.streambundle = new StreamBundle(bundleUrl);

    // subscriptionmanager — plugin's subscribemanager.subscribe() interface
    this.subscriptionmanager = this._createSubscriptionManager();

    // Wire up transport callbacks
    transport.on('put', ({ pluginId, path: skPath, value, requestId, respond }) => {
      const handler = this._putHandlers.get(pluginId + ':' + skPath)
        || this._findPutHandler(pluginId, skPath);
      if (handler) {
        Promise.resolve(handler(skPath, value))
          .then(() => respond('COMPLETED', 200))
          .catch(() => respond('FAILED', 500));
      } else {
        respond('FAILED', 404);
      }
    });
  }

  // ─── Data access ────────────────────────────────────────────────────────────

  /** Get current value from the self vessel. Returns undefined if not found. */
  getSelfPath(skPath) {
    return this._transport.queryPath(skPath)
      .then((resp) => resp ? resp.value : undefined);
  }

  /** Get value from the full data model root (any vessel). */
  getPath(fullPath) {
    // fullPath: "vessels.self.navigation.speedOverGround"
    // → strip "vessels.self." prefix and use getSelfPath
    if (fullPath.startsWith('vessels.self.')) {
      return this.getSelfPath(fullPath.slice('vessels.self.'.length));
    }
    // TODO: cross-vessel path queries (M7+)
    return Promise.resolve(undefined);
  }

  /** Write a value to the self vessel. */
  putSelfPath(skPath, value) {
    return this._transport.writePath(skPath, value);
  }

  /** Write a value to the full data model root. */
  putPath(fullPath, value) {
    if (fullPath.startsWith('vessels.self.')) {
      return this.putSelfPath(fullPath.slice('vessels.self.'.length), value);
    }
    return Promise.reject(new Error(`putPath: cross-vessel writes not yet supported`));
  }

  /**
   * Get all source values for a path on the self vessel.
   * Returns an object mapping source_ref → value, or null if not found.
   */
  getSelfPathSources(skPath) {
    return this._transport.querySources(skPath);
  }

  // ─── Delta emission ──────────────────────────────────────────────────────────

  /**
   * Send a delta to signalk-rs (the primary way plugins emit data).
   * Accepts the same delta format as the SignalK spec.
   */
  handleMessage(pluginId, delta) {
    // Ensure context is set
    if (!delta.context) delta = { context: 'vessels.self', ...delta };
    return this._transport.sendDelta(delta);
  }

  /**
   * Emit a notification delta (convenience wrapper around handleMessage).
   *
   * @param {string} pluginId - plugin identifier (used as source label)
   * @param {string} skPath   - notification path without "notifications." prefix
   * @param {object} notification
   * @param {string} notification.state   - "alarm", "warn", "alert", "normal", etc.
   * @param {string[]} notification.method - ["visual", "sound"]
   * @param {string} notification.message - human-readable message
   */
  notify(pluginId, skPath, { state, method, message }) {
    return this.handleMessage(pluginId, {
      context: 'vessels.self',
      updates: [{
        source: { label: pluginId, type: 'plugin' },
        values: [{
          path: `notifications.${skPath}`,
          value: { state, method, message },
        }],
      }],
    });
  }

  // ─── PUT handler registration ────────────────────────────────────────────────

  /**
   * Register a PUT handler for a path pattern.
   * signalk-rs will call this when a PUT arrives for the path.
   *
   * @param {string}   context   - vessel context e.g. 'vessels.self'
   * @param {string}   skPath    - path or pattern e.g. 'steering.autopilot.target.*'
   * @param {Function} handler   - async (path, value) => void
   * @param {string}   pluginId  - plugin identifier
   */
  async registerPutHandler(context, skPath, handler, pluginId) {
    const key = pluginId + ':' + skPath;
    this._putHandlers.set(key, handler);
    await this._transport.registerHandler(pluginId, skPath);
  }

  /** Find the best matching PUT handler for a path. */
  _findPutHandler(pluginId, skPath) {
    for (const [key, handler] of this._putHandlers) {
      const [pid, pattern] = key.split(':');
      if (pid === pluginId) {
        // Simple prefix match — full wildcard matching done by signalk-rs
        if (skPath.startsWith(pattern.replace('.*', '').replace('*', ''))) {
          return handler;
        }
      }
    }
    return null;
  }

  // ─── Router registration (plugin REST endpoints) ─────────────────────────────

  /**
   * Register an Express router for plugin REST endpoints.
   * Called by plugins: app.registerWithRouter(router => { router.get('/...', ...) })
   *
   * @param {Function} routerCallback - (router) => void, receives an Express router
   * @param {string}   pluginId       - plugin identifier
   */
  async registerWithRouter(routerCallback, pluginId) {
    const express = require('express');
    const router = express.Router();
    routerCallback(router);

    // Store router for proxying
    this._pluginRouters = this._pluginRouters || new Map();
    this._pluginRouters.set(pluginId, router);

    await this._transport.registerPluginRoutes(pluginId, `/plugins/${pluginId}`);

    // Wire up proxy requests from transport
    this._transport.on('proxy', ({ pluginId: pid, method, path: ppath, body, respond }) => {
      if (pid !== pluginId) return;
      // Create a minimal mock req/res for Express
      const mockReq = { method, url: ppath, body, headers: {} };
      const mockRes = {
        status(code) { this._code = code; return this; },
        json(data) { respond(this._code || 200, data); },
        send(data) { respond(this._code || 200, data, 'text/plain'); },
        _code: 200,
      };
      router.handle(mockReq, mockRes, (err) => {
        if (err) respond(500, { message: err.message });
        else respond(404, { message: 'Not found' });
      });
    });
  }

  // ─── Plugin status & error reporting ─────────────────────────────────────────

  setPluginStatus(pluginId, message) {
    this._pluginStatuses.set(pluginId, { state: 'ok', message, ts: Date.now() });
    // Emit for internal monitoring
    this.emit('plugin:status', { pluginId, message });
  }

  setPluginError(pluginId, message) {
    this._pluginStatuses.set(pluginId, { state: 'error', message, ts: Date.now() });
    this.emit('plugin:error', { pluginId, message });
    console.error(`[plugin:${pluginId}] ERROR: ${message}`);
  }

  // ─── Plugin configuration persistence ────────────────────────────────────────

  savePluginOptions(pluginId, options) {
    const file = this._pluginConfigPath(pluginId);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify(options, null, 2));
  }

  readPluginOptions(pluginId) {
    const file = this._pluginConfigPath(pluginId);
    if (!fs.existsSync(file)) return {};
    try {
      return JSON.parse(fs.readFileSync(file, 'utf8'));
    } catch {
      return {};
    }
  }

  /** Plugin-specific data directory. */
  getDataDirPath(pluginId) {
    const dir = path.join(this._dataDir, 'plugin-data', pluginId);
    fs.mkdirSync(dir, { recursive: true });
    return dir;
  }

  _pluginConfigPath(pluginId) {
    return path.join(this._dataDir, 'plugin-config', `${pluginId}.json`);
  }

  // ─── StreamBundle lifecycle ─────────────────────────────────────────────────

  /** Start the streambundle WebSocket. Call once after construction. */
  startStreams() {
    this.streambundle.start();
  }

  /** Stop the streambundle WebSocket and end all buses. */
  stopStreams() {
    this.streambundle.stop();
  }

  // ─── Metadata ───────────────────────────────────────────────────────────────

  /** Get metadata for a path. */
  getMetadata(skPath) {
    return this._transport.queryMetadata(skPath);
  }

  // ─── Delta input handler registration ────────────────────────────────────────

  /**
   * Register a handler for incoming deltas before they hit the store.
   * Plugins use this to transform or filter data.
   */
  registerDeltaInputHandler(handler) {
    this.on('delta:input', handler);
  }

  // ─── Internal helpers ────────────────────────────────────────────────────────

  _createSubscriptionManager() {
    const transport = this._transport;
    const selfUri = this._selfUri;

    return {
      /**
       * Subscribe to SignalK data updates.
       *
       * @param {object}   subscription
       * @param {string}   subscription.context  - vessel context
       * @param {Array}    subscription.subscribe - array of { path, period, policy }
       * @param {Function} callback               - called with each delta update
       * @param {object}   [unsubscribes]         - accumulates unsubscribe functions
       * @param {string}   [pluginId]             - plugin identifier
       */
      subscribe(subscription, unsubscribes, callback, pluginId) {
        // Connect to signalk-rs WebSocket stream for subscriptions
        // The WS URL comes from config
        const baseUrl = process.env.SIGNALK_WS_URL || 'ws://localhost:3000/signalk/v1/stream';
        const WS_URL = baseUrl + (baseUrl.includes('?') ? '&' : '?') + 'subscribe=none';
        const WebSocket = require('ws');
        const ws = new WebSocket(WS_URL);

        ws.on('open', () => {
          ws.send(JSON.stringify(subscription));
        });

        ws.on('message', (data) => {
          try {
            const delta = JSON.parse(data);
            if (delta.updates) callback(delta);
          } catch (e) {
            console.error('[bridge] Failed to parse delta:', e);
          }
        });

        ws.on('error', (e) => console.error('[bridge] WS error:', e));

        // Return unsubscribe function
        const unsubFn = () => ws.close();
        if (unsubscribes && Array.isArray(unsubscribes)) {
          unsubscribes.push(unsubFn);
        }
        return unsubFn;
      },
    };
  }
}

module.exports = { SignalKApp };
