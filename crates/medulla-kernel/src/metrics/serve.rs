//! The Prometheus exposition endpoint.

use super::render;

/// Minimal HTTP/1.1 exposition endpoint. Every request to the bound address
/// (whatever the path) receives the full registry — keeps the listener free
/// of an HTTP-framework dependency and completely separate from the
/// length-prefixed MessagePack RPC ports.
pub async fn serve(listener: tokio::net::TcpListener) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(async move {
            // Drain (up to one read of) the request head; the path is
            // irrelevant for a single-purpose exposition endpoint.
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let body = render();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain; version=0.0.4; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}
