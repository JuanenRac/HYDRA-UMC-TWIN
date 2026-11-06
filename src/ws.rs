// =============================================================================
// HYDRA-UMC-TWIN - src/ws.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The WebSocket (RFC 6455) through which the twin pushes its state to HYDRA-UMC-STUDIO and to the other clients that watch the robot.
//!
//! The protocol is written here and not taken from a crate: the opening handshake (`Sec-WebSocket-Accept` needs SHA-1 and Base64, both
//! small and tested against the vectors of the RFC), the frames in both directions, the control frames (ping, pong, close) and the limits that
//! keep one client from holding the twin: a bounded number of sessions, a read and a write timeout, a size cap on what a client may send, a
//! time limit per session and a drop of a client that stays silent.
//!
//! It has its own listener (`ws.listen` of the twin configuration, loopback by default) because the HTTP server of the twin hands over no
//! stream it could time out. The opening request needs the same bearer token as the other routes of the twin (or a signed token of the
//! SERVER when the identity is on); a request that carries an `Origin` (a browser) is refused unless that origin is in `ws.allowed_origins`.
//! The twin only pushes: what a client sends is read to answer pings and closes, and to see that the client is alive, and is otherwise ignored.
//!
//! Off unless the server is started with `--ws`.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::twincfg::{say, Lang, Say, WsCfg};

/// Whether the value of an `Authorization` header (None: there is none) may open a session.
pub type Auth = dyn Fn(Option<&str>) -> bool + Send + Sync;
/// The messages (JSON text) to push now, in the language asked for.
pub type Source = dyn Fn(Lang) -> Vec<String> + Send + Sync;

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// The longest opening request read.
const MAX_HEAD_BYTES: usize = 8 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the twin waits for the answer of a client to its close before it drops the connection.
const CLOSE_LINGER: Duration = Duration::from_secs(1);

pub const OP_CONT: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

// ---- SHA-1 and Base64: only what the handshake needs ----

/// SHA-1 (RFC 3174). Used only to answer the handshake of RFC 6455, never to protect anything.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for block in message.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard Base64 with padding.
pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The `Sec-WebSocket-Accept` for a `Sec-WebSocket-Key`.
pub fn accept_key(key: &str) -> String {
    let mut joined = key.as_bytes().to_vec();
    joined.extend_from_slice(GUID.as_bytes());
    base64(&sha1(&joined))
}

/// A `Sec-WebSocket-Key` is the Base64 of 16 bytes: 22 characters of the alphabet and `==`.
fn valid_key(key: &str) -> bool {
    key.len() == 24
        && key.ends_with("==")
        && key.as_bytes()[..22].iter().all(|b| B64[..].contains(b))
}

// ---- frames ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The peer broke the protocol (close with 1002).
    Protocol(&'static str),
    /// The frame is larger than the limit (close with 1009).
    TooBig,
}

/// A frame of the server: not masked, not fragmented.
pub fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 10);
    out.push(0x80 | (opcode & 0x0F));
    match payload.len() {
        0..=125 => out.push(payload.len() as u8),
        126..=65_535 => {
            out.push(126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        _ => {
            out.push(127);
            out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    out
}

/// The payload of a close frame: the code and a reason of at most 123 bytes (cut at a character).
pub fn close_payload(code: u16, reason: &str) -> Vec<u8> {
    let mut end = reason.len().min(123);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&reason.as_bytes()[..end]);
    out
}

fn known_opcode(opcode: u8) -> bool {
    matches!(
        opcode,
        OP_CONT | OP_TEXT | OP_BINARY | OP_CLOSE | OP_PING | OP_PONG
    )
}

/// Reads one frame of a client from the front of `buf`: None when it is not complete yet, otherwise the frame (unmasked) and how many bytes it took.
/// A client must mask every frame, use the shortest length encoding and set no reserved bit; a control frame is short and not fragmented.
pub fn parse_client_frame(
    buf: &[u8],
    max_payload: usize,
) -> Result<Option<(Frame, usize)>, FrameError> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let (b0, b1) = (buf[0], buf[1]);
    if b0 & 0x70 != 0 {
        return Err(FrameError::Protocol("a reserved bit is set"));
    }
    let opcode = b0 & 0x0F;
    if !known_opcode(opcode) {
        return Err(FrameError::Protocol("unknown opcode"));
    }
    let fin = b0 & 0x80 != 0;
    if b1 & 0x80 == 0 {
        return Err(FrameError::Protocol("a client frame is not masked"));
    }
    let short = (b1 & 0x7F) as usize;
    let control = opcode >= OP_CLOSE;
    if control && (!fin || short > 125) {
        return Err(FrameError::Protocol(
            "a control frame is fragmented or longer than 125 bytes",
        ));
    }
    let (length, mut at) = match short {
        126 => {
            if buf.len() < 4 {
                return Ok(None);
            }
            let n = u16::from_be_bytes([buf[2], buf[3]]) as usize;
            if n < 126 {
                return Err(FrameError::Protocol(
                    "the length is not in its shortest form",
                ));
            }
            (n, 4)
        }
        127 => {
            if buf.len() < 10 {
                return Ok(None);
            }
            let mut eight = [0u8; 8];
            eight.copy_from_slice(&buf[2..10]);
            let n = u64::from_be_bytes(eight);
            if n >> 63 != 0 {
                return Err(FrameError::Protocol("the length has its top bit set"));
            }
            if n <= 65_535 {
                return Err(FrameError::Protocol(
                    "the length is not in its shortest form",
                ));
            }
            if n > max_payload as u64 {
                return Err(FrameError::TooBig);
            }
            (n as usize, 10)
        }
        n => (n, 2),
    };
    if length > max_payload {
        return Err(FrameError::TooBig);
    }
    if buf.len() < at + 4 + length {
        return Ok(None);
    }
    let mask = [buf[at], buf[at + 1], buf[at + 2], buf[at + 3]];
    at += 4;
    let payload: Vec<u8> = buf[at..at + length]
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ mask[i % 4])
        .collect();
    Ok(Some((
        Frame {
            fin,
            opcode,
            payload,
        },
        at + length,
    )))
}

/// Whether a close code may be on the wire (RFC 6455, 7.4).
fn valid_close_code(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1011 | 3000..=4999)
}

// ---- the opening request ----

struct Request {
    path: String,
    query: String,
    headers: Vec<(String, String)>,
}

impl Request {
    /// The value of a header that must appear once: Err when it appears more than once.
    fn one(&self, name: &str) -> Result<Option<&str>, ()> {
        let mut found = self.headers.iter().filter(|(n, _)| n == name);
        let first = found.next().map(|(_, v)| v.as_str());
        if found.next().is_some() {
            return Err(());
        }
        Ok(first)
    }

    fn has_token(&self, name: &str, token: &str) -> bool {
        self.headers
            .iter()
            .filter(|(n, _)| n == name)
            .flat_map(|(_, v)| v.split(','))
            .any(|part| part.trim().eq_ignore_ascii_case(token))
    }
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn parse_request(head: &str) -> Result<Request, &'static str> {
    let mut lines = head.split("\r\n");
    let line = lines.next().ok_or("empty request")?;
    let mut parts = line.split(' ');
    let (method, target, version) = (parts.next(), parts.next(), parts.next());
    let (Some(method), Some(target), Some(version), None) = (method, target, version, parts.next())
    else {
        return Err("malformed request line");
    };
    if version != "HTTP/1.1" {
        return Err("only HTTP/1.1 opens a WebSocket");
    }
    if method != "GET" {
        return Err("method");
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').ok_or("malformed header")?;
        if name.is_empty() || name.contains(' ') {
            return Err("malformed header");
        }
        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
    }
    Ok(Request {
        path: path.to_string(),
        query: query.to_string(),
        headers,
    })
}

enum HeadError {
    Io,
    TooBig,
    Timeout,
}

/// Reads up to the blank line that ends the head; what follows it (a client that already sends frames) is returned too.
fn read_head(stream: &mut TcpStream, limit: Duration) -> Result<(String, Vec<u8>), HeadError> {
    let deadline = Instant::now() + limit;
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..end]).to_string();
            return Ok((head, raw[end + 4..].to_vec()));
        }
        if raw.len() > MAX_HEAD_BYTES {
            return Err(HeadError::TooBig);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(HeadError::Timeout);
        }
        let _ = stream.set_read_timeout(Some(left));
        match stream.read(&mut buf) {
            Ok(0) => return Err(HeadError::Io),
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err(HeadError::Timeout)
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Err(HeadError::Io),
        }
    }
}

fn respond(stream: &mut TcpStream, status: u16, reason: &str, extra: &str, message: &str) {
    let body = json!({"error": message}).to_string();
    let text = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

// ---- the server ----

/// The address of `ws.listen` must be a loopback one unless `ws.allow_remote` says otherwise: the token of the opening request would travel in clear text.
pub fn check_listen(cfg: &WsCfg) -> Result<(), String> {
    use std::net::ToSocketAddrs;
    let addresses: Vec<_> = cfg
        .listen
        .to_socket_addrs()
        .map_err(|e| format!("ws.listen {} is not an address: {e}", cfg.listen))?
        .collect();
    if addresses.is_empty() {
        return Err(format!("ws.listen {} gives no address", cfg.listen));
    }
    if !cfg.allow_remote && addresses.iter().any(|a| !a.ip().is_loopback()) {
        return Err("ws.listen is not a loopback address: the token would travel in clear text; set ws.allow_remote to accept that".into());
    }
    Ok(())
}

struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Accepts connections on `listener` for ever (run it on a thread of its own).
pub fn serve(
    listener: TcpListener,
    cfg: WsCfg,
    default_lang: Lang,
    auth: Arc<Auth>,
    source: Arc<Source>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    let cfg = Arc::new(cfg);
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        if active.fetch_add(1, Ordering::SeqCst) >= cfg.max_clients {
            active.fetch_sub(1, Ordering::SeqCst);
            crate::metrics::count(&crate::metrics::WS_REFUSED);
            respond(
                &mut stream,
                503,
                "Service Unavailable",
                "Retry-After: 5\r\n",
                "too many WebSocket sessions are open",
            );
            continue;
        }
        let slot = Slot(active.clone());
        let (cfg, auth, source) = (cfg.clone(), auth.clone(), source.clone());
        // when the thread cannot be started the closure, and with it the slot, is dropped
        let _ = std::thread::Builder::new()
            .name("twin-ws".into())
            .spawn(move || {
                let _slot = slot;
                connection(stream, &cfg, default_lang, auth.as_ref(), source.as_ref());
            });
    }
}

fn connection(
    mut stream: TcpStream,
    cfg: &WsCfg,
    default_lang: Lang,
    auth: &Auth,
    source: &Source,
) {
    let _ = stream.set_nodelay(true);
    let (head, leftover) =
        match read_head(&mut stream, Duration::from_millis(cfg.handshake_timeout_ms)) {
            Ok(read) => read,
            Err(HeadError::Io) => return,
            Err(HeadError::TooBig) => {
                respond(
                    &mut stream,
                    431,
                    "Request Header Fields Too Large",
                    "",
                    "the request is too large",
                );
                return;
            }
            Err(HeadError::Timeout) => {
                respond(
                    &mut stream,
                    408,
                    "Request Timeout",
                    "",
                    "the request did not arrive in time",
                );
                return;
            }
        };
    let request = match parse_request(&head) {
        Ok(request) => request,
        Err("method") => {
            respond(
                &mut stream,
                405,
                "Method Not Allowed",
                "Allow: GET\r\n",
                "method not allowed",
            );
            return;
        }
        Err(message) => {
            respond(&mut stream, 400, "Bad Request", "", message);
            return;
        }
    };
    if request.path != "/twin/ws" {
        respond(&mut stream, 404, "Not Found", "", "not found");
        return;
    }
    let (Ok(authorization), Ok(origin), Ok(key), Ok(version)) = (
        request.one("authorization"),
        request.one("origin"),
        request.one("sec-websocket-key"),
        request.one("sec-websocket-version"),
    ) else {
        respond(
            &mut stream,
            400,
            "Bad Request",
            "",
            "a header that must appear once appears more than once",
        );
        return;
    };
    if !auth(authorization) {
        crate::metrics::count(&crate::metrics::UNAUTHORIZED);
        crate::metrics::count(&crate::metrics::WS_REFUSED);
        respond(
            &mut stream,
            401,
            "Unauthorized",
            "WWW-Authenticate: Bearer\r\n",
            "a valid bearer token is required",
        );
        return;
    }
    if let Some(origin) = origin {
        if !cfg.allowed_origins.iter().any(|allowed| allowed == origin) {
            crate::metrics::count(&crate::metrics::WS_REFUSED);
            respond(
                &mut stream,
                403,
                "Forbidden",
                "",
                "this origin is not allowed (ws.allowed_origins)",
            );
            return;
        }
    }
    if !request.has_token("upgrade", "websocket") || !request.has_token("connection", "upgrade") {
        respond(
            &mut stream,
            400,
            "Bad Request",
            "",
            "this route opens a WebSocket: Upgrade: websocket and Connection: Upgrade are needed",
        );
        return;
    }
    if version != Some("13") {
        respond(
            &mut stream,
            426,
            "Upgrade Required",
            "Sec-WebSocket-Version: 13\r\n",
            "only version 13 of the WebSocket protocol is spoken",
        );
        return;
    }
    let Some(key) = key.filter(|k| valid_key(k)) else {
        respond(
            &mut stream,
            400,
            "Bad Request",
            "",
            "Sec-WebSocket-Key is missing or is not the Base64 of 16 bytes",
        );
        return;
    };
    let lang = query_param(&request.query, "lang")
        .and_then(|tag| Lang::from_tag(&tag))
        .or_else(|| {
            request
                .one("accept-language")
                .ok()
                .flatten()
                .and_then(Lang::from_accept_language)
        })
        .unwrap_or(default_lang);
    let reply = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    );
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    if stream.write_all(reply.as_bytes()).is_err() {
        return;
    }
    crate::metrics::count(&crate::metrics::WS_SESSIONS);
    session(stream, cfg, lang, source, leftover);
}

/// Why the twin ends a session.
enum End {
    /// The client (or the network) ended it.
    Gone,
    /// The twin closes with this code and reason.
    Close(u16, String),
}

fn send(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    stream.write_all(&encode_frame(opcode, payload))?;
    stream.flush()
}

/// What the client sent as one message (data frames can be split in fragments): kept only to check it is well formed.
#[derive(Default)]
struct Assembly {
    text: Option<bool>,
    data: Vec<u8>,
}

fn session(mut stream: TcpStream, cfg: &WsCfg, lang: Lang, source: &Source, mut inbuf: Vec<u8>) {
    let tick = Duration::from_millis((cfg.interval_ms / 2).clamp(10, 250));
    let _ = stream.set_read_timeout(Some(tick));
    let started = Instant::now();
    let mut last_heard = Instant::now();
    let mut next_push = Instant::now();
    let mut next_ping = Instant::now() + Duration::from_millis(cfg.ping_interval_ms);
    let mut pings: u64 = 0;
    let mut last_pushed: Vec<String> = Vec::new();
    let mut assembly = Assembly::default();
    let mut chunk = [0u8; 2048];
    let end = 'run: loop {
        let now = Instant::now();
        if now.duration_since(started) >= Duration::from_secs(cfg.max_session_s) {
            // the sentence for the person goes in a message (a close reason holds 123 bytes); the reason is a short fixed text
            let notice = json!({
                "type": "status",
                "code": "session_ended",
                "message": say(lang, Say::WsSessionEnded),
            })
            .to_string();
            let _ = send(&mut stream, OP_TEXT, notice.as_bytes());
            break End::Close(1001, "session time limit".to_string());
        }
        if now.duration_since(last_heard) >= Duration::from_millis(cfg.idle_timeout_ms) {
            break End::Close(1001, "idle".to_string());
        }
        if now >= next_push {
            next_push = now + Duration::from_millis(cfg.interval_ms);
            let messages = source(lang);
            for message in messages.iter().filter(|m| !last_pushed.contains(m)) {
                if send(&mut stream, OP_TEXT, message.as_bytes()).is_err() {
                    break 'run End::Gone;
                }
            }
            last_pushed = messages;
        }
        if now >= next_ping {
            next_ping = now + Duration::from_millis(cfg.ping_interval_ms);
            pings += 1;
            if send(&mut stream, OP_PING, &pings.to_be_bytes()).is_err() {
                break End::Gone;
            }
        }
        // what the client sent, frame by frame
        loop {
            match parse_client_frame(&inbuf, cfg.max_frame_bytes) {
                Ok(None) => break,
                Ok(Some((frame, used))) => {
                    inbuf.drain(..used);
                    match handle_frame(&mut stream, frame, &mut assembly, cfg.max_frame_bytes) {
                        Ok(None) => {}
                        Ok(Some(end)) => break 'run end,
                        Err(_) => break 'run End::Gone,
                    }
                }
                Err(FrameError::Protocol(why)) => break 'run End::Close(1002, why.to_string()),
                Err(FrameError::TooBig) => {
                    break 'run End::Close(1009, "the frame is too large".to_string())
                }
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break End::Gone,
            Ok(n) => {
                last_heard = Instant::now();
                inbuf.extend_from_slice(&chunk[..n]);
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => break End::Gone,
        }
    };
    if let End::Close(code, reason) = end {
        if send(&mut stream, OP_CLOSE, &close_payload(code, &reason)).is_ok() {
            linger(&mut stream);
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// After the twin's close frame: give the client a moment to answer, so the close is not lost to a reset.
fn linger(stream: &mut TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let deadline = Instant::now() + CLOSE_LINGER;
    let _ = stream.set_read_timeout(Some(CLOSE_LINGER));
    let mut sink = [0u8; 512];
    while Instant::now() < deadline {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

/// One frame of the client. Ok(Some) ends the session; Err is a failed write.
fn handle_frame(
    stream: &mut TcpStream,
    frame: Frame,
    assembly: &mut Assembly,
    max_message: usize,
) -> std::io::Result<Option<End>> {
    match frame.opcode {
        OP_PING => {
            send(stream, OP_PONG, &frame.payload)?;
            Ok(None)
        }
        OP_PONG => Ok(None),
        OP_CLOSE => {
            let payload = &frame.payload;
            let answer = match payload.len() {
                0 => Some(1000),
                1 => None,
                _ => {
                    let code = u16::from_be_bytes([payload[0], payload[1]]);
                    (valid_close_code(code) && std::str::from_utf8(&payload[2..]).is_ok())
                        .then_some(code)
                }
            };
            match answer {
                Some(code) => {
                    let _ = send(stream, OP_CLOSE, &code.to_be_bytes());
                    Ok(Some(End::Gone))
                }
                None => Ok(Some(End::Close(1002, "malformed close frame".to_string()))),
            }
        }
        OP_TEXT | OP_BINARY | OP_CONT => {
            if frame.opcode == OP_CONT {
                if assembly.text.is_none() {
                    return Ok(Some(End::Close(
                        1002,
                        "a continuation without a message".to_string(),
                    )));
                }
            } else if assembly.text.is_some() {
                return Ok(Some(End::Close(
                    1002,
                    "a new message in the middle of another".to_string(),
                )));
            } else {
                assembly.text = Some(frame.opcode == OP_TEXT);
            }
            if assembly.data.len() + frame.payload.len() > max_message {
                return Ok(Some(End::Close(
                    1009,
                    "the message is too large".to_string(),
                )));
            }
            assembly.data.extend_from_slice(&frame.payload);
            if frame.fin {
                let was_text = assembly.text == Some(true);
                let valid = !was_text || std::str::from_utf8(&assembly.data).is_ok();
                *assembly = Assembly::default();
                if !valid {
                    return Ok(Some(End::Close(1007, "the text is not UTF-8".to_string())));
                }
            }
            Ok(None)
        }
        _ => Ok(Some(End::Close(1002, "unknown opcode".to_string()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut text, b| {
            let _ = write!(text, "{b:02x}");
            text
        })
    }

    #[test]
    fn sha1_gives_the_published_digests() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        // a message that fills more than one block, and one that ends exactly on the padding boundary
        assert_eq!(
            hex(&sha1(&[b'a'; 1000])),
            "291e9a6c66994949b57ba5e650361e98fc36b1ba"
        );
        assert_eq!(sha1(&[7u8; 55]).len(), 20);
        assert_ne!(sha1(&[7u8; 55]), sha1(&[7u8; 56]));
    }

    #[test]
    fn base64_follows_rfc_4648() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), coded);
        }
    }

    #[test]
    fn the_accept_key_is_the_one_of_the_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert!(valid_key("dGhlIHNhbXBsZSBub25jZQ=="));
        for bad in [
            "",
            "short==",
            "dGhlIHNhbXBsZSBub25jZQ!!",
            "dGhlIHNhbXBsZSBub25jZQAA",
        ] {
            assert!(!valid_key(bad), "{bad}");
        }
    }

    /// A frame as a client sends it: masked.
    fn client_frame(first: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
        let mut out = vec![first];
        match payload.len() {
            0..=125 => out.push(0x80 | payload.len() as u8),
            126..=65_535 => {
                out.push(0x80 | 126);
                out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            }
            _ => {
                out.push(0x80 | 127);
                out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
            }
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        out
    }

    #[test]
    fn the_masked_hello_of_the_rfc_is_read() {
        // RFC 6455, 5.7: a single-frame masked text message "Hello"
        let raw = [
            0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        let (frame, used) = parse_client_frame(&raw, 1024).unwrap().unwrap();
        assert_eq!(used, raw.len());
        assert!(frame.fin);
        assert_eq!(frame.opcode, OP_TEXT);
        assert_eq!(frame.payload, b"Hello");
    }

    #[test]
    fn a_frame_that_is_not_complete_waits_and_two_frames_are_read_one_by_one() {
        let one = client_frame(0x81, b"abc", [1, 2, 3, 4]);
        for cut in 0..one.len() {
            assert_eq!(parse_client_frame(&one[..cut], 1024), Ok(None), "{cut}");
        }
        let mut two = one.clone();
        two.extend(client_frame(0x89, b"", [9, 9, 9, 9]));
        let (first, used) = parse_client_frame(&two, 1024).unwrap().unwrap();
        assert_eq!(first.payload, b"abc");
        let (second, _) = parse_client_frame(&two[used..], 1024).unwrap().unwrap();
        assert_eq!(second.opcode, OP_PING);
    }

    #[test]
    fn the_lengths_of_126_and_65536_bytes_are_read() {
        let payload = vec![0x5a; 300];
        let frame = client_frame(0x82, &payload, [5, 6, 7, 8]);
        let (parsed, used) = parse_client_frame(&frame, 1024).unwrap().unwrap();
        assert_eq!((parsed.payload.len(), used), (300, frame.len()));
        let big = vec![1u8; 70_000];
        let frame = client_frame(0x82, &big, [5, 6, 7, 8]);
        let (parsed, _) = parse_client_frame(&frame, 100_000).unwrap().unwrap();
        assert_eq!(parsed.payload, big);
        // the limit is checked from the header: the payload is not waited for
        assert_eq!(
            parse_client_frame(&frame[..14], 1_000),
            Err(FrameError::TooBig)
        );
    }

    #[test]
    fn a_client_that_breaks_the_protocol_is_refused() {
        let mask = [1, 2, 3, 4];
        // not masked
        assert!(matches!(
            parse_client_frame(&[0x81, 0x01, b'a'], 100),
            Err(FrameError::Protocol(_))
        ));
        // reserved bit
        assert!(matches!(
            parse_client_frame(&client_frame(0xC1, b"a", mask), 100),
            Err(FrameError::Protocol(_))
        ));
        // unknown opcode
        assert!(matches!(
            parse_client_frame(&client_frame(0x83, b"a", mask), 100),
            Err(FrameError::Protocol(_))
        ));
        // a fragmented control frame
        assert!(matches!(
            parse_client_frame(&client_frame(0x09, b"a", mask), 100),
            Err(FrameError::Protocol(_))
        ));
        // a control frame of 126 bytes
        let long = client_frame(0x89, &[0u8; 126], mask);
        assert!(matches!(
            parse_client_frame(&long, 1000),
            Err(FrameError::Protocol(_))
        ));
        // a length written with more bytes than it needs
        let padded = [0x81, 0x80 | 126, 0, 5, 1, 2, 3, 4, 0, 0, 0, 0, 0];
        assert!(matches!(
            parse_client_frame(&padded, 100),
            Err(FrameError::Protocol(_))
        ));
        // the top bit of a 64-bit length
        let mut huge = vec![0x82, 0x80 | 127, 0x80, 0, 0, 0, 0, 0, 0, 0];
        huge.extend_from_slice(&mask);
        assert!(matches!(
            parse_client_frame(&huge, 100),
            Err(FrameError::Protocol(_))
        ));
    }

    #[test]
    fn the_frames_of_the_server_have_the_right_header() {
        assert_eq!(encode_frame(OP_TEXT, b"hi"), vec![0x81, 2, b'h', b'i']);
        let medium = encode_frame(OP_BINARY, &[0u8; 200]);
        assert_eq!(&medium[..4], &[0x82, 126, 0, 200]);
        let large = encode_frame(OP_TEXT, &vec![0u8; 70_000]);
        assert_eq!(&large[..2], &[0x81, 127]);
        assert_eq!(u64::from_be_bytes(large[2..10].try_into().unwrap()), 70_000);
        assert_eq!(large.len(), 10 + 70_000);
    }

    #[test]
    fn a_close_reason_is_cut_at_a_character_and_a_code_is_checked() {
        let reason = "é".repeat(100);
        let payload = close_payload(1001, &reason);
        assert!(payload.len() <= 125);
        assert!(std::str::from_utf8(&payload[2..]).is_ok());
        for ok in [1000, 1001, 1003, 1007, 1011, 3000, 4999] {
            assert!(valid_close_code(ok), "{ok}");
        }
        for bad in [0, 999, 1004, 1005, 1006, 1012, 2999, 5000] {
            assert!(!valid_close_code(bad), "{bad}");
        }
    }

    #[test]
    fn the_listener_is_loopback_unless_the_configuration_allows_more() {
        let mut cfg = WsCfg::default();
        assert!(check_listen(&cfg).is_ok());
        cfg.listen = "localhost:8112".into();
        assert!(check_listen(&cfg).is_ok());
        cfg.listen = "0.0.0.0:8112".into();
        assert!(check_listen(&cfg).is_err());
        cfg.allow_remote = true;
        assert!(check_listen(&cfg).is_ok());
        cfg.listen = "no-port".into();
        assert!(check_listen(&cfg).is_err());
    }

    #[test]
    fn the_opening_request_is_read_and_a_repeated_header_is_noticed() {
        let head = "GET /twin/ws?lang=es HTTP/1.1\r\nHost: x\r\nUpgrade: WebSocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Key: a\r\nSec-WebSocket-Key: b";
        let request = parse_request(head).unwrap();
        assert_eq!(request.path, "/twin/ws");
        assert_eq!(query_param(&request.query, "lang").as_deref(), Some("es"));
        assert!(request.has_token("upgrade", "websocket"));
        assert!(request.has_token("connection", "upgrade"));
        assert!(request.one("sec-websocket-key").is_err());
        assert_eq!(request.one("host"), Ok(Some("x")));
        assert!(parse_request("GET / HTTP/1.0\r\n").is_err());
        assert!(matches!(
            parse_request("POST /twin/ws HTTP/1.1\r\n"),
            Err("method")
        ));
        assert!(parse_request("GET /twin/ws HTTP/1.1\r\nbroken header\r\n").is_err());
    }

    // ---- a real server on a real socket ----

    const TOKEN: &str = "a-ws-token-of-sixteen+";
    const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

    fn test_cfg() -> WsCfg {
        WsCfg {
            interval_ms: 50,
            ping_interval_ms: 1_000,
            idle_timeout_ms: 3_000,
            handshake_timeout_ms: 500,
            max_session_s: 60,
            ..WsCfg::default()
        }
    }

    /// Starts a server whose source pushes `messages` (a constant list) and returns its port.
    fn start(cfg: WsCfg, messages: Vec<String>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected = format!("Bearer {TOKEN}");
        let auth: Arc<Auth> =
            Arc::new(move |header: Option<&str>| header == Some(expected.as_str()));
        let source: Arc<Source> = Arc::new(move |lang: Lang| -> Vec<String> {
            messages
                .iter()
                .map(|m| m.replace("{lang}", lang.code()))
                .collect()
        });
        std::thread::spawn(move || serve(listener, cfg, Lang::En, auth, source));
        port
    }

    fn open_request(path: &str, extra: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {KEY}\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n")
    }

    fn authorised() -> String {
        format!("Authorization: Bearer {TOKEN}\r\n")
    }

    /// Sends a raw request and reads the head of the answer (and what follows it, for a while).
    fn connect(port: u16, request: &str) -> (TcpStream, String, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&raw[..end]).to_string();
                return (stream, head, raw[end + 4..].to_vec());
            }
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0, "closed before a head: {raw:?}");
            raw.extend_from_slice(&buf[..n]);
        }
    }

    fn status_of(head: &str) -> u16 {
        head.split_whitespace().nth(1).unwrap().parse().unwrap()
    }

    /// Reads one server frame (they are never masked), using `pending` as the bytes already read.
    fn read_frame(stream: &mut TcpStream, pending: &mut Vec<u8>) -> Option<(u8, Vec<u8>)> {
        let mut buf = [0u8; 4096];
        loop {
            if pending.len() >= 2 {
                let opcode = pending[0] & 0x0F;
                let (length, header) = match pending[1] & 0x7F {
                    126 if pending.len() >= 4 => {
                        (u16::from_be_bytes([pending[2], pending[3]]) as usize, 4)
                    }
                    127 | 126 => (usize::MAX, 0),
                    n => (n as usize, 2),
                };
                if length != usize::MAX && pending.len() >= header + length {
                    let payload = pending[header..header + length].to_vec();
                    pending.drain(..header + length);
                    return Some((opcode, payload));
                }
            }
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return None,
                Ok(n) => pending.extend_from_slice(&buf[..n]),
            }
        }
    }

    #[test]
    fn a_client_with_the_token_is_upgraded_and_gets_the_messages_in_its_language() {
        let port = start(
            test_cfg(),
            vec![r#"{"type":"x","lang":"{lang}"}"#.to_string()],
        );
        let (mut stream, head, mut pending) =
            connect(port, &open_request("/twin/ws?lang=pt_PT", &authorised()));
        assert_eq!(status_of(&head), 101, "{head}");
        assert!(
            head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "{head}"
        );
        let (opcode, payload) = read_frame(&mut stream, &mut pending).unwrap();
        assert_eq!(opcode, OP_TEXT);
        assert_eq!(payload, br#"{"type":"x","lang":"pt"}"#);
    }

    #[test]
    fn what_has_not_changed_is_not_pushed_again() {
        let port = start(test_cfg(), vec![r#"{"same":true}"#.to_string()]);
        let (mut stream, _, mut pending) = connect(port, &open_request("/twin/ws", &authorised()));
        assert_eq!(read_frame(&mut stream, &mut pending).unwrap().0, OP_TEXT);
        // the next frame is the ping of the second, not another copy of the message
        let (opcode, _) = read_frame(&mut stream, &mut pending).unwrap();
        assert_eq!(opcode, OP_PING);
    }

    #[test]
    fn without_the_token_or_with_a_wrong_one_nothing_is_opened() {
        let port = start(test_cfg(), vec!["{}".to_string()]);
        let (_, head, _) = connect(port, &open_request("/twin/ws", ""));
        assert_eq!(status_of(&head), 401, "{head}");
        assert!(head.contains("WWW-Authenticate: Bearer"));
        let (_, head, _) = connect(
            port,
            &open_request("/twin/ws", "Authorization: Bearer nope\r\n"),
        );
        assert_eq!(status_of(&head), 401);
    }

    #[test]
    fn a_browser_origin_is_refused_unless_it_is_listed() {
        let cfg = WsCfg {
            allowed_origins: vec!["https://studio.example".into()],
            ..test_cfg()
        };
        let port = start(cfg, vec!["{}".to_string()]);
        let other = format!("{}Origin: https://evil.example\r\n", authorised());
        let (_, head, _) = connect(port, &open_request("/twin/ws", &other));
        assert_eq!(status_of(&head), 403, "{head}");
        let listed = format!("{}Origin: https://studio.example\r\n", authorised());
        let (_, head, _) = connect(port, &open_request("/twin/ws", &listed));
        assert_eq!(status_of(&head), 101, "{head}");
        // with no list at all a request that carries an Origin never gets in
        let port = start(test_cfg(), vec!["{}".to_string()]);
        let (_, head, _) = connect(port, &open_request("/twin/ws", &listed));
        assert_eq!(status_of(&head), 403);
    }

    #[test]
    fn a_request_that_is_not_a_valid_opening_is_answered_with_its_status() {
        let port = start(test_cfg(), vec!["{}".to_string()]);
        let auth = authorised();
        let (_, head, _) = connect(port, &open_request("/elsewhere", &auth));
        assert_eq!(status_of(&head), 404);
        let (_, head, _) = connect(
            port,
            &format!("POST /twin/ws HTTP/1.1\r\nHost: x\r\n{auth}\r\n"),
        );
        assert_eq!(status_of(&head), 405);
        let (_, head, _) = connect(
            port,
            &format!("GET /twin/ws HTTP/1.1\r\nHost: x\r\n{auth}\r\n"),
        );
        assert_eq!(status_of(&head), 400, "no Upgrade");
        let old = open_request("/twin/ws", &auth).replace("Version: 13", "Version: 8");
        let (_, head, _) = connect(port, &old);
        assert_eq!(status_of(&head), 426);
        assert!(head.contains("Sec-WebSocket-Version: 13"));
        let bad_key = open_request("/twin/ws", &auth).replace(KEY, "not-a-key");
        let (_, head, _) = connect(port, &bad_key);
        assert_eq!(status_of(&head), 400);
        // a head that never ends
        let (_, head, _) = connect(
            port,
            "GET /twin/ws HTTP/1.1\r\nHost: x\r\nX-Slow: 1\r\n\r\n",
        );
        assert_ne!(status_of(&head), 101);
    }

    #[test]
    fn a_client_that_never_finishes_its_request_is_timed_out_and_does_not_hold_a_slot() {
        let cfg = WsCfg {
            max_clients: 1,
            ..test_cfg()
        };
        let port = start(cfg, vec!["{}".to_string()]);
        let mut slow = TcpStream::connect(("127.0.0.1", port)).unwrap();
        slow.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        slow.write_all(b"GET /twin/ws HTTP/1.1\r\nHost: x\r\n")
            .unwrap();
        let mut answer = String::new();
        let _ = slow.read_to_string(&mut answer);
        assert!(answer.starts_with("HTTP/1.1 408"), "{answer}");
        let mut status = 0;
        for _ in 0..50 {
            let (_, head, _) = connect(port, &open_request("/twin/ws", &authorised()));
            status = status_of(&head);
            if status == 101 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(status, 101);
    }

    #[test]
    fn one_session_more_than_the_limit_is_a_503_and_a_closed_one_frees_its_slot() {
        let cfg = WsCfg {
            max_clients: 1,
            ..test_cfg()
        };
        let port = start(cfg, vec!["{}".to_string()]);
        let (mut first, head, _) = connect(port, &open_request("/twin/ws", &authorised()));
        assert_eq!(status_of(&head), 101);
        let (_, head, _) = connect(port, &open_request("/twin/ws", &authorised()));
        assert_eq!(status_of(&head), 503, "{head}");
        assert!(head.contains("Retry-After"));
        // the first client closes; the slot comes back
        first
            .write_all(&client_frame(0x88, &1000u16.to_be_bytes(), [1, 2, 3, 4]))
            .unwrap();
        let mut rest = Vec::new();
        let _ = first.read_to_end(&mut rest);
        let mut status = 0;
        for _ in 0..50 {
            let (_, head, _) = connect(port, &open_request("/twin/ws", &authorised()));
            status = status_of(&head);
            if status == 101 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(status, 101);
    }

    #[test]
    fn a_ping_is_answered_with_the_same_payload_and_a_close_is_echoed() {
        let port = start(test_cfg(), vec!["{}".to_string()]);
        let (mut stream, _, mut pending) = connect(port, &open_request("/twin/ws", &authorised()));
        stream
            .write_all(&client_frame(0x89, b"are you there", [9, 8, 7, 6]))
            .unwrap();
        let mut got_pong = false;
        for _ in 0..10 {
            let (opcode, payload) = read_frame(&mut stream, &mut pending).unwrap();
            if opcode == OP_PONG {
                assert_eq!(payload, b"are you there");
                got_pong = true;
                break;
            }
        }
        assert!(got_pong);
        let mut close = 1000u16.to_be_bytes().to_vec();
        close.extend_from_slice(b"bye");
        stream
            .write_all(&client_frame(0x88, &close, [1, 1, 1, 1]))
            .unwrap();
        loop {
            match read_frame(&mut stream, &mut pending) {
                Some((OP_CLOSE, payload)) => {
                    assert_eq!(payload, 1000u16.to_be_bytes());
                    break;
                }
                Some(_) => {}
                None => panic!("the connection ended without the close of the twin"),
            }
        }
    }

    #[test]
    fn a_frame_that_breaks_the_protocol_closes_the_session_with_1002_or_1009_or_1007() {
        let port = start(test_cfg(), vec!["{}".to_string()]);
        for (frame, code) in [
            (vec![0x81, 0x01, b'a'], 1002u16),
            (client_frame(0x81, &[0xff, 0xfe], [1, 2, 3, 4]), 1007),
            (client_frame(0x82, &vec![0u8; 5_000], [1, 2, 3, 4]), 1009),
            (client_frame(0x80, b"x", [1, 2, 3, 4]), 1002),
        ] {
            let (mut stream, _, mut pending) =
                connect(port, &open_request("/twin/ws", &authorised()));
            stream.write_all(&frame).unwrap();
            let mut closed_with = None;
            while let Some((opcode, payload)) = read_frame(&mut stream, &mut pending) {
                if opcode == OP_CLOSE {
                    closed_with = Some(u16::from_be_bytes([payload[0], payload[1]]));
                    break;
                }
            }
            assert_eq!(closed_with, Some(code));
        }
    }

    #[test]
    fn a_client_that_stays_silent_is_dropped_and_a_long_session_is_closed_with_a_sentence() {
        let cfg = WsCfg {
            idle_timeout_ms: 2_000,
            ping_interval_ms: 1_000,
            ..test_cfg()
        };
        let port = start(cfg, vec!["{}".to_string()]);
        let (mut stream, _, mut pending) = connect(port, &open_request("/twin/ws", &authorised()));
        let started = Instant::now();
        let mut reason = None;
        while let Some((opcode, payload)) = read_frame(&mut stream, &mut pending) {
            if opcode == OP_CLOSE {
                reason = Some(String::from_utf8_lossy(&payload[2..]).to_string());
                break;
            }
        }
        assert_eq!(reason.as_deref(), Some("idle"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_session_past_its_time_limit_is_closed_with_a_sentence_in_the_language_of_the_client() {
        let cfg = WsCfg {
            max_session_s: 1,
            ..test_cfg()
        };
        let port = start(cfg, vec!["{}".to_string()]);
        let (mut stream, _, mut pending) =
            connect(port, &open_request("/twin/ws?lang=es", &authorised()));
        let (mut sentence, mut closed) = (None, false);
        while let Some((opcode, payload)) = read_frame(&mut stream, &mut pending) {
            if opcode == OP_TEXT {
                let message: serde_json::Value = serde_json::from_slice(&payload).unwrap();
                if message["code"] == "session_ended" {
                    sentence = message["message"].as_str().map(str::to_string);
                }
            }
            if opcode == OP_CLOSE {
                assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1001);
                closed = true;
                break;
            }
        }
        assert!(closed);
        assert_eq!(
            sentence.as_deref(),
            Some(say(Lang::Es, Say::WsSessionEnded))
        );
    }
}
