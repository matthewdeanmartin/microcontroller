//! Board side of the incident history (`crate::incidents`):
//!
//! - its RTC mirror, which keeps the newest incidents and the counters
//!   across a crash, watchdog or software reset;
//! - ESP-IDF's failed-allocation hook, so every failed `heap_caps_*`
//!   allocation (an esp-aes DMA bounce buffer, an mbedTLS record, a Rust
//!   `try_reserve`) is an incident with its size and capabilities;
//! - the sampler: heap, Wi-Fi and worker progress every five seconds, from
//!   its own task, so it still reports when the serving loop is stuck.
use crate::incidents::{Kind, LOG, WORDS};
use core::ffi::c_char;
use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::sys;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

/// RTC memory keeps its contents across every reset but power loss. See
/// `logcap` for why these are atomics.
#[link_section = ".rtc_noinit"]
static RTC: [AtomicU32; WORDS] = [const { AtomicU32::new(0) }; WORDS];

/// Runs in whichever task's allocation failed, after the heap released its
/// lock: record only (no logging, no allocation).
unsafe extern "C" fn allocation_failed(size: usize, caps: u32, _function: *const c_char) {
    LOG.record(
        crate::uptime_ms(),
        Kind::AllocationFailed,
        caps as i32,
        0,
        size.min(u32::MAX as usize) as u32,
    );
}

/// Takes over the previous boot's mirror and starts this boot's. Called
/// from `esp::init`, before any task that could record.
pub(super) fn install() {
    // SAFETY: plain query.
    let reset = unsafe { sys::esp_reset_reason() } as i32;
    LOG.boot(crate::uptime_ms(), reset, Some(&RTC));
    // SAFETY: `allocation_failed` matches esp_alloc_failed_hook_t and is
    // re-entrant (atomics and a try-lock).
    let hooked = unsafe { sys::heap_caps_register_failed_alloc_callback(Some(allocation_failed)) };
    if hooked != sys::ESP_OK {
        log::warn!("incidents: failed-allocation hook not installed ({hooked})");
    }
}

/// Starts the sampler. It never writes flash, so a PSRAM stack is safe and
/// keeps internal RAM for what needs it.
pub(super) fn start_sampler(psram: bool, core: Option<Core>) {
    let spawned = super::spawn_task(c"mf-incidents", 6 * 1024, psram, core, 3, || loop {
        let (free, low, largest, _) =
            super::heap_info(sys::MALLOC_CAP_INTERNAL | sys::MALLOC_CAP_8BIT);
        let mut ap = sys::wifi_ap_record_t::default();
        // SAFETY: thread-safe query into a local record; fails cleanly
        // while the station is down or Wi-Fi is not started.
        let rssi =
            (unsafe { sys::esp_wifi_sta_get_ap_info(&mut ap) } == sys::ESP_OK).then_some(ap.rssi);
        LOG.sample(crate::uptime_ms(), free, largest, low, rssi);
        std::thread::sleep(Duration::from_secs(5));
    });
    if let Err(e) = spawned {
        log::warn!("incidents: sampler failed to start: {e}");
    }
}
