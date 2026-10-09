//! The board's error log: a bounded incident history served at
//! `/.well-known/incidents`, in the place of the OS logs a board lacks.
//!
//! - Every transport and Wi-Fi [`Event`](crate::events::Event) is recorded
//!   here before any app observer sees it; apps add their own with
//!   [`record`]. Writers never wait, allocate or format: a noisy fault is
//!   coalesced and counted, never queued.
//! - Counters per kind never wrap out of view, however fast the text log
//!   (`/api/v1/log`) scrolls.
//! - On the board an independent sampler adds heap/Wi-Fi/worker samples and
//!   [`Kind::HeapLow`], and ESP-IDF's failed-allocation hook adds
//!   [`Kind::AllocationFailed`] with the request's size and capabilities.
//! - On the board the counters and newest events are mirrored to RTC
//!   memory, so the next boot serves them as `previous` after a crash,
//!   watchdog or software reset (not after power loss).
//!
//! No request data is ever recorded: no URLs, headers, bodies, credentials
//! or client addresses. Codes are numeric reasons (IDF, mbedTLS, HTTP).
use crate::message;
use std::sync::{
    atomic::{AtomicU32, Ordering::Relaxed},
    Mutex, OnceLock,
};

pub static LOG: Recorder = Recorder::new();
const EVENTS: usize = 48;
const SAMPLES: usize = 32;
const KINDS: usize = ALL.len();
/// A worker without a turn for this long is stalled.
pub const STALL_MS: u32 = 2_000;
/// Internal free RAM below this is worth an incident: TLS, Wi-Fi and the
/// AES DMA bounce buffers all need it.
pub const HEAP_LOW: u32 = 16 * 1024;
/// Repeats of one kind and code this close together share an entry.
const COALESCE_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    #[default]
    Boot,
    /// The board started serving.
    Ready,
    /// Code: IDF disconnect reason.
    WifiDown,
    WifiUp,
    Reconnect,
    ReconnectFailed,
    TlsInitFailed,
    /// A handshake failed. Code: esp-tls/mbedTLS error.
    TlsFailed,
    TlsTimeout,
    HandoffFull,
    /// Code 1: a TLS slot, 0: an HTTP slot.
    AdmissionRejected,
    SocketError,
    /// Code 0: receiving the request, 1: sending the response.
    RequestTimeout,
    /// Code: HTTP status.
    SlowRequest,
    /// Code: heap capability bits (0: the framework's own buffer); bytes:
    /// the largest failed request.
    AllocationFailed,
    StorageFailed,
    /// Code 0: TLS handshake task, 1: serving loop.
    WorkerStalled,
    WorkerRecovered,
    /// Refused HTTP framing. Code: HTTP status (400, 408, 413, 431).
    InvalidRequest,
    /// Count only.
    IdleExpired,
    /// An established TLS connection failed mid-read or mid-write (the
    /// client sees a network error; browsers often call it CORS). Code:
    /// mbedTLS error.
    TlsConnectionFailed,
    /// The peer reset a TLS connection: browsers do this routinely. Count
    /// only.
    PeerReset,
    /// Internal RAM fell to a new low under [`HEAP_LOW`]. Bytes: the low;
    /// code: open connections then.
    HeapLow,
    /// The app's own incident. Code: the app's.
    App,
    /// We ended an HTTP/2 connection for a protocol violation (every
    /// request on it fails). Code: HTTP/2 error (1 protocol, 3 flow
    /// control, 6 frame size, 9 compression, ...).
    H2GoAway,
    /// The client ended an HTTP/2 connection. Code: its HTTP/2 error.
    H2PeerGoAway,
    /// A stream was refused (REFUSED_STREAM). Code 0: too many streams at
    /// once, 1: header block too large.
    H2Refused,
    /// An idle keep-alive connection was closed to admit a new one. Code 1:
    /// a TLS slot, 0: an HTTP slot.
    Evicted,
}

const ALL: [Kind; 28] = [
    Kind::Boot,
    Kind::Ready,
    Kind::WifiDown,
    Kind::WifiUp,
    Kind::Reconnect,
    Kind::ReconnectFailed,
    Kind::TlsInitFailed,
    Kind::TlsFailed,
    Kind::TlsTimeout,
    Kind::HandoffFull,
    Kind::AdmissionRejected,
    Kind::SocketError,
    Kind::RequestTimeout,
    Kind::SlowRequest,
    Kind::AllocationFailed,
    Kind::StorageFailed,
    Kind::WorkerStalled,
    Kind::WorkerRecovered,
    Kind::InvalidRequest,
    Kind::IdleExpired,
    Kind::TlsConnectionFailed,
    Kind::PeerReset,
    Kind::HeapLow,
    Kind::App,
    Kind::H2GoAway,
    Kind::H2PeerGoAway,
    Kind::H2Refused,
    Kind::Evicted,
];
const _: () = {
    let mut i = 0;
    while i < ALL.len() {
        assert!(ALL[i] as usize == i);
        i += 1;
    }
};

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Boot => "boot",
            Kind::Ready => "ready",
            Kind::WifiDown => "wifi_down",
            Kind::WifiUp => "wifi_up",
            Kind::Reconnect => "reconnect",
            Kind::ReconnectFailed => "reconnect_failed",
            Kind::TlsInitFailed => "tls_init_failed",
            Kind::TlsFailed => "tls_failed",
            Kind::TlsTimeout => "tls_timeout",
            Kind::HandoffFull => "handoff_full",
            Kind::AdmissionRejected => "admission_rejected",
            Kind::SocketError => "socket_error",
            Kind::RequestTimeout => "request_timeout",
            Kind::SlowRequest => "slow_request",
            Kind::AllocationFailed => "allocation_failed",
            Kind::StorageFailed => "storage_failed",
            Kind::WorkerStalled => "worker_stalled",
            Kind::WorkerRecovered => "worker_recovered",
            Kind::InvalidRequest => "invalid_request",
            Kind::IdleExpired => "idle_expired",
            Kind::TlsConnectionFailed => "tls_connection_failed",
            Kind::PeerReset => "peer_reset",
            Kind::HeapLow => "heap_low",
            Kind::App => "app",
            Kind::H2GoAway => "h2_goaway",
            Kind::H2PeerGoAway => "h2_peer_goaway",
            Kind::H2Refused => "h2_refused",
            Kind::Evicted => "evicted",
        }
    }

    fn from_u32(n: u32) -> Option<Kind> {
        ALL.get(n as usize).copied()
    }

    /// Counted, but too routine for an entry in the history.
    fn count_only(self) -> bool {
        matches!(self, Kind::IdleExpired | Kind::PeerReset)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Entry {
    first_ms: u64,
    last_ms: u64,
    count: u32,
    duration_ms: u32,
    bytes: u32,
    code: i32,
    kind: Kind,
}

#[derive(Clone, Copy, Default)]
struct Sampled {
    at_ms: u64,
    free_heap: u32,
    largest_block: u32,
    tls_gap_ms: u32,
    http_gap_ms: u32,
    /// None means the station is disconnected (or unavailable).
    rssi: Option<i8>,
    pending_tls: u8,
    tls_clients: u8,
    http_clients: u8,
}

const _: () = assert!(std::mem::size_of::<Entry>() <= 40);
const _: () = assert!(std::mem::size_of::<Sampled>() <= 32);

#[derive(Clone, Copy)]
struct State {
    events: [Entry; EVENTS],
    samples: [Sampled; SAMPLES],
    event_next: usize,
    sample_next: usize,
    event_len: usize,
    sample_len: usize,
    overwritten: u32,
    wifi: Option<bool>,
    stalled: [bool; 2],
    heap_low: u32,
}

pub struct Recorder {
    state: Mutex<State>,
    counts: [AtomicU32; KINDS],
    dropped: AtomicU32,
    boot_id: AtomicU32,
    beats: [AtomicU32; 2],
    active: [AtomicU32; 2],
    connections: [AtomicU32; 3],
    high_water: [AtomicU32; 3],
    last_handshake_ms: AtomicU32,
    max_handshake_ms: AtomicU32,
    retained: OnceLock<Retained>,
    previous: OnceLock<Previous>,
}

const _: () = assert!(std::mem::size_of::<Recorder>() < 4096);

message! {
    /// One coalesced incident: repeats of a kind and code within five
    /// seconds share an entry.
    pub struct Incident {
        1 kind: String,
        2 code: i32,
        3 count: u32,
        /// Uptime of the first and latest repeat.
        4 first_ms: u64,
        5 last_ms: u64,
        /// The longest duration among the repeats (0: none).
        6 duration_ms: u32,
        /// Bytes involved: the largest failed allocation, or the lowest
        /// heap low, among the repeats.
        7 bytes: u32,
    }
}

message! {
    /// Taken every five seconds by an independent task on the board.
    pub struct Sample {
        1 at_ms: u64,
        /// Internal RAM.
        2 free_heap: u32,
        3 largest_block: u32,
        /// Time since the TLS handshake task / serving loop last turned.
        4 tls_gap_ms: u32,
        5 http_gap_ms: u32,
        /// `null`: the station is disconnected (or unavailable).
        6 rssi: Option<i32>,
        7 pending_tls: u32,
        8 tls_clients: u32,
        9 http_clients: u32,
    }
}

message! {
    pub struct Counter {
        1 kind: String,
        2 count: u32,
    }
}

message! {
    /// What the previous boot retained in RTC memory before it reset.
    pub struct Previous {
        1 boot_id: u32,
        /// Its newest incidents (up to 16), oldest first.
        2 events: Vec<Incident>,
        /// Its counters, nonzero only.
        3 counters: Vec<Counter>,
    }
}

message! {
    /// `GET /.well-known/incidents`.
    pub struct Incidents {
        /// Random per boot.
        1 boot_id: u32,
        /// Why the chip last reset, in words ("" on desktop).
        2 reset_reason: String,
        /// RAM held by the recorder, plus its RTC mirror.
        3 retained_bytes: u32,
        /// No RTC mirror: the history ends with this process or boot.
        4 volatile: bool,
        /// Updates lost because the recorder was busy (never blocks).
        5 dropped: u32,
        /// Older incidents pushed out of the 48-entry history.
        6 overwritten: u32,
        /// Oldest latest-repeat first.
        7 events: Vec<Incident>,
        8 samples: Vec<Sample>,
        /// Every kind, since boot.
        9 counters: Vec<Counter>,
        /// Peak pending TLS, established TLS, established HTTP.
        10 high_water: Vec<u32>,
        11 last_handshake_ms: u32,
        12 max_handshake_ms: u32,
        13 previous: Option<Previous>,
    }
}

impl Entry {
    fn message(&self) -> Incident {
        Incident {
            kind: self.kind.name().into(),
            code: self.code,
            count: self.count,
            first_ms: self.first_ms,
            last_ms: self.last_ms,
            duration_ms: self.duration_ms,
            bytes: self.bytes,
        }
    }
}

/// Starts a history without an RTC mirror (the desktop runners) and
/// records [`Kind::Ready`]. Only the first call does anything.
#[cfg(not(target_os = "espidf"))]
pub(crate) fn start_volatile() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        LOG.boot(crate::uptime_ms(), 0, None);
        record(Kind::Ready, 0);
    });
}

/// Records an incident at the current uptime.
pub fn record(kind: Kind, code: i32) {
    LOG.record(crate::uptime_ms(), kind, code, 0, 0);
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    pub const fn new() -> Self {
        const ENTRY: Entry = Entry {
            first_ms: 0,
            last_ms: 0,
            count: 0,
            duration_ms: 0,
            bytes: 0,
            code: 0,
            kind: Kind::Boot,
        };
        const SAMPLE: Sampled = Sampled {
            at_ms: 0,
            free_heap: 0,
            largest_block: 0,
            tls_gap_ms: 0,
            http_gap_ms: 0,
            rssi: None,
            pending_tls: 0,
            tls_clients: 0,
            http_clients: 0,
        };
        Self {
            state: Mutex::new(State {
                events: [ENTRY; EVENTS],
                samples: [SAMPLE; SAMPLES],
                event_next: 0,
                sample_next: 0,
                event_len: 0,
                sample_len: 0,
                overwritten: 0,
                wifi: None,
                stalled: [false; 2],
                heap_low: u32::MAX,
            }),
            counts: [const { AtomicU32::new(0) }; KINDS],
            dropped: AtomicU32::new(0),
            boot_id: AtomicU32::new(0),
            beats: [const { AtomicU32::new(0) }; 2],
            active: [const { AtomicU32::new(0) }; 2],
            connections: [const { AtomicU32::new(0) }; 3],
            high_water: [const { AtomicU32::new(0) }; 3],
            last_handshake_ms: AtomicU32::new(0),
            max_handshake_ms: AtomicU32::new(0),
            retained: OnceLock::new(),
            previous: OnceLock::new(),
        }
    }

    /// Starts this boot's history. With `retained` words (RTC memory on the
    /// board), first takes over what the previous boot left there.
    pub(crate) fn boot(&self, now: u64, reset_code: i32, retained: Option<&'static [AtomicU32]>) {
        // ESP-IDF lazily allocates native mutex backing. Initialize it here,
        // before workers start, never on an error-recording path.
        drop(self.state.lock().unwrap_or_else(|e| e.into_inner()));
        let mut id = [0u8; 4];
        let _ = getrandom::getrandom(&mut id);
        let id = u32::from_le_bytes(id);
        self.boot_id.store(id, Relaxed);
        if let Some(words) = retained {
            let retained = Retained(words);
            if let Some(previous) = retained.recover() {
                let _ = self.previous.set(previous);
            }
            retained.reset(id);
            let _ = self.retained.set(retained);
        }
        self.record(now, Kind::Boot, reset_code, 0, 0);
    }

    pub fn record(&self, now: u64, kind: Kind, code: i32, duration_ms: u32, bytes: u32) {
        let count = self.counts[kind as usize]
            .fetch_add(1, Relaxed)
            .saturating_add(1);
        if let Some(retained) = self.retained.get() {
            retained.count(kind, count);
        }
        if kind.count_only() {
            return;
        }
        let Ok(mut state) = self.state.try_lock() else {
            self.dropped.fetch_add(1, Relaxed);
            return;
        };
        // Coalesce by kind/code even with interleaved events. Keep first/latest
        // and the worst duration; a noisy fault cannot allocate more memory.
        let next = state.event_next;
        for (at, entry) in state.events.iter_mut().enumerate() {
            if entry.count > 0
                && entry.kind == kind
                && entry.code == code
                && now.saturating_sub(entry.last_ms) <= COALESCE_MS
            {
                entry.last_ms = now;
                entry.count = entry.count.saturating_add(1);
                entry.duration_ms = entry.duration_ms.max(duration_ms);
                entry.bytes = if kind == Kind::HeapLow {
                    entry.bytes.min(bytes)
                } else {
                    entry.bytes.max(bytes)
                };
                // Only the newest RING entries are mirrored, at `at % RING`.
                if (next + EVENTS - at - 1) % EVENTS < RING {
                    if let Some(retained) = self.retained.get() {
                        retained.event(at % RING, entry);
                    }
                }
                return;
            }
        }
        let entry = Entry {
            first_ms: now,
            last_ms: now,
            count: 1,
            duration_ms,
            bytes,
            code,
            kind,
        };
        state.events[next] = entry;
        if let Some(retained) = self.retained.get() {
            retained.event(next % RING, &entry);
        }
        state.event_next = (next + 1) % EVENTS;
        if state.event_len == EVENTS {
            state.overwritten = state.overwritten.saturating_add(1);
        }
        state.event_len = (state.event_len + 1).min(EVENTS);
    }

    /// Worker 0 = TLS handshakes; worker 1 = serving loop.
    pub fn beat(&self, worker: usize, now: u64) {
        // One writer per worker. Simple atomic loads/stores also avoid
        // Xtensa codegen issues with atomic max/swap under LTO.
        let old = self.beats[worker].load(Relaxed);
        self.beats[worker].store(now as u32, Relaxed);
        let active = self.active[worker].load(Relaxed);
        self.active[worker].store(1, Relaxed);
        let gap = (now as u32).wrapping_sub(old);
        if active != 0 && gap >= STALL_MS {
            self.record(now, Kind::WorkerRecovered, worker as i32, gap, 0);
        }
    }

    pub fn connections(&self, slot: usize, count: usize) {
        let count = count.min(u8::MAX as usize) as u32;
        self.connections[slot].store(count, Relaxed);
        self.high_water[slot].store(self.high_water[slot].load(Relaxed).max(count), Relaxed);
    }

    /// Lock-free liveness, for a status light. An unstarted worker is not
    /// healthy. Tolerates a beat newer than the caller's clock.
    pub fn workers_healthy(&self, now: u64) -> bool {
        (0..2).all(|i| {
            if self.active[i].load(Relaxed) == 0 {
                return false;
            }
            let gap = (now as u32).wrapping_sub(self.beats[i].load(Relaxed));
            gap < STALL_MS || gap > i32::MAX as u32
        })
    }

    /// Records a framework event (called by [`crate::events::emit`]).
    /// Connection slots: 0 pending handshakes, 1 TLS, 2 HTTP.
    pub fn apply(&self, now: u64, event: &crate::events::Event) {
        use crate::events::{Event as E, Task};
        let record = |kind, code, duration_ms| self.record(now, kind, code, duration_ms, 0);
        match *event {
            E::Turn {
                task: Task::Handshake,
                tls,
                ..
            } => {
                self.beat(0, now);
                self.connections(0, tls.into());
            }
            E::Turn {
                task: Task::Serve,
                tls,
                http,
            } => {
                self.beat(1, now);
                self.connections(1, tls.into());
                self.connections(2, http.into());
            }
            E::Handshake { ms } => self.handshake(ms),
            E::TlsInitFailed { code } => record(Kind::TlsInitFailed, code, 0),
            E::TlsFailed { code, ms } => record(Kind::TlsFailed, code, ms),
            E::TlsTimeout { ms } => record(Kind::TlsTimeout, 0, ms),
            E::TlsConnectionFailed { code } => record(Kind::TlsConnectionFailed, code, 0),
            E::PeerReset => record(Kind::PeerReset, 0, 0),
            E::HandoffFull => record(Kind::HandoffFull, 0, 0),
            E::AdmissionRejected { secure } => {
                record(Kind::AdmissionRejected, i32::from(secure), 0)
            }
            E::SocketError { code } => record(Kind::SocketError, code, 0),
            E::RequestTimeout { ms } => record(Kind::RequestTimeout, 0, ms),
            E::WriteStalled { ms } => record(Kind::RequestTimeout, 1, ms),
            E::SlowRequest { status, ms } => record(Kind::SlowRequest, i32::from(status), ms),
            E::InvalidRequest { status } => record(Kind::InvalidRequest, i32::from(status), 0),
            E::IdleExpired => record(Kind::IdleExpired, 0, 0),
            E::AllocationFailed => record(Kind::AllocationFailed, 0, 0),
            E::WifiDown { reason, .. } => record(Kind::WifiDown, i32::from(reason), 0),
            E::WifiUp => record(Kind::WifiUp, 0, 0),
            E::Reconnect => record(Kind::Reconnect, 0, 0),
            E::ReconnectFailed { code } => record(Kind::ReconnectFailed, code, 0),
        }
    }

    pub fn count(&self, kind: Kind) -> u32 {
        self.counts[kind as usize].load(Relaxed)
    }

    pub fn handshake(&self, ms: u32) {
        self.last_handshake_ms.store(ms, Relaxed);
        // Xtensa's pinned LLVM fails to emit an object for atomic unsigned max.
        // A CAS loop preserves the concurrent maximum using supported codegen.
        let mut previous = self.max_handshake_ms.load(Relaxed);
        while ms > previous {
            match self
                .max_handshake_ms
                .compare_exchange_weak(previous, ms, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(actual) => previous = actual,
            }
        }
    }

    /// Called by an independent sampler without any application/network
    /// lock. Throttled to five seconds even if the caller samples more often.
    /// `low` is internal RAM's minimum free since boot.
    pub fn sample(&self, now: u64, free_heap: u32, largest_block: u32, low: u32, rssi: Option<i8>) {
        let gaps: [u32; 2] = std::array::from_fn(|i| {
            if self.active[i].load(Relaxed) == 0 {
                0
            } else {
                let gap = (now as u32).wrapping_sub(self.beats[i].load(Relaxed));
                // A worker can beat after the sampler captured `now`.
                if gap > i32::MAX as u32 {
                    0
                } else {
                    gap
                }
            }
        });
        let Ok(mut state) = self.state.try_lock() else {
            self.dropped.fetch_add(1, Relaxed);
            return;
        };
        if state.sample_len > 0
            && now.saturating_sub(state.samples[(state.sample_next + SAMPLES - 1) % SAMPLES].at_ms)
                < 5_000
        {
            return;
        }
        let wifi_changed = state.wifi != Some(rssi.is_some());
        state.wifi = Some(rssi.is_some());
        let stalled = gaps.map(|gap| gap >= STALL_MS);
        let new_stalls: [bool; 2] = std::array::from_fn(|i| stalled[i] && !state.stalled[i]);
        state.stalled = stalled;
        let new_low = low < HEAP_LOW && low < state.heap_low;
        if new_low {
            state.heap_low = low;
        }
        let open: [u32; 3] = std::array::from_fn(|i| self.connections[i].load(Relaxed));
        let at = state.sample_next;
        state.samples[at] = Sampled {
            at_ms: now,
            free_heap,
            largest_block,
            tls_gap_ms: gaps[0],
            http_gap_ms: gaps[1],
            rssi,
            pending_tls: open[0] as u8,
            tls_clients: open[1] as u8,
            http_clients: open[2] as u8,
        };
        state.sample_next = (at + 1) % SAMPLES;
        state.sample_len = (state.sample_len + 1).min(SAMPLES);
        drop(state);
        if wifi_changed {
            let kind = if rssi.is_some() {
                Kind::WifiUp
            } else {
                Kind::WifiDown
            };
            self.record(now, kind, 0, 0, 0);
        }
        for (i, &new) in new_stalls.iter().enumerate() {
            if new {
                self.record(now, Kind::WorkerStalled, i as i32, gaps[i], 0);
            }
        }
        if new_low {
            let open = open.iter().sum::<u32>() as i32;
            self.record(now, Kind::HeapLow, open, 0, low);
        }
    }

    pub fn snapshot(&self, reset_reason: &str) -> Incidents {
        // Copy only under the lock. Allocation and encoding happen later.
        let state = *self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries: Vec<_> = state.events.iter().filter(|e| e.count > 0).collect();
        entries.sort_by_key(|e| e.last_ms);
        let samples = (0..state.sample_len)
            .map(|i| {
                let s =
                    state.samples[(state.sample_next + SAMPLES - state.sample_len + i) % SAMPLES];
                Sample {
                    at_ms: s.at_ms,
                    free_heap: s.free_heap,
                    largest_block: s.largest_block,
                    tls_gap_ms: s.tls_gap_ms,
                    http_gap_ms: s.http_gap_ms,
                    rssi: s.rssi.map(i32::from),
                    pending_tls: s.pending_tls.into(),
                    tls_clients: s.tls_clients.into(),
                    http_clients: s.http_clients.into(),
                }
            })
            .collect();
        let retained = self.retained.get().is_some();
        Incidents {
            boot_id: self.boot_id.load(Relaxed),
            reset_reason: reset_reason.into(),
            retained_bytes: (std::mem::size_of::<Self>() + if retained { WORDS * 4 } else { 0 })
                as u32,
            volatile: !retained,
            dropped: self.dropped.load(Relaxed),
            overwritten: state.overwritten,
            events: entries.into_iter().map(Entry::message).collect(),
            samples,
            counters: ALL
                .iter()
                .map(|&kind| Counter {
                    kind: kind.name().into(),
                    count: self.count(kind),
                })
                .collect(),
            high_water: self.high_water.iter().map(|h| h.load(Relaxed)).collect(),
            last_handshake_ms: self.last_handshake_ms.load(Relaxed),
            max_handshake_ms: self.max_handshake_ms.load(Relaxed),
            previous: self.previous.get().cloned(),
        }
    }
}

/// Newest incidents mirrored to retained (RTC) memory.
const RING: usize = 16;
const ENTRY_WORDS: usize = 10;
const MAGIC: u32 = 0x4d46_494e;
/// magic, boot ID, counters, then the entry ring.
pub(crate) const WORDS: usize = 2 + KINDS + RING * ENTRY_WORDS;
const _: () = assert!(EVENTS.is_multiple_of(RING));

/// The RTC mirror. Each entry slot carries its own checksum, so a reset in
/// the middle of a write loses that slot, not the history.
struct Retained(&'static [AtomicU32]);

impl Retained {
    fn get(&self, i: usize) -> u32 {
        self.0[i].load(Relaxed)
    }

    fn set(&self, i: usize, value: u32) {
        self.0[i].store(value, Relaxed);
    }

    fn reset(&self, boot_id: u32) {
        self.set(0, 0);
        for i in 1..WORDS {
            self.set(i, 0);
        }
        self.set(1, boot_id);
        self.set(0, MAGIC);
    }

    fn count(&self, kind: Kind, count: u32) {
        self.set(2 + kind as usize, count);
    }

    fn event(&self, slot: usize, e: &Entry) {
        let words = [
            e.first_ms as u32,
            (e.first_ms >> 32) as u32,
            e.last_ms as u32,
            (e.last_ms >> 32) as u32,
            e.count,
            e.duration_ms,
            e.bytes,
            e.code as u32,
            e.kind as u32,
        ];
        let base = 2 + KINDS + slot * ENTRY_WORDS;
        self.set(base + 9, 0);
        for (i, &w) in words.iter().enumerate() {
            self.set(base + i, w);
        }
        self.set(base + 9, checksum(&words));
    }

    fn recover(&self) -> Option<Previous> {
        if self.0.len() < WORDS || self.get(0) != MAGIC {
            return None;
        }
        let counters = ALL
            .iter()
            .map(|&kind| Counter {
                kind: kind.name().into(),
                count: self.get(2 + kind as usize),
            })
            .filter(|c| c.count > 0)
            .collect();
        let mut entries: Vec<Entry> = (0..RING)
            .filter_map(|slot| {
                let base = 2 + KINDS + slot * ENTRY_WORDS;
                let w: [u32; 9] = std::array::from_fn(|i| self.get(base + i));
                if w[4] == 0 || self.get(base + 9) != checksum(&w) {
                    return None;
                }
                Some(Entry {
                    first_ms: u64::from(w[0]) | u64::from(w[1]) << 32,
                    last_ms: u64::from(w[2]) | u64::from(w[3]) << 32,
                    count: w[4],
                    duration_ms: w[5],
                    bytes: w[6],
                    code: w[7] as i32,
                    kind: Kind::from_u32(w[8])?,
                })
            })
            .collect();
        entries.sort_by_key(|e| e.last_ms);
        Some(Previous {
            boot_id: self.get(1),
            events: entries.iter().map(Entry::message).collect(),
            counters,
        })
    }
}

fn checksum(words: &[u32; 9]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    for w in words {
        hasher.update(&w.to_le_bytes());
    }
    // Never 0, so zeroed memory is never a valid slot.
    hasher.finalize() | 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retained() -> &'static [AtomicU32] {
        Box::leak((0..WORDS).map(|_| AtomicU32::new(0)).collect())
    }

    #[test]
    fn flood_is_bounded_and_coalesced() {
        let log = Recorder::new();
        for i in 0..10_000 {
            log.record(i, Kind::TlsFailed, -42, i as u32, 0);
        }
        let snapshot = log.snapshot("");
        assert!(snapshot.retained_bytes < 4096);
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(snapshot.events[0].count, 10_000);
        for i in 0..60 {
            log.record(20_000 + i * 6_000, Kind::TlsFailed, i as i32, 0, 0);
        }
        let snapshot = log.snapshot("");
        assert_eq!(snapshot.events.len(), 48);
        assert_eq!(snapshot.overwritten, 13);
    }

    #[test]
    fn contention_never_waits_and_counts_loss() {
        let log = Recorder::new();
        let _held = log.state.lock().unwrap();
        log.record(1, Kind::TlsTimeout, 0, 0, 0);
        log.sample(2, 100, 50, 100, None);
        assert_eq!(log.dropped.load(Relaxed), 2);
        assert_eq!(log.count(Kind::TlsTimeout), 1);
    }

    #[test]
    fn routine_kinds_are_counted_not_listed() {
        let log = Recorder::new();
        for i in 0..100 {
            log.record(i * 10_000, Kind::PeerReset, 0, 0, 0);
            log.record(i * 10_000, Kind::IdleExpired, 0, 0, 0);
        }
        assert_eq!(log.count(Kind::PeerReset), 100);
        assert!(log.snapshot("").events.is_empty());
    }

    #[test]
    fn history_survives_recovery_and_wraps_in_order() {
        let log = Recorder::new();
        log.boot(0, 7, None);
        log.beat(0, 1);
        log.beat(1, 1);
        log.sample(5_001, 100, 40, 100_000, Some(-60));
        log.beat(1, 6_001);
        assert!(log
            .snapshot("")
            .events
            .iter()
            .any(|e| e.kind == "worker_recovered" && e.duration_ms == 6_000));
        for i in 2..40 {
            log.sample(i * 5_000, 90, 30, 100_000, None);
        }
        let snapshot = log.snapshot("");
        assert_eq!(snapshot.samples.len(), 32);
        assert!(snapshot.samples.windows(2).all(|w| w[0].at_ms < w[1].at_ms));
        assert!(snapshot.volatile);
        assert_eq!(snapshot.events[0].kind, "boot");
        assert_eq!(snapshot.events[0].code, 7);
    }

    #[test]
    fn heartbeat_handles_u32_millisecond_wrap() {
        let log = Recorder::new();
        log.beat(1, u32::MAX as u64 - 100);
        log.beat(1, u32::MAX as u64 + 50);
        assert!(log.snapshot("").events.is_empty());
    }

    #[test]
    fn status_requires_both_workers_to_make_progress() {
        let log = Recorder::new();
        assert!(!log.workers_healthy(0));
        log.beat(0, 100);
        assert!(!log.workers_healthy(100));
        log.beat(1, 101);
        assert!(log.workers_healthy(100)); // concurrent newer beat
        assert!(!log.workers_healthy(2100));
        log.beat(0, u32::MAX as u64 - 100);
        log.beat(1, u32::MAX as u64 - 100);
        assert!(log.workers_healthy(u32::MAX as u64 + 50));
        assert!(!log.workers_healthy(u32::MAX as u64 + 1900));
    }

    #[test]
    fn concurrent_new_heartbeat_is_not_a_four_billion_ms_stall() {
        let log = Recorder::new();
        log.beat(0, 1001);
        log.sample(1000, 100, 50, 100_000, Some(-60));
        assert_eq!(log.snapshot("").samples[0].tls_gap_ms, 0);
        assert!(!log
            .snapshot("")
            .events
            .iter()
            .any(|e| e.kind == "worker_stalled"));
    }

    #[test]
    fn a_new_heap_low_is_an_incident_with_the_open_connections() {
        let log = Recorder::new();
        log.connections(1, 3);
        log.connections(2, 1);
        log.sample(0, 40_000, 8_000, 30_000, None); // above HEAP_LOW
        log.sample(5_000, 40_000, 8_000, 9_000, None);
        log.sample(10_000, 40_000, 8_000, 23, None);
        log.sample(15_000, 40_000, 8_000, 23, None); // not a new low
        let lows: Vec<_> = log
            .snapshot("")
            .events
            .into_iter()
            .filter(|e| e.kind == "heap_low")
            .collect();
        assert_eq!(lows.len(), 1, "coalesced within five seconds");
        assert_eq!(lows[0].count, 2);
        assert_eq!(lows[0].bytes, 23, "the lowest repeat");
        assert_eq!(lows[0].code, 4);
        assert_eq!(log.count(Kind::HeapLow), 2);
    }

    #[test]
    fn full_history_fits_a_bounded_wire_response() {
        let log = Recorder::new();
        for i in 0..60 {
            let now = u64::MAX - (60 - i) * 6000;
            log.record(
                now,
                Kind::AdmissionRejected,
                i32::MIN + i as i32,
                u32::MAX,
                u32::MAX,
            );
            log.sample(now, u32::MAX, u32::MAX, u32::MAX, Some(i8::MIN));
        }
        let snapshot = log.snapshot("crash");
        assert_eq!(snapshot.events.len(), 48);
        assert_eq!(snapshot.samples.len(), 32);
        let json = crate::wire::to_vec(crate::Format::Json, &snapshot).unwrap();
        assert!(json.len() < 32_768);
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value["samples"][0]["rssi"], -128);
        assert_eq!(value["previous"], serde_json::Value::Null);
    }

    #[test]
    fn framework_events_become_incidents() {
        use crate::events::{Event, Task};
        let log = Recorder::new();
        log.apply(10, &Event::TlsTimeout { ms: 4000 });
        log.apply(11, &Event::AdmissionRejected { secure: true });
        log.apply(
            12,
            &Event::WifiDown {
                reason: 201,
                rssi: -70,
            },
        );
        log.apply(13, &Event::TlsConnectionFailed { code: -1 });
        log.apply(14, &Event::PeerReset);
        log.apply(
            15,
            &Event::Turn {
                task: Task::Serve,
                tls: 3,
                http: 1,
            },
        );
        assert_eq!(log.count(Kind::TlsTimeout), 1);
        assert_eq!(log.count(Kind::AdmissionRejected), 1);
        assert_eq!(log.count(Kind::PeerReset), 1);
        let events = log.snapshot("").events;
        assert!(events
            .iter()
            .any(|e| e.kind == "wifi_down" && e.code == 201));
        assert!(events
            .iter()
            .any(|e| e.kind == "tls_connection_failed" && e.code == -1));
        assert!(!events.iter().any(|e| e.kind == "peer_reset"));
    }

    #[test]
    fn the_next_boot_serves_what_rtc_kept() {
        let words = retained();
        let first = Recorder::new();
        first.boot(0, 1, Some(words));
        assert!(!first.snapshot("").volatile);
        first.record(100, Kind::TlsConnectionFailed, -1, 0, 0);
        first.record(200, Kind::AllocationFailed, 0x804, 0, 1600);
        first.record(300, Kind::AllocationFailed, 0x804, 0, 1700); // coalesced
        first.record(400, Kind::PeerReset, 0, 0, 0);
        for i in 0..40 {
            first.record(10_000 + i * 6_000, Kind::SlowRequest, 200, 600, 0);
        }
        let boot_id = first.snapshot("").boot_id;

        let second = Recorder::new();
        second.boot(0, 4, Some(words));
        let previous = second.snapshot("crash").previous.unwrap();
        assert_eq!(previous.boot_id, boot_id);
        assert_eq!(previous.events.len(), RING, "the newest sixteen");
        assert!(previous
            .events
            .windows(2)
            .all(|w| w[0].last_ms <= w[1].last_ms));
        assert!(previous.events.iter().all(|e| e.kind == "slow_request"));
        let count = |kind: &str| {
            previous
                .counters
                .iter()
                .find(|c| c.kind == kind)
                .map_or(0, |c| c.count)
        };
        assert_eq!(count("allocation_failed"), 2);
        assert_eq!(count("peer_reset"), 1);
        assert_eq!(count("tls_connection_failed"), 1);
        assert_eq!(count("wifi_up"), 0);
        // The mirror now holds the second boot only.
        let third = Recorder::new();
        third.boot(0, 4, Some(words));
        let previous = third.snapshot("").previous.unwrap();
        assert_eq!(previous.events.len(), 1);
        assert_eq!(previous.events[0].kind, "boot");
    }

    #[test]
    fn a_coalesced_repeat_updates_its_rtc_slot() {
        let words = retained();
        let first = Recorder::new();
        first.boot(0, 1, Some(words));
        first.record(200, Kind::AllocationFailed, 0x804, 0, 1600);
        first.record(300, Kind::AllocationFailed, 0x804, 0, 1700);
        let second = Recorder::new();
        second.boot(0, 4, Some(words));
        let previous = second.snapshot("").previous.unwrap();
        let alloc = previous
            .events
            .iter()
            .find(|e| e.kind == "allocation_failed")
            .unwrap();
        assert_eq!((alloc.count, alloc.bytes, alloc.last_ms), (2, 1700, 300));
    }

    #[test]
    fn torn_or_foreign_rtc_contents_are_not_served() {
        let words = retained();
        // Power-on garbage: no magic.
        words.iter().for_each(|w| w.store(0xdead_beef, Relaxed));
        let log = Recorder::new();
        log.boot(0, 1, Some(words));
        assert!(log.snapshot("").previous.is_none());
        log.record(100, Kind::HeapLow, 2, 0, 23);
        // A reset in the middle of rewriting the heap-low slot.
        let slot = 2 + KINDS + ENTRY_WORDS; // slot 1: boot is slot 0
        words[slot + 4].store(99, Relaxed);
        let next = Recorder::new();
        next.boot(0, 4, Some(words));
        let previous = next.snapshot("").previous.unwrap();
        assert_eq!(previous.events.len(), 1, "only the intact boot slot");
        assert_eq!(previous.events[0].kind, "boot");
    }
}
