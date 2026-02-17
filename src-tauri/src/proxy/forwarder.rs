//! Local Proxy Forwarder
//!
//! A local HTTP proxy that forwards requests to an authenticated upstream proxy.
//! This solves Chrome's limitation of not supporting inline proxy credentials.
//!
//! Flow:
//! 1. Chrome connects to localhost:{port} (no auth needed)
//! 2. Local proxy connects to Oxylabs with Proxy-Authorization header
//! 3. Tunnels traffic transparently between Chrome and target

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncBufReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tracing::{info, debug, warn, error};
use base64::Engine;

/// Port range for local proxy forwarders (18080..48080)
const PORT_BASE: u32 = 18080;
const PORT_RANGE: u32 = 30000;

/// Global port counter for allocating unique local ports
static PORT_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Allocate a unique local port for a proxy forwarder.
/// Wraps around within the range 18080..48080 to avoid overflow.
pub fn allocate_port() -> u16 {
    let offset = PORT_COUNTER.fetch_add(1, Ordering::Relaxed) % PORT_RANGE;
    (PORT_BASE + offset) as u16
}

/// Max number of headers to read from a single request/response
const MAX_HEADERS: usize = 100;
/// Max size of a single header line (8KB)
const MAX_HEADER_LINE: usize = 8192;

/// Local proxy forwarder that handles authentication to upstream proxy
pub struct LocalProxyForwarder {
    /// Local port to listen on
    local_port: u16,
    /// Upstream proxy host
    upstream_host: String,
    /// Upstream proxy port
    upstream_port: u16,
    /// Upstream proxy username
    username: String,
    /// Upstream proxy password
    password: String,
    /// Whether the forwarder is running
    running: Arc<AtomicBool>,
    /// Shutdown signal sender
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl LocalProxyForwarder {
    /// Create a new local proxy forwarder
    pub fn new(
        local_port: u16,
        upstream_host: &str,
        upstream_port: u16,
        username: &str,
        password: &str,
    ) -> Self {
        Self {
            local_port,
            upstream_host: upstream_host.to_string(),
            upstream_port,
            username: username.to_string(),
            password: password.to_string(),
            running: Arc::new(AtomicBool::new(false)),
            shutdown_tx: None,
        }
    }

    /// Create a forwarder with auto-allocated port
    pub fn with_auto_port(
        upstream_host: &str,
        upstream_port: u16,
        username: &str,
        password: &str,
    ) -> Self {
        Self::new(allocate_port(), upstream_host, upstream_port, username, password)
    }

    /// Get the local proxy URL for Chrome
    pub fn local_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.local_port)
    }

    /// Get the local port
    pub fn port(&self) -> u16 {
        self.local_port
    }

    /// Check if the forwarder is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Build the Proxy-Authorization header value
    fn auth_header(&self) -> String {
        let credentials = format!("{}:{}", self.username, self.password);
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes());
        info!("Auth for user '{}', pass_len: {}",
               crate::safe_truncate(&self.username, 40),
               self.password.len());
        format!("Basic {}", encoded)
    }

    /// Start the local proxy forwarder
    pub async fn start(&mut self) -> Result<(), std::io::Error> {
        if self.running.load(Ordering::Relaxed) {
            return Ok(());
        }

        let addr = format!("127.0.0.1:{}", self.local_port);
        let listener = TcpListener::bind(&addr).await?;

        info!("Local proxy forwarder started on {}", addr);

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);
        self.running.store(true, Ordering::Relaxed);

        let running = self.running.clone();
        let upstream_host = self.upstream_host.clone();
        let upstream_port = self.upstream_port;
        let auth_header = self.auth_header();

        // Spawn the accept loop
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        info!("Local proxy forwarder shutting down");
                        break;
                    }
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((stream, addr)) => {
                                debug!("Accepted connection from {}", addr);
                                let upstream_host = upstream_host.clone();
                                let auth_header = auth_header.clone();

                                tokio::spawn(async move {
                                    if let Err(e) = handle_connection(
                                        stream,
                                        &upstream_host,
                                        upstream_port,
                                        &auth_header,
                                    ).await {
                                        warn!("Connection error: {}", e);
                                    }
                                });
                            }
                            Err(e) => {
                                error!("Accept error: {}", e);
                            }
                        }
                    }
                }
            }

            running.store(false, Ordering::Relaxed);
        });

        Ok(())
    }

    /// Stop the local proxy forwarder
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.running.store(false, Ordering::Relaxed);
        info!("Local proxy forwarder stopped on port {}", self.local_port);
    }
}

impl Drop for LocalProxyForwarder {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Handle a single client connection
async fn handle_connection(
    client: TcpStream,
    upstream_host: &str,
    upstream_port: u16,
    auth_header: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Wrap client in BufReader for efficient reading
    let mut client = BufReader::new(client);

    // Read the request line
    let mut request_line = String::new();
    let bytes_read = client.read_line(&mut request_line).await?;

    if bytes_read == 0 {
        return Err("Connection closed before request".into());
    }

    debug!("Received request: {}", request_line.trim());

    // Parse request line
    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 2 {
        return Err(format!("Invalid HTTP request line: {}", request_line.trim()).into());
    }

    let method = parts[0];
    let target = parts[1];

    // Read all headers (bounded to prevent memory exhaustion)
    let mut headers = Vec::new();
    for _ in 0..MAX_HEADERS {
        let mut line = String::with_capacity(256);
        let n = client.read_line(&mut line).await?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if line.len() > MAX_HEADER_LINE {
            return Err("Header line too long".into());
        }
        headers.push(line);
    }

    if method == "CONNECT" {
        // HTTPS tunneling via CONNECT method
        handle_connect(client, target, upstream_host, upstream_port, auth_header, &request_line).await
    } else {
        // Regular HTTP request - forward with auth header
        handle_http(client, upstream_host, upstream_port, auth_header, &request_line, headers).await
    }
}

/// Handle CONNECT request (HTTPS tunneling)
async fn handle_connect(
    client: BufReader<TcpStream>,
    target: &str,
    upstream_host: &str,
    upstream_port: u16,
    auth_header: &str,
    request_line: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!("CONNECT tunnel to {} via {}:{}", target, upstream_host, upstream_port);

    let upstream_addr = format!("{}:{}", upstream_host, upstream_port);
    let connect_request = format!(
        "{}\r\nHost: {}\r\nProxy-Authorization: {}\r\nProxy-Connection: keep-alive\r\nUser-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36\r\n\r\n",
        request_line.trim(),
        target,
        auth_header
    );

    info!("CONNECT to upstream: {} -> {}", target, upstream_host);

    // Retry loop for transient upstream errors (e.g. Oxylabs 522 timeouts)
    let max_retries = 2u32;
    let mut upstream: Option<TcpStream> = None;
    let mut last_error_response = String::new();
    let mut last_error_headers: Vec<String> = Vec::new();

    for attempt in 0..=max_retries {
        if attempt > 0 {
            let backoff_ms = if attempt == 1 { 200 } else { 400 };
            warn!("CONNECT retry {}/{} for {} after {}ms backoff", attempt, max_retries, target, backoff_ms);
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }

        // Connect to upstream proxy with timeout
        let mut conn = match tokio::time::timeout(
            Duration::from_secs(10),
            TcpStream::connect(&upstream_addr)
        ).await {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                warn!("CONNECT attempt {} failed to connect: {}", attempt + 1, e);
                continue;
            }
            Err(_) => {
                warn!("CONNECT attempt {} timed out connecting to {}", attempt + 1, upstream_addr);
                continue;
            }
        };

        debug!("Sending to upstream: {}", connect_request.lines().next().unwrap_or(""));
        if conn.write_all(connect_request.as_bytes()).await.is_err() { continue; }
        if conn.flush().await.is_err() { continue; }

        // Read response from upstream proxy
        let mut upstream_reader = BufReader::new(&mut conn);
        let mut response_line = String::new();
        if upstream_reader.read_line(&mut response_line).await.is_err() { continue; }

        debug!("Upstream proxy response: {}", response_line.trim());

        // Read remaining response headers (bounded)
        let mut response_headers = Vec::new();
        let mut header_err = false;
        for _ in 0..MAX_HEADERS {
            let mut line = String::with_capacity(256);
            match upstream_reader.read_line(&mut line).await {
                Ok(n) => {
                    if n == 0 || line == "\r\n" || line == "\n" { break; }
                    if line.len() > MAX_HEADER_LINE { header_err = true; break; }
                    response_headers.push(line);
                }
                Err(_) => { header_err = true; break; }
            }
        }
        if header_err { continue; }

        if response_line.contains("200") {
            // Success — use this connection
            upstream = Some(conn);
            break;
        }

        // Check if this is a retryable error (522 = Oxylabs timeout)
        let is_522 = response_line.contains("522");
        if is_522 && attempt < max_retries {
            warn!("Proxy CONNECT got 522 (attempt {}), will retry: {}", attempt + 1, response_line.trim());
            drop(conn); // Close the failed connection
            continue;
        }

        // Non-retryable error or final attempt — save for forwarding to client
        error!("Proxy CONNECT failed: {}", response_line.trim());
        for h in &response_headers {
            error!("  Response header: {}", h.trim());
        }
        last_error_response = response_line;
        last_error_headers = response_headers;
        break;
    }

    // If we didn't get a successful upstream connection, forward the error to client
    let upstream = match upstream {
        Some(u) => u,
        None => {
            let mut client_stream = client.into_inner();
            if !last_error_response.is_empty() {
                client_stream.write_all(last_error_response.as_bytes()).await?;
                for header in &last_error_headers {
                    client_stream.write_all(header.as_bytes()).await?;
                }
                client_stream.write_all(b"\r\n").await?;
                client_stream.flush().await?;
                return Err(format!("Upstream proxy rejected CONNECT: {}", last_error_response.trim()).into());
            }
            return Err(format!("Failed to establish CONNECT tunnel to {} after {} retries", target, max_retries).into());
        }
    };

    // Send 200 Connection Established to client
    let mut client_stream = client.into_inner();
    client_stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
    client_stream.flush().await?;

    debug!("CONNECT tunnel established for {}", target);

    // Now tunnel data bidirectionally between client and upstream
    let (mut client_read, mut client_write) = client_stream.into_split();
    let (mut upstream_read, mut upstream_write) = upstream.into_split();

    // Copy data in both directions concurrently
    let mut client_to_upstream = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match client_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if upstream_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    if upstream_write.flush().await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut upstream_to_client = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match upstream_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if client_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    if client_write.flush().await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Wait for either direction to complete, then abort the other
    // Add small delay after abort to let Windows flush/close sockets cleanly
    tokio::select! {
        _ = &mut client_to_upstream => {
            upstream_to_client.abort();
            tokio::time::sleep(Duration::from_millis(50)).await;
        },
        _ = &mut upstream_to_client => {
            client_to_upstream.abort();
            tokio::time::sleep(Duration::from_millis(50)).await;
        },
    }

    debug!("CONNECT tunnel closed for {}", target);
    Ok(())
}

/// Handle regular HTTP request (GET, POST, etc.)
async fn handle_http(
    client: BufReader<TcpStream>,
    upstream_host: &str,
    upstream_port: u16,
    auth_header: &str,
    request_line: &str,
    headers: Vec<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!("HTTP request: {}", request_line.trim());

    // Connect to upstream proxy with timeout
    let upstream_addr = format!("{}:{}", upstream_host, upstream_port);
    let mut upstream = tokio::time::timeout(
        Duration::from_secs(10),
        TcpStream::connect(&upstream_addr)
    ).await
        .map_err(|_| format!("Timeout connecting to upstream proxy {}", upstream_addr))?
        .map_err(|e| format!("Failed to connect to upstream proxy {}: {}", upstream_addr, e))?;

    // Build request with Proxy-Authorization header
    let mut request = String::new();
    request.push_str(request_line);
    request.push_str(&format!("Proxy-Authorization: {}\r\n", auth_header));

    for header in &headers {
        request.push_str(header);
    }
    request.push_str("\r\n");

    // Send request to upstream
    upstream.write_all(request.as_bytes()).await?;
    upstream.flush().await?;

    // Get inner streams
    let client_stream = client.into_inner();

    // Tunnel remaining data bidirectionally
    let (mut client_read, mut client_write) = client_stream.into_split();
    let (mut upstream_read, mut upstream_write) = upstream.into_split();

    let mut client_to_upstream = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match client_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if upstream_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut upstream_to_client = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match upstream_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if client_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Add small delay after abort to let Windows flush/close sockets cleanly
    tokio::select! {
        _ = &mut client_to_upstream => {
            upstream_to_client.abort();
            tokio::time::sleep(Duration::from_millis(50)).await;
        },
        _ = &mut upstream_to_client => {
            client_to_upstream.abort();
            tokio::time::sleep(Duration::from_millis(50)).await;
        },
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_allocation() {
        let port1 = allocate_port();
        let port2 = allocate_port();
        assert_ne!(port1, port2);
        assert!(port2 > port1);
    }

    #[test]
    fn test_auth_header() {
        let forwarder = LocalProxyForwarder::new(
            18080,
            "proxy.example.com",
            8080,
            "user",
            "pass",
        );
        let header = forwarder.auth_header();
        assert!(header.starts_with("Basic "));
        // "user:pass" in base64 is "dXNlcjpwYXNz"
        assert!(header.contains("dXNlcjpwYXNz"));
    }

    #[test]
    fn test_local_url() {
        let forwarder = LocalProxyForwarder::new(
            18080,
            "proxy.example.com",
            8080,
            "user",
            "pass",
        );
        assert_eq!(forwarder.local_url(), "http://127.0.0.1:18080");
    }
}
