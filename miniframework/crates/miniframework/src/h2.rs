//! HTTP/2 (RFC 9113) for the connection loop, feature `http2`.
//!
//! What it buys on a board: one connection (one TLS handshake, one TLS
//! session's memory) carries every request a page makes, instead of a browser
//! opening up to six HTTP/1.1 connections and paying a ~1 s handshake each.
//! Throughput is unchanged: the link, not the protocol, is the limit.
//!
//! Browsers use HTTP/2 only over TLS, chosen by ALPN during the handshake.
//! Cleartext "prior knowledge" HTTP/2 (h2c, which starts with the client
//! preface) is also accepted, for tests and tools (`curl
//! --http2-prior-knowledge`).
//!
//! Bounded like the rest of the loop: frames are at most 16 KiB (larger is a
//! connection error), one read buffer per connection, a fixed number of
//! concurrent streams, request bodies up to `Config::body_limit` (larger gets
//! 413). Header blocks are decoded with `fluke-hpack` (4 KiB table);
//! responses are encoded without a dynamic table, so no peer setting can make
//! the encoder misbehave. Not supported: server push (never sent), priority
//! (ignored, streams are served round-robin).
//!
//! Uploads to a `Service::streamed_body` route stream as they do over
//! HTTP/1.1: the request is dispatched when its headers arrive (it needs a
//! `content-length`), and the handler's reader keeps the connection going
//! (frames, pings, settings) while it waits for data. Each stream may get at
//! most 16 KiB ahead of the handler (the receive window we advertise), so an
//! upload holds at most that much memory.
use crate::http::{self, Body, Response, TURN_BUDGET, WRITE_CHUNK_LIMIT};
use crate::incidents::{self, Kind};
use crate::mux::{Conn, Limits, STREAM_STALL};
use crate::site::{Scratch, Service, Site};
use std::io::{self, Read};
use std::time::{Duration, Instant};

/// What a client sends first.
pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
/// Largest frame payload either side sends (the protocol's default; we never
/// raise it).
pub const MAX_FRAME: usize = 16_384;
/// Header blocks (HEADERS + CONTINUATION) larger than this are refused.
const MAX_HEADER_BLOCK: usize = 16 * 1024;
/// Bodiless requests (GET, OPTIONS, HEAD...) beyond the advertised
/// MAX_CONCURRENT_STREAMS that wait, as headers only, for a stream to
/// finish. Browsers send their first burst before our SETTINGS arrive,
/// assuming 100 streams; a page's startup burst (a Mastodon client's,
/// with a CORS preflight per request) easily passes 8. Refusing those
/// streams surfaces in the browser as network (CORS) errors. Answers are
/// still produced at most `max_streams` at a time, so response memory is
/// bounded as before.
const QUEUED_STREAMS: usize = 32;
const HPACK_TABLE: usize = 4096;
const DEFAULT_WINDOW: i64 = 65_535;
/// The receive window we advertise per stream: how far a client may send
/// ahead of what the app has consumed.
const RECV_WINDOW: u32 = 16_384;
const MAX_WINDOW: i64 = (1 << 31) - 1;

mod kind {
    pub const DATA: u8 = 0x0;
    pub const HEADERS: u8 = 0x1;
    pub const PRIORITY: u8 = 0x2;
    pub const RST_STREAM: u8 = 0x3;
    pub const SETTINGS: u8 = 0x4;
    pub const PUSH_PROMISE: u8 = 0x5;
    pub const PING: u8 = 0x6;
    pub const GOAWAY: u8 = 0x7;
    pub const WINDOW_UPDATE: u8 = 0x8;
    pub const CONTINUATION: u8 = 0x9;
}

mod flag {
    pub const END_STREAM: u8 = 0x1;
    pub const ACK: u8 = 0x1;
    pub const END_HEADERS: u8 = 0x4;
    pub const PADDED: u8 = 0x8;
    pub const PRIORITY: u8 = 0x20;
}

/// Error codes (RFC 9113 section 7).
pub mod code {
    pub const NO_ERROR: u32 = 0x0;
    pub const PROTOCOL_ERROR: u32 = 0x1;
    pub const FLOW_CONTROL_ERROR: u32 = 0x3;
    pub const STREAM_CLOSED: u32 = 0x5;
    pub const FRAME_SIZE_ERROR: u32 = 0x6;
    pub const REFUSED_STREAM: u32 = 0x7;
    pub const COMPRESSION_ERROR: u32 = 0x9;
}

/// Response headers HTTP/2 forbids (connection-specific).
const HOP_BY_HOP: [&str; 5] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
];

struct Stream {
    id: u32,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// The body passed `body_limit`; the rest is dropped and the answer is 413.
    too_large: bool,
    /// END_STREAM received: the request is complete.
    complete: bool,
    /// A streamed upload: `body` is a small buffer the handler drains, and
    /// the stream's window is returned only as it does.
    streamed: Option<usize>,
    response: Option<Outgoing>,
    send_window: i64,
}

struct Outgoing {
    body: Body,
    /// Bytes of an in-memory body already framed.
    offset: usize,
}

impl Outgoing {
    fn remaining(&self) -> bool {
        match &self.body {
            Body::Stream(stream) => !stream.pending().is_empty(),
            body => self.offset < body.bytes().len(),
        }
    }

    fn chunk(&self, max: usize) -> &[u8] {
        let rest = match &self.body {
            Body::Stream(stream) => stream.pending(),
            body => &body.bytes()[self.offset..],
        };
        &rest[..rest.len().min(max)]
    }

    fn consume(&mut self, n: usize) {
        match &mut self.body {
            Body::Stream(stream) => stream.consume(n),
            _ => self.offset += n,
        }
    }

    fn failed(&self) -> bool {
        matches!(&self.body, Body::Stream(s) if s.failed())
    }
}

/// One HTTP/2 connection's state.
pub struct Connection {
    decoder: fluke_hpack::Decoder<'static>,
    input: Vec<u8>,
    used: usize,
    preface: bool,
    out: Vec<u8>,
    out_sent: usize,
    streams: Vec<Stream>,
    max_streams: usize,
    last_stream: u32,
    send_window: i64,
    peer_initial_window: i64,
    peer_max_frame: usize,
    /// A header block still arriving in CONTINUATION frames.
    continuation: Option<(u32, Vec<u8>, bool)>,
    /// GOAWAY sent or received: finish what is open, then close.
    closing: bool,
    active: Instant,
}

/// Why a connection must end, with the GOAWAY code to send.
struct Fatal(u32);

impl Connection {
    /// A connection whose first `already` bytes have been read (an h2c
    /// client's preface, say). Queues our SETTINGS.
    pub fn new(already: &[u8], max_streams: usize) -> Self {
        let mut decoder = fluke_hpack::Decoder::new();
        decoder.set_max_allowed_table_size(HPACK_TABLE);
        let mut input = vec![0; 9 + MAX_FRAME];
        let used = already.len().min(input.len());
        input[..used].copy_from_slice(&already[..used]);
        let mut connection = Self {
            decoder,
            input,
            used,
            preface: false,
            out: Vec::with_capacity(1024),
            out_sent: 0,
            streams: Vec::new(),
            max_streams: max_streams.max(1),
            last_stream: 0,
            send_window: DEFAULT_WINDOW,
            peer_initial_window: DEFAULT_WINDOW,
            peer_max_frame: MAX_FRAME,
            continuation: None,
            closing: false,
            active: Instant::now(),
        };
        let mut settings = Vec::new();
        for (id, value) in [
            (0x1u16, HPACK_TABLE as u32),     // HEADER_TABLE_SIZE
            (0x2, 0), // ENABLE_PUSH (meaningless from a server, but explicit)
            (0x3, max_streams.max(1) as u32), // MAX_CONCURRENT_STREAMS
            (0x4, RECV_WINDOW), // INITIAL_WINDOW_SIZE
            (0x5, MAX_FRAME as u32), // MAX_FRAME_SIZE
        ] {
            settings.extend_from_slice(&id.to_be_bytes());
            settings.extend_from_slice(&value.to_be_bytes());
        }
        connection.frame(kind::SETTINGS, 0, 0, &settings);
        connection
    }

    /// True when the bytes so far could be (the start of) the client preface.
    pub fn looks_like_preface(input: &[u8]) -> bool {
        let n = input.len().min(PREFACE.len());
        n > 0 && input[..n] == PREFACE[..n]
    }

    /// Bytes held for unsent output (for the loop's response budget).
    pub fn retained_bytes(&self) -> usize {
        self.out.len()
            + self
                .streams
                .iter()
                .filter_map(|s| s.response.as_ref())
                .map(|o| match &o.body {
                    Body::Owned(bytes) => bytes.len(),
                    _ => 0,
                })
                .sum::<usize>()
    }

    /// Idle: nothing in flight for a while.
    pub fn evictable(&self) -> bool {
        self.streams.is_empty()
            && self.out_sent >= self.out.len()
            && self.active.elapsed() > Duration::from_millis(500)
    }

    fn frame(&mut self, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
        let len = payload.len() as u32;
        self.out.extend_from_slice(&len.to_be_bytes()[1..]);
        self.out.push(kind);
        self.out.push(flags);
        self.out
            .extend_from_slice(&(stream & 0x7fff_ffff).to_be_bytes());
        self.out.extend_from_slice(payload);
    }

    fn goaway(&mut self, error: u32) {
        let mut payload = self.last_stream.to_be_bytes().to_vec();
        payload.extend_from_slice(&error.to_be_bytes());
        self.frame(kind::GOAWAY, 0, 0, &payload);
        self.closing = true;
    }

    fn reset(&mut self, stream: u32, error: u32) {
        self.frame(kind::RST_STREAM, 0, stream, &error.to_be_bytes());
        self.streams.retain(|s| s.id != stream);
    }

    /// One bounded turn: write, read, handle frames, answer complete
    /// requests, frame response data. Returns (keep, made progress).
    pub fn poll<C: Conn, S: Service>(
        &mut self,
        conn: &mut C,
        site: &Site<S>,
        scratch: &mut Scratch,
        limits: &Limits,
        body_limit: usize,
        may_dispatch: bool,
    ) -> (bool, bool) {
        let mut progress = false;
        match self.flush(conn) {
            Ok(moved) => progress |= moved,
            Err(_) => return (false, true),
        }
        // Read only with room for a whole frame; a peer that floods us is
        // held back by TCP, not by our memory.
        if self.used < self.input.len() {
            match conn.read(&mut self.input[self.used..]) {
                Ok(0) => return (false, true),
                Ok(n) => {
                    self.used += n;
                    self.active = Instant::now();
                    progress = true;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => return (false, true),
            }
        }
        let stream_limit = |method: &str, path: &str| site.stream_limit(method, path);
        if let Err(Fatal(error)) = self.frames(body_limit, &stream_limit) {
            incidents::record(Kind::H2GoAway, error as i32);
            self.goaway(error);
        }
        if may_dispatch && !self.closing {
            progress |= self.dispatch(conn, site, scratch, body_limit);
        }
        progress |= self.produce();
        match self.flush(conn) {
            Ok(moved) => progress |= moved,
            Err(_) => return (false, true),
        }
        let flushed = self.out_sent >= self.out.len();
        if flushed {
            self.out.clear();
            self.out_sent = 0;
        }
        if self.closing && flushed && self.streams.iter().all(|s| s.response.is_none()) {
            return (false, progress);
        }
        if self.streams.is_empty() && flushed && self.active.elapsed() > limits.idle {
            self.goaway(code::NO_ERROR);
            let _ = self.flush(conn);
            return (false, progress);
        }
        if progress {
            self.active = Instant::now();
        }
        (true, progress)
    }

    /// Writes queued frames, at most [`TURN_BUDGET`] per turn, in
    /// [`WRITE_CHUNK_LIMIT`] pieces; a WouldBlock retries the same bytes.
    fn flush<C: Conn>(&mut self, conn: &mut C) -> io::Result<bool> {
        let mut budget = TURN_BUDGET;
        let mut moved = false;
        while budget > 0 && self.out_sent < self.out.len() {
            let end = self.out.len().min(self.out_sent + WRITE_CHUNK_LIMIT);
            match conn.write(&self.out[self.out_sent..end]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    self.out_sent += n;
                    budget = budget.saturating_sub(n);
                    moved = true;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    break
                }
                Err(e) => return Err(e),
            }
        }
        Ok(moved)
    }

    /// Handles every complete frame in the input buffer.
    fn frames(
        &mut self,
        body_limit: usize,
        stream_limit: &dyn Fn(&str, &str) -> Option<usize>,
    ) -> Result<(), Fatal> {
        let mut at = 0;
        if !self.preface {
            // Over TLS the connection starts before the client has sent a
            // byte: nothing yet is not a bad preface.
            if self.used < PREFACE.len() {
                return if self.used == 0 || Self::looks_like_preface(&self.input[..self.used]) {
                    Ok(())
                } else {
                    Err(Fatal(code::PROTOCOL_ERROR))
                };
            }
            if &self.input[..PREFACE.len()] != PREFACE {
                return Err(Fatal(code::PROTOCOL_ERROR));
            }
            self.preface = true;
            at = PREFACE.len();
        }
        let mut result = Ok(());
        while self.used - at >= 9 {
            let head = &self.input[at..at + 9];
            let len = u32::from_be_bytes([0, head[0], head[1], head[2]]) as usize;
            if len > MAX_FRAME {
                result = Err(Fatal(code::FRAME_SIZE_ERROR));
                break;
            }
            if self.used - at < 9 + len {
                break;
            }
            let kind = head[3];
            let flags = head[4];
            let stream = u32::from_be_bytes([head[5], head[6], head[7], head[8]]) & 0x7fff_ffff;
            let payload = self.input[at + 9..at + 9 + len].to_vec();
            at += 9 + len;
            if let Err(fatal) =
                self.frame_in(kind, flags, stream, &payload, body_limit, stream_limit)
            {
                result = Err(fatal);
                break;
            }
        }
        self.input.copy_within(at..self.used, 0);
        self.used -= at;
        result
    }

    fn frame_in(
        &mut self,
        kind: u8,
        flags: u8,
        stream: u32,
        payload: &[u8],
        body_limit: usize,
        stream_limit: &dyn Fn(&str, &str) -> Option<usize>,
    ) -> Result<(), Fatal> {
        // Nothing may interleave with a header block in progress.
        if let Some((id, _, _)) = &self.continuation {
            if kind != kind::CONTINUATION || stream != *id {
                return Err(Fatal(code::PROTOCOL_ERROR));
            }
        }
        match kind {
            kind::SETTINGS => {
                if stream != 0 || (flags & flag::ACK != 0 && !payload.is_empty()) {
                    return Err(Fatal(code::PROTOCOL_ERROR));
                }
                if flags & flag::ACK != 0 {
                    return Ok(());
                }
                if !payload.len().is_multiple_of(6) {
                    return Err(Fatal(code::FRAME_SIZE_ERROR));
                }
                for entry in payload.chunks(6) {
                    let id = u16::from_be_bytes([entry[0], entry[1]]);
                    let value = u32::from_be_bytes([entry[2], entry[3], entry[4], entry[5]]);
                    match id {
                        0x4 => {
                            if i64::from(value) > MAX_WINDOW {
                                return Err(Fatal(code::FLOW_CONTROL_ERROR));
                            }
                            let delta = i64::from(value) - self.peer_initial_window;
                            self.peer_initial_window = i64::from(value);
                            for s in &mut self.streams {
                                s.send_window += delta;
                            }
                        }
                        0x5 => {
                            if !(16_384..=16_777_215).contains(&value) {
                                return Err(Fatal(code::PROTOCOL_ERROR));
                            }
                            self.peer_max_frame = (value as usize).min(MAX_FRAME);
                        }
                        // Our responses never use the dynamic table, so the
                        // peer's HEADER_TABLE_SIZE needs no action.
                        _ => {}
                    }
                }
                self.frame(kind::SETTINGS, flag::ACK, 0, &[]);
            }
            kind::PING => {
                if stream != 0 || payload.len() != 8 {
                    return Err(Fatal(code::PROTOCOL_ERROR));
                }
                if flags & flag::ACK == 0 {
                    self.frame(kind::PING, flag::ACK, 0, payload);
                }
            }
            kind::WINDOW_UPDATE => {
                if payload.len() != 4 {
                    return Err(Fatal(code::FRAME_SIZE_ERROR));
                }
                let increment = i64::from(
                    u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]])
                        & 0x7fff_ffff,
                );
                if stream == 0 {
                    if increment == 0 {
                        return Err(Fatal(code::PROTOCOL_ERROR));
                    }
                    self.send_window += increment;
                    if self.send_window > MAX_WINDOW {
                        return Err(Fatal(code::FLOW_CONTROL_ERROR));
                    }
                } else if let Some(s) = self.streams.iter_mut().find(|s| s.id == stream) {
                    s.send_window += increment;
                    if increment == 0 || s.send_window > MAX_WINDOW {
                        let error = if increment == 0 {
                            code::PROTOCOL_ERROR
                        } else {
                            code::FLOW_CONTROL_ERROR
                        };
                        self.reset(stream, error);
                    }
                }
            }
            kind::HEADERS => {
                if stream == 0 || stream.is_multiple_of(2) {
                    return Err(Fatal(code::PROTOCOL_ERROR));
                }
                let mut block = strip_padding(flags, payload)?;
                if flags & flag::PRIORITY != 0 {
                    if block.len() < 5 {
                        return Err(Fatal(code::PROTOCOL_ERROR));
                    }
                    block = &block[5..];
                }
                let end_stream = flags & flag::END_STREAM != 0;
                if flags & flag::END_HEADERS == 0 {
                    self.continuation = Some((stream, block.to_vec(), end_stream));
                } else {
                    let block = block.to_vec();
                    self.header_block(stream, &block, end_stream, body_limit, stream_limit)?;
                }
            }
            kind::CONTINUATION => {
                let Some((id, mut block, end_stream)) = self.continuation.take() else {
                    return Err(Fatal(code::PROTOCOL_ERROR));
                };
                block.extend_from_slice(payload);
                if block.len() > MAX_HEADER_BLOCK {
                    return Err(Fatal(code::PROTOCOL_ERROR));
                }
                if flags & flag::END_HEADERS == 0 {
                    self.continuation = Some((id, block, end_stream));
                } else {
                    self.header_block(id, &block, end_stream, body_limit, stream_limit)?;
                }
            }
            kind::DATA => {
                if stream == 0 {
                    return Err(Fatal(code::PROTOCOL_ERROR));
                }
                let data = strip_padding(flags, payload)?;
                // We consume everything at once: give the window straight back.
                if !payload.is_empty() {
                    self.frame(
                        kind::WINDOW_UPDATE,
                        0,
                        0,
                        &(payload.len() as u32).to_be_bytes(),
                    );
                }
                let end_stream = flags & flag::END_STREAM != 0;
                let Some(s) = self
                    .streams
                    .iter_mut()
                    .find(|s| s.id == stream && !s.complete)
                else {
                    if stream > self.last_stream {
                        return Err(Fatal(code::PROTOCOL_ERROR));
                    }
                    // A stream we already answered or refused.
                    return Ok(());
                };
                s.complete |= end_stream;
                if let Some(remaining) = s.streamed.as_mut() {
                    // An upload: buffered until the handler reads it, which
                    // returns the window. More than declared is an error.
                    if data.len() > *remaining {
                        self.reset(stream, code::PROTOCOL_ERROR);
                        return Ok(());
                    }
                    // A client that ignores our window is cut off rather
                    // than buffered (64 KiB: what it may send before it
                    // has seen our SETTINGS).
                    if s.body.len() + data.len() > DEFAULT_WINDOW as usize {
                        self.reset(stream, code::FLOW_CONTROL_ERROR);
                        return Ok(());
                    }
                    *remaining -= data.len();
                    s.body.extend_from_slice(data);
                    let padding = payload.len() - data.len();
                    if padding > 0 && !end_stream {
                        self.frame(
                            kind::WINDOW_UPDATE,
                            0,
                            stream,
                            &(padding as u32).to_be_bytes(),
                        );
                    }
                    return Ok(());
                }
                if s.body.len() + data.len() > body_limit {
                    s.too_large = true;
                    s.body = Vec::new();
                } else if !s.too_large {
                    s.body.extend_from_slice(data);
                }
                if !end_stream && !payload.is_empty() {
                    self.frame(
                        kind::WINDOW_UPDATE,
                        0,
                        stream,
                        &(payload.len() as u32).to_be_bytes(),
                    );
                }
            }
            kind::RST_STREAM => {
                if stream == 0 || payload.len() != 4 {
                    return Err(Fatal(code::PROTOCOL_ERROR));
                }
                self.streams.retain(|s| s.id != stream);
            }
            kind::GOAWAY => {
                if stream != 0 {
                    return Err(Fatal(code::PROTOCOL_ERROR));
                }
                let error = payload
                    .get(4..8)
                    .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                incidents::record(Kind::H2PeerGoAway, error as i32);
                self.closing = true;
            }
            kind::PUSH_PROMISE => return Err(Fatal(code::PROTOCOL_ERROR)),
            kind::PRIORITY if stream == 0 || payload.len() != 5 => {
                return Err(Fatal(code::PROTOCOL_ERROR));
            }
            // Unknown frame types are ignored (RFC 9113 section 5.5).
            _ => {}
        }
        Ok(())
    }

    /// Decodes a complete header block (always, to keep the HPACK table in
    /// step) and opens the stream, or refuses it.
    fn header_block(
        &mut self,
        stream: u32,
        block: &[u8],
        end_stream: bool,
        body_limit: usize,
        stream_limit: &dyn Fn(&str, &str) -> Option<usize>,
    ) -> Result<(), Fatal> {
        let mut headers = Vec::new();
        let mut size = 0usize;
        let decoded = self.decoder.decode_with_cb(block, |name, value| {
            size += name.len() + value.len() + 32;
            headers.push((
                String::from_utf8_lossy(&name).into_owned(),
                String::from_utf8_lossy(&value).into_owned(),
            ));
        });
        if decoded.is_err() {
            return Err(Fatal(code::COMPRESSION_ERROR));
        }
        if let Some(open) = self.streams.iter_mut().find(|s| s.id == stream) {
            // Trailers: ignored, but they end the request.
            open.complete |= end_stream;
            return Ok(());
        }
        if stream <= self.last_stream {
            return Err(Fatal(code::PROTOCOL_ERROR));
        }
        self.last_stream = stream;
        if self.closing {
            return Ok(());
        }
        let limit = if end_stream {
            self.max_streams + QUEUED_STREAMS
        } else {
            self.max_streams
        };
        if self.streams.len() >= limit {
            incidents::record(Kind::H2Refused, 0);
            self.reset(stream, code::REFUSED_STREAM);
            return Ok(());
        }
        if size > MAX_HEADER_BLOCK {
            incidents::record(Kind::H2Refused, 1);
            self.reset(stream, code::REFUSED_STREAM);
            return Ok(());
        }
        // A large upload to a streaming route is read by its handler as it
        // arrives; anything else larger than `body_limit` will be a 413.
        let field = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        let length = field("content-length").and_then(|v| v.parse::<usize>().ok());
        let mut too_large = false;
        let mut streamed = None;
        if let (false, Some(length)) = (end_stream, length.filter(|&n| n > body_limit)) {
            match stream_limit(field(":method").unwrap_or(""), field(":path").unwrap_or("")) {
                Some(max) if length <= max => streamed = Some(length),
                _ => too_large = true,
            }
        }
        self.streams.push(Stream {
            id: stream,
            headers,
            body: Vec::new(),
            too_large,
            complete: end_stream,
            streamed,
            response: None,
            send_window: self.peer_initial_window,
        });
        Ok(())
    }

    /// Answers every complete request that has no answer yet, and starts
    /// streamed uploads as soon as their headers are in.
    fn dispatch<C: Conn, S: Service>(
        &mut self,
        conn: &mut C,
        site: &Site<S>,
        scratch: &mut Scratch,
        body_limit: usize,
    ) -> bool {
        let secure = conn.secure();
        let mut progress = false;
        // Dispatching takes a stream's headers; at most `max_streams`
        // answers are in progress, the rest wait in arrival order.
        let active = self.streams.iter().filter(|s| s.headers.is_empty()).count();
        let ready: Vec<u32> = self
            .streams
            .iter()
            .filter(|s| {
                s.response.is_none()
                    && !s.headers.is_empty()
                    && (s.complete || s.streamed.is_some())
            })
            .map(|s| s.id)
            .take(self.max_streams.saturating_sub(active))
            .collect();
        for id in ready {
            let Some(index) = self.streams.iter().position(|s| s.id == id) else {
                continue;
            };
            let s = &mut self.streams[index];
            let mut method = String::new();
            let mut path = String::new();
            let mut authority = String::new();
            let mut scheme = false;
            let mut regular = Vec::new();
            let mut malformed = false;
            for (name, value) in std::mem::take(&mut s.headers) {
                match name.as_str() {
                    ":method" => method = value,
                    ":path" => path = value,
                    ":authority" => authority = value,
                    ":scheme" => scheme = true,
                    n if n.starts_with(':') => malformed = true,
                    "cookie" => match regular
                        .iter_mut()
                        .find(|(k, _): &&mut (String, String)| k == "cookie")
                    {
                        Some((_, v)) => {
                            v.push_str("; ");
                            v.push_str(&value);
                        }
                        None => regular.push((name, value)),
                    },
                    n if HOP_BY_HOP.contains(&n) || n == "te" && value != "trailers" => {
                        malformed = true
                    }
                    _ => regular.push((name, value)),
                }
            }
            if malformed || method.is_empty() || !scheme || !path.starts_with('/') {
                self.reset(id, code::PROTOCOL_ERROR);
                progress = true;
                continue;
            }
            if !authority.is_empty() && !regular.iter().any(|(k, _)| k == "host") {
                regular.push(("host".into(), authority));
            }
            let response = if s.too_large {
                let origin = regular
                    .iter()
                    .find(|(k, _)| k == "origin")
                    .map_or("", |(_, v)| v.as_str());
                let host = regular
                    .iter()
                    .find(|(k, _)| k == "host")
                    .map_or("", |(_, v)| v.as_str());
                let head = format!("GET / HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\n\r\n");
                site.refuse(413, head.as_bytes())
            } else if let Some(length) = s.streamed {
                let request = http::Request {
                    method,
                    uri: path,
                    headers: regular,
                    body: Vec::new(),
                    consumed: 0,
                    close: false,
                    streamed: Some(length),
                };
                let stream_limit = |method: &str, path: &str| site.stream_limit(method, path);
                let mut body = Upload {
                    h2: self,
                    conn: &mut *conn,
                    id,
                    remaining: length,
                    body_limit,
                    stream_limit: &stream_limit,
                    last_progress: Instant::now(),
                };
                let response = site.respond_with(&request, secure, scratch, Some(&mut body));
                // Read what the handler left, as HTTP/1.1 does: the length
                // is bounded by the route.
                let mut sink = [0u8; 1024];
                let drained = loop {
                    match body.read(&mut sink) {
                        Ok(0) => break true,
                        Ok(_) => {}
                        Err(_) => break false,
                    }
                };
                let _ = self.flush(conn);
                if drained {
                    response
                } else {
                    let head = format!("PUT / HTTP/1.1\r\nHost: {}\r\n\r\n", "");
                    site.refuse(408, head.as_bytes())
                }
            } else {
                let request = http::Request {
                    method,
                    uri: path,
                    headers: regular,
                    body: std::mem::take(&mut s.body),
                    consumed: 0,
                    close: false,
                    streamed: None,
                };
                site.respond(&request, secure, scratch)
            };
            // The stream may have been reset while its upload was read.
            if let Some(index) = self.streams.iter().position(|s| s.id == id) {
                self.respond(index, response);
            }
            progress = true;
        }
        progress
    }

    fn respond(&mut self, index: usize, response: Response) {
        let id = self.streams[index].id;
        let (status, headers, length, body) = response.into_parts();
        let mut block = Vec::with_capacity(256);
        encode_status(&mut block, status);
        for (name, value) in &headers {
            let name = name.to_ascii_lowercase();
            if HOP_BY_HOP.contains(&name.as_str()) || name == "content-length" {
                continue;
            }
            encode_literal(&mut block, name.as_bytes(), value.as_bytes());
        }
        if let Some(length) = length {
            encode_literal(&mut block, b"content-length", length.to_string().as_bytes());
        }
        let outgoing = Outgoing { body, offset: 0 };
        let end_stream = !outgoing.remaining();
        // HEADERS, then CONTINUATION if the block exceeds a frame.
        let max = self.peer_max_frame;
        let mut chunks = block.chunks(max).peekable();
        let first = chunks.next().unwrap_or(&[]);
        let mut flags = if end_stream { flag::END_STREAM } else { 0 };
        if chunks.peek().is_none() {
            flags |= flag::END_HEADERS;
        }
        self.frame(kind::HEADERS, flags, id, first);
        while let Some(chunk) = chunks.next() {
            let last = chunks.peek().is_none();
            self.frame(
                kind::CONTINUATION,
                if last { flag::END_HEADERS } else { 0 },
                id,
                chunk,
            );
        }
        if end_stream {
            self.streams.remove(index);
        } else {
            self.streams[index].response = Some(outgoing);
        }
    }

    /// Frames response data, round-robin across streams, within the flow
    /// control windows and this turn's budget. Generates only what can be
    /// written soon, so unsent frames never pile up in memory.
    fn produce(&mut self) -> bool {
        let mut progress = false;
        let mut budget = TURN_BUDGET;
        loop {
            let mut moved = false;
            let mut finished = Vec::new();
            for i in 0..self.streams.len() {
                if budget == 0
                    || self.send_window <= 0
                    || self.out.len() - self.out_sent > 2 * MAX_FRAME
                {
                    break;
                }
                let s = &self.streams[i];
                let Some(out) = &s.response else { continue };
                if out.failed() {
                    finished.push((s.id, true));
                    continue;
                }
                let room = (s.send_window.min(self.send_window).max(0) as usize)
                    .min(self.peer_max_frame)
                    .min(budget);
                if room == 0 {
                    continue;
                }
                let chunk = out.chunk(room).to_vec();
                let n = chunk.len();
                let mut out_state = self.streams[i].response.take().unwrap();
                out_state.consume(n);
                let end = !out_state.remaining() && !out_state.failed();
                self.frame(
                    kind::DATA,
                    if end { flag::END_STREAM } else { 0 },
                    self.streams[i].id,
                    &chunk,
                );
                self.streams[i].send_window -= n as i64;
                self.send_window -= n as i64;
                budget -= n.min(budget);
                moved = true;
                if end {
                    finished.push((self.streams[i].id, false));
                } else {
                    self.streams[i].response = Some(out_state);
                }
            }
            for (id, failed) in finished {
                if failed {
                    // The body's source ended early: tell the client the
                    // stream is incomplete rather than ending it cleanly.
                    self.reset(id, code::STREAM_CLOSED);
                } else {
                    self.streams.retain(|s| s.id != id);
                }
                moved = true;
            }
            progress |= moved;
            if !moved {
                return progress;
            }
        }
    }
}

fn strip_padding(flags: u8, payload: &[u8]) -> Result<&[u8], Fatal> {
    if flags & flag::PADDED == 0 {
        return Ok(payload);
    }
    let Some((&pad, rest)) = payload.split_first() else {
        return Err(Fatal(code::PROTOCOL_ERROR));
    };
    if usize::from(pad) > rest.len() {
        return Err(Fatal(code::PROTOCOL_ERROR));
    }
    Ok(&rest[..rest.len() - usize::from(pad)])
}

/// HPACK integer with an `n`-bit prefix (RFC 7541 section 5.1).
fn encode_int(out: &mut Vec<u8>, first: u8, prefix_bits: u8, value: usize) {
    let max = (1usize << prefix_bits) - 1;
    if value < max {
        out.push(first | value as u8);
        return;
    }
    out.push(first | max as u8);
    let mut rest = value - max;
    while rest >= 128 {
        out.push((rest % 128) as u8 | 0x80);
        rest /= 128;
    }
    out.push(rest as u8);
}

/// `:status`, indexed from the static table when it is there.
fn encode_status(out: &mut Vec<u8>, status: u16) {
    let index = match status {
        200 => 8,
        204 => 9,
        206 => 10,
        304 => 11,
        400 => 12,
        404 => 13,
        500 => 14,
        _ => 0,
    };
    if index > 0 {
        encode_int(out, 0x80, 7, index);
    } else {
        // Literal without indexing, name from static entry 8 (":status").
        encode_int(out, 0x00, 4, 8);
        let text = status.to_string();
        encode_int(out, 0x00, 7, text.len());
        out.extend_from_slice(text.as_bytes());
    }
}

/// Literal header field without indexing, new name, no Huffman.
fn encode_literal(out: &mut Vec<u8>, name: &[u8], value: &[u8]) {
    out.push(0x00);
    encode_int(out, 0x00, 7, name.len());
    out.extend_from_slice(name);
    encode_int(out, 0x00, 7, value.len());
    out.extend_from_slice(value);
}

/// A streamed upload's body: the stream's buffered DATA, then more from the
/// connection, which keeps being served (frames, pings, settings) while the
/// handler waits. Returns each chunk's window to the client as it is read.
struct Upload<'a, C: Conn> {
    h2: &'a mut Connection,
    conn: &'a mut C,
    id: u32,
    remaining: usize,
    body_limit: usize,
    stream_limit: &'a dyn Fn(&str, &str) -> Option<usize>,
    last_progress: Instant,
}

impl<C: Conn> Read for Upload<'_, C> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 || out.is_empty() {
            return Ok(0);
        }
        loop {
            let Some(s) = self.h2.streams.iter_mut().find(|s| s.id == self.id) else {
                return Err(io::ErrorKind::ConnectionReset.into());
            };
            if !s.body.is_empty() {
                let n = out.len().min(s.body.len()).min(self.remaining);
                out[..n].copy_from_slice(&s.body[..n]);
                s.body.drain(..n);
                let complete = s.complete;
                self.remaining -= n;
                self.last_progress = Instant::now();
                if !complete {
                    self.h2
                        .frame(kind::WINDOW_UPDATE, 0, self.id, &(n as u32).to_be_bytes());
                }
                return Ok(n);
            }
            if s.complete {
                // Ended before its declared length.
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.h2.flush(self.conn)?;
            let used = self.h2.used;
            if used < self.h2.input.len() {
                match self.conn.read(&mut self.h2.input[used..]) {
                    Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                    Ok(n) => {
                        self.h2.used += n;
                        self.last_progress = Instant::now();
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        if self.last_progress.elapsed() >= STREAM_STALL {
                            return Err(io::ErrorKind::TimedOut.into());
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => return Err(e),
                }
            }
            if let Err(Fatal(error)) = self.h2.frames(self.body_limit, self.stream_limit) {
                incidents::record(Kind::H2GoAway, error as i32);
                self.h2.goaway(error);
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
        }
    }
}

#[cfg(test)]
mod tests;
