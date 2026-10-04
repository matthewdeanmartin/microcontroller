//! housemetrics: a household metrics store and dashboard, built on
//! miniframework. Boards push samples (Influx lines or any wire format) or
//! get scraped; the Angular app graphs them.
pub mod api;
pub mod auth;
pub mod deployment;
pub mod gorilla;
pub mod ingest;
pub mod messages;
pub mod scrape;
pub mod tsdb;
pub mod views;

use api::{App, Tuning};
use auth::{Admin, Devices};
use miniframework::influx::Precision;
use miniframework::kv::Kv;
use miniframework::sys::{influx_line, SysInfo};
use miniframework::wall_ms;
use scrape::Scrapes;
use std::sync::{Arc, Mutex};
use tsdb::{Limits, Store};

/// Sizes for one kind of machine.
pub struct Profile {
    pub store: Limits,
    pub tuning: Tuning,
    pub body_limit: usize,
    pub response_limit: usize,
}

impl Profile {
    /// ESP32-S2 Mini (2 MiB PSRAM): about 0.5 MiB for the store, leaving
    /// room for TLS sessions, connection buffers, responses and one gzip.
    pub fn s2() -> Self {
        Self {
            store: Limits {
                max_series: 64,
                block_bytes: 256,
                blocks: 1152,
                rollup_secs: 900,
                rollup_slots: 288,
            },
            tuning: Tuning {
                raw_page: 5000,
                max_points: 2000,
                max_ids: 8,
                max_rows: 2000,
            },
            body_limit: 8 * 1024,
            response_limit: 96 * 1024,
        }
    }

    pub fn desktop() -> Self {
        Self {
            store: Limits {
                max_series: 4096,
                block_bytes: 256,
                blocks: 256 * 1024,
                rollup_secs: 900,
                rollup_slots: 30 * 96,
            },
            tuning: Tuning {
                raw_page: 100_000,
                max_points: 10_000,
                max_ids: 32,
                max_rows: 100_000,
            },
            body_limit: 1024 * 1024,
            response_limit: 16 * 1024 * 1024,
        }
    }
}

pub fn build(
    kv: Arc<dyn Kv>,
    admin_password: &str,
    allow_http_admin: bool,
    profile: &Profile,
) -> App {
    App {
        store: Arc::new(Mutex::new(Store::new(profile.store.clone()))),
        devices: Devices::load(kv.clone()),
        scrapes: Arc::new(Scrapes::load(kv)),
        admin: Admin::new(admin_password, allow_http_admin),
        tuning: profile.tuning.clone(),
    }
}

/// Records the board's own health (heap, RSSI, requests...) as series.
pub fn record_self(store: &Mutex<Store>, info: &SysInfo) {
    let Some(now) = wall_ms() else { return };
    let line = influx_line(info, &[]);
    let mut store = store.lock().unwrap();
    ingest::write_lines(&mut store, &line, Precision::Ns, Some(now as i64), &[]);
}

/// Apply this build's managed targets before starting the scrape worker.
pub fn configure_scrapes(scrapes: &Scrapes) -> Result<(), miniframework::ApiError> {
    let config = deployment::parse(include_bytes!(concat!(env!("OUT_DIR"), "/scrapes.json")))
        .map_err(miniframework::ApiError::bad_request)?;
    scrapes.configure(&config.targets)
}
