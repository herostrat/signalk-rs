'use strict';

/**
 * signalk-test-plugin — compatibility spy plugin.
 *
 * Records all interactions with the signalk-rs bridge and exposes them via
 * REST endpoints so an external test runner can assert correct behaviour.
 *
 * Covered API surface:
 *   - app.subscriptionmanager.subscribe   (delta fanout via WebSocket)
 *   - app.registerPutHandler              (PUT command forwarding)
 *   - app.registerWithRouter              (plugin REST proxy)
 *   - app.handleMessage                   (delta emission from plugin)
 *   - app.getSelfPath                     (path query via Internal API)
 *   - app.setPluginStatus                 (lifecycle reporting)
 *
 * REST endpoints (all under /plugins/signalk-test-plugin/):
 *   GET  /recorded      — returns { deltas, puts, getSelfPathResults }
 *   POST /inject        — call app.handleMessage with req.body as delta
 *   POST /getSelfPath   — call app.getSelfPath({ path }) and record result
 *   POST /reset         — clear all recorded state
 */

const PLUGIN_ID = 'signalk-test-plugin';

module.exports = function factory(app) {
  const state = {
    deltas: [],
    puts: [],
    getSelfPathResults: {},
  };

  return {
    id: PLUGIN_ID,
    name: 'SignalK Test Plugin',
    description: 'Spy plugin for automated compatibility testing',
    schema: () => ({ type: 'object', properties: {} }),

    start(_options, _restart) {
      // Subscribe to navigation and steering paths.
      app.subscriptionmanager.subscribe(
        {
          context: 'vessels.self',
          subscribe: [
            { path: 'navigation.*' },
            { path: 'steering.*' },
          ],
        },
        [],            // unsubscribes accumulator
        (delta) => {
          state.deltas.push(delta);
        },
        PLUGIN_ID
      );

      // Register a PUT handler for autopilot heading.
      app.registerPutHandler(
        'vessels.self',
        'steering.autopilot.target.headingTrue',
        (path, value) => {
          state.puts.push({ path, value, ts: Date.now() });
          return Promise.resolve();   // signals COMPLETED
        },
        PLUGIN_ID
      );

      // Register REST endpoints (tested via the plugin-route proxy).
      app.registerWithRouter((router) => {
        // Return all recorded state.
        router.get('/recorded', (_req, res) => {
          res.json(state);
        });

        // Inject a delta via app.handleMessage (tests the full round-trip:
        // plugin → handleMessage → signalk-rs → WS fanout → subscription callback).
        router.post('/inject', (req, res) => {
          app.handleMessage(PLUGIN_ID, req.body);
          res.json({ ok: true });
        });

        // Query a path via app.getSelfPath and record the result.
        router.post('/getSelfPath', (req, res) => {
          const skPath = req.body && req.body.path;
          if (!skPath) {
            res.status(400).json({ error: 'body.path is required' });
            return;
          }
          app.getSelfPath(skPath)
            .then((value) => {
              state.getSelfPathResults[skPath] = value;
              res.json({ path: skPath, value });
            })
            .catch((err) => {
              res.status(500).json({ error: err.message });
            });
        });

        // Reset recorded state between test runs.
        router.post('/reset', (_req, res) => {
          state.deltas = [];
          state.puts = [];
          state.getSelfPathResults = {};
          res.json({ ok: true });
        });
      }, PLUGIN_ID);

      app.setPluginStatus(PLUGIN_ID, 'Running');
    },

    stop() {},
  };
};
