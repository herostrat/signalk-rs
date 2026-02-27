'use strict';

/**
 * Unix Domain Socket transport for the signalk-rs internal API.
 *
 * Bridge → signalk-rs: HTTP over /run/signalk/rs.sock
 * signalk-rs → Bridge: HTTP server on /run/signalk/bridge.sock
 *
 * This module implements the BridgeTransport interface using UDS.
 * See transport/index.js for the interface definition.
 */

const http = require('http');
const fs = require('fs');
const path = require('path');
const { EventEmitter } = require('events');

/**
 * Make an HTTP request over a Unix Domain Socket.
 *
 * @param {string} socketPath - Path to the UDS socket
 * @param {string} method     - HTTP method
 * @param {string} endpoint   - URL path (e.g. '/internal/v1/delta')
 * @param {object} [body]     - JSON body (optional)
 * @param {string} token      - X-Bridge-Token header value
 * @returns {Promise<{status: number, body: any}>}
 */
function udsRequest(socketPath, method, endpoint, body, token) {
  return new Promise((resolve, reject) => {
    const payload = body != null ? JSON.stringify(body) : null;

    const options = {
      socketPath,
      path: endpoint,
      method,
      headers: {
        'X-Bridge-Token': token,
        'Content-Type': 'application/json',
        ...(payload != null ? { 'Content-Length': Buffer.byteLength(payload) } : {}),
      },
    };

    const req = http.request(options, (res) => {
      const chunks = [];
      res.on('data', (chunk) => chunks.push(chunk));
      res.on('end', () => {
        const raw = Buffer.concat(chunks).toString();
        let parsed;
        try { parsed = raw.length ? JSON.parse(raw) : null; } catch { parsed = raw; }
        resolve({ status: res.statusCode, body: parsed });
      });
    });

    req.on('error', reject);
    if (payload != null) req.write(payload);
    req.end();
  });
}

/**
 * UDS transport implementation.
 *
 * Emits:
 *   'put'       { requestId, pluginId, path, value, source }
 *   'lifecycle' { event, pluginId }
 *   'proxy'     { pluginId, method, path, body, respond }
 */
class UdsTransport extends EventEmitter {
  /**
   * @param {object} config
   * @param {string} config.rsSocket     - signalk-rs socket path
   * @param {string} config.bridgeSocket - bridge socket path
   * @param {string} config.bridgeToken  - shared secret
   */
  constructor(config) {
    super();
    this.rsSocket = config.rsSocket || '/run/signalk/rs.sock';
    this.bridgeSocket = config.bridgeSocket || '/run/signalk/bridge.sock';
    this.token = config.bridgeToken;
    this._server = null;
  }

  /** Send a delta to signalk-rs (plugin's handleMessage). */
  async sendDelta(delta) {
    const { status } = await udsRequest(
      this.rsSocket, 'POST', '/internal/v1/delta', delta, this.token
    );
    if (status !== 204) throw new Error(`sendDelta failed: HTTP ${status}`);
  }

  /** Query a path value from signalk-rs (plugin's getSelfPath). */
  async queryPath(skPath) {
    const endpoint = '/internal/v1/api/vessels/self/' + skPath.replace(/\./g, '/');
    const { status, body } = await udsRequest(
      this.rsSocket, 'GET', endpoint, null, this.token
    );
    if (status === 404) return null;
    if (status !== 200) throw new Error(`queryPath failed: HTTP ${status}`);
    return body;
  }

  /** Write a path value to signalk-rs (plugin's putSelfPath). */
  async writePath(skPath, value) {
    const endpoint = '/internal/v1/api/vessels/self/' + skPath.replace(/\./g, '/');
    const { status } = await udsRequest(
      this.rsSocket, 'PUT', endpoint, { value }, this.token
    );
    if (status !== 204) throw new Error(`writePath failed: HTTP ${status}`);
  }

  /** Register a PUT handler with signalk-rs (plugin's registerPutHandler). */
  async registerHandler(pluginId, path) {
    const { status } = await udsRequest(
      this.rsSocket, 'POST', '/internal/v1/handlers',
      { pluginId, path }, this.token
    );
    if (status !== 204) throw new Error(`registerHandler failed: HTTP ${status}`);
  }

  /** Register plugin REST routes with signalk-rs (plugin's registerWithRouter). */
  async registerPluginRoutes(pluginId, pathPrefix) {
    const { status } = await udsRequest(
      this.rsSocket, 'POST', '/internal/v1/plugin-routes',
      { pluginId, pathPrefix }, this.token
    );
    if (status !== 204) throw new Error(`registerPluginRoutes failed: HTTP ${status}`);
  }

  /** Query all source values for a path from signalk-rs (plugin's getSelfPathSources). */
  async querySources(skPath) {
    const endpoint = '/internal/v1/api-sources/vessels/self/' + skPath.replace(/\./g, '/');
    const { status, body } = await udsRequest(
      this.rsSocket, 'GET', endpoint, null, this.token
    );
    if (status === 404) return null;
    if (status !== 200) throw new Error(`querySources failed: HTTP ${status}`);
    return body;
  }

  /** Query metadata for a path from signalk-rs (plugin's getMetadata). */
  async queryMetadata(skPath) {
    const endpoint = '/internal/v1/metadata/' + skPath.replace(/\./g, '/');
    const { status, body } = await udsRequest(
      this.rsSocket, 'GET', endpoint, null, this.token
    );
    if (status === 404) return null;
    if (status !== 200) throw new Error(`queryMetadata failed: HTTP ${status}`);
    return body;
  }

  /** Report loaded plugins to signalk-rs. */
  async reportPlugins(plugins) {
    const { status } = await udsRequest(
      this.rsSocket, 'POST', '/internal/v1/bridge/plugins',
      { plugins }, this.token
    );
    if (status !== 204) throw new Error(`reportPlugins failed: HTTP ${status}`);
  }

  /** Register this bridge with signalk-rs. Called once on startup. */
  async register(version) {
    const { status } = await udsRequest(
      this.rsSocket, 'POST', '/internal/v1/bridge/register',
      { bridgeToken: this.token, version }, this.token
    );
    if (status !== 204) throw new Error(`register failed: HTTP ${status}`);
  }

  /**
   * Start the bridge-side HTTP server on the bridge UDS socket.
   * signalk-rs calls back into this server for PUT forwards, lifecycle events,
   * and plugin HTTP proxy requests.
   */
  startServer() {
    // Remove stale socket
    if (fs.existsSync(this.bridgeSocket)) {
      fs.unlinkSync(this.bridgeSocket);
    }

    // Ensure socket directory exists
    const dir = path.dirname(this.bridgeSocket);
    if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });

    // allowHalfOpen: true is required so that async route handlers can send
    // a response after signalk-rs half-closes the request connection.
    // Without it Node.js auto-closes the write side when the client shuts
    // down its write end, destroying `res` before the handler can call res.end().
    this._server = http.createServer({ allowHalfOpen: true }, (req, res) => {
      this._handleCallback(req, res);
    });

    this._server.listen(this.bridgeSocket, () => {
      console.log(`[bridge] Callback server listening on ${this.bridgeSocket}`);
    });
  }

  /** Handle incoming callback requests from signalk-rs. */
  _handleCallback(req, res) {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      const body = Buffer.concat(chunks).toString();
      let parsed;
      try { parsed = body.length ? JSON.parse(body) : {}; } catch { parsed = {}; }

      const url = req.url;
      const method = req.method;

      // PUT /put/{pluginId}/{path}
      const putMatch = url.match(/^\/put\/([^/]+)\/(.+)$/);
      if (putMatch) {
        const [, pluginId, skPath] = putMatch;
        const respond = (state, statusCode = 200) => {
          const responseBody = JSON.stringify({
            requestId: parsed.requestId,
            state,
            statusCode,
          });
          res.writeHead(200, {
            'Content-Type': 'application/json',
            'Content-Length': Buffer.byteLength(responseBody),
          });
          res.end(responseBody);
        };
        this.emit('put', { ...parsed, pluginId, path: skPath.replace(/\//g, '.'), respond });
        return;
      }

      // POST /lifecycle
      if (url === '/lifecycle' && method === 'POST') {
        this.emit('lifecycle', parsed);
        res.writeHead(204);
        res.end();
        return;
      }

      // ANY /plugins/{pluginId}/{path} — proxy to plugin router
      const proxyMatch = url.match(/^\/plugins\/([^/]+)(\/.*)?$/);
      if (proxyMatch) {
        const [, pluginId, pluginPath = '/'] = proxyMatch;
        const respond = (statusCode, body, contentType = 'application/json') => {
          const bodyStr = typeof body === 'string' ? body : JSON.stringify(body);
          // Always set Content-Length so Node.js does not use chunked transfer
          // encoding. signalk-rs's uds_proxy reads raw bytes and cannot decode
          // chunked responses.
          res.writeHead(statusCode, {
            'Content-Type': contentType,
            'Content-Length': Buffer.byteLength(bodyStr),
          });
          res.end(bodyStr);
        };
        this.emit('proxy', { pluginId, method, path: pluginPath, body: parsed, respond });
        return;
      }

      res.writeHead(404, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ message: `Unknown callback path: ${url}` }));
    });
  }

  async stop() {
    if (this._server) {
      await new Promise((resolve) => this._server.close(resolve));
    }
  }
}

module.exports = { UdsTransport, udsRequest };
