//! Sprint 0 hardware gate (`spec/sprints.md#sprint-0-skeleton-and-the-s3-gate`).
//! Three measurements, each run on the gate worker so the board keeps
//! serving while they run:
//!
//! - **storage:** write `n` records of `size` bytes into a scratch NVS
//!   namespace, read every one back (what boot does), then remove them.
//! - **crypto:** gzip then AES-256-GCM a buffer, as publication will.
//! - **outbound:** for `secs`, fetch each HTTPS target in turn (public CA
//!   bundle) while a bench loads the server.
//!
//! Heap and largest free block come from `/api/v1/sys`. Built only with the
//! `gate` feature, never in a release.
use miniframework::kv::Kv;
use miniframework::message;
use std::io;
use std::sync::Mutex;
use std::time::{Duration, Instant};

message! {
    pub struct GateTarget {
        1 name: String,
        2 url: String,
        3 ok: u32,
        4 failed: u32,
        5 last_ms: u32,
        6 max_ms: u32,
        7 last_error: String,
        8 last_status: u32,
        9 last_bytes: u32,
    }
}

message! {
    pub struct StorageRun {
        1 records: u32,
        2 bytes_each: u32,
        3 write_ms: u32,
        /// Reading every record back: what boot costs at this size.
        4 read_ms: u32,
        5 remove_ms: u32,
        /// Writes that failed (the partition filled up), and the first error.
        6 failed: u32,
        7 error: String,
        /// Mean and worst single read.
        8 read_us_mean: u32,
        9 read_us_max: u32,
    }
}

message! {
    pub struct CryptoRun {
        1 bytes: u32,
        2 gzip_ms: u32,
        3 gzipped: u32,
        4 encrypt_ms: u32,
        5 decrypt_ms: u32,
        6 ok: bool,
    }
}

message! {
    /// `GET /api/v1/gate`.
    pub struct GateReport {
        /// `idle`, or the measurement running now.
        1 running: String,
        2 outbound_remaining_secs: u32,
        3 targets: Vec<GateTarget>,
        4 storage: Option<StorageRun>,
        5 crypto: Option<CryptoRun>,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Storage { records: u32, bytes: u32 },
    Crypto { bytes: u32 },
}

#[derive(Default)]
struct Inner {
    outbound_until: Option<Instant>,
    targets: Vec<GateTarget>,
    pending: Option<Job>,
    running: Option<Job>,
    storage: Option<StorageRun>,
    crypto: Option<CryptoRun>,
}

#[derive(Default)]
pub struct Gate {
    inner: Mutex<Inner>,
}

/// One outbound fetch: (status, body bytes).
pub type FetchFn = dyn Fn(&str) -> io::Result<(u16, usize)> + Send + Sync;

/// The scratch records' keys (a namespace of their own on the board).
fn key(i: u32) -> String {
    format!("z{i}")
}

pub fn storage(kv: &dyn Kv, records: u32, bytes: u32) -> StorageRun {
    let mut run = StorageRun {
        records,
        bytes_each: bytes,
        ..Default::default()
    };
    let mut value = vec![0u8; bytes as usize];
    let started = Instant::now();
    let mut written = 0;
    for i in 0..records {
        // Distinct contents, so nothing can be deduplicated.
        let n = value.len().min(4);
        value[..n].copy_from_slice(&i.to_le_bytes()[..n]);
        match kv.set(&key(i), &value) {
            Ok(()) => written += 1,
            Err(e) => {
                run.failed += 1;
                if run.error.is_empty() {
                    run.error = format!("record {i}: {e}");
                }
            }
        }
    }
    run.write_ms = started.elapsed().as_millis() as u32;
    let started = Instant::now();
    let mut worst = Duration::ZERO;
    for i in 0..written {
        let one = Instant::now();
        let _ = kv.get(&key(i));
        worst = worst.max(one.elapsed());
    }
    let read = started.elapsed();
    run.read_ms = read.as_millis() as u32;
    run.read_us_mean = (read.as_micros() / u128::from(written.max(1))) as u32;
    run.read_us_max = worst.as_micros() as u32;
    let started = Instant::now();
    for i in 0..records {
        let _ = kv.remove(&key(i));
    }
    run.remove_ms = started.elapsed().as_millis() as u32;
    run
}

pub fn crypto(bytes: u32) -> CryptoRun {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    // Text-like input (publication is JSON): compressible but not trivial.
    let input: Vec<u8> = (0..bytes)
        .map(|i| {
            b"{\"id\":12,\"title\":\"Sunday dinner\",\"going\":[3,7]}"[(i % 47) as usize]
                ^ ((i / 997) as u8 & 1)
        })
        .collect();
    let mut run = CryptoRun {
        bytes,
        ..Default::default()
    };
    let started = Instant::now();
    let mut gz = Vec::new();
    miniframework::wire::gzip::compress(&input, 6, &mut gz);
    run.gzip_ms = started.elapsed().as_millis() as u32;
    run.gzipped = gz.len() as u32;
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    if getrandom::getrandom(&mut key).is_err() || getrandom::getrandom(&mut nonce).is_err() {
        return run;
    }
    let cipher = Aes256Gcm::new(&key.into());
    let nonce = Nonce::from_slice(&nonce);
    let started = Instant::now();
    let Ok(sealed) = cipher.encrypt(nonce, gz.as_slice()) else {
        return run;
    };
    run.encrypt_ms = started.elapsed().as_millis() as u32;
    let started = Instant::now();
    let opened = cipher.decrypt(nonce, sealed.as_slice());
    run.decrypt_ms = started.elapsed().as_millis() as u32;
    run.ok = opened.is_ok_and(|o| o == gz);
    run
}

impl Gate {
    pub fn set_targets(&self, targets: &[(&str, &str)]) {
        self.inner.lock().unwrap().targets = targets
            .iter()
            .map(|(name, url)| GateTarget {
                name: name.to_string(),
                url: url.to_string(),
                ..Default::default()
            })
            .collect();
    }

    pub fn start_outbound(&self, secs: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.outbound_until = Some(Instant::now() + Duration::from_secs(secs));
        for t in &mut inner.targets {
            *t = GateTarget {
                name: std::mem::take(&mut t.name),
                url: std::mem::take(&mut t.url),
                ..Default::default()
            };
        }
    }

    /// Queues a storage or crypto run; false if one is already queued or running.
    pub fn queue(&self, job: Job) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.pending.is_some() || inner.running.is_some() {
            return false;
        }
        inner.pending = Some(job);
        true
    }

    pub fn report(&self) -> GateReport {
        let inner = self.inner.lock().unwrap();
        let remaining = inner.outbound_until.map_or(0, |end| {
            end.saturating_duration_since(Instant::now()).as_secs()
        });
        GateReport {
            running: match inner.running.or(inner.pending) {
                None => "idle".into(),
                Some(Job::Storage { .. }) => "storage".into(),
                Some(Job::Crypto { .. }) => "crypto".into(),
            },
            outbound_remaining_secs: remaining as u32,
            targets: inner.targets.clone(),
            storage: inner.storage.clone(),
            crypto: inner.crypto.clone(),
        }
    }

    /// The worker's step: a queued job, then one round of outbound fetches
    /// if that is running. Never holds the lock during work.
    pub fn run_once(&self, scratch: &dyn Kv, fetchers: &[&FetchFn]) {
        let job = {
            let mut inner = self.inner.lock().unwrap();
            let job = inner.pending.take();
            inner.running = job;
            job
        };
        match job {
            Some(Job::Storage { records, bytes }) => {
                let run = storage(scratch, records, bytes);
                let mut inner = self.inner.lock().unwrap();
                inner.storage = Some(run);
                inner.running = None;
            }
            Some(Job::Crypto { bytes }) => {
                let run = crypto(bytes);
                let mut inner = self.inner.lock().unwrap();
                inner.crypto = Some(run);
                inner.running = None;
            }
            None => {}
        }
        let urls: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            if inner.outbound_until.is_none_or(|end| Instant::now() >= end) {
                return;
            }
            inner.targets.iter().map(|t| t.url.clone()).collect()
        };
        for (i, url) in urls.iter().enumerate() {
            let Some(fetch) = fetchers.get(i) else { break };
            let started = Instant::now();
            let result = fetch(url);
            let ms = started.elapsed().as_millis() as u32;
            let mut inner = self.inner.lock().unwrap();
            let Some(t) = inner.targets.get_mut(i) else {
                break;
            };
            t.last_ms = ms;
            t.max_ms = t.max_ms.max(ms);
            match result {
                // Any answer below 500 proves the handshake and request
                // worked (Brevo's root says 401 without a key).
                Ok((status, bytes)) if status < 500 => {
                    t.ok += 1;
                    t.last_status = status as u32;
                    t.last_bytes = bytes as u32;
                }
                Ok((status, _)) => {
                    t.failed += 1;
                    t.last_status = status as u32;
                    t.last_error = format!("HTTP {status}");
                }
                Err(e) => {
                    t.failed += 1;
                    t.last_error = e.to_string();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniframework::kv::MemKv;

    #[test]
    fn storage_run_writes_reads_and_cleans_up() {
        let kv = MemKv::default();
        let run = storage(&kv, 50, 200);
        assert_eq!((run.records, run.failed), (50, 0));
        assert!(kv.get("z0").is_none(), "scratch records are removed");
    }

    #[test]
    fn crypto_round_trips() {
        let run = crypto(64 * 1024);
        assert!(run.ok);
        assert!(run.gzipped < run.bytes);
    }

    #[test]
    fn jobs_run_once_and_outbound_counts() {
        let gate = Gate::default();
        let kv = MemKv::default();
        gate.set_targets(&[("a", "https://a"), ("b", "https://b")]);
        assert!(gate.queue(Job::Crypto { bytes: 1024 }));
        assert!(!gate.queue(Job::Crypto { bytes: 1024 }), "one at a time");
        let ok = |_: &str| Ok((200u16, 10usize));
        let bad = |_: &str| Err(io::Error::other("tls alloc failed"));
        gate.run_once(&kv, &[&ok, &bad]);
        let report = gate.report();
        assert!(report.crypto.is_some_and(|c| c.ok));
        assert_eq!(report.targets[0].ok, 0, "outbound idle until started");
        gate.start_outbound(60);
        gate.run_once(&kv, &[&ok, &bad]);
        let report = gate.report();
        assert_eq!(report.targets[0].ok, 1);
        assert_eq!(report.targets[1].last_error, "tls alloc failed");
        assert_eq!(report.running, "idle");
    }
}
