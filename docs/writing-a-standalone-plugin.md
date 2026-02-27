# Writing a Standalone Plugin (Tier 3)

A standalone plugin runs as its own process and communicates with signalk-rs
via the Internal API over Unix Domain Sockets.

## When to Use Tier 3

- CPU-intensive work (route optimization, ML inference, GPU)
- Plugins written in languages other than Rust (Python, Go, C++)
- Plugins that need full process isolation (crash, OOM)
- Plugins with heavy native dependencies

## Rust: Using `signalk-plugin-client`

### Setup

```toml
# Cargo.toml
[dependencies]
signalk-plugin-client = "0.1"
signalk-plugin-api = "0.1"
signalk-types = "0.1"
tokio = { version = "1", features = ["full"] }
```

### Example

```rust
use signalk_plugin_client::RemotePluginContext;
use signalk_plugin_api::PluginContext;
use signalk_types::{Delta, PathValue, Source, Update};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket = std::env::var("SIGNALK_UDS_SOCKET")
        .unwrap_or_else(|_| "/run/signalk/rs.sock".to_string());
    let token = std::env::var("SIGNALK_BRIDGE_TOKEN")
        .expect("SIGNALK_BRIDGE_TOKEN must be set");

    let ctx = RemotePluginContext::connect(&socket, &token, "my-standalone-plugin")
        .await?;

    // Read data
    if let Some(speed) = ctx.get_self_path("navigation.speedOverGround").await? {
        println!("Speed: {speed}");
    }

    // Write data
    let delta = Delta::self_vessel(vec![Update::new(
        Source::plugin("my-standalone-plugin"),
        vec![PathValue::new(
            "environment.outside.temperature",
            serde_json::json!(293.15),  // 20C in Kelvin
        )],
    )]);

    ctx.handle_message(delta).await?;
    println!("Delta injected");

    Ok(())
}
```

## Any Language: Direct HTTP over UDS

The Internal API is plain HTTP served over a Unix Domain Socket. Any language
with UDS + HTTP support can be a plugin.

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/internal/v1/api/vessels/self/{path}` | Read self vessel value |
| POST | `/internal/v1/delta` | Inject a delta |
| POST | `/internal/v1/handlers` | Register a PUT handler |
| POST | `/internal/v1/plugin-routes` | Register REST routes |
| POST | `/internal/v1/bridge/register` | Register on startup |

All requests require `Authorization: Bearer <token>` header.

### Python Example

```python
import socket
import json

SOCKET_PATH = "/run/signalk/rs.sock"
TOKEN = "your-bridge-token"

def request(method, path, body=None):
    """Send HTTP request over UDS."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(SOCKET_PATH)

    body_bytes = json.dumps(body).encode() if body else b""
    headers = f"{method} {path} HTTP/1.1\r\n"
    headers += "Host: localhost\r\n"
    headers += f"Authorization: Bearer {TOKEN}\r\n"
    if body_bytes:
        headers += "Content-Type: application/json\r\n"
        headers += f"Content-Length: {len(body_bytes)}\r\n"
    headers += "Connection: close\r\n\r\n"

    sock.sendall(headers.encode() + body_bytes)

    response = b""
    while True:
        chunk = sock.recv(4096)
        if not chunk:
            break
        response += chunk
    sock.close()

    # Parse status and body
    text = response.decode()
    status = int(text.split(" ")[1])
    body = text.split("\r\n\r\n", 1)[1] if "\r\n\r\n" in text else ""
    return status, json.loads(body) if body else None

# Read speed
status, data = request("GET", "/internal/v1/api/vessels/self/navigation.speedOverGround")
if status == 200:
    print(f"Speed: {data['value']}")

# Inject delta
delta = {
    "updates": [{
        "source": {"type": "plugin", "label": "my-python-plugin"},
        "values": [{
            "path": "environment.outside.temperature",
            "value": 293.15
        }]
    }]
}
request("POST", "/internal/v1/delta", delta)
```

### Go Example

```go
package main

import (
    "encoding/json"
    "fmt"
    "net"
    "net/http"
)

func main() {
    client := &http.Client{
        Transport: &http.Transport{
            DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
                return net.Dial("unix", "/run/signalk/rs.sock")
            },
        },
    }

    req, _ := http.NewRequest("GET",
        "http://localhost/internal/v1/api/vessels/self/navigation.speedOverGround", nil)
    req.Header.Set("Authorization", "Bearer "+os.Getenv("SIGNALK_BRIDGE_TOKEN"))

    resp, err := client.Do(req)
    // ... handle response
}
```

## Docker Deployment

```dockerfile
FROM rust:slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p my-standalone-plugin

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/my-standalone-plugin /usr/local/bin/
CMD ["my-standalone-plugin"]
```

```yaml
# docker-compose.yml
services:
  signalk-rs:
    build: .
    volumes:
      - socket:/run/signalk

  my-plugin:
    build: ./plugins/my-plugin
    environment:
      - SIGNALK_UDS_SOCKET=/run/signalk/rs.sock
      - SIGNALK_BRIDGE_TOKEN=${BRIDGE_TOKEN}
    volumes:
      - socket:/run/signalk

volumes:
  socket:
```

## Limitations

Standalone plugins cannot:
- Subscribe to delta streams (use WebSocket at `ws://host:3000/signalk/v1/stream`)
- Register PUT handlers directly (use `POST /internal/v1/handlers`)
- Register REST routes on the server (serve your own HTTP endpoints)

These are possible via the Internal API but require different patterns than
the in-process `PluginContext` trait.
