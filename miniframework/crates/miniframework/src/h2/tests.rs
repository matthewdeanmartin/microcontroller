//! HTTP/2 against an independent client: requests are encoded and responses
//! decoded with `fluke-hpack`, frames are parsed here from the bytes.
use super::*;
use crate::desktop::DesktopPlatform;
use crate::site::{Config, Reply, Request};
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Pipe {
    input: Arc<Mutex<Vec<u8>>>,
    output: Arc<Mutex<Vec<u8>>>,
}

impl Read for Pipe {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut input = self.input.lock().unwrap();
        if input.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = out.len().min(input.len());
        out[..n].copy_from_slice(&input[..n]);
        input.drain(..n);
        Ok(n)
    }
}

impl Write for Pipe {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.output.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Conn for Pipe {
    fn secure(&self) -> bool {
        true
    }
}

struct App;
impl Service for App {
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        match req.path {
            "/api/hello" => {
                reply.text(200, "text/plain", &format!("hello {}", req.query));
                reply.header("X-Test", "yes");
            }
            "/api/big" => {
                let n: usize = req.query.parse().unwrap_or(100_000);
                reply.bytes(200, "application/octet-stream", &vec![b'z'; n]);
            }
            "/api/file" => {
                let data: Vec<u8> = (0..50_000u32).map(|i| i as u8).collect();
                reply.stream(
                    200,
                    "image/png",
                    50_000,
                    Box::new(std::io::Cursor::new(data)),
                );
            }
            "/api/echo" => {
                reply.bytes(201, "text/plain", req.body);
            }
            "/api/teapot" => reply.text(418, "text/plain", "short and stout"),
            "/api/upload" => {
                let mut bytes = Vec::new();
                match req.body_reader().read_to_end(&mut bytes) {
                    Ok(_) => {
                        let sum: u64 = bytes.iter().map(|&b| u64::from(b)).sum();
                        reply.text(201, "text/plain", &format!("{} {sum}", bytes.len()));
                    }
                    Err(e) => reply.text(400, "text/plain", &format!("{e}")),
                }
            }
            "/api/refuse-upload" => reply.text(413, "text/plain", "no room"),
            _ => {}
        }
    }
    fn streamed_body(&self, method: &str, path: &str) -> Option<usize> {
        (method == "PUT" && path.starts_with("/api/")).then_some(200_000)
    }
}

fn site() -> Site<App> {
    let mut config = Config::new("test", "board.local");
    config.response_limit = 256 * 1024;
    Site::new(config, App, DesktopPlatform)
}

fn limits() -> Limits {
    Limits {
        tls_clients: 4,
        http_clients: 4,
        h2_streams: 8,
        handshakes: 1,
        response_budget: 1 << 20,
        idle: Duration::from_secs(30),
        request_deadline: Duration::from_secs(5),
    }
}

fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_be_bytes()[1..].to_vec();
    out.push(kind);
    out.push(flags);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

#[derive(Debug)]
struct Frame {
    kind: u8,
    flags: u8,
    stream: u32,
    payload: Vec<u8>,
}

fn frames(mut bytes: &[u8]) -> Vec<Frame> {
    let mut found = Vec::new();
    while bytes.len() >= 9 {
        let len = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]) as usize;
        found.push(Frame {
            kind: bytes[3],
            flags: bytes[4],
            stream: u32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]),
            payload: bytes[9..9 + len].to_vec(),
        });
        bytes = &bytes[9 + len..];
    }
    assert!(bytes.is_empty(), "a partial frame was written");
    found
}

/// The client side of one connection.
struct Client {
    pipe: Pipe,
    server: Connection,
    site: Site<App>,
    scratch: Scratch,
    encoder: fluke_hpack::Encoder<'static>,
    decoder: fluke_hpack::Decoder<'static>,
    /// Bytes received and not yet parsed into frames.
    received: Vec<u8>,
}

#[derive(Debug, Default)]
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    ended: bool,
    reset: Option<u32>,
}

impl Answer {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map_or("", |(_, v)| v.as_str())
    }
}

impl Client {
    fn new(max_streams: usize) -> Self {
        let pipe = Pipe::default();
        pipe.input.lock().unwrap().extend_from_slice(PREFACE);
        pipe.input
            .lock()
            .unwrap()
            .extend_from_slice(&frame(kind::SETTINGS, 0, 0, &[]));
        Self {
            pipe,
            server: Connection::new(&[], max_streams),
            site: site(),
            scratch: Scratch::new(256 * 1024),
            encoder: fluke_hpack::Encoder::new(),
            decoder: fluke_hpack::Decoder::new(),
            received: Vec::new(),
        }
    }

    fn send(&self, bytes: &[u8]) {
        self.pipe.input.lock().unwrap().extend_from_slice(bytes);
    }

    fn request(
        &mut self,
        stream: u32,
        method: &str,
        path: &str,
        extra: &[(&str, &str)],
        end: bool,
    ) {
        let mut headers: Vec<(&[u8], &[u8])> = vec![
            (b":method", method.as_bytes()),
            (b":scheme", b"https"),
            (b":authority", b"board.local"),
            (b":path", path.as_bytes()),
        ];
        headers.extend(extra.iter().map(|(k, v)| (k.as_bytes(), v.as_bytes())));
        let block = self.encoder.encode(headers);
        let flags = flag::END_HEADERS | if end { flag::END_STREAM } else { 0 };
        self.send(&frame(kind::HEADERS, flags, stream, &block));
    }

    /// Runs the server until it has nothing more to do; returns whether it
    /// kept the connection.
    fn run(&mut self) -> bool {
        for _ in 0..64 {
            let (keep, progress) = self.server.poll(
                &mut self.pipe,
                &self.site,
                &mut self.scratch,
                &limits(),
                1024,
                true,
            );
            if !keep {
                return false;
            }
            if !progress {
                break;
            }
        }
        true
    }

    /// Everything the server wrote since the last call, as frames.
    fn take(&mut self) -> Vec<Frame> {
        let bytes = std::mem::take(&mut *self.pipe.output.lock().unwrap());
        self.received.extend_from_slice(&bytes);
        let all = frames(&self.received);
        self.received.clear();
        all
    }

    /// Collects answers per stream from frames, decoding header blocks in
    /// order (as a real client must, to keep its table in step).
    fn answers(&mut self, frames: &[Frame], into: &mut std::collections::BTreeMap<u32, Answer>) {
        let mut block: Option<(u32, Vec<u8>, bool)> = None;
        for f in frames {
            match f.kind {
                kind::HEADERS | kind::CONTINUATION => {
                    let (id, mut bytes, end) = if f.kind == kind::HEADERS {
                        (f.stream, Vec::new(), f.flags & flag::END_STREAM != 0)
                    } else {
                        block.take().expect("CONTINUATION without HEADERS")
                    };
                    bytes.extend_from_slice(&f.payload);
                    if f.flags & flag::END_HEADERS == 0 {
                        block = Some((id, bytes, end));
                        continue;
                    }
                    let answer = into.entry(id).or_default();
                    for (name, value) in self.decoder.decode(&bytes).expect("valid HPACK") {
                        let (name, value) = (
                            String::from_utf8(name).unwrap(),
                            String::from_utf8(value).unwrap(),
                        );
                        if name == ":status" {
                            answer.status = value.parse().unwrap();
                        } else {
                            answer.headers.push((name, value));
                        }
                    }
                    answer.ended |= end;
                }
                kind::DATA => {
                    let answer = into.entry(f.stream).or_default();
                    answer.body.extend_from_slice(&f.payload);
                    answer.ended |= f.flags & flag::END_STREAM != 0;
                }
                kind::RST_STREAM => {
                    into.entry(f.stream).or_default().reset =
                        Some(u32::from_be_bytes(f.payload[..4].try_into().unwrap()));
                }
                _ => {}
            }
        }
    }
}

#[test]
fn settings_are_exchanged_and_pings_answered() {
    let mut c = Client::new(4);
    c.send(&frame(kind::PING, 0, 0, b"12345678"));
    assert!(c.run());
    let got = c.take();
    let settings: Vec<_> = got.iter().filter(|f| f.kind == kind::SETTINGS).collect();
    assert_eq!(settings.len(), 2, "ours, and the ACK of theirs");
    assert_eq!(settings[0].flags, 0);
    assert!(
        settings[0]
            .payload
            .chunks(6)
            .any(|e| e == [0, 3, 0, 0, 0, 4]),
        "max 4 streams"
    );
    assert_eq!(settings[1].flags, flag::ACK);
    let pong = got.iter().find(|f| f.kind == kind::PING).unwrap();
    assert_eq!(
        (pong.flags, &pong.payload[..]),
        (flag::ACK, &b"12345678"[..])
    );
}

#[test]
fn concurrent_requests_on_one_connection() {
    let mut c = Client::new(8);
    for (i, stream) in [1u32, 3, 5].into_iter().enumerate() {
        c.request(stream, "GET", &format!("/api/hello?{i}"), &[], true);
    }
    assert!(c.run());
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    for (i, stream) in [1u32, 3, 5].into_iter().enumerate() {
        let a = &answers[&stream];
        assert_eq!(a.status, 200);
        assert_eq!(a.body, format!("hello {i}").as_bytes());
        assert_eq!(a.header("x-test"), "yes");
        assert_eq!(a.header("content-type"), "text/plain");
        assert_eq!(
            a.header("content-length"),
            ("hello ".len() + i.to_string().len()).to_string()
        );
        assert!(
            a.headers.iter().all(|(k, _)| k != "connection"),
            "no HTTP/1.1 headers"
        );
        assert!(a.ended);
    }
    // Unusual statuses are encoded literally.
    c.request(7, "GET", "/api/teapot", &[], true);
    c.run();
    let frames = c.take();
    c.answers(&frames, &mut answers);
    assert_eq!(answers[&7].status, 418);
}

#[test]
fn large_responses_respect_flow_control() {
    let mut c = Client::new(4);
    c.request(1, "GET", "/api/big?150000", &[], true);
    c.run();
    let frames = c.take();
    let sent: usize = frames
        .iter()
        .filter(|f| f.kind == kind::DATA)
        .map(|f| f.payload.len())
        .sum();
    assert_eq!(sent, 65_535, "stops at the initial window");
    assert!(frames.iter().all(|f| f.payload.len() <= MAX_FRAME));
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    // Open both windows; the rest follows.
    c.send(&frame(kind::WINDOW_UPDATE, 0, 0, &200_000u32.to_be_bytes()));
    c.send(&frame(kind::WINDOW_UPDATE, 0, 1, &200_000u32.to_be_bytes()));
    c.run();
    let frames = c.take();
    c.answers(&frames, &mut answers);
    let a = &answers[&1];
    assert_eq!(a.body.len(), 150_000);
    assert!(a.ended);
    assert_eq!(a.header("content-length"), "150000");
}

#[test]
fn a_raised_initial_window_applies_to_streams() {
    let mut c = Client::new(4);
    let mut settings = 0x4u16.to_be_bytes().to_vec();
    settings.extend_from_slice(&1_000_000u32.to_be_bytes());
    c.send(&frame(kind::SETTINGS, 0, 0, &settings));
    c.send(&frame(
        kind::WINDOW_UPDATE,
        0,
        0,
        &1_000_000u32.to_be_bytes(),
    ));
    c.request(1, "GET", "/api/big?150000", &[], true);
    c.run();
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    assert_eq!(answers[&1].body.len(), 150_000);
}

#[test]
fn streamed_files_and_head_requests() {
    let mut c = Client::new(4);
    c.send(&frame(kind::WINDOW_UPDATE, 0, 0, &100_000u32.to_be_bytes()));
    let mut settings = 0x4u16.to_be_bytes().to_vec();
    settings.extend_from_slice(&100_000u32.to_be_bytes());
    c.send(&frame(kind::SETTINGS, 0, 0, &settings));
    c.request(1, "GET", "/api/file", &[], true);
    c.request(3, "HEAD", "/api/file", &[], true);
    c.run();
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    let file = &answers[&1];
    assert_eq!(file.header("content-type"), "image/png");
    assert_eq!(file.body.len(), 50_000);
    assert_eq!(file.body[49_999], 49_999u32 as u8);
    let head = &answers[&3];
    assert_eq!(head.header("content-length"), "50000");
    assert!(head.body.is_empty() && head.ended);
}

#[test]
fn request_bodies_up_to_the_limit_and_413_beyond() {
    let mut c = Client::new(4);
    c.request(
        1,
        "POST",
        "/api/echo",
        &[("content-type", "text/plain")],
        false,
    );
    c.send(&frame(kind::DATA, 0, 1, b"hello "));
    c.send(&frame(kind::DATA, flag::END_STREAM, 1, b"world"));
    c.request(3, "POST", "/api/echo", &[], false);
    c.send(&frame(kind::DATA, flag::END_STREAM, 3, &vec![b'x'; 2000]));
    c.run();
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    assert_eq!(
        (answers[&1].status, &answers[&1].body[..]),
        (201, &b"hello world"[..])
    );
    assert_eq!(answers[&3].status, 413);
    // The connection window was given back for every DATA byte.
    let returned: u32 = frames
        .iter()
        .filter(|f| f.kind == kind::WINDOW_UPDATE && f.stream == 0)
        .map(|f| u32::from_be_bytes(f.payload[..4].try_into().unwrap()))
        .sum();
    assert_eq!(returned, 11 + 2000);
}

#[test]
fn streams_over_the_limit_are_refused() {
    let mut c = Client::new(2);
    // Requests whose bodies never finish keep their streams open.
    for stream in [1u32, 3, 5] {
        c.request(stream, "POST", "/api/echo", &[], false);
    }
    c.run();
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    assert_eq!(answers[&5].reset, Some(code::REFUSED_STREAM));
    assert!(!answers.contains_key(&1) && !answers.contains_key(&3));
}

#[test]
fn protocol_violations_end_the_connection_with_goaway() {
    // Not a preface at all.
    let mut c = Client::new(4);
    c.pipe.input.lock().unwrap().clear();
    c.send(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(!c.run());
    let goaway = c
        .take()
        .into_iter()
        .find(|f| f.kind == kind::GOAWAY)
        .unwrap();
    assert_eq!(goaway.payload[4..8], code::PROTOCOL_ERROR.to_be_bytes());
    // A frame over 16 KiB.
    let mut c = Client::new(4);
    let mut big = ((MAX_FRAME + 1) as u32).to_be_bytes()[1..].to_vec();
    big.extend_from_slice(&[kind::DATA, 0, 0, 0, 0, 1]);
    c.send(&big);
    assert!(!c.run());
    let goaway = c
        .take()
        .into_iter()
        .find(|f| f.kind == kind::GOAWAY)
        .unwrap();
    assert_eq!(goaway.payload[4..8], code::FRAME_SIZE_ERROR.to_be_bytes());
    // Even stream ids are the server's.
    let mut c = Client::new(4);
    c.request(2, "GET", "/api/hello", &[], true);
    assert!(!c.run());
    // A missing :path is a malformed request: that stream only.
    let mut c = Client::new(4);
    let block = c
        .encoder
        .encode(vec![(&b":method"[..], &b"GET"[..]), (b":scheme", b"https")]);
    c.send(&frame(
        kind::HEADERS,
        flag::END_HEADERS | flag::END_STREAM,
        1,
        &block,
    ));
    assert!(c.run());
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    assert_eq!(answers[&1].reset, Some(code::PROTOCOL_ERROR));
}

#[test]
fn malformed_padding_continuations_and_window_updates_are_bounded() {
    let attacks = [
        frame(kind::HEADERS, flag::PADDED | flag::END_HEADERS, 1, &[10, 0]),
        frame(kind::CONTINUATION, flag::END_HEADERS, 1, &[0x82]),
        frame(kind::WINDOW_UPDATE, 0, 0, &[0, 0, 0, 0]),
        frame(kind::WINDOW_UPDATE, 0, 0, &0x7fff_ffffu32.to_be_bytes()),
        frame(kind::SETTINGS, 0, 0, &[0, 4, 0xff, 0xff, 0xff, 0xff]),
    ];
    for attack in attacks {
        let mut c = Client::new(4);
        c.send(&attack);
        assert!(!c.run(), "accepted {attack:?}");
        assert!(c.take().iter().any(|f| f.kind == kind::GOAWAY));
        assert!(c.server.retained_bytes() < 64 * 1024);
    }
    let mut c = Client::new(4);
    c.send(&frame(kind::HEADERS, 0, 1, &[0x82]));
    c.send(&frame(kind::PING, 0, 0, &[0; 8]));
    assert!(!c.run(), "accepted interleaved header continuation");
}

#[test]
fn header_blocks_may_span_continuation_frames() {
    let mut c = Client::new(4);
    let block = c.encoder.encode(vec![
        (&b":method"[..], &b"GET"[..]),
        (b":scheme", b"https"),
        (b":authority", b"board.local"),
        (b":path", b"/api/hello?split"),
    ]);
    let (a, b) = block.split_at(block.len() / 2);
    c.send(&frame(kind::HEADERS, flag::END_STREAM, 1, a));
    c.send(&frame(kind::CONTINUATION, flag::END_HEADERS, 1, b));
    c.run();
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    assert_eq!(answers[&1].body, b"hello split");
}

#[test]
fn the_preface_may_arrive_in_pieces_and_h2c_is_recognized() {
    assert!(Connection::looks_like_preface(b"PRI * HT"));
    assert!(Connection::looks_like_preface(PREFACE));
    assert!(!Connection::looks_like_preface(b"GET / HTTP/1.1"));
    assert!(!Connection::looks_like_preface(b""));
    // The first 10 bytes were read by the HTTP/1 path before it noticed.
    let pipe = Pipe::default();
    pipe.input.lock().unwrap().extend_from_slice(&PREFACE[10..]);
    let mut server = Connection::new(&PREFACE[..10], 4);
    let (keep, _) = server.poll(
        &mut pipe.clone(),
        &site(),
        &mut Scratch::new(4096),
        &limits(),
        1024,
        true,
    );
    assert!(keep);
    assert!(server.preface);
}

fn upload_frames(c: &mut Client, stream: u32, path: &str, body: &[u8]) {
    let length = body.len().to_string();
    c.request(
        stream,
        "PUT",
        path,
        &[("content-length", length.as_str())],
        false,
    );
    let chunks: Vec<&[u8]> = body.chunks(MAX_FRAME).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let end = if i + 1 == chunks.len() {
            flag::END_STREAM
        } else {
            0
        };
        c.send(&frame(kind::DATA, end, stream, chunk));
    }
}

#[test]
fn a_large_upload_streams_to_the_handler_and_returns_its_window() {
    let mut c = Client::new(4);
    let body: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    let expected: u64 = body.iter().map(|&b| u64::from(b)).sum();
    upload_frames(&mut c, 1, "/api/upload", &body);
    c.request(3, "GET", "/api/hello?after", &[], true);
    c.run();
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    assert_eq!(answers[&1].status, 201);
    assert_eq!(answers[&1].body, format!("40000 {expected}").as_bytes());
    assert_eq!(
        answers[&3].body, b"hello after",
        "other streams still served"
    );
    // The stream's window came back as the handler read it (not past the
    // end), and the connection's for every byte.
    let stream_updates: u32 = frames
        .iter()
        .filter(|f| f.kind == kind::WINDOW_UPDATE && f.stream == 1)
        .map(|f| u32::from_be_bytes(f.payload[..4].try_into().unwrap()))
        .sum();
    assert!(
        stream_updates > 0 && stream_updates < 40_000,
        "{stream_updates}"
    );
    let settings = frames
        .iter()
        .find(|f| f.kind == kind::SETTINGS && f.flags == 0)
        .unwrap();
    assert!(
        settings
            .payload
            .chunks(6)
            .any(|e| e == [0, 4, 0, 0, 0x40, 0]),
        "16 KiB window"
    );
}

#[test]
fn an_unread_upload_is_drained_and_answered() {
    let mut c = Client::new(4);
    upload_frames(&mut c, 1, "/api/refuse-upload", &[7u8; 30_000]);
    c.request(3, "GET", "/api/hello?next", &[], true);
    c.run();
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    assert_eq!(answers[&1].status, 413);
    assert_eq!(answers[&3].body, b"hello next");
}

#[test]
fn uploads_over_the_route_limit_or_without_a_length_are_413() {
    let mut c = Client::new(4);
    // Declared larger than the route allows: refused without reading.
    c.request(
        1,
        "PUT",
        "/api/upload",
        &[("content-length", "250000")],
        false,
    );
    c.send(&frame(kind::DATA, flag::END_STREAM, 1, &[0u8; 2000]));
    // No content-length: cannot stream, and over the buffer limit.
    c.request(3, "PUT", "/api/upload", &[], false);
    c.send(&frame(kind::DATA, flag::END_STREAM, 3, &[0u8; 2000]));
    c.run();
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    assert_eq!(answers[&1].status, 413);
    assert_eq!(answers[&3].status, 413);
}

#[test]
fn a_client_ignoring_the_window_is_cut_off() {
    // Through the loop this cannot pile up: the connection reads one frame
    // at a time and the upload's handler drains it. The cap is the
    // backstop, so drive the frame layer directly with nobody reading.
    let mut server = Connection::new(&[], 4);
    let none = |_: &str, _: &str| None;
    server.streams.push(Stream {
        id: 1,
        headers: Vec::new(),
        body: Vec::new(),
        too_large: false,
        complete: false,
        streamed: Some(150_000),
        response: None,
        send_window: DEFAULT_WINDOW,
    });
    server.last_stream = 1;
    for _ in 0..3 {
        assert!(server
            .frame_in(kind::DATA, 0, 1, &[1u8; MAX_FRAME], 1024, &none)
            .is_ok());
    }
    assert_eq!(server.streams[0].body.len(), 3 * MAX_FRAME);
    // A fourth frame takes it past the 64 KiB anyone may send unasked.
    assert!(server
        .frame_in(kind::DATA, 0, 1, &[1u8; MAX_FRAME], 1024, &none)
        .is_ok());
    assert!(server.streams.is_empty(), "the stream was reset");
    let reset = frames(&server.out)
        .into_iter()
        .find(|f| f.kind == kind::RST_STREAM)
        .unwrap();
    assert_eq!(reset.payload, code::FLOW_CONTROL_ERROR.to_be_bytes());

    // More data than the declared length is a protocol error.
    let mut c = Client::new(4);
    c.request(
        1,
        "PUT",
        "/api/upload",
        &[("content-length", "20000")],
        false,
    );
    c.send(&frame(kind::DATA, 0, 1, &[1u8; MAX_FRAME]));
    c.send(&frame(kind::DATA, 0, 1, &[1u8; MAX_FRAME]));
    c.run();
    let frames = c.take();
    let mut answers = Default::default();
    c.answers(&frames, &mut answers);
    assert_eq!(answers[&1].reset, Some(code::PROTOCOL_ERROR));
}

#[test]
fn a_tls_connection_may_be_polled_before_the_client_speaks() {
    // ALPN chose h2, so the connection starts with nothing read yet.
    let pipe = Pipe::default();
    let mut server = Connection::new(&[], 4);
    let site = site();
    let mut scratch = Scratch::new(4096);
    for _ in 0..3 {
        let (keep, _) = server.poll(
            &mut pipe.clone(),
            &site,
            &mut scratch,
            &limits(),
            1024,
            true,
        );
        assert!(keep, "an empty first turn is not a protocol error");
    }
    let sent = frames(&pipe.output.lock().unwrap());
    assert!(sent.iter().all(|f| f.kind != kind::GOAWAY));
    assert_eq!(sent[0].kind, kind::SETTINGS);
    // The client then speaks and is served.
    pipe.input.lock().unwrap().extend_from_slice(PREFACE);
    pipe.input
        .lock()
        .unwrap()
        .extend_from_slice(&frame(kind::SETTINGS, 0, 0, &[]));
    let (keep, _) = server.poll(
        &mut pipe.clone(),
        &site,
        &mut scratch,
        &limits(),
        1024,
        true,
    );
    assert!(keep && server.preface);
}
