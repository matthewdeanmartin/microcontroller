//! `GET /.well-known/ops/health`: the board judging itself, so an SRE bot
//! (mastomini-bots' `sre`) can read every board the same way instead of
//! knowing each app's admin API. Feature `ops`; see `spec/OPS.md`.
//!
//! - The framework adds its own checks from what it already knows:
//!   internal RAM, the largest free block, Wi-Fi signal, the clock, why
//!   the board last reset, the serving loop's health word, and failed
//!   allocations since boot ([`builtin`]).
//! - The app adds checks only it can judge (a store filling up, a journal
//!   overdue for a checkpoint) with [`Service::checks`](crate::Service::checks).
//! - Read-only and public, like `/metrics` and `/.well-known/incidents`:
//!   a check's summary must be safe to show anyone on the network. Never
//!   put names, messages or other people's data in one.
//!
//! A check is a judgement, not a history: `status` is how things are now.
//! Trends and "since when" are the reader's job (housemetrics, the bot).
use crate::message;
use crate::sys::SysInfo;

/// The schema id in every answer; bumped only for incompatible changes.
pub const SCHEMA: &str = "miniframework.health/1";

/// How bad a check is. Ordered: the worst check is the board's status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
        }
    }
}

message! {
    /// One judgement.
    pub struct Check {
        /// Stable dotted id (`heap.internal`, `store.entries`): the bot
        /// remembers checks by it. Never rename one that is in use.
        1 id: String,
        /// `ok`, `warn` or `fail`.
        2 status: String,
        /// One short line for people ("12 KiB free of internal RAM").
        3 summary: String,
        /// The measured value and the limit it was judged against, in
        /// `unit`; both 0 when the check is not a number.
        4 value: f64,
        5 limit: f64,
        6 unit: String,
        /// What to do about it: a runbook id from `spec/OPS.md`
        /// (`restart-planned`, `checkpoint`), or "" when there is none.
        7 runbook: String,
    }
}

message! {
    /// `GET /.well-known/ops/health`.
    pub struct Health {
        /// Always [`SCHEMA`].
        1 schema: String,
        2 app: String,
        3 host: String,
        4 version: String,
        5 build: String,
        6 uptime_ms: u64,
        /// The worst check's status.
        7 status: String,
        /// Framework checks first, then the app's.
        8 checks: Vec<Check>,
    }
}

impl Check {
    /// A check that is not a measurement.
    pub fn new(id: &str, status: Status, summary: impl Into<String>) -> Check {
        Check {
            id: id.into(),
            status: status.name().into(),
            summary: summary.into(),
            ..Default::default()
        }
    }

    /// A measurement judged against `limit` (in `unit`).
    pub fn measured(mut self, value: f64, limit: f64, unit: &str) -> Check {
        self.value = value;
        self.limit = limit;
        self.unit = unit.into();
        self
    }

    pub fn runbook(mut self, runbook: &str) -> Check {
        self.runbook = runbook.into();
        self
    }

    /// Judges `used` of `total` (entries, bytes): warn from `warn`%, fail
    /// from `fail`%. `what` names it in the summary ("store entries").
    pub fn usage(id: &str, what: &str, used: f64, total: f64, warn: f64, fail: f64) -> Check {
        let pct = if total > 0.0 {
            used * 100.0 / total
        } else {
            0.0
        };
        let status = if pct >= fail {
            Status::Fail
        } else if pct >= warn {
            Status::Warn
        } else {
            Status::Ok
        };
        Check::new(
            id,
            status,
            format!("{what}: {pct:.0}% used ({used} of {total})"),
        )
        .measured(pct, warn, "%")
    }

    pub fn level(&self) -> Status {
        match self.status.as_str() {
            "fail" => Status::Fail,
            "warn" => Status::Warn,
            _ => Status::Ok,
        }
    }

    /// Byte-capacity variant; keeps the percentage measurement and formats
    /// only the human summary. Storage apps opt in without changing check IDs.
    pub fn usage_bytes(id: &str, what: &str, used: u64, total: u64, warn: f64, fail: f64) -> Check {
        fn bytes(n: u64) -> String {
            for (scale, unit) in [(1u64 << 30, "GiB"), (1 << 20, "MiB"), (1 << 10, "KiB")] {
                if n >= scale {
                    return format!("{:.1} {unit}", n as f64 / scale as f64);
                }
            }
            format!("{n} B")
        }
        let mut check = Self::usage(id, what, used as f64, total as f64, warn, fail);
        check.summary = format!(
            "{what}: {:.0}% used ({} of {})",
            check.value,
            bytes(used),
            bytes(total)
        );
        check
    }
}

/// Thresholds for the framework's checks (`Config::ops`). The defaults
/// suit the S2/S3/C6 boards; the desktop runner never comes near them.
#[derive(Clone, Debug)]
pub struct OpsConfig {
    /// Free internal RAM: warn below, fail below (bytes).
    pub heap_warn: u32,
    pub heap_fail: u32,
    /// Largest free internal block: warn below (bytes). A TLS handshake
    /// needs one contiguous block, so fragmentation shows here first.
    pub largest_warn: u32,
    /// Wi-Fi signal: warn below, fail below (dBm).
    pub rssi_warn: i32,
    pub rssi_fail: i32,
}

impl Default for OpsConfig {
    fn default() -> Self {
        Self {
            heap_warn: 32 * 1024,
            heap_fail: crate::incidents::HEAP_LOW,
            largest_warn: 8 * 1024,
            rssi_warn: -75,
            rssi_fail: -85,
        }
    }
}

/// The framework's checks, from the board's own information.
pub fn builtin(info: &SysInfo, config: &OpsConfig, allocation_failures: u32) -> Vec<Check> {
    let mut checks = Vec::new();
    let heap = &info.heap;
    if heap.internal_total > 0 {
        let free = heap.internal_free;
        let status = if free < config.heap_fail {
            Status::Fail
        } else if free < config.heap_warn {
            Status::Warn
        } else {
            Status::Ok
        };
        checks.push(
            Check::new(
                "heap.internal",
                status,
                format!(
                    "{} KiB internal RAM free, lowest {} KiB",
                    free / 1024,
                    heap.internal_min / 1024
                ),
            )
            .measured(free.into(), config.heap_warn.into(), "B")
            .runbook("restart-planned"),
        );
        let largest = heap.internal_largest;
        let status = if largest < config.largest_warn {
            Status::Warn
        } else {
            Status::Ok
        };
        checks.push(
            Check::new(
                "heap.largest",
                status,
                format!("Largest free block {} KiB", largest / 1024),
            )
            .measured(largest.into(), config.largest_warn.into(), "B")
            .runbook("restart-planned"),
        );
    }
    let alloc = if allocation_failures > 0 {
        Status::Warn
    } else {
        Status::Ok
    };
    checks.push(
        Check::new(
            "heap.allocations",
            alloc,
            format!("{allocation_failures} failed allocations since boot"),
        )
        .measured(allocation_failures.into(), 0.0, "")
        .runbook("restart-planned"),
    );
    // No station (the desktop, or an AP-only board): nothing to judge.
    if !info.wifi.ssid.is_empty() {
        let rssi = info.wifi.rssi;
        let status = if rssi < config.rssi_fail {
            Status::Fail
        } else if rssi < config.rssi_warn {
            Status::Warn
        } else {
            Status::Ok
        };
        checks.push(
            Check::new(
                "wifi.signal",
                status,
                format!(
                    "{rssi} dBm, {} disconnects since boot",
                    info.wifi.disconnects
                ),
            )
            .measured(rssi.into(), config.rssi_warn.into(), "dBm")
            .runbook("wifi"),
        );
    }
    checks.push(if info.wall_ms > 0 {
        Check::new("clock", Status::Ok, "Clock set")
    } else {
        Check::new(
            "clock",
            Status::Warn,
            "Clock not set: SNTP has not answered",
        )
        .runbook("clock")
    });
    let (status, runbook) = match info.reset_reason.as_str() {
        "crash" | "watchdog" => (Status::Warn, "crash"),
        "brownout" => (Status::Warn, "power"),
        _ => (Status::Ok, ""),
    };
    let reason = if info.reset_reason.is_empty() {
        "unknown"
    } else {
        &info.reset_reason
    };
    checks.push(Check::new("reset", status, format!("Last reset: {reason}")).runbook(runbook));
    let status = match info.status.as_str() {
        "healthy" | "" => Status::Ok,
        "stalled" => Status::Fail,
        _ => Status::Warn,
    };
    let word = if info.status.is_empty() {
        "serving"
    } else {
        &info.status
    };
    checks.push(Check::new("server", status, format!("Server {word}")));
    checks
}

/// The answer: framework checks, then the app's, worst status on top.
pub fn health(info: &SysInfo, mut checks: Vec<Check>, app: Vec<Check>) -> Health {
    checks.extend(app);
    let worst = checks.iter().map(Check::level).max().unwrap_or(Status::Ok);
    Health {
        schema: SCHEMA.into(),
        app: info.app.clone(),
        host: info.host.clone(),
        version: info.version.clone(),
        build: info.build.clone(),
        uptime_ms: info.uptime_ms,
        status: worst.name().into(),
        checks,
    }
}

#[cfg(all(test, not(target_os = "espidf")))]
mod tests {
    #[test]
    fn byte_usage_preserves_measurement_but_formats_capacity() {
        let check =
            super::Check::usage_bytes("sd.space", "SD card", 507904, 31251120128, 80.0, 95.0);
        assert!(check.summary.contains("496.0 KiB of 29.1 GiB"));
        assert_eq!(check.value, 507904.0 * 100.0 / 31251120128.0);
        assert_eq!(check.unit, "%");
        assert_eq!(check.status, "ok");
    }
    use super::*;
    use crate::desktop::DesktopPlatform;
    use crate::site::{Config, Scratch};
    use crate::{http, Reply, Request, Service, Site};

    fn board() -> SysInfo {
        SysInfo {
            app: "nanacoin".into(),
            heap: crate::sys::Heap {
                internal_total: 300 * 1024,
                internal_free: 100 * 1024,
                internal_min: 60 * 1024,
                internal_largest: 40 * 1024,
                ..Default::default()
            },
            wifi: crate::sys::Wifi {
                ssid: "home".into(),
                rssi: -60,
                ..Default::default()
            },
            wall_ms: 1_800_000_000_000,
            reset_reason: "power on".into(),
            status: "healthy".into(),
            ..Default::default()
        }
    }

    fn status_of<'a>(checks: &'a [Check], id: &str) -> &'a str {
        &checks.iter().find(|c| c.id == id).unwrap().status
    }

    #[test]
    fn a_healthy_board_is_ok_everywhere() {
        let checks = builtin(&board(), &OpsConfig::default(), 0);
        assert!(checks.iter().all(|c| c.status == "ok"), "{checks:?}");
        assert_eq!(health(&board(), checks, vec![]).status, "ok");
    }

    #[test]
    fn low_memory_fragmentation_weak_wifi_and_a_crash_are_flagged() {
        let mut info = board();
        info.heap.internal_free = 12 * 1024;
        info.heap.internal_largest = 4 * 1024;
        info.wifi.rssi = -80;
        info.wall_ms = 0;
        info.reset_reason = "watchdog".into();
        info.status = "stalled".into();
        let checks = builtin(&info, &OpsConfig::default(), 3);
        assert_eq!(status_of(&checks, "heap.internal"), "fail");
        assert_eq!(status_of(&checks, "heap.largest"), "warn");
        assert_eq!(status_of(&checks, "heap.allocations"), "warn");
        assert_eq!(status_of(&checks, "wifi.signal"), "warn");
        assert_eq!(status_of(&checks, "clock"), "warn");
        assert_eq!(status_of(&checks, "reset"), "warn");
        assert_eq!(status_of(&checks, "server"), "fail");
        let reset = checks.iter().find(|c| c.id == "reset").unwrap();
        assert_eq!(reset.runbook, "crash");
    }

    #[test]
    fn the_desktop_has_no_wifi_or_heap_to_judge() {
        let info = SysInfo::default();
        let ids: Vec<String> = builtin(&info, &OpsConfig::default(), 0)
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert!(!ids
            .iter()
            .any(|i| i == "wifi.signal" || i == "heap.internal"));
    }

    #[test]
    fn usage_turns_warn_then_fail() {
        let at = |used| Check::usage("store.entries", "Store", used, 100.0, 80.0, 95.0).status;
        assert_eq!(
            (at(10.0), at(85.0), at(99.0)),
            ("ok".into(), "warn".into(), "fail".into())
        );
        assert_eq!(Check::usage("x", "X", 1.0, 0.0, 80.0, 95.0).status, "ok");
    }

    struct Filling;
    impl Service for Filling {
        fn handle(&self, _: &Request<'_>, _: &mut Reply<'_>) {}
        fn checks(&self) -> Vec<Check> {
            vec![Check::usage("store.entries", "Store", 97.0, 100.0, 80.0, 95.0).runbook("storage")]
        }
    }

    #[test]
    fn served_with_the_apps_checks_and_its_worst_status() {
        // A SPA fallback must not shadow it, as with /.well-known/miniframework.
        static ASSETS: &[crate::web::Asset] = &[crate::web::Asset {
            path: "/index.html",
            mime: "text/html; charset=utf-8",
            raw: b"<!doctype html>",
            gzip: b"",
            etag: "x",
            immutable: false,
        }];
        let mut config = Config::new("gamematcher", "gamematcher.local");
        config.assets = ASSETS;
        let site = Site::new(config, Filling, DesktopPlatform);
        let raw = b"GET /.well-known/ops/health HTTP/1.1\r\nHost: x\r\n\r\n";
        let request = http::parse(raw, 1024).unwrap().unwrap();
        let response = site.respond(&request, false, &mut Scratch::new(64 * 1024));
        let (status, _, _, body) = response.into_parts();
        assert_eq!(status, 200);
        let http::Body::Owned(body) = body else {
            panic!("health is built per request");
        };
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["schema"], SCHEMA);
        assert_eq!(body["status"], "fail");
        let last = body["checks"].as_array().unwrap().last().unwrap();
        assert_eq!(
            (&last["id"], &last["runbook"]),
            (&"store.entries".into(), &"storage".into())
        );
        let links = site.about().links;
        assert!(links
            .iter()
            .any(|l| l.rel == "health" && l.href == "/.well-known/ops/health"));
        let raw = b"GET /.well-known/ops/actions HTTP/1.1\r\nHost: x\r\n\r\n";
        let request = http::parse(raw, 1024).unwrap().unwrap();
        let (status, _, _, body) = site
            .respond(&request, false, &mut Scratch::new(4096))
            .into_parts();
        assert_eq!(status, 200);
        let http::Body::Owned(body) = body else {
            panic!("catalog must be JSON");
        };
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["schema"], "miniframework.actions/1");
        assert_eq!(body["actions"], serde_json::json!([]));
    }
}
