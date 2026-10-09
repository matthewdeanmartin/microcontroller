//! Optional admission wrapper. Keys must come from app-validated identities,
//! never an untrusted forwarded-IP header. No tasks or dependencies are added.
use crate::wire::Schema;
use crate::{Cors, Reply, Request, Service};
use std::sync::{
    atomic::{AtomicU32, AtomicUsize, Ordering},
    Mutex,
};
use std::time::Instant;

/// Token bucket: `burst` work units initially, refilled at `per_second`.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub burst: u32,
    pub per_second: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub global: Budget,
    pub caller: Budget,
    /// Caller records are preallocated and never grow beyond this count.
    pub callers: usize,
    pub concurrent: usize,
}

struct Bucket {
    tokens: f64,
    updated: Instant,
}
impl Bucket {
    fn new(b: Budget, now: Instant) -> Self {
        Self {
            tokens: b.burst as f64,
            updated: now,
        }
    }
    fn refill(&mut self, b: Budget, now: Instant) {
        self.tokens = (self.tokens
            + now.saturating_duration_since(self.updated).as_secs_f64() * b.per_second as f64)
            .min(b.burst as f64);
        self.updated = now;
    }
    fn wait(&self, cost: u32, b: Budget) -> u64 {
        ((cost as f64 - self.tokens).max(0.0) / b.per_second as f64)
            .ceil()
            .max(1.0) as u64
    }
}
struct Caller {
    key: [u8; 16],
    bucket: Bucket,
}
struct State {
    global: Bucket,
    callers: Vec<Caller>,
}

/// Result of attempting to reserve rate budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// This identity has used its allowance (HTTP 429).
    Caller { retry_after: u64 },
    /// Global budget, caller table or concurrency is exhausted (HTTP 503).
    Capacity { retry_after: u64 },
    /// Work cost is zero or larger than a bucket's entire burst.
    InvalidCost,
}

/// Bounded, thread-safe token buckets. No identity is evicted until its
/// allowance has fully replenished, so table churn cannot reset allowances.
pub struct Limiter {
    limits: Limits,
    state: Mutex<State>,
}
impl Limiter {
    pub fn new(limits: Limits) -> Result<Self, &'static str> {
        if limits.callers == 0
            || limits.concurrent == 0
            || limits.global.burst == 0
            || limits.global.per_second == 0
            || limits.caller.burst == 0
            || limits.caller.per_second == 0
        {
            return Err("limits must be nonzero");
        }
        let mut callers = Vec::new();
        callers
            .try_reserve_exact(limits.callers)
            .map_err(|_| "caller allocation failed")?;
        Ok(Self {
            limits,
            state: Mutex::new(State {
                global: Bucket::new(limits.global, Instant::now()),
                callers,
            }),
        })
    }
    pub fn admit(&self, key: [u8; 16], cost: u32) -> Result<(), Rejection> {
        self.admit_at(key, cost, Instant::now())
    }
    fn admit_at(&self, key: [u8; 16], cost: u32, now: Instant) -> Result<(), Rejection> {
        let l = self.limits;
        if cost == 0 || cost > l.global.burst || cost > l.caller.burst {
            return Err(Rejection::InvalidCost);
        }
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.global.refill(l.global, now);
        for caller in &mut s.callers {
            caller.bucket.refill(l.caller, now);
        }
        let index = match s.callers.iter().position(|c| c.key == key) {
            Some(i) => i,
            None => {
                let caller = Caller {
                    key,
                    bucket: Bucket::new(l.caller, now),
                };
                if s.callers.len() < l.callers {
                    s.callers.push(caller);
                    s.callers.len() - 1
                } else if let Some(i) = s
                    .callers
                    .iter()
                    .position(|c| c.bucket.tokens >= l.caller.burst as f64)
                {
                    s.callers[i] = caller;
                    i
                } else {
                    return Err(Rejection::Capacity { retry_after: 1 });
                }
            }
        };
        if s.callers[index].bucket.tokens < cost as f64 {
            return Err(Rejection::Caller {
                retry_after: s.callers[index].bucket.wait(cost, l.caller),
            });
        }
        if s.global.tokens < cost as f64 {
            return Err(Rejection::Capacity {
                retry_after: s.global.wait(cost, l.global),
            });
        }
        s.global.tokens -= cost as f64;
        s.callers[index].bucket.tokens -= cost as f64;
        Ok(())
    }
}

/// Classify returns a validated identity and work cost, or `None` to bypass.
/// Authentication/validation must happen here or before this wrapper; the
/// inner handler has not run yet. Use separate wrappers/limiters for distinct
/// route policies. Built-in Site routes bypass Service wrappers.
pub struct RateLimited<S, F> {
    inner: S,
    classify: F,
    limiter: Limiter,
    active: AtomicUsize,
    accepted: AtomicU32,
    rejected: AtomicU32,
}
/// Saturating counters for protected app requests; explicit bypasses and
/// Site built-ins are not counted. Active is a current handler snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub accepted: u32,
    pub rejected: u32,
    pub active: usize,
}
impl<S, F> RateLimited<S, F> {
    pub fn new(inner: S, limits: Limits, classify: F) -> Result<Self, &'static str> {
        Ok(Self {
            inner,
            classify,
            limiter: Limiter::new(limits)?,
            active: AtomicUsize::new(0),
            accepted: AtomicU32::new(0),
            rejected: AtomicU32::new(0),
        })
    }
    pub fn inner(&self) -> &S {
        &self.inner
    }
    pub fn stats(&self) -> Stats {
        Stats {
            accepted: self.accepted.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            active: self.active.load(Ordering::Relaxed),
        }
    }
}
fn count(counter: &AtomicU32) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        Some(n.saturating_add(1))
    });
}
struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl<S, F> Service for RateLimited<S, F>
where
    S: Service,
    F: Fn(&Request<'_>) -> Option<([u8; 16], u32)> + Send + Sync + 'static,
{
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        let Some((key, cost)) = (self.classify)(req) else {
            self.inner.handle(req, reply);
            return;
        };
        let admitted = self
            .active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < self.limiter.limits.concurrent).then(|| n + 1)
            });
        let _active = admitted.is_ok().then(|| Active(&self.active));
        let result = if admitted.is_err() {
            Err(Rejection::Capacity { retry_after: 1 })
        } else {
            self.limiter.admit(key, cost)
        };
        match result {
            Ok(()) => {
                count(&self.accepted);
                self.inner.handle(req, reply);
            }
            Err(rejection) => {
                count(&self.rejected);
                let (status, code, wait) = match rejection {
                    Rejection::Caller { retry_after } => (429, "rate_limited", retry_after),
                    Rejection::Capacity { retry_after } => (503, "overloaded", retry_after),
                    Rejection::InvalidCost => (503, "invalid_work_cost", 1),
                };
                reply.error(req, status, code, "Request admission refused");
                reply.header("Retry-After", wait.to_string());
                reply.header("Cache-Control", "no-store");
            }
        }
    }
    fn streamed_body(&self, method: &str, path: &str) -> Option<usize> {
        self.inner.streamed_body(method, path)
    }
    #[cfg(feature = "wifi-setup")]
    fn setup_route(&self, path: &str) -> bool {
        self.inner.setup_route(path)
    }
    fn origin_allowed(&self, origin: &str) -> bool {
        self.inner.origin_allowed(origin)
    }
    fn https_required(&self) -> bool {
        self.inner.https_required()
    }
    fn schemas(&self) -> Vec<&'static Schema> {
        self.inner.schemas()
    }
    fn cors(&self, path: &str) -> Cors {
        self.inner.cors(path)
    }
    fn metrics(&self) -> Vec<(&'static str, f64)> {
        let mut metrics = self.inner.metrics();
        let stats = self.stats();
        metrics.extend([
            ("admission_accepted", stats.accepted as f64),
            ("admission_rejected", stats.rejected as f64),
            ("admission_active", stats.active as f64),
        ]);
        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn limits() -> Limits {
        Limits {
            global: Budget {
                burst: 4,
                per_second: 1,
            },
            caller: Budget {
                burst: 2,
                per_second: 1,
            },
            callers: 2,
            concurrent: 1,
        }
    }
    #[test]
    fn budgets_refill_and_rejected_work_does_not_consume_global() {
        let l = Limiter::new(limits()).unwrap();
        let now = Instant::now();
        assert_eq!(l.admit_at([1; 16], 2, now), Ok(()));
        assert_eq!(
            l.admit_at([1; 16], 1, now),
            Err(Rejection::Caller { retry_after: 1 })
        );
        assert_eq!(l.admit_at([2; 16], 2, now), Ok(()));
        assert_eq!(l.admit_at([1; 16], 2, now + Duration::from_secs(2)), Ok(()));
    }
    #[test]
    fn table_is_bounded_and_churn_cannot_reset_budget() {
        let l = Limiter::new(limits()).unwrap();
        let now = Instant::now();
        l.admit_at([1; 16], 1, now).unwrap();
        l.admit_at([2; 16], 1, now).unwrap();
        assert_eq!(
            l.admit_at([3; 16], 1, now),
            Err(Rejection::Capacity { retry_after: 1 })
        );
        assert_eq!(l.admit_at([3; 16], 1, now + Duration::from_secs(1)), Ok(()));
        assert_eq!(l.state.lock().unwrap().callers.len(), 2);
    }
    #[test]
    fn global_capacity_and_invalid_cost() {
        let mut cfg = limits();
        cfg.global.burst = 2;
        let l = Limiter::new(cfg).unwrap();
        let now = Instant::now();
        l.admit_at([1; 16], 2, now).unwrap();
        assert_eq!(
            l.admit_at([2; 16], 1, now),
            Err(Rejection::Capacity { retry_after: 1 })
        );
        assert_eq!(l.admit_at([2; 16], 3, now), Err(Rejection::InvalidCost));
    }

    struct App;
    impl Service for App {
        fn handle(&self, _: &Request<'_>, reply: &mut Reply<'_>) {
            reply.text(200, "text/plain", "ok");
        }
        fn cors(&self, _: &str) -> Cors {
            Cors::Public
        }
        fn https_required(&self) -> bool {
            true
        }
        fn origin_allowed(&self, origin: &str) -> bool {
            origin == "trusted"
        }
        fn streamed_body(&self, _: &str, _: &str) -> Option<usize> {
            Some(123)
        }
        fn metrics(&self) -> Vec<(&'static str, f64)> {
            vec![("app_count", 1.0)]
        }
    }
    #[test]
    fn wrapper_delegates_policies_and_serializes_429_headers() {
        let service =
            RateLimited::new(App, limits(), |_: &Request<'_>| Some(([1; 16], 2))).unwrap();
        assert_eq!(service.cors("/api/test"), Cors::Public);
        assert!(service.https_required());
        assert!(service.origin_allowed("trusted"));
        assert_eq!(service.streamed_body("POST", "/api/test"), Some(123));
        assert!(service.metrics().contains(&("app_count", 1.0)));
        let site = crate::Site::new(
            crate::Config::new("test", "localhost"),
            service,
            crate::desktop::DesktopPlatform,
        );
        let request =
            crate::http::parse(b"GET /api/test HTTP/1.1\r\nHost: localhost\r\n\r\n", 1024)
                .unwrap()
                .unwrap();
        let mut scratch = crate::site::Scratch::new(2048);
        site.respond(&request, true, &mut scratch);
        let mut response = site.respond(&request, true, &mut scratch);
        let mut bytes = Vec::new();
        while !response.next().is_empty() {
            let n = response.next().len();
            bytes.extend_from_slice(response.next());
            response.advance(n);
        }
        let wire = String::from_utf8(bytes).unwrap();
        assert!(wire.starts_with("HTTP/1.1 429 Too Many Requests"));
        assert!(wire.contains("Retry-After: 2\r\n"));
        assert!(wire.contains("Cache-Control: no-store\r\n"));
        assert!(wire.contains("Access-Control-Allow-Origin: *\r\n"));
        assert_eq!(
            site.service.stats(),
            Stats {
                accepted: 1,
                rejected: 1,
                active: 0
            }
        );
    }
    #[test]
    fn concurrent_work_is_shed_and_permit_is_released() {
        use std::sync::{mpsc, Arc, Barrier};
        struct Blocking {
            entered: mpsc::SyncSender<()>,
            release: Arc<Barrier>,
        }
        impl Service for Blocking {
            fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
                if req.path == "/block" {
                    self.entered.send(()).unwrap();
                    self.release.wait();
                }
                reply.text(200, "text/plain", "ok");
            }
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let barrier = Arc::new(Barrier::new(2));
        let service = Arc::new(
            RateLimited::new(
                Blocking {
                    entered: tx,
                    release: Arc::clone(&barrier),
                },
                limits(),
                |_: &Request<'_>| Some(([1; 16], 1)),
            )
            .unwrap(),
        );
        let worker = Arc::clone(&service);
        let thread = std::thread::spawn(move || {
            let mut body = Vec::new();
            let mut spare = Vec::new();
            worker.handle(
                &Request::new("GET", "/block", &[], &[], false),
                &mut Reply::new(&mut body, &mut spare, 1024),
            );
        });
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut body = Vec::new();
        let mut spare = Vec::new();
        let mut reply = Reply::new(&mut body, &mut spare, 1024);
        service.handle(&Request::new("GET", "/other", &[], &[], false), &mut reply);
        assert_eq!(reply.status(), 503);
        barrier.wait();
        thread.join().unwrap();
        service.handle(&Request::new("GET", "/other", &[], &[], false), &mut reply);
        assert_eq!(reply.status(), 200);
        assert_eq!(service.active.load(Ordering::Relaxed), 0);
    }
}
