pub mod ntp;

use serde::Serialize;
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::{
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub const CAPACITY: usize = 8;
pub const RETENTION: Duration = Duration::from_secs(120);
pub const NTP_INTERVAL: Duration = Duration::from_secs(5);
pub const DEFAULT_NTP_ADDR: &str = "129.6.15.28:123";

#[derive(Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub status: &'static str,
    pub result: Option<ntp::TimeSample>,
    pub error: Option<String>,
    #[serde(skip)]
    finished: Option<Instant>,
}

struct State {
    slots: [Option<Job>; CAPACITY],
}

#[derive(Clone)]
pub struct Service {
    state: Arc<Mutex<State>>,
    queue: SyncSender<usize>,
}

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub location: Option<String>,
    pub retry_after: Option<&'static str>,
}

impl Reply {
    fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(&value).unwrap(),
            location: None,
            retry_after: None,
        }
    }
}

impl Service {
    pub fn start(server: SocketAddr) -> std::io::Result<Self> {
        let state = Arc::new(Mutex::new(State {
            slots: std::array::from_fn(|_| None),
        }));
        let (queue, receive) = mpsc::sync_channel::<usize>(CAPACITY);
        let worker_state = state.clone();
        thread::Builder::new()
            .name("time-worker".into())
            .stack_size(12 * 1024)
            .spawn(move || {
                let mut next_call = Instant::now();
                while let Ok(index) = receive.recv() {
                    // No mutex held during pacing or external I/O. The HTTP task stays available.
                    thread::sleep(next_call.saturating_duration_since(Instant::now()));
                    worker_state.lock().unwrap().slots[index]
                        .as_mut()
                        .unwrap()
                        .status = "running";
                    let outcome = ntp::query(server);
                    // Also pace failures; never retry upstream automatically.
                    next_call = Instant::now() + NTP_INTERVAL;
                    let mut state = worker_state.lock().unwrap();
                    let job = state.slots[index].as_mut().unwrap();
                    match outcome {
                        Ok(sample) => {
                            job.status = "succeeded";
                            job.result = Some(sample);
                        }
                        Err(error) => {
                            job.status = "failed";
                            job.error = Some(error);
                        }
                    }
                    job.finished = Some(Instant::now());
                }
            })?;
        Ok(Self { state, queue })
    }

    pub fn handle(&self, method: &str, uri: &str) -> Reply {
        let path = uri.split('?').next().unwrap_or(uri);
        if path == "/" && method == "GET" {
            return Reply {
                status: 200,
                content_type: "text/html; charset=utf-8",
                body: include_bytes!("../web/index.html").to_vec(),
                location: None,
                retry_after: None,
            };
        }
        let mut state = self.state.lock().unwrap();
        for slot in &mut state.slots {
            if slot
                .as_ref()
                .and_then(|j| j.finished)
                .is_some_and(|t| t.elapsed() >= RETENTION)
            {
                *slot = None;
            }
        }
        if path == "/api/health" && method == "GET" {
            return Reply::json(
                200,
                json!({"status":"ok", "capacity":CAPACITY, "occupied":state.slots.iter().flatten().count(), "transport":"202 + polling"}),
            );
        }
        if path == "/api/jobs" && method == "POST" {
            let Some(index) = state.slots.iter().position(Option::is_none) else {
                let mut reply = Reply::json(
                    503,
                    json!({"error":"job table full; completed jobs expire after 120 seconds"}),
                );
                reply.retry_after = Some("5");
                return reply;
            };
            let mut random = [0u8; 16];
            if getrandom::getrandom(&mut random).is_err() {
                return Reply::json(503, json!({"error":"random source unavailable"}));
            }
            let id: String = random.iter().map(|b| format!("{b:02x}")).collect();
            state.slots[index] = Some(Job {
                id: id.clone(),
                status: "queued",
                result: None,
                error: None,
                finished: None,
            });
            match self.queue.try_send(index) {
                Ok(()) => {
                    let url = format!("/api/jobs/{id}");
                    let mut reply =
                        Reply::json(202, json!({"id":id, "status":"queued", "poll_url":url}));
                    reply.location = Some(url);
                    reply.retry_after = Some("1");
                    return reply;
                }
                Err(error) => {
                    state.slots[index] = None;
                    return Reply::json(
                        503,
                        json!({"error": match error { TrySendError::Full(_) => "queue full", TrySendError::Disconnected(_) => "worker unavailable" }}),
                    );
                }
            }
        }
        if let Some(id) = path.strip_prefix("/api/jobs/") {
            if method != "GET" {
                return Reply::json(405, json!({"error":"use GET"}));
            }
            let snapshot = state.slots.iter().flatten().find(|j| j.id == id).cloned();
            drop(state);
            return match snapshot {
                Some(job) => {
                    let pending = job.finished.is_none();
                    let mut reply = Reply::json(200, json!(job));
                    if pending {
                        reply.retry_after = Some("1");
                    }
                    reply
                }
                None => Reply::json(404, json!({"error":"unknown, expired, or lost on restart"})),
            };
        }
        Reply::json(
            if matches!(path, "/" | "/api/jobs" | "/api/health") {
                405
            } else {
                404
            },
            json!({"error":"route or method not supported"}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_reclaims_only_completed_jobs() {
        let (queue, _receiver) = mpsc::sync_channel(CAPACITY);
        let mut slots = std::array::from_fn(|_| None);
        for (index, status) in ["succeeded", "failed", "queued", "running"]
            .iter()
            .enumerate()
        {
            slots[index] = Some(Job {
                id: index.to_string(),
                status,
                result: None,
                error: None,
                finished: (index < 2).then(|| Instant::now() - RETENTION),
            });
        }
        let service = Service {
            queue,
            state: Arc::new(Mutex::new(State { slots })),
        };
        for index in 0..4 {
            assert_eq!(
                service.handle("GET", &format!("/api/jobs/{index}")).status,
                if index < 2 { 404 } else { 200 }
            );
        }
        assert_eq!(service.handle("POST", "/api/jobs").status, 202);
    }
}
