use async_worker_rs::{ntp, Service, CAPACITY};
use std::{
    net::UdpSocket,
    thread,
    time::{Duration, Instant},
};

fn fake_ntp(delay: Duration, valid: bool) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let address = socket.local_addr().unwrap();
    let task = thread::spawn(move || {
        let mut request = [0u8; 48];
        let (_, peer) = socket.recv_from(&mut request).unwrap();
        thread::sleep(delay);
        let mut reply = [0u8; 48];
        reply[0] = 0x24;
        reply[1] = 1;
        if valid {
            reply[24..32].copy_from_slice(&request[40..48]);
        }
        reply[40..44].copy_from_slice(&3_900_000_000u32.to_be_bytes());
        socket.send_to(&reply, peer).unwrap();
    });
    (address, task)
}

#[test]
fn acceptance_and_polling_do_not_wait_for_upstream() {
    let (address, task) = fake_ntp(Duration::from_millis(400), true);
    let service = Service::start(address).unwrap();
    let started = Instant::now();
    let reply = service.handle("POST", "/api/jobs");
    assert_eq!(reply.status, 202);
    let url = reply.location.unwrap();
    assert_eq!(service.handle("GET", "/api/health").status, 200);
    let pending: serde_json::Value =
        serde_json::from_slice(&service.handle("GET", &url).body).unwrap();
    assert!(matches!(
        pending["status"].as_str(),
        Some("queued" | "running")
    ));
    assert!(started.elapsed() < Duration::from_millis(300));
    task.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let job: serde_json::Value =
            serde_json::from_slice(&service.handle("GET", &url).body).unwrap();
        if job["status"] == "succeeded" {
            assert_eq!(job["result"]["unix_ms"], 1_691_011_200_000u64);
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn capacity_rejects_without_evicting_live_jobs() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let service = Service::start(socket.local_addr().unwrap()).unwrap();
    let mut urls = Vec::new();
    for _ in 0..CAPACITY {
        let reply = service.handle("POST", "/api/jobs");
        assert_eq!(reply.status, 202);
        urls.push(reply.location.unwrap());
    }
    assert_eq!(service.handle("POST", "/api/jobs").status, 503);
    for url in urls {
        assert_eq!(service.handle("GET", &url).status, 200);
    }
    assert_eq!(service.handle("GET", "/api/jobs/missing").status, 404);
    assert_eq!(service.handle("GET", "/api/jobs").status, 405);
}

#[test]
fn rejects_reply_for_another_request() {
    let (address, task) = fake_ntp(Duration::ZERO, false);
    assert!(ntp::query(address).unwrap_err().contains("invalid"));
    task.join().unwrap();
}

#[test]
fn failed_upstream_becomes_a_terminal_job() {
    let (address, task) = fake_ntp(Duration::ZERO, false);
    let service = Service::start(address).unwrap();
    let url = service.handle("POST", "/api/jobs").location.unwrap();
    task.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let reply = service.handle("GET", &url);
        let job: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        if job["status"] == "failed" {
            assert!(job["error"].as_str().unwrap().contains("invalid"));
            assert!(reply.retry_after.is_none());
            assert!(job["result"].is_null());
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn silent_upstream_fails_in_bounded_time() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let started = Instant::now();
    assert!(ntp::query(socket.local_addr().unwrap()).is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}
