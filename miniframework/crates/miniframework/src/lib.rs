//! miniframework: small web apps served from ESP32 boards.
//!
//! An app is a [`Service`] (your API) plus a bundled Angular site. The
//! framework supplies the rest: a nonblocking HTTP/HTTPS connection loop
//! sized for a 2 MiB board, five wire formats from one schema
//! ([`message!`]), optional gzip, static files, the household-CA trust page,
//! system information, a key-value store and an HTTP client, with the same
//! code running on the desktop for development.
//!
//! Start with `docs/RECIPES.md` and the `housemetrics` app.
pub mod events;
pub mod fetch;
pub mod http;
pub mod influx;
pub mod kv;
pub mod logbuf;
pub mod mux;
pub mod site;
pub mod status;
pub mod sys;
pub mod web;
pub mod wire;

#[cfg(not(target_os = "espidf"))]
pub mod desktop;

#[cfg(all(feature = "esp32", target_os = "espidf"))]
pub mod esp;

pub use site::{ApiError, Config, Reply, Request, Service, Site};
pub use wire::{Encode, Format, Message, Writer};

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the process (or board) started serving. Runners call
/// it at startup so it counts from boot.
pub fn uptime_ms() -> u64 {
    STARTED.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Unix time in ms, or `None` while the clock is unset (a board before
/// SNTP has answered reports 1970).
pub fn wall_ms() -> Option<u64> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    // 2024-01-01: anything earlier is an unsynchronized clock.
    (ms >= 1_704_067_200_000).then_some(ms)
}
