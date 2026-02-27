'use strict';

/**
 * StreamBundle — per-path Bacon.js reactive streams over SignalK data.
 *
 * Many SignalK plugins (signalk-to-nmea0183, signalk-derived-data, etc.) use
 * `app.streambundle.getSelfStream(path)` as their primary data access pattern.
 *
 * This implementation opens a single WebSocket connection to signalk-rs,
 * subscribes to all self-vessel paths, and fans out updates to per-path
 * Bacon.js Buses. Plugins get EventStreams that emit raw values.
 */

const Bacon = require('baconjs');
const WebSocket = require('ws');

class StreamBundle {
  /**
   * @param {string} wsUrl - WebSocket URL, e.g. ws://localhost:3000/signalk/v1/stream
   */
  constructor(wsUrl) {
    this._wsUrl = wsUrl;
    /** @type {Map<string, Bacon.Bus>} per-path buses */
    this._buses = new Map();
    /** @type {Map<string, *>} latest value per path (for late subscribers) */
    this._latest = new Map();
    this._ws = null;
    this._connected = false;
    this._reconnectTimer = null;
  }

  /**
   * Open the WebSocket and subscribe to all self-vessel updates.
   * Call this once during bridge startup.
   */
  start() {
    this._connect();
  }

  /** Close the WebSocket and end all buses. */
  stop() {
    if (this._reconnectTimer) {
      clearTimeout(this._reconnectTimer);
      this._reconnectTimer = null;
    }
    if (this._ws) {
      this._ws.close();
      this._ws = null;
    }
    for (const bus of this._buses.values()) {
      bus.end();
    }
    this._buses.clear();
    this._latest.clear();
    this._connected = false;
  }

  /**
   * Get a Bacon.js EventStream for a self-vessel path.
   *
   * The stream emits raw values (not wrapped in {value, $source, timestamp}).
   * If a value has already been received for this path, the stream starts
   * with that cached value (Bacon.once + merge).
   *
   * This is the primary API that plugins use:
   *   app.streambundle.getSelfStream('navigation.speedOverGround')
   *     .onValue(sog => console.log('SOG:', sog))
   *
   * @param {string} skPath - dot-notation SignalK path
   * @returns {Bacon.EventStream}
   */
  getSelfStream(skPath) {
    // In baconjs 1.x, Bus IS an EventStream (no toEventStream() needed).
    const bus = this._getOrCreateBus(skPath);

    // If we already have a cached value, prepend it so late subscribers
    // get the current state immediately (like Bacon.combineWith expects).
    if (this._latest.has(skPath)) {
      return Bacon.once(this._latest.get(skPath)).merge(bus);
    }
    return bus;
  }

  /**
   * Get the Bacon.js Bus for a self-vessel path (writable + readable).
   * Plugins can push values into the bus as well as subscribe.
   *
   * @param {string} skPath - dot-notation SignalK path
   * @returns {Bacon.Bus}
   */
  getSelfBus(skPath) {
    return this._getOrCreateBus(skPath);
  }

  /**
   * Get a bus for an arbitrary topic (used by some plugins).
   * In the original server, this wraps any key in the data model.
   *
   * @param {string} topic
   * @returns {Bacon.Bus}
   */
  getBus(topic) {
    return this._getOrCreateBus(topic);
  }

  // ─── Internal ──────────────────────────────────────────────────────────────

  _getOrCreateBus(key) {
    let bus = this._buses.get(key);
    if (!bus) {
      bus = new Bacon.Bus();
      this._buses.set(key, bus);
    }
    return bus;
  }

  _connect() {
    const url = this._wsUrl + (this._wsUrl.includes('?') ? '&' : '?') + 'subscribe=none';
    this._ws = new WebSocket(url);

    this._ws.on('open', () => {
      this._connected = true;
      console.log('[streambundle] WebSocket connected');

      // Subscribe to all self-vessel paths (** matches any depth)
      this._ws.send(JSON.stringify({
        context: 'vessels.self',
        subscribe: [{ path: '**' }],
      }));
    });

    this._ws.on('message', (data) => {
      try {
        const msg = JSON.parse(data);
        if (msg.updates) {
          this._processDelta(msg);
        }
      } catch (e) {
        // Ignore parse errors (hello message, etc.)
      }
    });

    this._ws.on('close', () => {
      this._connected = false;
      console.log('[streambundle] WebSocket closed, reconnecting in 2s...');
      this._reconnectTimer = setTimeout(() => this._connect(), 2000);
    });

    this._ws.on('error', (e) => {
      console.error('[streambundle] WebSocket error:', e.message);
    });
  }

  /**
   * Extract path+value pairs from a delta and push to the corresponding buses.
   */
  _processDelta(delta) {
    if (!delta.updates) return;

    for (const update of delta.updates) {
      if (!update.values) continue;
      for (const { path: skPath, value } of update.values) {
        if (!skPath || value === undefined) continue;

        // Cache the latest value
        this._latest.set(skPath, value);

        // Push to the bus (if anyone is listening)
        const bus = this._buses.get(skPath);
        if (bus) {
          bus.push(value);
        }
      }
    }
  }
}

module.exports = { StreamBundle };
