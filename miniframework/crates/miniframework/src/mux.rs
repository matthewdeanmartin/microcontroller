//! The connection loop: one task multiplexes every established HTTP and
//! HTTPS connection with nonblocking I/O (NanaCoin's board design).
//!
//! Each client gets one bounded turn per loop: read what is there, answer a
//! complete request, write what the socket accepts. A slow client never
//! blocks another. Request buffers, connection counts and the bytes held by
//! unsent responses are all bounded. TLS handshakes happen elsewhere (they
//! take about a second on a board) and arrive here finished.
use crate::events::{self, Event, Task};
use crate::http::{self, Response};
use crate::site::{Scratch, Service, Site};
use crate::sys::STATS;
use std::io::{self, Read, Write};
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

/// An established connection: plain TCP or finished TLS.
pub trait Conn: Read + Write + Send {
    fn secure(&self) -> bool;
}

impl Conn for std::net::TcpStream {
    fn secure(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub tls_clients: usize,
    pub http_clients: usize,
    /// TLS handshakes in progress at once. One on a single core: two
    /// concurrent ECDHE computations just take twice as long.
    pub handshakes: usize,
    /// Stop answering new requests while unsent responses hold this much.
    pub response_budget: usize,
    /// Close keep-alive connections idle this long.
    pub idle: Duration,
    /// A request must arrive completely within this time.
    pub request_deadline: Duration,
}

impl Limits {
    /// ESP32-S2: each TLS session keeps a 16 KiB receive record in PSRAM.
    pub fn small_board() -> Self {
        Self {
            tls_clients: 3,
            http_clients: 3,
            handshakes: 1,
            response_budget: 192 * 1024,
            idle: Duration::from_secs(60),
            request_deadline: Duration::from_secs(5),
        }
    }
    /// ESP32-S3 with 8 MiB PSRAM and two cores (NanaCoin's main bank):
    /// handshakes on one core, serving on the other.
    pub fn large_board() -> Self {
        Self {
            tls_clients: 8,
            http_clients: 4,
            handshakes: 2,
            response_budget: 2 * 1024 * 1024,
            idle: Duration::from_secs(60),
            request_deadline: Duration::from_secs(5),
        }
    }
    pub fn desktop() -> Self {
        Self {
            tls_clients: 0,
            http_clients: 64,
            handshakes: 0,
            response_budget: 64 * 1024 * 1024,
            idle: Duration::from_secs(60),
            request_deadline: Duration::from_secs(10),
        }
    }
}

struct Client<C> {
    conn: C,
    input: Vec<u8>,
    used: usize,
    response: Option<Response>,
    active: Instant,
    request_started: Option<Instant>,
    /// When the response in flight was dispatched (for slow-request events).
    dispatched: Option<(Instant, u16)>,
    /// The request was refused mid-stream: after the reply, read and drop
    /// what the peer already sent before closing (see [`LINGER`]).
    refused: bool,
    /// Lingering close in progress since then.
    draining: Option<Instant>,
}

/// After refusing a request whose body is still arriving, keep reading (and
/// discarding) for up to this long before closing. Closing a socket with
/// unread data makes the OS send a reset, which can destroy the error reply
/// before the client reads it (always on Windows, sometimes on lwIP).
pub const LINGER: Duration = Duration::from_millis(500);
/// Bytes a lingering close will read and discard at most.
const LINGER_BYTES: usize = 64 * 1024;

pub struct Mux<C: Conn> {
    clients: Vec<Client<C>>,
    limits: Limits,
    scratch: Scratch,
    input_limit: usize,
    body_limit: usize,
}

impl<C: Conn> Mux<C> {
    pub fn new(limits: Limits, body_limit: usize, response_limit: usize) -> Self {
        STATS.tls_slots.store(limits.tls_clients as u32, Relaxed);
        STATS.http_slots.store(limits.http_clients as u32, Relaxed);
        Self {
            clients: Vec::with_capacity(limits.tls_clients + limits.http_clients),
            limits,
            scratch: Scratch::new(response_limit),
            input_limit: http::HEADER_LIMIT + body_limit,
            body_limit,
        }
    }

    pub fn len(&self) -> usize {
        self.clients.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// Admits a connection, evicting the longest-idle one of the same kind
    /// if every slot is taken. Never evicts a request or response in flight.
    pub fn add(&mut self, conn: C) {
        let secure = conn.secure();
        let limit = if secure {
            self.limits.tls_clients
        } else {
            self.limits.http_clients
        };
        let same = self
            .clients
            .iter()
            .filter(|c| c.conn.secure() == secure)
            .count();
        if same >= limit {
            let idle = self
                .clients
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.conn.secure() == secure
                        && c.response.is_none()
                        && c.used == 0
                        && c.active.elapsed() > Duration::from_millis(500)
                })
                .max_by_key(|(_, c)| c.active.elapsed())
                .map(|(i, _)| i);
            match idle {
                Some(i) => {
                    self.clients.swap_remove(i);
                }
                None => {
                    STATS.rejected.fetch_add(1, Relaxed);
                    events::emit(Event::AdmissionRejected { secure });
                    return;
                }
            }
        }
        self.clients.push(Client {
            conn,
            input: vec![0; self.input_limit],
            used: 0,
            response: None,
            active: Instant::now(),
            request_started: None,
            dispatched: None,
            refused: false,
            draining: None,
        });
    }

    /// One turn for every client. Returns true if any made progress, so a
    /// desktop caller can sleep when idle.
    pub fn turn<S: Service>(&mut self, site: &Site<S>) -> bool {
        let mut queued: usize = self
            .clients
            .iter()
            .filter_map(|c| c.response.as_ref())
            .map(Response::retained_bytes)
            .sum();
        let budget = self.limits.response_budget;
        let mut progress = false;
        let limits = self.limits.clone();
        let body_limit = self.body_limit;
        let scratch = &mut self.scratch;
        self.clients.retain_mut(|client| {
            let before = client.response.as_ref().map_or(0, Response::retained_bytes);
            let (keep, moved) = client.poll(site, scratch, &limits, body_limit, queued < budget);
            progress |= moved;
            queued -= before;
            if keep {
                queued += client.response.as_ref().map_or(0, Response::retained_bytes);
            }
            keep
        });
        let tls = self.clients.iter().filter(|c| c.conn.secure()).count();
        let http = self.clients.len() - tls;
        STATS.tls_open.store(tls as u32, Relaxed);
        STATS.http_open.store(http as u32, Relaxed);
        events::emit(Event::Turn {
            task: Task::Serve,
            tls: tls.min(255) as u8,
            http: http.min(255) as u8,
        });
        progress
    }
}

impl<C: Conn> Client<C> {
    /// One turn of a lingering close; false once the connection can go.
    fn drain(&mut self, since: Instant) -> bool {
        if since.elapsed() >= LINGER || self.used >= LINGER_BYTES {
            return false;
        }
        loop {
            match self.conn.read(&mut self.input) {
                // The peer closed its side: nothing left to cause a reset.
                Ok(0) => return false,
                Ok(n) => {
                    self.used += n;
                    if self.used >= LINGER_BYTES {
                        return false;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return true,
                Err(_) => return false,
            }
        }
    }

    /// Returns (keep the connection, made progress).
    fn poll<S: Service>(
        &mut self,
        site: &Site<S>,
        scratch: &mut Scratch,
        limits: &Limits,
        body_limit: usize,
        may_dispatch: bool,
    ) -> (bool, bool) {
        let mut progress = false;
        if let Some(since) = self.draining {
            return (self.drain(since), true);
        }
        if self.response.is_none() && self.active.elapsed() > limits.idle {
            events::emit(Event::IdleExpired);
            return (false, false);
        }
        if self
            .request_started
            .is_some_and(|t| t.elapsed() > limits.request_deadline)
        {
            events::emit(Event::RequestTimeout {
                ms: limits.request_deadline.as_millis() as u32,
            });
            return (false, false);
        }
        if self.response.is_none() {
            if !may_dispatch {
                return (true, false);
            }
            // A pipelined request may already be buffered: answer it before
            // reading again (the peer may have half-closed after sending).
            if self.used > 0 {
                self.request_started.get_or_insert_with(Instant::now);
            }
            let mut parsed = http::parse(&self.input[..self.used], body_limit);
            if matches!(parsed, Ok(None)) {
                match self.conn.read(&mut self.input[self.used..]) {
                    Ok(0) => return (false, true),
                    Ok(n) => {
                        progress = true;
                        self.used += n;
                        self.active = Instant::now();
                        self.request_started.get_or_insert(self.active);
                        parsed = http::parse(&self.input[..self.used], body_limit);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        if !self.conn.secure() {
                            events::emit(Event::SocketError {
                                code: e.raw_os_error().unwrap_or(0),
                            });
                        }
                        return (false, true);
                    }
                }
            }
            match parsed {
                Ok(Some(request)) => {
                    progress = true;
                    let consumed = request.consumed;
                    let began = Instant::now();
                    let response = site.respond(&request, self.conn.secure(), scratch);
                    self.dispatched = Some((began, response.status));
                    self.response = Some(response);
                    self.input.copy_within(consumed..self.used, 0);
                    self.used -= consumed;
                    self.request_started = None;
                }
                Ok(None) if self.used < self.input.len() => {}
                other => {
                    let status = other.err().unwrap_or(413);
                    STATS.errors.fetch_add(1, Relaxed);
                    events::emit(Event::InvalidRequest { status });
                    self.response = Some(site.refuse(status, &self.input[..self.used]));
                    self.refused = true;
                    self.request_started = None;
                }
            }
        }
        if let Some(response) = &mut self.response {
            match response.send(&mut self.conn, Instant::now()) {
                Ok(finished) => {
                    progress = true;
                    self.active = Instant::now();
                    if finished {
                        if let Some((at, status)) = self.dispatched.take() {
                            let ms = at.elapsed().as_millis() as u32;
                            if ms >= events::SLOW_MS {
                                events::emit(Event::SlowRequest { status, ms });
                            }
                        }
                        if response.close {
                            if self.refused {
                                self.response = None;
                                self.used = 0;
                                self.draining = Some(Instant::now());
                                return (true, true);
                            }
                            return (false, true);
                        }
                        self.response = None;
                    }
                }
                Err(e) => {
                    if e.kind() == io::ErrorKind::TimedOut {
                        events::emit(Event::WriteStalled {
                            ms: http::WRITE_STALL_TIMEOUT.as_millis() as u32,
                        });
                    } else if !self.conn.secure() {
                        events::emit(Event::SocketError {
                            code: e.raw_os_error().unwrap_or(0),
                        });
                    }
                    return (false, true);
                }
            }
        }
        (true, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::DesktopPlatform;
    use crate::events::capture;
    use crate::site::{Config, Reply, Request};
    use std::sync::{Arc, Mutex};

    /// An in-memory connection: reads what the test queued (then
    /// WouldBlock), and records what the server wrote.
    #[derive(Clone)]
    struct Pipe {
        secure: bool,
        input: Arc<Mutex<Vec<u8>>>,
        output: Arc<Mutex<Vec<u8>>>,
        /// Bytes the "socket" accepts before WouldBlock; None is unlimited.
        window: Arc<Mutex<Option<usize>>>,
    }

    impl Pipe {
        fn new(secure: bool, input: &[u8]) -> Self {
            Self {
                secure,
                input: Arc::new(Mutex::new(input.to_vec())),
                output: Arc::default(),
                window: Arc::default(),
            }
        }
        fn written(&self) -> String {
            String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
        }
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
            let mut window = self.window.lock().unwrap();
            let n = match *window {
                Some(0) => return Err(io::ErrorKind::WouldBlock.into()),
                Some(w) => w.min(bytes.len()),
                None => bytes.len(),
            };
            if let Some(w) = window.as_mut() {
                *w -= n;
            }
            self.output.lock().unwrap().extend_from_slice(&bytes[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Conn for Pipe {
        fn secure(&self) -> bool {
            self.secure
        }
    }

    struct Slow;
    impl Service for Slow {
        fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
            if req.path == "/api/slow" {
                std::thread::sleep(Duration::from_millis(u64::from(events::SLOW_MS) + 20));
            }
            reply.text(200, "text/plain", "hello");
        }
    }

    fn site() -> Site<Slow> {
        Site::new(Config::new("test", "board.local"), Slow, DesktopPlatform)
    }

    fn limits() -> Limits {
        Limits {
            tls_clients: 1,
            http_clients: 1,
            handshakes: 1,
            response_budget: 1024 * 1024,
            idle: Duration::from_millis(400),
            request_deadline: Duration::from_millis(100),
        }
    }

    const GET: &[u8] = b"GET /api/x HTTP/1.1\r\nHost: board.local\r\n\r\n";

    #[test]
    fn answers_keep_alive_and_pipelined_requests() {
        capture::start();
        let site = site();
        let mut mux = Mux::new(limits(), 1024, 4096);
        let pipe = Pipe::new(false, &[GET, GET].concat());
        mux.add(pipe.clone());
        for _ in 0..4 {
            mux.turn(&site);
        }
        assert_eq!(pipe.written().matches("HTTP/1.1 200 OK").count(), 2);
        assert_eq!(mux.len(), 1, "keep-alive connection stays");
        assert!(capture::take().is_empty());
    }

    #[test]
    fn a_full_slot_with_a_request_in_flight_rejects_the_newcomer() {
        capture::start();
        let site = site();
        let mut mux = Mux::new(limits(), 1024, 4096);
        // Half a request: in flight, so it may not be evicted.
        let busy = Pipe::new(false, b"GET /api/x HTTP/1.1\r\n");
        mux.add(busy.clone());
        mux.turn(&site);
        mux.add(Pipe::new(false, GET));
        assert_eq!(mux.len(), 1);
        assert_eq!(
            capture::take(),
            vec![Event::AdmissionRejected { secure: false }]
        );
        // The other kind of slot is separate.
        mux.add(Pipe::new(true, GET));
        assert_eq!(mux.len(), 2);
    }

    #[test]
    fn an_idle_keep_alive_connection_gives_way_to_a_new_one() {
        capture::start();
        let site = site();
        let mut mux = Mux::new(limits(), 1024, 4096);
        let old = Pipe::new(false, GET);
        mux.add(old.clone());
        mux.turn(&site);
        mux.turn(&site);
        std::thread::sleep(Duration::from_millis(550));
        let new = Pipe::new(false, GET);
        mux.add(new.clone());
        mux.turn(&site);
        assert_eq!(mux.len(), 1);
        assert!(new.written().starts_with("HTTP/1.1 200"));
        assert!(capture::take().is_empty(), "evicted, not rejected");
    }

    #[test]
    fn idle_and_incomplete_requests_are_closed_with_events() {
        capture::start();
        let site = site();
        let mut mux = Mux::new(limits(), 1024, 4096);
        mux.add(Pipe::new(false, b""));
        mux.add(Pipe::new(true, b"GET /api/x HTTP/1.1\r\n"));
        mux.turn(&site);
        std::thread::sleep(Duration::from_millis(150));
        mux.turn(&site);
        assert_eq!(mux.len(), 1, "the half request missed its deadline");
        assert_eq!(capture::take(), vec![Event::RequestTimeout { ms: 100 }]);
        std::thread::sleep(Duration::from_millis(300));
        mux.turn(&site);
        assert!(mux.is_empty(), "the silent connection went idle");
        assert_eq!(capture::take(), vec![Event::IdleExpired]);
    }

    #[test]
    fn bad_framing_is_answered_then_closed() {
        capture::start();
        let site = site();
        let mut mux = Mux::new(limits(), 16, 4096);
        let pipe = Pipe::new(
            false,
            b"POST /api/x HTTP/1.1\r\nHost: b\r\nContent-Length: 999\r\n\r\n",
        );
        mux.add(pipe.clone());
        mux.turn(&site);
        mux.turn(&site);
        assert!(pipe.written().starts_with("HTTP/1.1 413"));
        assert!(pipe.written().contains("Connection: close"));
        assert_eq!(capture::take(), vec![Event::InvalidRequest { status: 413 }]);
        // Lingering close: the body the client was still sending is read
        // and dropped (closing on unread data would reset the connection
        // and could destroy the 413 before the client reads it)...
        pipe.input.lock().unwrap().extend_from_slice(&[b'x'; 999]);
        mux.turn(&site);
        assert_eq!(mux.len(), 1);
        assert!(pipe.input.lock().unwrap().is_empty(), "drained");
        assert_eq!(pipe.written().matches("HTTP/1.1").count(), 1, "not parsed");
        // ...then the connection goes.
        std::thread::sleep(LINGER);
        mux.turn(&site);
        assert!(mux.is_empty());
    }

    #[test]
    fn slow_requests_are_reported_after_the_last_byte() {
        capture::start();
        let site = site();
        let mut mux = Mux::new(limits(), 1024, 4096);
        let pipe = Pipe::new(false, b"GET /api/slow HTTP/1.1\r\nHost: b\r\n\r\n");
        // Hold the response back for a turn to show the event waits for it.
        *pipe.window.lock().unwrap() = Some(0);
        mux.add(pipe.clone());
        mux.turn(&site);
        assert!(capture::take().is_empty());
        *pipe.window.lock().unwrap() = None;
        mux.turn(&site);
        match capture::take().as_slice() {
            [Event::SlowRequest { status: 200, ms }] => assert!(*ms >= events::SLOW_MS),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn board_profiles_keep_their_bounds() {
        let small = Limits::small_board();
        let large = Limits::large_board();
        assert_eq!((small.tls_clients, small.handshakes), (3, 1));
        assert_eq!(
            (large.tls_clients, large.http_clients, large.handshakes),
            (8, 4, 2)
        );
        assert!(large.response_budget > small.response_budget);
        assert_eq!(Limits::desktop().tls_clients, 0);
    }
}
