use async_worker_rs::{Service, DEFAULT_NTP_ADDR};
use tiny_http::{Header, Response, Server};

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bind = std::env::var("WORKER_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let upstream = std::env::var("WORKER_NTP_ADDR")
        .unwrap_or_else(|_| DEFAULT_NTP_ADDR.into())
        .parse()?;
    let jobs = Service::start(upstream)?;
    let server = Server::http(&bind)?;
    println!("Async worker: http://{bind}; NTP upstream: {upstream}");
    for request in server.incoming_requests() {
        let reply = jobs.handle(request.method().as_str(), request.url());
        let mut response = Response::from_data(reply.body).with_status_code(reply.status);
        for (key, value) in [
            ("Content-Type", Some(reply.content_type)),
            ("Cache-Control", Some("no-store")),
            ("Connection", Some("close")),
            ("Location", reply.location.as_deref()),
            ("Retry-After", reply.retry_after),
        ] {
            if let Some(value) = value {
                response.add_header(Header::from_bytes(key, value).unwrap());
            }
        }
        if let Err(error) = request.respond(response) {
            eprintln!("HTTP client: {error}");
        }
    }
    Ok(())
}
