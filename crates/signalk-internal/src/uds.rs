/// Unix Domain Socket transport implementation.
///
/// signalk-rs serves HTTP on `/run/signalk/rs.sock`.
/// The bridge serves HTTP on `/run/signalk/bridge.sock`.
///
/// Both use HTTP/1.1 over Unix sockets — familiar protocol,
/// zero TCP overhead, no port conflicts.
use std::path::Path;
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
}
