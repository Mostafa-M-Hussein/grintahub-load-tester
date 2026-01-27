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
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncBufReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tracing::{info, debug, warn, error};
use base64::Engine;

/// Global port counter for allocating unique local ports
static PORT_COUNTER: AtomicU16 = AtomicU16::new(18080);

/// Allocate a unique local port for a proxy forwarder
pub fn allocate_port() -> u16 {
    PORT_COUNTER.fetch_add(1, Ordering::Relaxed)
}

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
        // Log auth info (mask most of the password)
        let pass_preview = if self.password.len() > 4 {
            format!("{}...{}", &self.password[..2], &self.password[self.password.len()-2..])
        } else {
            "****".to_string()
        };
        info!("Auth for user '{}', pass: '{}' (len={})",
               crate::safe_truncate(&self.username, 40),
               pass_preview,
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

    // Read all headers
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        let n = client.read_line(&mut line).await?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
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
    mut client: BufReader<TcpStream>,
    target: &str,
    upstream_host: &str,
    upstream_port: u16,
    auth_header: &str,
    request_line: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!("CONNECT tunnel to {} via {}:{}", target, upstream_host, upstream_port);

    // Connect to upstream proxy
    let upstream_addr = format!("{}:{}", upstream_host, upstream_port);
    let mut upstream = TcpStream::connect(&upstream_addr).await
        .map_err(|e| format!("Failed to connect to upstream proxy {}: {}", upstream_addr, e))?;

    // Send CONNECT request to upstream with authentication
    // Include all headers that Oxylabs might expect
    let connect_request = format!(
        "{}\r\nHost: {}\r\nProxy-Authorization: {}\r\nProxy-Connection: keep-alive\r\nUser-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36\r\n\r\n",
        request_line.trim(),
        target,
        auth_header
    );

    info!("CONNECT to upstream: {} -> {}", target, upstream_host);

    debug!("Sending to upstream: {}", connect_request.lines().next().unwrap_or(""));
    upstream.write_all(connect_request.as_bytes()).await?;
    upstream.flush().await?;

    // Read response from upstream proxy
    let mut upstream_reader = BufReader::new(&mut upstream);
    let mut response_line = String::new();
    upstream_reader.read_line(&mut response_line).await?;

    debug!("Upstream proxy response: {}", response_line.trim());

    // Read remaining response headers
    let mut response_headers = Vec::new();
    loop {
        let mut line = String::new();
        let n = upstream_reader.read_line(&mut line).await?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        response_headers.push(line);
    }

    // Check if connection was established (200 response)
    if !response_line.contains("200") {
        // Log the full error response
        error!("Proxy CONNECT failed: {}", response_line.trim());
        for h in &response_headers {
            error!("  Response header: {}", h.trim());
        }

        // Get the inner client stream
        let mut client_stream = client.into_inner();

        // Forward error response to client
        client_stream.write_all(response_line.as_bytes()).await?;
        for header in &response_headers {
            client_stream.write_all(header.as_bytes()).await?;
        }
        client_stream.write_all(b"\r\n").await?;
        client_stream.flush().await?;

        return Err(format!("Upstream proxy rejected CONNECT: {}", response_line.trim()).into());
    }

    // Send 200 Connection Established to client
    let mut client_stream = client.into_inner();
    client_stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
    client_stream.flush().await?;

    debug!("CONNECT tunnel established for {}", target);

    // Now tunnel data bidirectionally between client and upstream
    let (mut client_read, mut client_write) = client_stream.into_split();
    let (mut upstream_read, mut upstream_write) = upstream.into_split();

    // Copy data in both directions concurrently
    let client_to_upstream = tokio::spawn(async move {
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

    let upstream_to_client = tokio::spawn(async move {
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

    // Wait for either direction to complete (connection closed)
    tokio::select! {
        _ = client_to_upstream => {},
        _ = upstream_to_client => {},
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

    // Connect to upstream proxy
    let upstream_addr = format!("{}:{}", upstream_host, upstream_port);
    let mut upstream = TcpStream::connect(&upstream_addr).await
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
    let mut client_stream = client.into_inner();

    // Tunnel remaining data bidirectionally
    let (mut client_read, mut client_write) = client_stream.into_split();
    let (mut upstream_read, mut upstream_write) = upstream.into_split();

    let client_to_upstream = tokio::spawn(async move {
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

    let upstream_to_client = tokio::spawn(async move {
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

    tokio::select! {
        _ = client_to_upstream => {},
        _ = upstream_to_client => {},
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
