//! Sprint 0 hardware gate (`spec/01-overview.md#hardware`): while running,
//! a worker fetches each target over HTTPS in turn, one connection at a
//! time, so outbound TLS overlaps whatever load the bench puts on the
//! server. Results are counts and timings; heap comes from `/api/v1/sys`.
//! Built only with the `gate` feature, never in a release.
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
    pub struct GateStats {
        1 running: bool,
        2 remaining_secs: u32,
        3 targets: Vec<GateTarget>,
    }
}

#[derive(Default)]
pub struct Gate {
    inner: Mutex<(Option<Instant>, Vec<GateTarget>)>,
}

/// One outbound fetch: (status, body bytes).
pub type FetchFn = dyn Fn(&str) -> io::Result<(u16, usize)> + Send + Sync;

impl Gate {
    pub fn set_targets(&self, targets: &[(&str, &str)]) {
        let mut inner = self.inner.lock().unwrap();
        inner.1 = targets
            .iter()
            .map(|(name, url)| GateTarget {
                name: name.to_string(),
                url: url.to_string(),
                ..Default::default()
            })
            .collect();
    }

    pub fn start(&self, secs: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.0 = Some(Instant::now() + Duration::from_secs(secs));
        for t in &mut inner.1 {
            *t = GateTarget {
                name: std::mem::take(&mut t.name),
                url: std::mem::take(&mut t.url),
                ..Default::default()
            };
        }
    }

    pub fn stats(&self) -> GateStats {
        let inner = self.inner.lock().unwrap();
        let remaining = inner.0.map_or(0, |end| {
            end.saturating_duration_since(Instant::now()).as_secs()
        });
        GateStats {
            running: remaining > 0,
            remaining_secs: remaining as u32,
            targets: inner.1.clone(),
        }
    }

    /// One round: fetch every target once if running. `fetchers[i]` serves
    /// target `i`. Never holds the lock during a fetch.
    pub fn run_once(&self, fetchers: &[&FetchFn]) {
        let urls: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            if inner.0.is_none_or(|end| Instant::now() >= end) {
                return;
            }
            inner.1.iter().map(|t| t.url.clone()).collect()
        };
        for (i, url) in urls.iter().enumerate() {
            let Some(fetch) = fetchers.get(i) else { break };
            let started = Instant::now();
            let result = fetch(url);
            let ms = started.elapsed().as_millis() as u32;
            let mut inner = self.inner.lock().unwrap();
            let Some(t) = inner.1.get_mut(i) else { break };
            t.last_ms = ms;
            t.max_ms = t.max_ms.max(ms);
            match result {
                Ok((status, bytes)) if (200..300).contains(&status) => {
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

    #[test]
    fn counts_successes_and_failures() {
        let gate = Gate::default();
        gate.set_targets(&[("a", "https://a"), ("b", "https://b")]);
        let ok = |_: &str| Ok((200u16, 10usize));
        let bad = |_: &str| Err(io::Error::other("tls alloc failed"));
        gate.run_once(&[&ok, &bad]);
        assert_eq!(gate.stats().targets[0].ok, 0, "idle until started");
        gate.start(60);
        gate.run_once(&[&ok, &bad]);
        let stats = gate.stats();
        assert!(stats.running);
        assert_eq!(stats.targets[0].ok, 1);
        assert_eq!(stats.targets[1].failed, 1);
        assert_eq!(stats.targets[1].last_error, "tls alloc failed");
    }
}
