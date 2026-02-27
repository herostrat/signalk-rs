# Plugin Architecture

signalk-rs uses a unified plugin architecture where **everything is a plugin** — input
providers (NMEA 0183, NMEA 2000), alarms, data processors, and external integrations
all share the same lifecycle, configuration, and API.

## 4-Tier Model

| | Tier 1: Rust | Tier 2: JS/Bridge | Tier 3: Standalone | Tier 4: WASM |
|---|---|---|---|---|
| **Process** | In-process | Bridge process | Own process | In-process (sandbox) |
| **Performance** | Native | ~IPC overhead | ~IPC overhead | ~2x WASM |
| **Crash isolation** | Panic-safe | Full | Full | Full |
| **OOM isolation** | No | Yes | Yes | Yes |
| **Language** | Rust | JS/TS | Any | Rust/Go/C/AS |
| **Install** | Compile/feature | npm install | Deploy binary | Copy .wasm |
| **Recompile** | Yes | No | No | No |
| **API** | `PluginContext` trait | `app.*` (JS) | Internal API (UDS) | Future |
| **Status** | Implemented | Implemented | Implemented | Future |

### Tier 1: Rust Plugins (in-process)

Workspace members under `crates/plugins/`. Compiled into the server binary.
Direct store access (zero IPC). Panic isolation via `tokio::spawn`.

Examples: `signalk-plugin-nmea0183`, `signalk-plugin-anchor-alarm`

### Tier 2: JS Plugins (Bridge process)

npm-installed in the Node.js bridge container. ~200 existing SignalK plugins.
Communicates via HTTP over Unix Domain Sockets (Internal API).

### Tier 3: Standalone Binary Plugins

Own OS process (any language). Connects to signalk-rs via Internal API.
Full process isolation. Use `signalk-plugin-client` crate for Rust clients.

### Tier 4: WASM Plugins (future)

`.wasm` files loaded at runtime. Fully sandboxed. Not yet implemented.

## Connecting Thread — One API, Four Transports

```
signalk-plugin-api (Trait definition = Single Source of Truth)
        |
        +-- Tier 1: RustPluginContext  (direct, in-process)
        +-- Tier 2: Bridge app.js     (HTTP over UDS, JS wrapper)
        +-- Tier 3: signalk-plugin-client (HTTP over UDS, Rust client)
        +-- Tier 4: WasmPluginContext  (WASM host calls, future)
```

## Runtime Model

```
+---------------------------------------------------+
|  signalk-server process (single OS process)        |
|                                                    |
|  tokio runtime (multi-threaded)                    |
|  +----------------------------------------------+ |
|  | Task: axum HTTP/WS server                    | |
|  | Task: Internal API server (UDS)              | |
|  | Task: Plugin "nmea0183-tcp" (Tier 1)         | |
|  |   +- Sub-task: TCP listener per connection   | |
|  | Task: Plugin "anchor-alarm" (Tier 1)         | |
|  |   +- Sub-task: Position subscription         | |
|  +----------------------------------------------+ |
+------------------------+--------------------------+
                         | UDS (/run/signalk/rs.sock)
            +------------+---------------+
            v                            v
+--------------------+     +------------------------+
| Node.js Bridge     |     | Standalone Binary       |
| (Tier 2)           |     | (Tier 3)                |
| JS plugins         |     | Any language             |
+--------------------+     +------------------------+
```

## Crash Behavior

| Scenario | Effect | Protection |
|---|---|---|
| Rust plugin panics | Plugin disabled, server + others continue | `tokio::spawn` catches panic |
| Rust plugin infinite loop | Blocks 1 worker thread | `task.abort()` via stop() |
| Rust plugin OOM | Can kill server | No runtime protection (trust model) |
| JS plugin crashes | Bridge process dies, server continues | Process isolation |
| Standalone crashes | Plugin process dies, server continues | Process isolation |

## Crate Structure

```
crates/
  signalk-types/              SignalK protocol types
  signalk-store/              In-memory data store + broadcast
  signalk-plugin-api/         Canonical plugin API definition (traits + types)
  signalk-plugin-client/      Rust client for Tier 3 standalone plugins
  signalk-internal/           UDS transport, Internal API server
  signalk-server/             axum server, PluginManager, RustPluginContext

crates/plugins/
  signalk-nmea0183/           NMEA 0183 TCP + serial as Plugin
  signalk-anchor-alarm/       Example: anchor alarm plugin
```

### Dependency Graph

```
signalk-types
    ^
signalk-plugin-api    (types, serde, async-trait, thiserror)
    ^            ^
    |       [Rust Plugins]        (only plugin-api + types)
    |       [plugin-client]       (plugin-api + types + tokio)
    |
signalk-store         (types)
    ^
signalk-internal      (types, plugin-api)
    ^
signalk-server        (everything + axum, tokio, plugins)
```

**Invariant:** Plugins never see `signalk-store`, `signalk-internal`, or `signalk-server`.

## Configuration

All plugins are configured via `[[plugins]]` in `signalk-rs.toml`:

```toml
[[plugins]]
id = "nmea0183-tcp"
config = { addr = "0.0.0.0:10110", source_label = "gps" }

[[plugins]]
id = "nmea0183-serial"
config = { path = "/dev/ttyUSB0", baud_rate = 4800, source_label = "depth" }

[[plugins]]
id = "anchor-alarm"
config = { position = { latitude = 49.27, longitude = -123.19 }, radius = 75.0 }

[[plugins]]
id = "course-provider"
enabled = false
```
