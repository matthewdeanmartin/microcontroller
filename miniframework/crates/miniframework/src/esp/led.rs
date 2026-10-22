//! The status-light task: samples health every 50 ms and drives the LED.
//! All patterns and their meaning are in `crate::status` (tested on the
//! desktop); this file only reads signals and sets the light, either a GPIO
//! or one WS2812 pixel over RMT (the RMT timing is mastomini's, measured on
//! the ESP32-S3 N16R8).
use super::{reset_reason, spawn_worker, Led, LedOutput};
use crate::status::{Light, State, SIGNALS};
use crate::sys::STATS;
use esp_idf_svc::hal::gpio::{AnyOutputPin, Level};
use esp_idf_svc::hal::rmt::config::{TransmitConfig, TxChannelConfig};
use esp_idf_svc::hal::rmt::encoder::CopyEncoder;
use esp_idf_svc::hal::rmt::{PinState, RmtChannel, Symbol, TxChannelDriver};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::sys;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};
use std::time::Duration;

/// A connection or TLS failure keeps the light "degraded" this long.
const ERROR_SHOWN_MS: u64 = 10_000;
/// A failed startup, stall or degraded state lights an RGB pixel this colour.
const TROUBLE: [u8; 3] = [32, 0, 0];
/// Connecting (no Wi-Fi) on an RGB pixel.
const CONNECTING: [u8; 3] = [24, 12, 0];

static STATE: AtomicU8 = AtomicU8::new(0);
/// A light was started (boards without one report no status).
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The state the light is showing, for `/api/v1/sys`.
pub fn state_name() -> &'static str {
    if !STARTED.load(Relaxed) {
        return "";
    }
    if SIGNALS.is_fatal() {
        return "failed";
    }
    match STATE.load(Relaxed) {
        1 => "connecting",
        2 => "starting",
        3 => "healthy",
        4 => "degraded",
        5 => "stalled",
        _ => "booting",
    }
}

fn code(state: State) -> u8 {
    match state {
        State::Connecting => 1,
        State::Starting => 2,
        State::Healthy => 3,
        State::Degraded => 4,
        State::Stalled => 5,
    }
}

/// Drives the light from health until a write fails. `show(None)` is dark;
/// `show(Some(rgb))` is lit (a plain LED ignores the colour).
fn run(mut light: Light, color: [u8; 3], mut show: impl FnMut(Option<[u8; 3]>) -> Result<(), sys::EspError>) {
    STARTED.store(true, Relaxed);
    let mut errors = 0u32;
    let mut error_until = 0u64;
    let mut previous = None;
    loop {
        let now = crate::uptime_ms();
        let seen = STATS
            .tls_failures
            .load(Relaxed)
            .wrapping_add(STATS.rejected.load(Relaxed));
        if seen != errors {
            errors = seen;
            error_until = now + ERROR_SHOWN_MS;
        }
        let state = SIGNALS.health(now < error_until).state();
        STATE.store(code(state), Relaxed);
        let fatal = SIGNALS.is_fatal().then(|| SIGNALS.current_stage());
        let on = light.on(now, fatal, state);
        // The rhythm carries the meaning; on a pixel, the colour helps.
        let lit = on.then(|| match (fatal, state) {
            (Some(_), _) | (None, State::Stalled | State::Degraded) => TROUBLE,
            (None, State::Connecting) => CONNECTING,
            _ => color,
        });
        if previous != Some(lit) {
            if let Err(e) = show(lit) {
                STARTED.store(false, Relaxed);
                log::warn!("Status light write failed ({e}); no light");
                return;
            }
            previous = Some(lit);
        }
        if now % 10_000 < 50 {
            // SAFETY: this task's own stack.
            super::LED_STACK_FREE.store(
                unsafe { sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut()) },
                Relaxed,
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One WS2812 GRB pixel on an RMT channel (mastomini's timing), for as long
/// as `run` keeps going.
fn pixel(pin: AnyOutputPin<'static>, light: Light, color: [u8; 3]) -> Result<(), sys::EspError> {
    let config = TxChannelConfig {
        resolution: Hertz(10_000_000),
        transaction_queue_depth: 1,
        ..Default::default()
    };
    let mut channel = TxChannelDriver::new(pin, &config)?;
    let symbol = |high_ns, low_ns| {
        Symbol::new_with(
            config.resolution,
            PinState::High,
            Duration::from_nanos(high_ns),
            PinState::Low,
            Duration::from_nanos(low_ns),
        )
    };
    let (zero, one) = (symbol(300, 900)?, symbol(800, 400)?);
    let latch = Symbol::new_half_split(config.resolution, PinState::Low, PinState::Low, Duration::from_micros(300))?;
    let mut queue = channel.queue([CopyEncoder::new()?]);
    let transmit = TransmitConfig {
        queue_non_blocking: true,
        ..Default::default()
    };
    run(light, color, |lit| {
        let [r, g, b] = lit.unwrap_or([0, 0, 0]);
        let mut symbols = [latch; 25];
        for (i, byte) in [g, r, b].into_iter().enumerate() {
            for bit in 0..8 {
                symbols[i * 8 + bit] = if byte & (0x80 >> bit) == 0 { zero } else { one };
            }
        }
        // The queue keeps its copy of the frame until RMT is done with it.
        queue.push(&symbols, &transmit)?;
        queue.channel().wait_all_done(Some(Duration::from_millis(20)))
    });
    // Stop the peripheral before the queue's buffers go; if that fails, keep
    // them (a few bytes) rather than free memory RMT may still read.
    if queue.channel().disable().is_err() {
        std::mem::forget(queue);
    }
    Ok(())
}

pub fn start(led: Led, psram_stack: bool) {
    let reason = reset_reason();
    let light = Light::new(led.phrase, reason);
    let kind = match &led.output {
        LedOutput::Gpio { .. } => "LED",
        LedOutput::Ws2812 { .. } => "WS2812 pixel",
    };
    log::info!(
        "Status {kind}: reset \"{reason}\" ({} flashes); healthy spells \"{}\"",
        light.flashes(),
        led.phrase
    );
    // Never writes flash, so its stack may live in PSRAM (RMT needs a little more).
    let stack = if matches!(led.output, LedOutput::Ws2812 { .. }) { 6 * 1024 } else { 4096 };
    let spawned = spawn_worker(c"mf-led", stack, psram_stack, move || match led.output {
        LedOutput::Gpio {
            mut driver,
            active_high,
        } => run(light, [0, 0, 0], |lit| {
            driver.set_level(if lit.is_some() == active_high { Level::High } else { Level::Low })
        }),
        LedOutput::Ws2812 { pin, color } => {
            if let Err(e) = pixel(pin, light, color) {
                log::warn!("Status pixel: RMT setup failed ({e}); no light");
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("Status LED task failed to start ({e}); no light");
    }
}
