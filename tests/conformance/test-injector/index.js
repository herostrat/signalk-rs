/**
 * Minimal SignalK plugin for delta injection.
 *
 * Provides: POST /plugins/signalk-test-injector/inject
 * Body: a SignalK delta JSON object
 * Calls app.handleMessage() to inject the delta into the server.
 *
 * Used by the data conformance test runner (data_compare.js).
 */
module.exports = function (app) {
  const plugin = {
    id: 'signalk-test-injector',
    name: 'Test Injector',
    description: 'Injects deltas via REST for conformance testing',
  };

  plugin.start = function () {
    app.setPluginStatus && app.setPluginStatus('Running');
  };

  plugin.stop = function () {};

  plugin.schema = function () {
    return { type: 'object', properties: {} };
  };

  plugin.registerWithRouter = function (router) {
    router.post('/inject', (req, res) => {
      try {
        app.handleMessage(plugin.id, req.body);
        res.json({ success: true });
      } catch (e) {
        res.status(500).json({ error: e.message });
      }
    });
  };

  return plugin;
};
