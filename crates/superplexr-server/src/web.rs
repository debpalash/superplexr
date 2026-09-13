//! The web shell's door: HTTP for the page, WebSocket for the wire.
//!
//! The gateway's TLS listener serves three things on one port: the wire_v3
//! protocol for native shells, the browser shell's few static files, and a
//! WebSocket that carries wire_v3 frames unchanged. The first bytes tell
//! them apart — the wire's magic, or an HTTP request line — so one port and
//! one pairing flow cover every kind of device.
//!
//! The wire is self-delimiting, so WebSocket message boundaries carry no
//! meaning: the browser concatenates what arrives and reads frames out of
//! it, and the runtime does the same with what the browser sends. That is
//! what lets [`WsRead`] and [`WsWrite`] hand the unchanged connection
//! handler a WebSocket as if it were a socket.
//!
//! This is deliberately not a web framework. The shell is a handful of files
//! and the socket layer is a few hundred lines of RFC 6455, which is smaller
//! and easier to review than the dependency it would replace.

use std::{
    collections::VecDeque,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
    time::Duration,
};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::{AppState, ServerError, Transport, handle_connection_over};

const INDEX_HTML: &str = include_str!("../../../web/index.html");
const STYLE_CSS: &str = include_str!("../../../web/style.css");
const APP_JS: &str = include_str!("../../../web/app.js");
const WIRE_JS: &str = include_str!("../../../web/wire.js");
const FRAMES_JS: &str = include_str!("../../../web/frames.js");
const RENDER_JS: &str = include_str!("../../../web/render.js");
const SW_JS: &str = include_str!("../../../web/sw.js");
const MANIFEST: &str = include_str!("../../../web/manifest.webmanifest");
const ICON_SVG: &str = include_str!("../../../web/icon.svg");

/// A browser gets this long to send its request line and headers.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Request line plus headers; anything longer is not a browser we serve.
const MAX_REQUEST_BYTES: usize = 16 * 1024;
/// One WebSocket message; the wire's own payload cap is smaller.
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Serve one accepted TLS stream, whatever is on the other end.
pub(crate) async fn serve<S>(
    mut stream: S,
    state: Arc<AppState>,
    peer: SocketAddr,
) -> Result<(), ServerError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut head = [0_u8; 4];
    tokio::time::timeout(REQUEST_TIMEOUT, stream.read_exact(&mut head))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no request within 10s"))??;
    if head == superplexr_protocol::wire_v3::MAGIC {
        let (read, write) = tokio::io::split(stream);
        let read = Prefixed {
            head: head.to_vec(),
            taken: 0,
            inner: read,
        };
        return handle_connection_over(Box::new(read), Box::new(write), state, Transport::Gateway, None)
            .await;
    }
    let mut raw = head.to_vec();
    let request = tokio::time::timeout(REQUEST_TIMEOUT, read_http_head(&mut stream, &mut raw))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no request within 10s"))??;
    match route(&request) {
        Route::Asset {
            content_type,
            body,
        } => {
            write_response(&mut stream, "200 OK", content_type, body.as_bytes(), &[]).await?;
            stream.shutdown().await?;
            Ok(())
        }
        Route::NotFound => {
            write_response(&mut stream, "404 Not Found", "text/plain; charset=utf-8", b"not here\n", &[])
                .await?;
            stream.shutdown().await?;
            Ok(())
        }
        Route::MethodNotAllowed => {
            write_response(
                &mut stream,
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                b"only GET\n",
                &[("Allow", "GET")],
            )
            .await?;
            stream.shutdown().await?;
            Ok(())
        }
        Route::Socket => match websocket_accept(&request) {
            Ok(accept) => {
                let headers = [
                    ("Upgrade", "websocket"),
                    ("Connection", "Upgrade"),
                    ("Sec-WebSocket-Accept", accept.as_str()),
                ];
                write_status_only(&mut stream, "101 Switching Protocols", &headers).await?;
                let (read, write) = tokio::io::split(stream);
                let result = handle_connection_over(
                    Box::new(WsRead::new(read)),
                    Box::new(WsWrite::new(write)),
                    state,
                    Transport::Gateway,
                    None,
                )
                .await;
                if let Err(error) = &result {
                    eprintln!("web shell connection from {peer} ended: {error}");
                }
                result
            }
            Err(reason) => {
                write_response(
                    &mut stream,
                    "400 Bad Request",
                    "text/plain; charset=utf-8",
                    format!("{reason}\n").as_bytes(),
                    &[],
                )
                .await?;
                stream.shutdown().await?;
                Ok(())
            }
        },
    }
}

/// Bytes already read while sniffing, then the rest of the stream.
struct Prefixed<R> {
    head: Vec<u8>,
    taken: usize,
    inner: R,
}

impl<R: AsyncRead + Unpin> AsyncRead for Prefixed<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.taken < self.head.len() {
            let remaining = &self.head[self.taken..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            self.taken += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

/// What a browser asked for, reduced to what this door answers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Route {
    Asset {
        content_type: &'static str,
        body: &'static str,
    },
    Socket,
    NotFound,
    MethodNotAllowed,
}

async fn read_http_head<S: AsyncRead + Unpin>(
    stream: &mut S,
    raw: &mut Vec<u8>,
) -> io::Result<HttpRequest> {
    loop {
        if let Some(end) = find_head_end(raw) {
            return parse_http_head(&raw[..end]);
        }
        if raw.len() >= MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head longer than 16 KiB",
            ));
        }
        let mut chunk = [0_u8; 2048];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before the request head",
            ));
        }
        raw.extend_from_slice(&chunk[..n]);
    }
}

fn find_head_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

pub(crate) fn parse_http_head(raw: &[u8]) -> io::Result<HttpRequest> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request head is not UTF-8"))?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed request line",
        ));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "only HTTP/1.x is served here",
        ));
    }
    let path = target.split('?').next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed header line",
            ));
        };
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    Ok(HttpRequest {
        method: method.to_owned(),
        path,
        headers,
    })
}

fn route(request: &HttpRequest) -> Route {
    if request.method != "GET" {
        return Route::MethodNotAllowed;
    }
    match request.path.as_str() {
        "/" | "/index.html" => Route::Asset {
            content_type: "text/html; charset=utf-8",
            body: INDEX_HTML,
        },
        "/style.css" => Route::Asset {
            content_type: "text/css; charset=utf-8",
            body: STYLE_CSS,
        },
        "/app.js" => Route::Asset {
            content_type: "text/javascript; charset=utf-8",
            body: APP_JS,
        },
        "/wire.js" => Route::Asset {
            content_type: "text/javascript; charset=utf-8",
            body: WIRE_JS,
        },
        "/frames.js" => Route::Asset {
            content_type: "text/javascript; charset=utf-8",
            body: FRAMES_JS,
        },
        "/render.js" => Route::Asset {
            content_type: "text/javascript; charset=utf-8",
            body: RENDER_JS,
        },
        "/sw.js" => Route::Asset {
            content_type: "text/javascript; charset=utf-8",
            body: SW_JS,
        },
        "/manifest.webmanifest" => Route::Asset {
            content_type: "application/manifest+json; charset=utf-8",
            body: MANIFEST,
        },
        "/icon.svg" => Route::Asset {
            content_type: "image/svg+xml; charset=utf-8",
            body: ICON_SVG,
        },
        "/ws" => Route::Socket,
        _ => Route::NotFound,
    }
}

/// The upgrade rules, and the one answer they produce. An `Origin` that is
/// not this host is refused: a page from elsewhere must not be able to open
/// a socket to a runtime the browser can reach.
pub(crate) fn websocket_accept(request: &HttpRequest) -> Result<String, &'static str> {
    let upgrade = request.header("Upgrade").unwrap_or_default();
    if !upgrade.eq_ignore_ascii_case("websocket") {
        return Err("expected Upgrade: websocket");
    }
    let connection = request.header("Connection").unwrap_or_default();
    if !connection
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
    {
        return Err("expected Connection: Upgrade");
    }
    if request.header("Sec-WebSocket-Version") != Some("13") {
        return Err("expected Sec-WebSocket-Version: 13");
    }
    if let (Some(origin), Some(host)) = (request.header("Origin"), request.header("Host")) {
        let origin_host = origin
            .split_once("://")
            .map_or(origin, |(_, rest)| rest)
            .trim_end_matches('/');
        if !origin_host.eq_ignore_ascii_case(host) {
            return Err("origin is not this host");
        }
    }
    let key = request
        .header("Sec-WebSocket-Key")
        .ok_or("missing Sec-WebSocket-Key")?;
    Ok(accept_key(key))
}

fn accept_key(key: &str) -> String {
    let digest = ring::digest::digest(
        &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        format!("{key}{WEBSOCKET_GUID}").as_bytes(),
    );
    base64(digest.as_ref())
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[usize::from(b0 >> 2)] as char);
        out.push(TABLE[usize::from((b0 & 0b11) << 4 | b1 >> 4)] as char);
        out.push(if chunk.len() > 1 {
            TABLE[usize::from((b1 & 0b1111) << 2 | b2 >> 6)] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[usize::from(b2 & 0b11_1111)] as char
        } else {
            '='
        });
    }
    out
}

async fn write_response<S: AsyncWrite + Unpin>(
    stream: &mut S,
    status: &str,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, &str)],
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'self'; img-src 'self' data:; \
         connect-src 'self' wss: ws:; frame-ancestors 'none'\r\n\
         Connection: close\r\n",
        body.len()
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

async fn write_status_only<S: AsyncWrite + Unpin>(
    stream: &mut S,
    status: &str,
    headers: &[(&str, &str)],
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await
}

/// One parsed client frame: its opcode and unmasked payload.
#[derive(Debug, PartialEq, Eq)]
struct WsFrame {
    opcode: u8,
    payload: Vec<u8>,
}

/// Parse one client-to-server frame from the front of `raw`. `None` means
/// more bytes are needed; the `usize` is how many bytes the frame took.
fn parse_client_frame(raw: &[u8]) -> io::Result<Option<(usize, WsFrame)>> {
    if raw.len() < 2 {
        return Ok(None);
    }
    if raw[0] & 0x70 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "websocket extension bits set without an extension",
        ));
    }
    let opcode = raw[0] & 0x0f;
    if raw[1] & 0x80 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "client frame is not masked",
        ));
    }
    let mut offset = 2;
    let length = match raw[1] & 0x7f {
        126 => {
            if raw.len() < 4 {
                return Ok(None);
            }
            offset = 4;
            usize::from(u16::from_be_bytes([raw[2], raw[3]]))
        }
        127 => {
            if raw.len() < 10 {
                return Ok(None);
            }
            offset = 10;
            let length = u64::from_be_bytes(raw[2..10].try_into().expect("eight bytes"));
            usize::try_from(length).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "websocket frame too long")
            })?
        }
        short => usize::from(short),
    };
    if length > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "websocket frame over 16 MiB",
        ));
    }
    if raw.len() < offset + 4 + length {
        return Ok(None);
    }
    let mask = &raw[offset..offset + 4];
    let payload = raw[offset + 4..offset + 4 + length]
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ mask[index % 4])
        .collect();
    Ok(Some((offset + 4 + length, WsFrame { opcode, payload })))
}

/// A server-to-client binary frame around `payload`; never masked.
fn encode_binary_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0x82);
    match payload.len() {
        short @ 0..=125 => frame.push(short as u8),
        medium @ 126..=65_535 => {
            frame.push(126);
            frame.extend_from_slice(&(medium as u16).to_be_bytes());
        }
        long => {
            frame.push(127);
            frame.extend_from_slice(&(long as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    frame
}

/// The payload bytes of a WebSocket, read as one stream. Text and binary
/// data frames and their continuations are concatenated; a close frame is
/// the end of the stream; pings from a browser do not happen and are ignored.
struct WsRead<R> {
    inner: R,
    raw: Vec<u8>,
    out: VecDeque<u8>,
    closed: bool,
}

impl<R> WsRead<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            raw: Vec::new(),
            out: VecDeque::new(),
            closed: false,
        }
    }

    fn drain_frames(&mut self) -> io::Result<()> {
        while let Some((used, frame)) = parse_client_frame(&self.raw)? {
            self.raw.drain(..used);
            match frame.opcode {
                0x0..=0x2 => self.out.extend(frame.payload),
                0x8 => {
                    self.closed = true;
                    self.raw.clear();
                    return Ok(());
                }
                0x9 | 0xA => {}
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unknown websocket opcode {other}"),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for WsRead<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if !self.out.is_empty() {
                let n = self.out.len().min(buf.remaining());
                for byte in self.out.drain(..n) {
                    buf.put_slice(&[byte]);
                }
                return Poll::Ready(Ok(()));
            }
            if self.closed {
                return Poll::Ready(Ok(()));
            }
            let mut chunk = [0_u8; 16 * 1024];
            let mut read_buf = ReadBuf::new(&mut chunk);
            ready!(Pin::new(&mut self.inner).poll_read(cx, &mut read_buf))?;
            let filled = read_buf.filled().len();
            if filled == 0 {
                self.closed = true;
                continue;
            }
            self.raw.extend_from_slice(&chunk[..filled]);
            self.drain_frames()?;
        }
    }
}

/// Each write becomes one binary frame. A write that could not be finished
/// in one poll is remembered and completed before the next one is accepted.
struct WsWrite<W> {
    inner: W,
    pending: Vec<u8>,
    written: usize,
    accepted: Option<usize>,
}

impl<W> WsWrite<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
            written: 0,
            accepted: None,
        }
    }
}

impl<W: AsyncWrite + Unpin> WsWrite<W> {
    fn flush_pending(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.written < self.pending.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.pending[self.written..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "websocket closed while writing",
                )));
            }
            self.written += n;
        }
        self.pending.clear();
        self.written = 0;
        Poll::Ready(Ok(()))
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for WsWrite<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.accepted.is_none() {
            self.pending = encode_binary_frame(buf);
            self.written = 0;
            self.accepted = Some(buf.len());
        }
        ready!(self.flush_pending(cx))?;
        let accepted = self.accepted.take().unwrap_or(buf.len());
        Poll::Ready(Ok(accepted))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.flush_pending(cx))?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.flush_pending(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(lines: &[&str]) -> HttpRequest {
        let raw = format!("{}\r\n\r\n", lines.join("\r\n"));
        parse_http_head(raw.trim_end().as_bytes()).expect("parses")
    }

    #[test]
    fn the_rfc_example_key_yields_the_rfc_example_accept() {
        // RFC 6455 §1.3.
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn upgrade_needs_the_headers_and_a_matching_origin() {
        let good = request(&[
            "GET /ws HTTP/1.1",
            "Host: runtime.local:7373",
            "Origin: https://runtime.local:7373",
            "Upgrade: websocket",
            "Connection: keep-alive, Upgrade",
            "Sec-WebSocket-Version: 13",
            "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
        ]);
        assert_eq!(
            websocket_accept(&good).expect("accepts"),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        let elsewhere = request(&[
            "GET /ws HTTP/1.1",
            "Host: runtime.local:7373",
            "Origin: https://evil.example",
            "Upgrade: websocket",
            "Connection: Upgrade",
            "Sec-WebSocket-Version: 13",
            "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
        ]);
        assert_eq!(websocket_accept(&elsewhere), Err("origin is not this host"));
        let plain = request(&["GET /ws HTTP/1.1", "Host: runtime.local:7373"]);
        assert!(websocket_accept(&plain).is_err());
    }

    #[test]
    fn routes_serve_only_the_shell_files() {
        assert!(matches!(route(&request(&["GET / HTTP/1.1"])), Route::Asset { .. }));
        assert!(matches!(
            route(&request(&["GET /app.js?v=1 HTTP/1.1"])),
            Route::Asset { content_type, .. } if content_type.starts_with("text/javascript")
        ));
        assert_eq!(route(&request(&["GET /ws HTTP/1.1"])), Route::Socket);
        assert_eq!(route(&request(&["GET /../Cargo.toml HTTP/1.1"])), Route::NotFound);
        assert_eq!(route(&request(&["POST / HTTP/1.1"])), Route::MethodNotAllowed);
    }

    #[test]
    fn client_frames_are_unmasked_and_server_frames_are_not() {
        // RFC 6455 §5.7: a single-frame masked text message "Hello".
        let masked = [0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58];
        let (used, frame) = parse_client_frame(&masked).expect("parses").expect("complete");
        assert_eq!(used, masked.len());
        assert_eq!(frame.opcode, 0x1);
        assert_eq!(frame.payload, b"Hello");
        assert_eq!(parse_client_frame(&masked[..7]).expect("parses"), None);
        let unmasked = [0x81, 0x05, b'H', b'e', b'l', b'l', b'o'];
        assert!(parse_client_frame(&unmasked).is_err());
        assert_eq!(encode_binary_frame(b"Hello"), [0x82, 0x05, b'H', b'e', b'l', b'l', b'o']);
        let long = encode_binary_frame(&[0; 300]);
        assert_eq!(&long[..4], &[0x82, 126, 0x01, 0x2c]);
    }

    #[tokio::test]
    async fn the_websocket_halves_carry_a_byte_stream() {
        let (client, server) = tokio::io::duplex(64);
        let (server_read, server_write) = tokio::io::split(server);
        let mut read = WsRead::new(server_read);
        let mut write = WsWrite::new(server_write);
        let (mut client_read, mut client_write) = tokio::io::split(client);
        // Two masked client frames, one text and one continuation, arrive as
        // one stream; a large server write is framed once and read whole.
        let masked = [0x01, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58];
        let tail = [0x80, 0x81, 0x00, 0x00, 0x00, 0x00, b'!'];
        let writer = tokio::spawn(async move {
            client_write.write_all(&masked).await.expect("client writes");
            client_write.write_all(&tail).await.expect("client writes");
            let mut echoed = vec![0; 2 + 300];
            client_read.read_exact(&mut echoed).await.expect("client reads");
            echoed
        });
        let mut got = [0; 6];
        read.read_exact(&mut got).await.expect("server reads");
        assert_eq!(&got, b"Hello!");
        write.write_all(&[7; 300]).await.expect("server writes");
        write.flush().await.expect("flushes");
        let echoed = writer.await.expect("client task");
        assert_eq!(&echoed[..4], &[0x82, 126, 0x01, 0x2c]);
        assert!(echoed[4..].iter().all(|byte| *byte == 7));
    }
}
