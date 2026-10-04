//! The status-light task: samples health every 50 ms and drives one GPIO.
//! All patterns and their meaning are in `crate::status` (tested on the
//! desktop); this file only reads signals and sets a pin.
use super::{reset_reason, spawn_worker, Led};
use crate::status::{Light, State, SIGNALS};
use crate::sys::STATS;
use esp_idf_svc::sys;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};
use std::time::Duration;

/// A connection or TLS failure keeps the light "degraded" this long.
const ERROR_SHOWN_MS: u64 = 10_000;

static STATE: AtomicU8 = AtomicU8::new(0);

/// The state the light is showing, for `/api/v1/sys`.
pub fn state_name() -> &'static str {
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

pub fn start(led: Led, psram_stack: bool) {
    let pin = led.gpio;
    // SAFETY: plain GPIO configuration of a pin the app dedicated to the LED.
    let configured = unsafe {
        sys::gpio_reset_pin(pin) == sys::ESP_OK
            && sys::gpio_set_direction(pin, sys::gpio_mode_t_GPIO_MODE_OUTPUT) == sys::ESP_OK
    };
    if !configured {
        log::warn!("Status LED: GPIO {pin} could not be configured; no light");
        return;
    }
    let reason = reset_reason();
    let mut light = Light::new(led.phrase, reason);
    log::info!(
        "Status LED on GPIO {pin}: reset \"{reason}\" ({} flashes); healthy spells \"{}\"",
        light.flashes(),
        led.phrase
    );
    // Never writes flash, so its 4 KiB stack may live in PSRAM.
    let spawned = spawn_worker(c"mf-led", 4096, psram_stack, move || {
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
            if previous != Some(on) {
                // SAFETY: configured output pin, owned by this task.
                unsafe { sys::gpio_set_level(pin, u32::from(on == led.active_high)) };
                previous = Some(on);
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
    });
    if let Err(e) = spawned {
        log::warn!("Status LED task failed to start ({e}); no light");
    }
}
