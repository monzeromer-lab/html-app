//! `net` — raw sockets and an embedded server (PRD §9.3 Tier 4).
//!
//! §9.3: "so an HTML App tool can be something other clients talk to." That is the interesting half
//! — a `.hta` that serves a dashboard on localhost, or accepts a webhook, is a different kind of
//! thing from a page that only makes outbound calls.
//!
//! Both directions are governed by `sockets` in the manifest: `connect` for outbound, `listen` for
//! inbound. Neither defaults to anything, because a page that can bind any port can also bind one
//! something else is expecting to.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_stream::stream;
use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::context::Ctx;
use crate::params::decode;

/// Cap on a single inbound HTTP request, so a peer cannot exhaust memory before the page sees it.
const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
/// Cap on one WebSocket frame's payload.
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Anything the page holds a numeric handle to.
enum Endpoint {
    /// A connected stream socket, split so reads can be streamed while writes still work.
    Stream {
        writer: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        reader: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>,
    },
    /// A bound UDP socket.
    Datagram {
        socket: Arc<tokio::net::UdpSocket>,
        peer: Option<String>,
    },
    /// A listener, with the stream of accepted events waiting to be taken.
    Server {
        events: Option<tokio::sync::mpsc::UnboundedReceiver<Value>>,
        shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    },
}

pub struct NetModule {
    ctx: Ctx,
    endpoints: Arc<Mutex<HashMap<u64, Endpoint>>>,
    /// Inbound HTTP requests awaiting a `respond` call.
    pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<HttpResponse>>>>,
    next: AtomicU64,
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(Deserialize)]
struct ConnectParams {
    kind: String,
    address: String,
}

#[derive(Deserialize)]
struct HandleParams {
    handle: u64,
}

#[derive(Deserialize)]
struct SendParams {
    handle: u64,
    data: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondParams {
    request_id: u64,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

impl NetModule {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            endpoints: Arc::new(Mutex::new(HashMap::new())),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next: AtomicU64::new(1),
        }
    }

    /// Check an address against one side of the `sockets` grant.
    fn check(&self, direction: &str, address: &str) -> Result<(), RpcError> {
        let Some(sockets) = self.ctx.permissions().sockets.as_ref() else {
            return Err(RpcError::denied("this document was not granted `net` sockets"));
        };
        let allowed = match direction {
            "connect" => &sockets.connect,
            _ => &sockets.listen,
        };
        if allowed.is_empty() {
            return Err(RpcError::denied(format!(
                "this document was not granted `sockets.{direction}`"
            )));
        }
        let permitted = allowed.iter().any(|pattern| {
            pattern == address
                || globset::Glob::new(pattern)
                    .map(|glob| glob.compile_matcher().is_match(address))
                    .unwrap_or(false)
        });
        if permitted {
            Ok(())
        } else {
            Err(RpcError::denied(format!(
                "`{address}` is not in this document's `sockets.{direction}` list ({})",
                allowed.join(", ")
            )))
        }
    }

    fn allocate(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }

    /// Wire a connected stream into a handle: a writer task and a reader channel.
    fn register_stream<S>(&self, socket: S) -> u64
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        let (mut read_half, mut write_half) = tokio::io::split(socket);
        let (write_tx, mut write_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (read_tx, read_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

        tokio::spawn(async move {
            while let Some(chunk) = write_rx.recv().await {
                if write_half.write_all(&chunk).await.is_err() {
                    break;
                }
                let _ = write_half.flush().await;
            }
        });

        tokio::spawn(async move {
            let mut buffer = vec![0u8; 16 * 1024];
            loop {
                match read_half.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if read_tx.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let handle = self.allocate();
        self.endpoints.lock().insert(
            handle,
            Endpoint::Stream {
                writer: write_tx,
                reader: Some(read_rx),
            },
        );
        handle
    }
}

// --- HTTP ---

/// Parse a request head. Returns `None` while the head is still incomplete.
fn parse_head(buffer: &[u8]) -> Option<(String, String, Vec<(String, String)>, usize)> {
    let end = buffer.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = std::str::from_utf8(&buffer[..end - 4]).ok()?;

    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();

    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();

    Some((method, path, headers, end))
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

/// The RFC 6455 handshake: SHA-1 of the client key plus the fixed GUID, base64-encoded.
fn websocket_accept(key: &str) -> String {
    use base64::Engine as _;
    use sha1::{Digest, Sha1};

    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

/// Decode one WebSocket frame. Returns the payload and how many bytes were consumed.
fn decode_frame(buffer: &[u8]) -> Option<(Option<Vec<u8>>, usize)> {
    if buffer.len() < 2 {
        return None;
    }
    let opcode = buffer[0] & 0x0f;
    let masked = buffer[1] & 0x80 != 0;
    let short_len = (buffer[1] & 0x7f) as usize;

    let (payload_len, mut offset) = match short_len {
        126 => {
            if buffer.len() < 4 {
                return None;
            }
            (u16::from_be_bytes([buffer[2], buffer[3]]) as usize, 4)
        }
        127 => {
            if buffer.len() < 10 {
                return None;
            }
            let bytes: [u8; 8] = buffer[2..10].try_into().ok()?;
            (u64::from_be_bytes(bytes) as usize, 10)
        }
        n => (n, 2),
    };

    if payload_len > MAX_FRAME_BYTES {
        // Reported as a close rather than parsed; the caller drops the connection.
        return Some((None, buffer.len()));
    }

    let mask = if masked {
        if buffer.len() < offset + 4 {
            return None;
        }
        let mask: [u8; 4] = buffer[offset..offset + 4].try_into().ok()?;
        offset += 4;
        Some(mask)
    } else {
        None
    };

    if buffer.len() < offset + payload_len {
        return None;
    }

    let mut payload = buffer[offset..offset + payload_len].to_vec();
    if let Some(mask) = mask {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }

    // 0x8 is close; anything else that is not text or binary is a control frame to ignore.
    let consumed = offset + payload_len;
    match opcode {
        0x8 => Some((None, consumed)),
        0x1 | 0x2 | 0x0 => Some((Some(payload), consumed)),
        _ => Some((Some(Vec::new()), consumed)),
    }
}

/// Encode a text frame. Server-to-client frames are never masked.
fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x81];
    match payload.len() {
        n if n < 126 => frame.push(n as u8),
        n if n <= u16::MAX as usize => {
            frame.push(126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            frame.push(127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    frame
}

impl NetModule {
    /// Serve one accepted connection, emitting events for the page.
    async fn serve_connection(
        mut socket: tokio::net::TcpStream,
        peer: String,
        kind: String,
        handle: u64,
        events: tokio::sync::mpsc::UnboundedSender<Value>,
        pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<HttpResponse>>>>,
        endpoints: Arc<Mutex<HashMap<u64, Endpoint>>>,
        next: Arc<AtomicU64>,
    ) {
        let _ = events.send(json!({ "kind": "connection", "handle": handle, "peer": peer }));

        if kind == "tcp" {
            // Raw TCP: hand the bytes through untouched.
            let mut buffer = vec![0u8; 16 * 1024];
            loop {
                match socket.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = String::from_utf8_lossy(&buffer[..n]).into_owned();
                        if events
                            .send(json!({ "kind": "message", "handle": handle, "data": data }))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            let _ = events.send(json!({ "kind": "close", "handle": handle }));
            return;
        }

        let mut buffer = Vec::new();
        loop {
            let mut chunk = vec![0u8; 16 * 1024];
            let read = match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.len() > MAX_REQUEST_BYTES {
                let _ = socket
                    .write_all(b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\n\r\n")
                    .await;
                break;
            }

            let Some((method, path, headers, head_len)) = parse_head(&buffer) else {
                continue;
            };

            // A WebSocket upgrade turns this connection into a frame stream for the rest of its life.
            let upgrading = header(&headers, "upgrade")
                .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
            if kind == "ws" && upgrading {
                let Some(key) = header(&headers, "sec-websocket-key") else {
                    break;
                };
                let response = format!(
                    "HTTP/1.1 101 Switching Protocols\r\n\
                     Upgrade: websocket\r\nConnection: Upgrade\r\n\
                     Sec-WebSocket-Accept: {}\r\n\r\n",
                    websocket_accept(key)
                );
                if socket.write_all(response.as_bytes()).await.is_err() {
                    break;
                }

                Self::pump_websocket(socket, handle, buffer[head_len..].to_vec(), events, endpoints)
                    .await;
                return;
            }

            let body_len = header(&headers, "content-length")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            while buffer.len() < head_len + body_len {
                let mut more = vec![0u8; 16 * 1024];
                match socket.read(&mut more).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buffer.extend_from_slice(&more[..n]),
                }
            }

            let body = String::from_utf8_lossy(
                &buffer[head_len..(head_len + body_len).min(buffer.len())],
            )
            .into_owned();

            let request_id = next.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = tokio::sync::oneshot::channel();
            pending.lock().insert(request_id, tx);

            let header_map: HashMap<String, String> = headers.into_iter().collect();
            let _ = events.send(json!({
                "kind": "request",
                "requestId": request_id,
                "method": method,
                "path": path,
                "headers": header_map,
                "body": body,
            }));

            // A page that never answers must not hold the connection open indefinitely.
            let response = match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
                Ok(Ok(response)) => response,
                _ => {
                    pending.lock().remove(&request_id);
                    HttpResponse {
                        status: 503,
                        headers: Vec::new(),
                        body: b"the document did not respond".to_vec(),
                    }
                }
            };

            let mut out = format!(
                "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                response.status,
                status_text(response.status),
                response.body.len()
            );
            for (name, value) in &response.headers {
                out.push_str(&format!("{name}: {value}\r\n"));
            }
            out.push_str("\r\n");

            let _ = socket.write_all(out.as_bytes()).await;
            let _ = socket.write_all(&response.body).await;
            let _ = socket.flush().await;
            break;
        }

        let _ = events.send(json!({ "kind": "close", "handle": handle }));
    }

    /// Drive an upgraded WebSocket connection.
    async fn pump_websocket(
        socket: tokio::net::TcpStream,
        handle: u64,
        mut buffer: Vec<u8>,
        events: tokio::sync::mpsc::UnboundedSender<Value>,
        endpoints: Arc<Mutex<HashMap<u64, Endpoint>>>,
    ) {
        let (mut read_half, mut write_half) = tokio::io::split(socket);
        let (write_tx, mut write_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

        // Registered so `net.send` can write to this connection by handle.
        endpoints.lock().insert(
            handle,
            Endpoint::Stream {
                writer: write_tx,
                reader: None,
            },
        );

        tokio::spawn(async move {
            while let Some(payload) = write_rx.recv().await {
                if write_half.write_all(&encode_frame(&payload)).await.is_err() {
                    break;
                }
                let _ = write_half.flush().await;
            }
        });

        loop {
            while let Some((payload, consumed)) = decode_frame(&buffer) {
                buffer.drain(..consumed);
                match payload {
                    Some(bytes) if !bytes.is_empty() => {
                        let data = String::from_utf8_lossy(&bytes).into_owned();
                        let _ = events
                            .send(json!({ "kind": "message", "handle": handle, "data": data }));
                    }
                    Some(_) => {}
                    // A close frame.
                    None => {
                        endpoints.lock().remove(&handle);
                        let _ = events.send(json!({ "kind": "close", "handle": handle }));
                        return;
                    }
                }
            }

            let mut chunk = vec![0u8; 16 * 1024];
            match read_half.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            }
        }

        endpoints.lock().remove(&handle);
        let _ = events.send(json!({ "kind": "close", "handle": handle }));
    }
}

impl ApiHandler for NetModule {
    fn name(&self) -> &'static str {
        "net"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "connect" => {
                    let params: ConnectParams = decode("net.connect", params)?;
                    self.check("connect", &params.address)?;

                    let handle = match params.kind.as_str() {
                        "tcp" => {
                            let socket = tokio::net::TcpStream::connect(&params.address)
                                .await
                                .map_err(operation_failed)?;
                            self.register_stream(socket)
                        }
                        "unix" => {
                            let socket = tokio::net::UnixStream::connect(&params.address)
                                .await
                                .map_err(operation_failed)?;
                            self.register_stream(socket)
                        }
                        "udp" => {
                            let socket = tokio::net::UdpSocket::bind("0.0.0.0:0")
                                .await
                                .map_err(operation_failed)?;
                            socket
                                .connect(&params.address)
                                .await
                                .map_err(operation_failed)?;
                            let handle = self.allocate();
                            self.endpoints.lock().insert(
                                handle,
                                Endpoint::Datagram {
                                    socket: Arc::new(socket),
                                    peer: Some(params.address),
                                },
                            );
                            handle
                        }
                        other => {
                            return Err(RpcError::invalid_params(format!(
                                "`{other}` is not a socket kind (expected tcp, udp, or unix)"
                            )));
                        }
                    };
                    Ok(json!(handle))
                }

                "send" => {
                    let params: SendParams = decode("net.send", params)?;
                    let bytes = params.data.into_bytes();

                    // The lock is resolved to a plain value in its own scope. Holding a
                    // `MutexGuard` across the `await` below would make this future `!Send`, which
                    // the dispatcher cannot box.
                    enum Target {
                        Queued,
                        Datagram(Arc<tokio::net::UdpSocket>),
                    }

                    let target = {
                        let endpoints = self.endpoints.lock();
                        match endpoints.get(&params.handle) {
                            Some(Endpoint::Stream { writer, .. }) => {
                                writer.send(bytes.clone()).map_err(|_| {
                                    RpcError::new(
                                        htmlapp_bridge::ErrorCode::OperationFailed,
                                        "that socket is closed",
                                    )
                                })?;
                                Target::Queued
                            }
                            Some(Endpoint::Datagram { socket, peer }) => {
                                if peer.is_none() {
                                    return Err(RpcError::invalid_params(
                                        "that datagram socket has no peer",
                                    ));
                                }
                                Target::Datagram(Arc::clone(socket))
                            }
                            _ => return Err(RpcError::invalid_params("no such socket handle")),
                        }
                    };

                    if let Target::Datagram(socket) = target {
                        socket.send(&bytes).await.map_err(operation_failed)?;
                    }
                    Ok(Value::Null)
                }

                "close" => {
                    let params: HandleParams = decode("net.close", params)?;
                    if let Some(Endpoint::Server { shutdown, .. }) =
                        self.endpoints.lock().remove(&params.handle)
                        && let Some(shutdown) = shutdown
                    {
                        let _ = shutdown.send(());
                    }
                    Ok(Value::Null)
                }

                "serve" => {
                    let params: ConnectParams = decode("net.serve", params)?;
                    self.check("listen", &params.address)?;

                    if !matches!(params.kind.as_str(), "http" | "ws" | "tcp") {
                        return Err(RpcError::invalid_params(format!(
                            "`{}` is not a server kind (expected http, ws, or tcp)",
                            params.kind
                        )));
                    }

                    let listener = tokio::net::TcpListener::bind(&params.address)
                        .await
                        .map_err(operation_failed)?;

                    let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
                    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

                    let handle = self.allocate();
                    let endpoints = Arc::clone(&self.endpoints);
                    let pending = Arc::clone(&self.pending);
                    let counter = Arc::new(AtomicU64::new(handle * 1_000_000 + 1));
                    let kind = params.kind.clone();

                    tokio::spawn(async move {
                        loop {
                            tokio::select! {
                                _ = &mut shutdown_rx => break,
                                accepted = listener.accept() => {
                                    let Ok((socket, peer)) = accepted else { break };
                                    let connection = counter.fetch_add(1, Ordering::SeqCst);
                                    tokio::spawn(NetModule::serve_connection(
                                        socket,
                                        peer.to_string(),
                                        kind.clone(),
                                        connection,
                                        events_tx.clone(),
                                        Arc::clone(&pending),
                                        Arc::clone(&endpoints),
                                        Arc::clone(&counter),
                                    ));
                                }
                            }
                        }
                    });

                    self.endpoints.lock().insert(
                        handle,
                        Endpoint::Server {
                            events: Some(events_rx),
                            shutdown: Some(shutdown_tx),
                        },
                    );
                    Ok(json!(handle))
                }

                "respond" => {
                    let params: RespondParams = decode("net.respond", params)?;
                    let sender = self
                        .pending
                        .lock()
                        .remove(&params.request_id)
                        .ok_or_else(|| {
                            RpcError::invalid_params(
                                "no such request, or it has already been answered",
                            )
                        })?;

                    let _ = sender.send(HttpResponse {
                        status: params.status.unwrap_or(200),
                        headers: params.headers.into_iter().collect(),
                        body: params.body.unwrap_or_default().into_bytes(),
                    });
                    Ok(Value::Null)
                }

                other => Err(RpcError::not_found(&format!("net.{other}"))),
            }
        })
    }

    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            let params: HandleParams = decode("net", params)?;
            let mut endpoints = self.endpoints.lock();
            let endpoint = endpoints
                .get_mut(&params.handle)
                .ok_or_else(|| RpcError::invalid_params("no such handle"))?;

            match (method, endpoint) {
                ("receive", Endpoint::Stream { reader, .. }) => {
                    let mut reader = reader.take().ok_or_else(|| {
                        RpcError::invalid_params("that socket is already being read")
                    })?;
                    let bytes = stream! {
                        while let Some(chunk) = reader.recv().await {
                            yield Ok(json!(String::from_utf8_lossy(&chunk)));
                        }
                    };
                    Ok(Box::pin(bytes) as ValueStream)
                }

                ("receive", Endpoint::Datagram { socket, .. }) => {
                    let socket = Arc::clone(socket);
                    let datagrams = stream! {
                        let mut buffer = vec![0u8; 64 * 1024];
                        loop {
                            match socket.recv(&mut buffer).await {
                                Ok(n) => yield Ok(json!(String::from_utf8_lossy(&buffer[..n]))),
                                Err(_) => break,
                            }
                        }
                    };
                    Ok(Box::pin(datagrams) as ValueStream)
                }

                ("accept", Endpoint::Server { events, .. }) => {
                    let mut events = events.take().ok_or_else(|| {
                        RpcError::invalid_params("that server is already being accepted from")
                    })?;
                    let accepted = stream! {
                        while let Some(event) = events.recv().await {
                            yield Ok(event);
                        }
                    };
                    Ok(Box::pin(accepted) as ValueStream)
                }

                (other, _) => Err(RpcError::not_found(&format!("net.{other}"))),
            }
        })
    }
}

fn operation_failed(error: std::io::Error) -> RpcError {
    RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, error.to_string())
}
