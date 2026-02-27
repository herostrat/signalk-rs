'use strict';

/**
 * NMEA 0183 TCP server — forwards plugin-generated NMEA sentences to TCP clients.
 *
 * Listens for `nmea0183out` events on the app (emitted by plugins like
 * signalk-to-nmea0183) and writes them to all connected TCP clients with
 * \r\n line termination.
 *
 * Also listens for `nmea0183` events (raw NMEA input passthrough).
 *
 * Default port: 10110 (standard NMEA TCP port), override with NMEA0183PORT.
 */

const net = require('net');

class NmeaTcpServer {
  /**
   * @param {EventEmitter} app - SignalKApp instance
   * @param {number} [port]    - TCP port (default: NMEA0183PORT env or 10110)
   */
  constructor(app, port) {
    this._app = app;
    this._port = port || parseInt(process.env.NMEA0183PORT, 10) || 10110;
    this._server = null;
    this._sockets = new Map(); // id → socket
    this._nextId = 0;
  }

  start() {
    this._server = net.createServer((socket) => {
      const id = this._nextId++;
      this._sockets.set(id, socket);
      console.log(`[nmea-tcp] Client ${id} connected from ${socket.remoteAddress}`);

      socket.on('close', () => {
        this._sockets.delete(id);
        console.log(`[nmea-tcp] Client ${id} disconnected`);
      });

      socket.on('error', (e) => {
        console.error(`[nmea-tcp] Client ${id} error:`, e.message);
        this._sockets.delete(id);
      });

      // Incoming data from TCP clients → emit for potential inbound processing
      socket.on('data', (data) => {
        this._app.emit('tcpserver0183data', data.toString());
      });
    });

    this._server.listen(this._port, () => {
      console.log(`[nmea-tcp] Listening on port ${this._port}`);
    });

    // Forward plugin-generated NMEA sentences to all TCP clients
    this._sendHandler = (sentence) => this._broadcast(sentence);
    this._app.on('nmea0183out', this._sendHandler);
    this._app.on('nmea0183', this._sendHandler);
  }

  stop() {
    if (this._sendHandler) {
      this._app.removeListener('nmea0183out', this._sendHandler);
      this._app.removeListener('nmea0183', this._sendHandler);
    }
    for (const socket of this._sockets.values()) {
      socket.destroy();
    }
    this._sockets.clear();
    if (this._server) {
      this._server.close();
      this._server = null;
    }
  }

  _broadcast(sentence) {
    const data = sentence + '\r\n';
    for (const [id, socket] of this._sockets) {
      try {
        socket.write(data);
      } catch (e) {
        console.error(`[nmea-tcp] Write error for client ${id}:`, e.message);
        this._sockets.delete(id);
      }
    }
  }
}

module.exports = { NmeaTcpServer };
