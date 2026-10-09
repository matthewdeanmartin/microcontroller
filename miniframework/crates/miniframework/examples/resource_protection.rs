//! Local-only example: POST a UTF-8 job, then poll its token for uppercase text.
//! RESOURCE_DEMO_TOKEN must be set. This is not a public deployment gateway.
use miniframework::{
    desktop::{self, DesktopPlatform},
    queue::{self, Queue},
    rate_limit::{self, Budget, RateLimited},
    Config, Reply, Request, Service, Site,
};
use std::sync::Arc;
use std::time::Duration;

struct App {
    queue: Arc<Queue<1024>>,
    authorization: String,
}
const OWNER: [u8; 16] = [1; 16]; // Single authenticated principal in this demo.
fn encode(id: queue::JobId) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}
fn decode(text: &str) -> Option<queue::JobId> {
    if text.len() != 32 || !text.is_ascii() {
        return None;
    }
    let mut id = [0; 16];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(id)
}
impl Service for App {
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        if req.path != "/api/v1/jobs" {
            return;
        }
        if req.header("Authorization") != self.authorization {
            reply.error(req, 401, "unauthorized", "Bearer token required");
            return;
        }
        reply.header("Cache-Control", "no-store");
        match req.method {
            "POST" => {
                if std::str::from_utf8(req.body).is_err() {
                    reply.error(req, 400, "invalid_text", "Job must be UTF-8");
                    return;
                }
                match self.queue.submit(OWNER, req.body) {
                    Ok(id) => {
                        let id = encode(id);
                        let url = format!("/api/v1/jobs?id={id}");
                        reply.text(
                            202,
                            "application/json",
                            &format!("{{\"job_id\":\"{id}\",\"status_url\":\"{url}\"}}"),
                        );
                        reply.header("Location", url);
                    }
                    Err(queue::Error::TooLarge) => {
                        reply.error(req, 413, "job_too_large", "Maximum job size is 1024 bytes")
                    }
                    Err(queue::Error::OwnerLimit) => {
                        reply.error(
                            req,
                            429,
                            "job_quota",
                            "Retrieve or wait for existing jobs to expire",
                        );
                        reply.header("Retry-After", "2");
                    }
                    Err(_) => {
                        reply.error(req, 503, "queue_unavailable", "Job was not accepted");
                        reply.header("Retry-After", "2");
                    }
                }
            }
            "GET" => {
                let Some(id) = req.param("id").and_then(|id| decode(&id)) else {
                    reply.error(req, 400, "invalid_job_id", "Expected 32 hex characters");
                    return;
                };
                let mut output = [0; 1024];
                match self.queue.poll(OWNER, id, &mut output) {
                    Ok(Some(queue::Status::Done { bytes })) => {
                        reply.bytes(200, "text/plain; charset=utf-8", &output[..bytes])
                    }
                    Ok(Some(queue::Status::Failed)) => {
                        reply.text(200, "application/json", "{\"status\":\"failed\"}")
                    }
                    Ok(Some(status)) => {
                        let state = if status == queue::Status::Pending {
                            "pending"
                        } else {
                            "running"
                        };
                        reply.text(
                            202,
                            "application/json",
                            &format!("{{\"status\":\"{state}\"}}"),
                        );
                        reply.header("Retry-After", "1");
                    }
                    Ok(None) => reply.error(req, 404, "job_not_found", "Unknown or expired job"),
                    Err(_) => reply.error(req, 500, "job_result", "Result could not be retrieved"),
                }
            }
            _ => {
                reply.error(req, 405, "method_not_allowed", "Use POST or GET");
                reply.header("Allow", "POST, GET");
            }
        }
    }
    fn metrics(&self) -> Vec<(&'static str, f64)> {
        let stats = self.queue.stats();
        vec![
            ("jobs_pending", stats.pending as f64),
            ("jobs_running", stats.running as f64),
            ("jobs_retained", stats.retained as f64),
        ]
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = std::env::var("RESOURCE_DEMO_TOKEN")
        .map_err(|_| "Set RESOURCE_DEMO_TOKEN to a nonempty demo secret")?;
    if token.is_empty() {
        return Err("RESOURCE_DEMO_TOKEN must not be empty".into());
    }
    let authorization = format!("Bearer {token}");
    // 8 KiB slot payload storage, plus metadata and at most 1 KiB claimed input.
    let queue = Arc::new(Queue::<1024>::new(queue::Limits {
        slots: 8,
        workers: 1,
        per_owner: 4,
        lifetime: Duration::from_secs(60),
    })?);
    let worker_queue = Arc::clone(&queue);
    std::thread::spawn(move || loop {
        match worker_queue.claim() {
            Ok(Some(job)) => {
                if job.expired() {
                    let _ = job.fail();
                    continue;
                }
                // ASCII-only conversion cannot expand the bounded payload.
                let mut result = job.payload().to_vec();
                result.make_ascii_uppercase();
                let _ = job.finish(&result);
            }
            _ => std::thread::sleep(Duration::from_millis(20)),
        }
    });
    let app = App {
        queue,
        authorization: authorization.clone(),
    };
    let service = RateLimited::new(
        app,
        rate_limit::Limits {
            global: Budget {
                burst: 20,
                per_second: 10,
            },
            caller: Budget {
                burst: 10,
                per_second: 5,
            },
            callers: 2,
            concurrent: 1,
        },
        move |req: &Request<'_>| {
            // Failed authentication attempts share a bounded anonymous bucket.
            let key = if req.header("Authorization") == authorization {
                OWNER
            } else {
                [0; 16]
            };
            Some((key, if req.method == "POST" { 2 } else { 1 }))
        },
    )?;
    let mut config = Config::new("resource-demo", "localhost");
    config.body_limit = 1024;
    config.response_limit = 2048;
    config.expose_headers = "Location, Retry-After";
    let site = Site::new(config, service, DesktopPlatform);
    desktop::serve(&site, "127.0.0.1:18090", || {})?;
    Ok(())
}
