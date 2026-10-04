//! System information: the built-in `/api/v1/sys` API and `/metrics`.
use crate::message;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

message! {
    /// Memory, in bytes. Internal RAM is what TLS, Wi-Fi and stacks need;
    /// PSRAM holds large buffers. Desktop reports process memory as internal.
    pub struct Heap {
        1 internal_free: u32,
        2 internal_min: u32,
        3 internal_largest: u32,
        4 internal_total: u32,
        5 psram_free: u32,
        6 psram_min: u32,
        7 psram_largest: u32,
        8 psram_total: u32,
    }
}

message! {
    pub struct Wifi {
        1 ssid: String,
        2 rssi: i32,
        3 channel: u32,
        4 ip: String,
        5 mac: String,
        /// Times the station lost its access point since boot.
        6 disconnects: u32,
        /// IDF reason code of the last disconnect (0: none yet). 2 auth
        /// expired, 15 4-way handshake timeout (often a wrong password),
        /// 201 no AP found, 202 auth failed, 203 association failed.
        7 last_reason: u32,
    }
}

message! {
    /// One flash partition, from the partition table.
    pub struct Partition {
        1 name: String,
        /// `app` or `data`.
        2 kind: String,
        /// IDF subtype number (data: 1 phy, 2 nvs, 3 coredump; app: 0
        /// factory, 16+n ota_n).
        3 subtype: u32,
        4 offset: u32,
        5 size: u32,
        /// The firmware currently running.
        6 running: bool,
    }
}

message! {
    /// Server counters since boot. Milliseconds are wall-clock handshake
    /// durations measured on the board.
    pub struct Net {
        1 requests: u32,
        2 errors: u32,
        /// Response bytes handed to sockets, in KiB.
        3 kib_out: u32,
        4 tls_handshakes: u32,
        5 tls_failures: u32,
        6 handshake_ms_last: u32,
        7 handshake_ms_avg: u32,
        8 tls_open: u32,
        9 http_open: u32,
        /// New connections refused because every slot was busy.
        10 rejected: u32,
        11 tls_slots: u32,
        12 http_slots: u32,
    }
}

message! {
    /// What the board is and how it is doing.
    pub struct SysInfo {
        1 app: String,
        2 version: String,
        /// Short build fingerprint, to tell firmware builds apart.
        3 build: String,
        4 host: String,
        5 platform: String,
        6 chip: String,
        7 cores: u32,
        8 cpu_mhz: u32,
        9 uptime_ms: u64,
        /// Wall clock (Unix ms), or 0 before time is synchronized.
        10 wall_ms: u64,
        11 reset_reason: String,
        12 heap: Heap,
        13 wifi: Wifi,
        14 net: Net,
        15 flash_bytes: u32,
        16 temp_c: Option<f32>,
        17 sdk: String,
        /// What the status light shows: booting, connecting, starting,
        /// healthy, degraded, stalled or failed. Empty without a light.
        18 status: String,
        /// The flash layout (empty on the desktop).
        19 partitions: Vec<Partition>,
    }
}

/// What a platform (desktop, ESP-IDF) knows about its hardware.
pub trait Platform: Send + Sync + 'static {
    /// Fill hardware fields; the server fills app, host and net.
    fn sysinfo(&self) -> SysInfo;
}

/// Server counters, shared by the connection loop and handshake task.
/// 32-bit atomics only: the Xtensa targets have no 64-bit atomics.
pub struct Stats {
    pub requests: AtomicU32,
    pub errors: AtomicU32,
    pub bytes_out: AtomicU32,
    pub kib_out: AtomicU32,
    pub tls_handshakes: AtomicU32,
    pub tls_failures: AtomicU32,
    pub handshake_ms_last: AtomicU32,
    pub handshake_ms_total: AtomicU32,
    pub tls_open: AtomicU32,
    pub http_open: AtomicU32,
    pub rejected: AtomicU32,
    pub tls_slots: AtomicU32,
    pub http_slots: AtomicU32,
    pub wifi_disconnects: AtomicU32,
    pub wifi_last_reason: AtomicU32,
}

pub static STATS: Stats = Stats {
    requests: AtomicU32::new(0),
    errors: AtomicU32::new(0),
    bytes_out: AtomicU32::new(0),
    kib_out: AtomicU32::new(0),
    tls_handshakes: AtomicU32::new(0),
    tls_failures: AtomicU32::new(0),
    handshake_ms_last: AtomicU32::new(0),
    handshake_ms_total: AtomicU32::new(0),
    tls_open: AtomicU32::new(0),
    http_open: AtomicU32::new(0),
    rejected: AtomicU32::new(0),
    tls_slots: AtomicU32::new(0),
    http_slots: AtomicU32::new(0),
    wifi_disconnects: AtomicU32::new(0),
    wifi_last_reason: AtomicU32::new(0),
};

impl Stats {
    pub fn sent(&self, bytes: usize) {
        let total = self.bytes_out.fetch_add(bytes as u32, Relaxed) as u64 + bytes as u64;
        if total >= 1024 {
            let kib = (total / 1024) as u32;
            self.bytes_out.fetch_sub(kib * 1024, Relaxed);
            self.kib_out.fetch_add(kib, Relaxed);
        }
    }

    pub fn handshake(&self, ms: u32) {
        self.tls_handshakes.fetch_add(1, Relaxed);
        self.handshake_ms_last.store(ms, Relaxed);
        self.handshake_ms_total.fetch_add(ms, Relaxed);
    }

    pub fn snapshot(&self) -> Net {
        let handshakes = self.tls_handshakes.load(Relaxed);
        Net {
            requests: self.requests.load(Relaxed),
            errors: self.errors.load(Relaxed),
            kib_out: self.kib_out.load(Relaxed),
            tls_handshakes: handshakes,
            tls_failures: self.tls_failures.load(Relaxed),
            handshake_ms_last: self.handshake_ms_last.load(Relaxed),
            handshake_ms_avg: self
                .handshake_ms_total
                .load(Relaxed)
                .checked_div(handshakes)
                .unwrap_or(0),
            tls_open: self.tls_open.load(Relaxed),
            http_open: self.http_open.load(Relaxed),
            rejected: self.rejected.load(Relaxed),
            tls_slots: self.tls_slots.load(Relaxed),
            http_slots: self.http_slots.load(Relaxed),
        }
    }
}

/// The numeric parts of `info` as one Influx line (for `/metrics` and for
/// recording a board's own health as time series). `tags` are added after
/// `host` and `app` (NanaCoin adds `bank`).
pub fn influx_line(info: &SysInfo, tags: &[(&str, String)]) -> String {
    let mut fields: Vec<(&str, f64)> = vec![
        ("uptime_s", (info.uptime_ms / 1000) as f64),
        ("heap_internal_free", info.heap.internal_free as f64),
        ("heap_internal_min", info.heap.internal_min as f64),
        ("heap_internal_largest", info.heap.internal_largest as f64),
        ("heap_psram_free", info.heap.psram_free as f64),
        ("heap_psram_largest", info.heap.psram_largest as f64),
        ("requests", info.net.requests as f64),
        ("errors", info.net.errors as f64),
        ("kib_out", info.net.kib_out as f64),
        ("tls_handshakes", info.net.tls_handshakes as f64),
        ("handshake_ms_last", info.net.handshake_ms_last as f64),
        ("tls_open", info.net.tls_open as f64),
        ("http_open", info.net.http_open as f64),
        ("rejected", info.net.rejected as f64),
    ];
    if info.wifi.rssi != 0 {
        fields.push(("rssi", info.wifi.rssi as f64));
    }
    if let Some(t) = info.temp_c {
        fields.push(("temp_c", t as f64));
    }
    if info.wifi.disconnects > 0 {
        fields.push(("wifi_disconnects", info.wifi.disconnects as f64));
    }
    let mut line = format!(
        "board,host={},app={}",
        crate::influx::escape_tag(&info.host),
        crate::influx::escape_tag(&info.app)
    );
    for (key, value) in tags {
        line.push(',');
        line.push_str(&crate::influx::escape_tag(key));
        line.push('=');
        line.push_str(&crate::influx::escape_tag(value));
    }
    line.push(' ');
    for (i, (name, value)) in fields.iter().enumerate() {
        if i > 0 {
            line.push(',');
        }
        line.push_str(name);
        line.push('=');
        line.push_str(&crate::influx::number(*value));
    }
    if info.wall_ms > 0 {
        line.push_str(&format!(" {}", info.wall_ms * 1_000_000));
    }
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn influx_line_has_app_tags_and_skips_absent_readings() {
        let mut info = SysInfo {
            app: "nanacoin".into(),
            host: "nanacoin.local".into(),
            uptime_ms: 61_000,
            ..Default::default()
        };
        info.heap.internal_free = 1234;
        let line = influx_line(&info, &[("bank", "s3".into())]);
        assert!(
            line.starts_with("board,host=nanacoin.local,app=nanacoin,bank=s3 uptime_s=61,"),
            "{line}"
        );
        assert!(line.contains("heap_internal_free=1234"));
        assert!(!line.contains("rssi="), "no Wi-Fi reading, no field");
        assert!(!line.contains("wifi_disconnects"));
        assert!(
            line.ends_with('\n') && !line.contains(" 0\n"),
            "no timestamp before SNTP"
        );

        info.wifi.rssi = -61;
        info.wifi.disconnects = 3;
        info.wall_ms = 1_800_000_000_000;
        let line = influx_line(&info, &[("odd tag", "a,b=c".into())]);
        assert!(line.contains(r"odd\ tag=a\,b\=c "), "{line}");
        assert!(line.contains(",rssi=-61") && line.contains(",wifi_disconnects=3"));
        assert!(line.ends_with(" 1800000000000000000\n"));
    }
}
