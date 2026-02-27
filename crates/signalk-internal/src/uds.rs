/// Unix Domain Socket transport implementation.
///
/// signalk-rs serves HTTP on `/run/signalk/rs.sock`.
/// The bridge serves HTTP on `/run/signalk/bridge.sock`.
///
/// Both use HTTP/1.1 over Unix sockets — familiar protocol,
/// zero TCP overhead, no port conflicts.
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tracing::info;

/// Create a UnixListener at the given socket path, removing any stale socket first.
pub fn bind_unix_socket(path: &Path) -> anyhow::Result<UnixListener> {
    // Remove stale socket from previous run
    if path.exists() {
        std::fs::remove_file(path)?;
    }

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(path)?;
    info!(socket = %path.display(), "Bound Unix socket");
    Ok(listener)
}

/// Build the URL for a Unix socket HTTP request.
/// hyper-util uses "unix:/path/to.sock/endpoint" format.
pub fn unix_socket_url(socket_path: &Path, endpoint: &str) -> String {
    format!("unix:{}:{}", socket_path.display(), endpoint)
}

/// POST a JSON body to an HTTP server over a Unix Domain Socket.
///
/// Sends a minimal HTTP/1.1 request and returns the parsed JSON response body.
/// Fails with an error if the server returns a non-2xx status or the connection
/// times out (10 s).
pub async fn uds_post(
    socket_path: &Path,
    url_path: &str,
    body: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let body_bytes = serde_json::to_vec(body)?;
    let request = format!(
        "POST {url_path} HTTP/1.1\r\nHost: bridge\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    );

    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::UnixStream::connect(socket_path),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Connection timeout to {}", socket_path.display()))??;

    stream.write_all(request.as_bytes()).await?;
    stream.write_all(&body_bytes).await?;
    stream.shutdown().await?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;
    let text = String::from_utf8_lossy(&raw);

    let status: u16 = text
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if !(200..300).contains(&status) {
        return Err(anyhow::anyhow!("HTTP {status} from bridge"));
    }

    let body_start = text
        .find("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("No HTTP body delimiter in response"))?
        + 4;

    Ok(serde_json::from_str(&text[body_start..])?)
}

/// Proxy an HTTP request to an HTTP server over a Unix Domain Socket.
///
/// Forwards `method`, `url_path`, optional `content_type`, and raw `body` bytes.
/// Returns `(status_code, response_body_bytes, response_content_type)`.
/// Does not modify the response body — pass-through for any content type.
pub async fn uds_proxy(
    socket_path: &Path,
    method: &str,
    url_path: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> anyhow::Result<(u16, Vec<u8>, Option<String>)> {
    let ct_header = match content_type {
        Some(ct) => format!("Content-Type: {ct}\r\nContent-Length: {}\r\n", body.len()),
        None if !body.is_empty() => format!("Content-Length: {}\r\n", body.len()),
        None => String::new(),
    };

    let request = format!(
        "{method} {url_path} HTTP/1.1\r\nHost: bridge\r\n{ct_header}Connection: close\r\n\r\n"
    );

    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::UnixStream::connect(socket_path),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Connection timeout to {}", socket_path.display()))??;

    stream.write_all(request.as_bytes()).await?;
    stream.write_all(body).await?;
    // No shutdown() here: HTTP/1.1 uses Content-Length for request framing.
    // The bridge knows the request is complete without a FIN.
    // After res.end(), Node.js closes the socket (Connection: close), which
    // causes read_to_end() to return. Calling shutdown() prematurely closes
    // the write side, which can cause Node.js to destroy the socket before
    // async handlers have a chance to send their response.

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;

    // Split headers and body at the first \r\n\r\n
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("No header/body separator in HTTP response"))?;

    let header_bytes = &raw[..header_end];
    let body_bytes = raw[header_end + 4..].to_vec();

    let header_str = std::str::from_utf8(header_bytes)
        .map_err(|_| anyhow::anyhow!("Non-UTF8 response headers"))?;
    let mut lines = header_str.lines();

    let status: u16 = lines
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);

    let resp_content_type = lines
        .find(|l| l.to_lowercase().starts_with("content-type:"))
        .and_then(|l| l.split_once(':').map(|x| x.1))
        .map(|s| s.trim().to_string());

    Ok((status, body_bytes, resp_content_type))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn can_bind_and_connect_unix_socket() {
        let path = PathBuf::from("/tmp/signalk-rs-test.sock");

        // Clean up from previous test run
        let _ = std::fs::remove_file(&path);

        let listener = bind_unix_socket(&path).unwrap();

        // Spawn a simple echo server
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 128];
            let n = stream.read(&mut buf).await.unwrap();
            stream.write_all(&buf[..n]).await.unwrap();
        });

        // Connect and send
        let mut client = UnixStream::connect(&path).await.unwrap();
        client.write_all(b"hello").await.unwrap();
        let mut resp = [0u8; 5];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(&resp, b"hello");

        server.await.unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn uds_post_sends_and_receives() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let path = PathBuf::from("/tmp/signalk-rs-uds-post-test.sock");
        let _ = std::fs::remove_file(&path);

        let listener = bind_unix_socket(&path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(req.contains("POST /put/test-plugin/steering/heading"));
            assert!(req.contains("\"value\""));
            let body = "{\"state\":\"COMPLETED\",\"statusCode\":200,\"requestId\":\"x\"}";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).await.unwrap();
        });

        let result = uds_post(
            &path,
            "/put/test-plugin/steering/heading",
            &serde_json::json!({"requestId": "x", "value": 1.57}),
        )
        .await
        .unwrap();

        assert_eq!(result["state"], "COMPLETED");
        assert_eq!(result["statusCode"], 200);

        server.await.unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn uds_post_unreachable_socket() {
        let result = uds_post(
            &PathBuf::from("/tmp/signalk-rs-nonexistent.sock"),
            "/put/plugin/path",
            &serde_json::json!({"requestId": "x", "value": 1}),
        )
        .await;
        assert!(result.is_err());
    }
}
