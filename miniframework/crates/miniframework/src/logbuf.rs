//! The board keeps its own log and serves it (`/api/v1/log`,
//! `/api/v1/log.txt`), so diagnosing a board never needs a USB cable.
//!
//! - Every `log::info!`/`warn!`/`error!` line (via [`Logger`]) and, on the
//!   board, every ESP-IDF C log line (Wi-Fi, TLS, mbedTLS) lands in one ring.
//! - The ring is a single byte buffer allocated once: no allocation per
//!   line, so logging never competes with the server for internal RAM.
//! - On the board the newest lines are also mirrored to RTC memory, which
//!   survives a crash reset; the next boot serves them as `previous`.
use crate::message;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Mutex;

message! {
    pub struct LogLine {
        /// Increases by one per line since boot; use it to page.
        1 seq: u32,
        /// Milliseconds since boot.
        2 t: u32,
        /// 1 error, 2 warn, 3 info, 4 debug, 5 verbose.
        3 level: u32,
        4 text: String,
    }
}

message! {
    /// Log lines after a sequence number, oldest first.
    pub struct LogPage {
        1 lines: Vec<LogLine>,
        /// Pass as `after` to get only newer lines.
        2 next: u32,
        /// Lines that were pushed out of the ring before they could be read.
        3 dropped: u32,
        /// The last lines of the previous boot, if it ended in a crash,
        /// watchdog or brownout (empty otherwise).
        4 previous: String,
        /// How the previous boot ended (the reset reason).
        5 previous_reason: String,
        /// Ring capacity in bytes.
        6 capacity: u32,
    }
}

pub const ERROR: u8 = 1;
pub const WARN: u8 = 2;
pub const INFO: u8 = 3;
pub const DEBUG: u8 = 4;

/// Header per record: seq (4), t (4), level (1), length (2).
const HEADER: usize = 11;
/// Longest stored line (longer ones are cut).
pub const MAX_LINE: usize = 240;

pub struct Ring {
    bytes: VecDeque<u8>,
    capacity: usize,
    next_seq: u32,
    /// Sequence number of the oldest record still held.
    first_seq: u32,
}

impl Ring {
    pub fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(capacity),
            capacity,
            next_seq: 1,
            first_seq: 1,
        }
    }

    fn pop_oldest(&mut self) {
        if self.bytes.len() < HEADER {
            self.bytes.clear();
            return;
        }
        let len = u16::from_le_bytes([self.bytes[9], self.bytes[10]]) as usize;
        self.bytes.drain(..(HEADER + len).min(self.bytes.len()));
        self.first_seq = self.first_seq.saturating_add(1);
    }

    pub fn push(&mut self, t: u32, level: u8, text: &str) {
        // Restart the cursor epoch before overflow. A cursor from the previous
        // epoch gets the retained new epoch, rather than waiting forever.
        if self.next_seq == u32::MAX {
            self.bytes.clear();
            self.first_seq = 1;
            self.next_seq = 1;
        }
        let mut end = text.len().min(MAX_LINE);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let text = &text.as_bytes()[..end];
        let need = HEADER + text.len();
        if need > self.capacity {
            return;
        }
        while self.bytes.len() + need > self.capacity {
            self.pop_oldest();
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.bytes.extend(seq.to_le_bytes());
        self.bytes.extend(t.to_le_bytes());
        self.bytes.push_back(level);
        self.bytes.extend((text.len() as u16).to_le_bytes());
        self.bytes.extend(text.iter().copied());
    }

    /// Lines with `seq > after`, at most `limit`, oldest first.
    pub fn page(&self, after: u32, limit: usize) -> (Vec<LogLine>, u32, u32) {
        let after = if after >= self.next_seq { 0 } else { after };
        let mut lines = Vec::new();
        let mut at = 0;
        let mut seq = self.first_seq;
        let (a, b) = self.bytes.as_slices();
        let byte = |i: usize| if i < a.len() { a[i] } else { b[i - a.len()] };
        let word = |i: usize| u32::from_le_bytes([byte(i), byte(i + 1), byte(i + 2), byte(i + 3)]);
        while at + HEADER <= self.bytes.len() && lines.len() < limit {
            let len = u16::from_le_bytes([byte(at + 9), byte(at + 10)]) as usize;
            if seq > after {
                let text: Vec<u8> = (at + HEADER..at + HEADER + len).map(byte).collect();
                lines.push(LogLine {
                    seq: word(at),
                    t: word(at + 4),
                    level: byte(at + 8) as u32,
                    text: String::from_utf8_lossy(&text).into_owned(),
                });
            }
            at += HEADER + len;
            seq = seq.saturating_add(1);
        }
        let next = lines
            .last()
            .map_or(after.max(self.first_seq.saturating_sub(1)), |l| l.seq);
        let dropped = self.first_seq.saturating_sub(after.saturating_add(1));
        (lines, next, dropped)
    }
}

static RING: Mutex<Option<Ring>> = Mutex::new(None);
/// Optional mirror (RTC memory on the board) for the crash "last words".
static MIRROR: Mutex<Option<Mirror>> = Mutex::new(None);

/// Receives (level, ms since boot, text) for each stored line.
pub type Mirror = fn(u8, u32, &str);
static PREVIOUS: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));

/// Allocates the ring (call once, early). Before this, lines are dropped.
pub fn init(capacity: usize) {
    let mut ring = RING.lock().unwrap_or_else(|e| e.into_inner());
    if ring.is_none() {
        *ring = Some(Ring::new(capacity));
    }
}

/// Also send each stored line to `mirror` (the board's RTC copy).
pub fn set_mirror(mirror: Mirror) {
    *MIRROR.lock().unwrap_or_else(|e| e.into_inner()) = Some(mirror);
}

/// Records what the previous boot left behind (board startup).
pub fn set_previous(reason: &str, last_lines: String) {
    *PREVIOUS.lock().unwrap_or_else(|e| e.into_inner()) = (reason.to_string(), last_lines);
}

pub fn push(level: u8, text: &str) {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.is_empty() {
        return;
    }
    let t = crate::uptime_ms() as u32;
    // try_lock: a log call from inside a log call (or a stuck holder) must
    // never block the caller; losing a line is the lesser evil.
    if let Ok(mut ring) = RING.try_lock() {
        if let Some(ring) = ring.as_mut() {
            ring.push(t, level, text);
        }
    }
    if let Ok(mirror) = MIRROR.try_lock() {
        if let Some(mirror) = *mirror {
            mirror(level, t, text);
        }
    }
}

pub fn level_char(level: u8) -> char {
    match level {
        ERROR => 'E',
        WARN => 'W',
        INFO => 'I',
        DEBUG => 'D',
        _ => 'V',
    }
}

/// A page of the log for the API.
pub fn page(after: u32, limit: usize) -> LogPage {
    let (lines, next, dropped, capacity) =
        match RING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            Some(ring) => {
                let (lines, next, dropped) = ring.page(after, limit);
                (lines, next, dropped, ring.capacity as u32)
            }
            None => (Vec::new(), after, 0, 0),
        };
    let previous = PREVIOUS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    LogPage {
        lines,
        next,
        dropped,
        previous_reason: previous.0,
        previous: previous.1,
        capacity,
    }
}

/// The page as plain text (`/api/v1/log.txt`).
pub fn text(page: &LogPage) -> String {
    let mut out = String::new();
    if !page.previous.is_empty() {
        let _ = writeln!(
            out,
            "--- previous boot ended: {} ---\n{}\n--- this boot ---",
            page.previous_reason,
            page.previous.trim_end()
        );
    }
    if page.dropped > 0 {
        let _ = writeln!(out, "({} older lines dropped)", page.dropped);
    }
    for l in &page.lines {
        let _ = writeln!(
            out,
            "{} {:>9.3} {}",
            level_char(l.level as u8),
            l.t as f64 / 1000.0,
            l.text
        );
    }
    out
}

/// Strips ESP-IDF's colour codes and `X (1234) ` prefix, returning the
/// level it named. C log lines arrive as `\x1b[0;31mE (1234) tag: text\x1b[0m\n`.
/// Returns a slice of the input: this runs inside ESP-IDF's log hook,
/// where allocating is best avoided.
pub fn parse_idf_line(raw: &str) -> (u8, &str) {
    let mut s = raw;
    if let Some(rest) = s.strip_prefix('\x1b') {
        s = rest.split_once('m').map_or(rest, |(_, after)| after);
    }
    if let Some(at) = s.find('\x1b') {
        s = &s[..at];
    }
    let s = s.trim_end();
    let bytes = s.as_bytes();
    let level = match bytes.first() {
        Some(b'E') => ERROR,
        Some(b'W') => WARN,
        Some(b'I') => INFO,
        Some(b'D') => DEBUG,
        Some(b'V') => 5,
        _ => return (INFO, s),
    };
    // "E (1234) tag: text" -> "tag: text" (the ring has its own timestamp).
    if bytes.get(1) == Some(&b' ') && bytes.get(2) == Some(&b'(') {
        if let Some(close) = s.find(") ") {
            return (level, &s[close + 2..]);
        }
    }
    (INFO, s)
}

/// The framework's `log` implementation: writes each line to the console
/// (stderr on desktop, the USB/UART console on a board) and into the ring.
pub struct Logger {
    pub level: log::LevelFilter,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            log::Level::Error => ERROR,
            log::Level::Warn => WARN,
            log::Level::Info => INFO,
            log::Level::Debug => DEBUG,
            log::Level::Trace => 5,
        };
        // Format into a fixed stack buffer: no allocation per line.
        let mut line = LineBuf::default();
        let target = record.target();
        let target = target.rsplit("::").next().unwrap_or(target);
        let _ = write!(line, "{target}: {}", record.args());
        let text = line.as_str();
        eprintln!("{} ({}) {text}", level_char(level), crate::uptime_ms());
        push(level, text);
    }

    fn flush(&self) {}
}

/// A line formatted on the stack, cut at `MAX_LINE` bytes.
pub struct LineBuf {
    buf: [u8; MAX_LINE],
    len: usize,
}

impl Default for LineBuf {
    fn default() -> Self {
        Self {
            buf: [0; MAX_LINE],
            len: 0,
        }
    }
}

impl LineBuf {
    pub fn as_str(&self) -> &str {
        match std::str::from_utf8(&self.buf[..self.len]) {
            Ok(s) => s,
            Err(e) => std::str::from_utf8(&self.buf[..e.valid_up_to()]).unwrap_or(""),
        }
    }
}

impl std::fmt::Write for LineBuf {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let room = MAX_LINE - self.len;
        let n = s.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Installs [`Logger`] (info and above) and allocates the ring.
pub fn install(capacity: usize) {
    init(capacity);
    static LOGGER: Logger = Logger {
        level: log::LevelFilter::Info,
    };
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_cursor_and_sequence_exhaustion_start_a_new_epoch() {
        let mut ring = Ring::new(100);
        ring.push(0, INFO, "old");
        assert_eq!(ring.page(u32::MAX, 10).0[0].text, "old");
        ring.next_seq = u32::MAX - 1;
        ring.push(1, INFO, "last");
        ring.push(2, INFO, "new epoch");
        let (lines, next, dropped) = ring.page(u32::MAX - 1, 10);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "new epoch");
        assert_eq!((next, dropped), (1, 0));
        for capacity in 0..=HEADER {
            let mut tiny = Ring::new(capacity);
            tiny.push(0, INFO, "é");
            assert!(tiny.page(u32::MAX, usize::MAX).0.is_empty());
        }
    }

    #[test]
    fn ring_keeps_newest_lines_and_pages() {
        let mut ring = Ring::new(200);
        for i in 0..20 {
            ring.push(i * 10, INFO, &format!("line {i}"));
        }
        let (lines, next, dropped) = ring.page(0, 100);
        // 200 bytes hold 11 + 6 or 7 bytes per line: about 11 lines.
        assert!(lines.len() >= 10 && lines.len() <= 12, "{}", lines.len());
        assert_eq!(lines.last().unwrap().text, "line 19");
        assert_eq!(next, 20);
        assert_eq!(dropped, lines[0].seq - 1);
        // Paging: nothing new after `next`, everything after an older seq.
        assert!(ring.page(next, 100).0.is_empty());
        let (newer, _, _) = ring.page(17, 100);
        assert_eq!(
            newer.iter().map(|l| l.seq).collect::<Vec<_>>(),
            [18, 19, 20]
        );
        let (two, n2, _) = ring.page(0, 2);
        assert_eq!(two.len(), 2);
        assert_eq!(n2, two[1].seq);
    }

    #[test]
    fn long_and_unicode_lines_are_cut_cleanly() {
        let mut ring = Ring::new(4096);
        ring.push(0, WARN, &"é".repeat(300));
        let (lines, _, _) = ring.page(0, 10);
        assert!(lines[0].text.len() <= MAX_LINE);
        assert!(lines[0].text.chars().all(|c| c == 'é'));
        assert_eq!(lines[0].level, WARN as u32);
    }

    #[test]
    fn idf_lines_are_parsed() {
        assert_eq!(
            parse_idf_line("\x1b[0;31mE (12345) esp-tls-mbedtls: read error :-0x7880:\x1b[0m\n"),
            (ERROR, "esp-tls-mbedtls: read error :-0x7880:")
        );
        assert_eq!(
            parse_idf_line("I (5) wifi:connected"),
            (INFO, "wifi:connected")
        );
        assert_eq!(parse_idf_line("plain text\n"), (INFO, "plain text"));
    }

    #[test]
    fn line_buffer_cuts_at_the_limit() {
        let mut b = LineBuf::default();
        let _ = write!(b, "{}", "x".repeat(1000));
        assert_eq!(b.as_str().len(), MAX_LINE);
    }

    #[test]
    fn text_view() {
        let page = LogPage {
            lines: vec![LogLine {
                seq: 3,
                t: 1500,
                level: 2,
                text: "tls: failed".into(),
            }],
            next: 3,
            dropped: 2,
            previous: "E 99 panic: boom".into(),
            previous_reason: "crash".into(),
            capacity: 100,
        };
        let t = text(&page);
        assert!(t.contains("previous boot ended: crash"));
        assert!(t.contains("(2 older lines dropped)"));
        assert!(t.contains("W     1.500 tls: failed"));
    }
}
