//! The status light: what the board's LED says, as pure functions of time.
//!
//! Ported from NanaCoin's single-colour S2 policy (`board_status.rs`). The
//! hardware side is `esp::led`; everything here runs and is tested on the
//! desktop. All patterns are on/off, so they work on a plain LED such as
//! the S2 Mini's blue GPIO 15.
//!
//! What the LED shows, in priority order:
//!
//! 1. **Power-on self test (POST), first ~3 s after boot:** 1.5 s on (the
//!    firmware started), 0.5 s off, then 1–6 short flashes for why the
//!    board reset: 1 power on, 2 reset button / software / USB, 3 crash,
//!    4 watchdog, 5 brownout, 6 other.
//! 2. **Startup failed:** the number of the step that failed, as blinks
//!    (0.2 s on, 0.2 s off), then 1.6 s dark, repeating. Steps: see
//!    [`Stage`].
//! 3. **Running**, one rhythm per state (2-second cycle unless noted):
//!
//! | State | Rhythm |
//! |---|---|
//! | Starting (Wi-Fi up, server not yet listening) | solid on |
//! | Connecting (no Wi-Fi) | slow blink: 1 s on, 1 s off |
//! | Degraded (no mDNS, or a TLS/connection failure in the last 10 s) | mostly on: 1.75 s on, 0.25 s off |
//! | Stalled (the serving loop stopped turning) | fast blink, 5 per second |
//! | Healthy | the app's name in Morse, then three slow blinks, then a pause |
//!
//! Healthy is the only state that spells anything, so any Morse at all
//! means "serving".
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering::Relaxed};

/// Startup steps; a failed startup blinks the step's number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    /// Peripherals and the system event loop.
    System = 1,
    /// The NVS flash partition.
    Storage = 2,
    /// Wi-Fi driver and configuration.
    WifiDriver = 3,
    /// Joining Wi-Fi (retries forever; the LED shows Connecting meanwhile).
    WifiJoin = 4,
    /// SNTP and mDNS.
    Network = 5,
    /// The app's own setup (key-value store, data structures).
    App = 6,
    /// Opening the HTTP and HTTPS listeners.
    Listen = 7,
}

/// The first number an app may use for its own startup steps
/// ([`Signals::step`]).
pub const FIRST_APP_STEP: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    Connecting,
    Healthy,
    Degraded,
    Stalled,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Connecting => "connecting",
            State::Healthy => "healthy",
            State::Degraded => "degraded",
            State::Stalled => "stalled",
        }
    }
}

/// Inputs to the state, sampled by the LED task.
#[derive(Clone, Copy, Debug)]
pub struct Health {
    pub ready: bool,
    pub wifi: bool,
    pub mdns: bool,
    /// The serving loop (and handshake task) turned recently.
    pub turning: bool,
    pub recent_error: bool,
}

impl Health {
    pub fn state(&self) -> State {
        if self.ready && !self.turning {
            State::Stalled
        } else if !self.wifi {
            State::Connecting
        } else if !self.ready {
            State::Starting
        } else if !self.mdns || self.recent_error {
            State::Degraded
        } else {
            State::Healthy
        }
    }
}

/// The non-Morse rhythms. Healthy here is the fallback heartbeat (on, with
/// a dark beat every 2 s) for an app without a name to spell.
pub fn rhythm(state: State, elapsed_ms: u64) -> bool {
    let t = elapsed_ms % 2000;
    match state {
        State::Starting => true,
        State::Connecting => t < 1000,
        State::Healthy => t >= 150,
        State::Degraded => t < 1750,
        State::Stalled => elapsed_ms % 200 < 100,
    }
}

/// A failed startup: blink the step number, then stay dark 1.6 s.
pub fn stage_code(stage: u8, elapsed_ms: u64) -> bool {
    if stage == 0 {
        return elapsed_ms % 200 < 100;
    }
    let blinks = u64::from(stage) * 400;
    let t = elapsed_ms % (blinks + 1600);
    t < blinks && t % 400 < 200
}

/// Flashes for a reset reason as `esp::reset_reason` names it.
pub fn reset_flashes(reason: &str) -> u64 {
    match reason {
        "power on" => 1,
        "software restart" | "reset pin" | "USB" => 2,
        "crash" => 3,
        "watchdog" => 4,
        "brownout" => 5,
        _ => 6,
    }
}

/// The POST marker: `Some(on)` while it plays, `None` once it is over.
pub fn post(elapsed_ms: u64, flashes: u64) -> Option<bool> {
    if elapsed_ms < 1500 {
        return Some(true);
    }
    if elapsed_ms < 2000 {
        return Some(false);
    }
    let t = elapsed_ms - 2000;
    if t >= flashes * 300 + 500 {
        None
    } else {
        Some(t < flashes * 300 && t % 300 < 100)
    }
}

/// One Morse unit. 200 ms is six words a minute: slow enough to read by eye.
pub const MORSE_UNIT_MS: u64 = 200;

fn morse(ch: u8) -> Option<&'static str> {
    Some(match ch.to_ascii_uppercase() {
        b'A' => ".-",
        b'B' => "-...",
        b'C' => "-.-.",
        b'D' => "-..",
        b'E' => ".",
        b'F' => "..-.",
        b'G' => "--.",
        b'H' => "....",
        b'I' => "..",
        b'J' => ".---",
        b'K' => "-.-",
        b'L' => ".-..",
        b'M' => "--",
        b'N' => "-.",
        b'O' => "---",
        b'P' => ".--.",
        b'Q' => "--.-",
        b'R' => ".-.",
        b'S' => "...",
        b'T' => "-",
        b'U' => "..-",
        b'V' => "...-",
        b'W' => ".--",
        b'X' => "-..-",
        b'Y' => "-.--",
        b'Z' => "--..",
        b'0' => "-----",
        b'1' => ".----",
        b'2' => "..---",
        b'3' => "...--",
        b'4' => "....-",
        b'5' => ".....",
        b'6' => "-....",
        b'7' => "--...",
        b'8' => "---..",
        b'9' => "----.",
        b'.' => ".-.-.-",
        b',' => "--..--",
        b'?' => "..--..",
        b'!' => "-.-.--",
        b'-' => "-....-",
        b'/' => "-..-.",
        b'@' => ".--.-.",
        b'_' => "..--.-",
        _ => return None,
    })
}

/// The healthy pattern: `phrase` in Morse (dot 1 unit, dash 3, 1 between
/// marks, 3 between letters, 7 between words), 7 units dark, three slow
/// blinks (3 on, 3 off), then a pause of `PAUSE_UNITS`. Characters with
/// no Morse code are skipped.
pub struct Morse {
    /// (start, end) in units.
    marks: Vec<(u32, u32)>,
    message_end: u32,
    cycle: u32,
}

/// Dark time after the three blinks before the message repeats (3 s).
pub const PAUSE_UNITS: u32 = 15;

impl Morse {
    pub fn new(phrase: &str) -> Self {
        let mut marks = Vec::new();
        let mut t = 0u32;
        for (w, word) in phrase.split_whitespace().enumerate() {
            if w > 0 {
                t += 7;
            }
            let mut first_letter = true;
            for code in word.bytes().filter_map(morse) {
                if !first_letter {
                    t += 3;
                }
                first_letter = false;
                for (i, mark) in code.bytes().enumerate() {
                    if i > 0 {
                        t += 1;
                    }
                    let end = t + if mark == b'.' { 1 } else { 3 };
                    marks.push((t, end));
                    t = end;
                }
            }
        }
        Self {
            marks,
            message_end: t,
            cycle: t + 7 + 18 + PAUSE_UNITS,
        }
    }

    pub fn cycle_ms(&self) -> u64 {
        u64::from(self.cycle) * MORSE_UNIT_MS
    }

    pub fn on(&self, elapsed_ms: u64) -> bool {
        let t = (elapsed_ms / MORSE_UNIT_MS % u64::from(self.cycle)) as u32;
        if t < self.message_end {
            // Marks are sorted; find the last one starting at or before t.
            let i = self.marks.partition_point(|&(start, _)| start <= t);
            i > 0 && t < self.marks[i - 1].1
        } else {
            let after = t - self.message_end;
            (7..25).contains(&after) && (after - 7) % 6 < 3
        }
    }
}

/// Signals from the rest of the firmware. Plain atomics: setting them never
/// blocks, and the LED task only reads them.
pub struct Signals {
    stage: AtomicU8,
    fatal: AtomicBool,
    wifi: AtomicBool,
    mdns: AtomicBool,
    ready: AtomicBool,
    /// Uptime (ms) of the last serving-loop and handshake-task turns.
    serve_beat: AtomicU32,
    tls_beat: AtomicU32,
    tls_running: AtomicBool,
    /// The setup network is open (`esp::WifiSetup`).
    setup: AtomicBool,
}

pub static SIGNALS: Signals = Signals {
    stage: AtomicU8::new(0),
    fatal: AtomicBool::new(false),
    wifi: AtomicBool::new(false),
    mdns: AtomicBool::new(false),
    ready: AtomicBool::new(false),
    serve_beat: AtomicU32::new(0),
    tls_beat: AtomicU32::new(0),
    tls_running: AtomicBool::new(false),
    setup: AtomicBool::new(false),
};

/// A loop that has not turned for this long counts as stalled.
pub const STALL_MS: u32 = 10_000;

fn now_ms() -> u32 {
    crate::uptime_ms() as u32
}

impl Signals {
    pub fn stage(&self, stage: Stage) {
        log::info!("startup step {}: {stage:?}", stage as u8);
        self.stage.store(stage as u8, Relaxed);
    }
    /// An app's own startup step, numbered from [`FIRST_APP_STEP`] (the
    /// app documents what its numbers mean). A failed startup blinks it.
    pub fn step(&self, number: u8, name: &str) {
        debug_assert!(number >= FIRST_APP_STEP, "steps 1-7 are the framework's");
        log::info!("startup step {number}: {name}");
        self.stage.store(number, Relaxed);
    }
    pub fn current_stage(&self) -> u8 {
        self.stage.load(Relaxed)
    }
    /// Startup failed: blink the current stage until reset.
    pub fn fatal(&self) {
        self.fatal.store(true, Relaxed);
    }
    pub fn is_fatal(&self) -> bool {
        self.fatal.load(Relaxed)
    }
    pub fn wifi(&self, up: bool) {
        self.wifi.store(up, Relaxed);
    }
    pub fn mdns(&self, up: bool) {
        self.mdns.store(up, Relaxed);
    }
    /// The board's setup network opened or closed (an app's light shows it).
    pub fn setup(&self, open: bool) {
        self.setup.store(open, Relaxed);
    }
    pub fn in_setup(&self) -> bool {
        self.setup.load(Relaxed)
    }
    pub fn ready(&self) {
        self.serve_beat();
        self.ready.store(true, Relaxed);
    }
    pub fn serve_beat(&self) {
        self.serve_beat.store(now_ms(), Relaxed);
    }
    pub fn tls_beat(&self) {
        self.tls_running.store(true, Relaxed);
        self.tls_beat.store(now_ms(), Relaxed);
    }

    /// Health right now. `recent_error` comes from the caller (it compares
    /// error counters between samples).
    pub fn health(&self, recent_error: bool) -> Health {
        let now = now_ms();
        let fresh = |beat: &AtomicU32| now.wrapping_sub(beat.load(Relaxed)) < STALL_MS;
        Health {
            ready: self.ready.load(Relaxed),
            wifi: self.wifi.load(Relaxed),
            mdns: self.mdns.load(Relaxed),
            turning: fresh(&self.serve_beat)
                && (!self.tls_running.load(Relaxed) || fresh(&self.tls_beat)),
            recent_error,
        }
    }
}

/// Everything the LED does, as one function of time and health.
pub struct Light {
    morse: Morse,
    flashes: u64,
    healthy_since: Option<u64>,
}

impl Light {
    pub fn new(phrase: &str, reset_reason: &str) -> Self {
        Self {
            morse: Morse::new(phrase),
            flashes: reset_flashes(reset_reason),
            healthy_since: None,
        }
    }

    pub fn flashes(&self) -> u64 {
        self.flashes
    }

    /// LED on or off at `now` (ms since boot). Morse starts from its first
    /// letter each time the board becomes healthy.
    pub fn on(&mut self, now: u64, fatal: Option<u8>, state: State) -> bool {
        if let Some(stage) = fatal {
            self.healthy_since = None;
            return stage_code(stage, now);
        }
        if let Some(on) = post(now, self.flashes) {
            return on;
        }
        if state != State::Healthy {
            self.healthy_since = None;
            return rhythm(state, now);
        }
        if self.morse.marks.is_empty() {
            return rhythm(State::Healthy, now);
        }
        let since = *self.healthy_since.get_or_insert(now);
        self.morse.on(now - since)
    }
}

/// The plain-text page a board serves on port 8080 after a failed startup,
/// so a board with no console (the S2's USB attaches late) still explains
/// itself to anyone who can reach its address.
pub fn failure_report(
    app: &str,
    host: &str,
    step: u8,
    error: &str,
    heap: &crate::sys::Heap,
) -> String {
    format!(
        "{app} ({host}) startup failed at step {step}: {error}\n\
         internal free {} largest {}; psram free {} largest {}\n\
         The status light blinks {step} times, pauses, and repeats.\n",
        heap.internal_free, heap.internal_largest, heap.psram_free, heap.psram_largest
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(f: impl Fn(u64) -> bool, until_ms: u64) -> Vec<bool> {
        (0..until_ms).step_by(50).map(f).collect()
    }

    #[test]
    fn states_have_distinct_rhythms_and_none_looks_like_morse() {
        let states = [
            State::Starting,
            State::Connecting,
            State::Degraded,
            State::Stalled,
        ];
        for (i, a) in states.iter().enumerate() {
            for b in &states[i + 1..] {
                assert_ne!(
                    trace(|t| rhythm(*a, t), 2000),
                    trace(|t| rhythm(*b, t), 2000),
                    "{a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn health_priorities() {
        let mut h = Health {
            ready: true,
            wifi: true,
            mdns: true,
            turning: true,
            recent_error: false,
        };
        assert_eq!(h.state(), State::Healthy);
        h.recent_error = true;
        assert_eq!(h.state(), State::Degraded);
        h.recent_error = false;
        h.mdns = false;
        assert_eq!(h.state(), State::Degraded);
        h.wifi = false;
        assert_eq!(h.state(), State::Connecting);
        h.turning = false;
        assert_eq!(
            h.state(),
            State::Stalled,
            "a stuck server never looks merely disconnected"
        );
        h = Health {
            ready: false,
            wifi: true,
            mdns: false,
            turning: false,
            recent_error: false,
        };
        assert_eq!(
            h.state(),
            State::Starting,
            "not stalled before the server ever started"
        );
    }

    #[test]
    fn stage_code_blinks_the_step_number() {
        for stage in 1..=7u8 {
            let cycle = u64::from(stage) * 400 + 1600;
            let ons = trace(|t| stage_code(stage, t), cycle);
            let rises = ons.windows(2).filter(|w| !w[0] && w[1]).count() + usize::from(ons[0]);
            assert_eq!(rises, usize::from(stage));
        }
    }

    #[test]
    fn post_marker_then_reset_flashes() {
        assert_eq!(post(0, 4), Some(true));
        assert_eq!(post(1600, 4), Some(false));
        assert_eq!(post(2000, 4), Some(true));
        assert_eq!(post(2150, 4), Some(false));
        let flashes = (2000..3700)
            .step_by(50)
            .filter(|&t| post(t, 4) == Some(true))
            .count();
        assert_eq!(flashes, 4 * 2, "four 100 ms flashes sampled every 50 ms");
        assert_eq!(post(3700, 4), None);
        assert_eq!(reset_flashes("power on"), 1);
        assert_eq!(reset_flashes("brownout"), 5);
    }

    #[test]
    fn morse_timing_matches_the_standard() {
        let m = Morse::new("E T");
        // E = dot at 0; word gap 7; T = dash 8..11.
        assert_eq!(m.marks, vec![(0, 1), (8, 11)]);
        assert!(m.on(0));
        assert!(!m.on(200));
        assert!(m.on(1600) && m.on(2100) && !m.on(2200));
        let blinks: Vec<u32> = (m.message_end..m.cycle)
            .filter(|&t| m.on(u64::from(t) * 200))
            .collect();
        assert_eq!(blinks, vec![18, 19, 20, 24, 25, 26, 30, 31, 32]);
        assert_eq!(m.on(m.cycle_ms()), m.on(0), "repeats");
        // Letters within a word: 3 units apart.
        assert_eq!(
            Morse::new("AA").marks,
            vec![(0, 1), (2, 5), (8, 9), (10, 13)]
        );
    }

    #[test]
    fn housemetrics_spells_and_pauses() {
        let m = Morse::new("housemetrics");
        // h .... o --- u ..- s ... e . m -- e . t - r .-. i .. c -.-. s ...
        assert_eq!(m.marks.len(), 4 + 3 + 3 + 3 + 1 + 2 + 1 + 1 + 3 + 2 + 4 + 3);
        let seconds = m.cycle_ms() as f64 / 1000.0;
        assert!((20.0..40.0).contains(&seconds), "one cycle is {seconds} s");
        // The last 3 s are dark.
        let end = m.cycle_ms();
        assert!((end - 3000..end).step_by(100).all(|t| !m.on(t)));
    }

    #[test]
    fn light_priorities() {
        let mut light = Light::new("E", "watchdog");
        assert!(light.on(0, None, State::Healthy), "POST first");
        assert!(
            light.on(0, Some(3), State::Healthy),
            "a failed stage overrides POST"
        );
        let after_post = 2000 + 4 * 300 + 500;
        assert_eq!(
            light.on(after_post, None, State::Connecting),
            rhythm(State::Connecting, after_post)
        );
        // Healthy starts the message from its beginning.
        assert!(light.on(after_post + 1000, None, State::Healthy));
        assert!(!light.on(after_post + 1200, None, State::Healthy));
    }
    #[test]
    fn failure_report_names_the_step_error_and_memory() {
        let heap = crate::sys::Heap {
            internal_free: 31_000,
            internal_largest: 12_000,
            psram_free: 900_000,
            psram_largest: 500_000,
            ..Default::default()
        };
        let text = failure_report(
            "nanacoin",
            "nanacoin-s2.local",
            9,
            "journal storage: Corrupt",
            &heap,
        );
        assert!(text.starts_with(
            "nanacoin (nanacoin-s2.local) startup failed at step 9: journal storage: Corrupt\n"
        ));
        assert_eq!(text.lines().count(), 3);
        assert!(text.lines().all(|line| !line.starts_with(' ')), "{text}");
        assert!(
            text.contains("internal free 31000 largest 12000; psram free 900000 largest 500000")
        );
        assert!(text.contains("blinks 9 times"));
    }

    #[test]
    fn app_steps_follow_the_framework_stages() {
        assert!(FIRST_APP_STEP > Stage::Listen as u8);
        let signals = Signals {
            stage: AtomicU8::new(0),
            fatal: AtomicBool::new(false),
            wifi: AtomicBool::new(false),
            mdns: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            serve_beat: AtomicU32::new(0),
            tls_beat: AtomicU32::new(0),
            tls_running: AtomicBool::new(false),
            setup: AtomicBool::new(false),
        };
        signals.stage(Stage::App);
        signals.step(FIRST_APP_STEP + 2, "ledger replay");
        assert_eq!(signals.current_stage(), 10);
    }
}
