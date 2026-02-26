'use strict';

/**
 * Plugin loader — discovers and loads SignalK plugins from node_modules.
 *
 * SignalK plugins identify themselves via package.json keywords:
 *   "keywords": ["signalk-node-server-plugin"]
 *
 * Plugin lifecycle:
 *   1. plugin = require(pluginPath)(app)     — instantiate with app object
 *   2. plugin.schema()                       — get config schema (optional)
 *   3. plugin.start(settings, restartFn)     — start plugin
 *   4. plugin.stop()                         — stop plugin (on shutdown or config change)
 */

const fs = require('fs');
const path = require('path');

const PLUGIN_KEYWORD = 'signalk-node-server-plugin';

/**
 * Discover all SignalK plugins in a node_modules directory.
 *
 * @param {string} modulesDir - path to node_modules
 * @returns {Array<{id: string, packagePath: string, pkg: object}>}
 */
function discoverPlugins(modulesDir) {
  const plugins = [];

  if (!fs.existsSync(modulesDir)) return plugins;

  for (const entry of fs.readdirSync(modulesDir)) {
    const pkgPath = path.join(modulesDir, entry, 'package.json');
    if (!fs.existsSync(pkgPath)) continue;

    let pkg;
    try {
      pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
    } catch {
      continue;
    }

    if (pkg.keywords && pkg.keywords.includes(PLUGIN_KEYWORD)) {
      plugins.push({
        id: pkg.name,
        packagePath: path.join(modulesDir, entry),
        pkg,
      });
    }
  }

  return plugins;
}

/**
 * Load and manage plugin lifecycle.
 */
class PluginLoader {
  constructor(app, modulesDir) {
    this._app = app;
    this._modulesDir = modulesDir;
    this._loaded = new Map();  // id → { plugin, info }
  }

  /** Load all discovered plugins. */
  async loadAll() {
    const discovered = discoverPlugins(this._modulesDir);
    console.log(`[bridge] Discovered ${discovered.length} plugin(s)`);

    for (const { id, packagePath, pkg } of discovered) {
      await this.load(id, packagePath, pkg);
    }
  }

  /** Load a single plugin by path. */
  async load(id, packagePath, pkg) {
    try {
      const mainFile = path.join(packagePath, pkg.main || 'index.js');
      const pluginFactory = require(mainFile);

      // Plugins export either a function (factory) or an object
      const plugin = typeof pluginFactory === 'function'
        ? pluginFactory(this._app)
        : pluginFactory;

      this._loaded.set(id, { plugin, pkg });
      console.log(`[bridge] Loaded plugin: ${id}@${pkg.version || 'unknown'}`);

      // Get config schema if available
      const schema = plugin.schema ? plugin.schema() : {};
      const savedOptions = this._app.readPluginOptions(id);

      // Start plugin
      await this._startPlugin(id, plugin, savedOptions);
    } catch (err) {
      console.error(`[bridge] Failed to load plugin ${id}:`, err);
      this._app.setPluginError(id, err.message);
    }
  }

  async _startPlugin(id, plugin, settings) {
    try {
      const restartFn = () => this._restartPlugin(id);
      await Promise.resolve(plugin.start(settings, restartFn));
      this._app.setPluginStatus(id, 'Started');
      console.log(`[bridge] Started plugin: ${id}`);
    } catch (err) {
      console.error(`[bridge] Failed to start plugin ${id}:`, err);
      this._app.setPluginError(id, `Start failed: ${err.message}`);
    }
  }

  async _restartPlugin(id) {
    const entry = this._loaded.get(id);
    if (!entry) return;
    await this._stopPlugin(id, entry.plugin);
    await this._startPlugin(id, entry.plugin, this._app.readPluginOptions(id));
  }

  async stopAll() {
    for (const [id, { plugin }] of this._loaded) {
      await this._stopPlugin(id, plugin);
    }
  }

  async _stopPlugin(id, plugin) {
    try {
      if (plugin.stop) await Promise.resolve(plugin.stop());
      console.log(`[bridge] Stopped plugin: ${id}`);
    } catch (err) {
      console.error(`[bridge] Error stopping plugin ${id}:`, err);
    }
  }

  /** Get a list of all loaded plugins with their status. */
  list() {
    return Array.from(this._loaded.entries()).map(([id, { pkg }]) => ({
      id,
      version: pkg.version,
      description: pkg.description,
    }));
  }
}

module.exports = { PluginLoader, discoverPlugins };
