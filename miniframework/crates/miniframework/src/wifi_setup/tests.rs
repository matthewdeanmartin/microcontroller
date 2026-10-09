use super::*;
use crate::{
    desktop::DesktopPlatform,
    kv::{Kv, MemKv},
    site::Scratch,
    Config, Site,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc, Barrier,
};

struct Data {
    active: AtomicBool,
    info: Mutex<Info>,
    kv: MemKv,
    scans: AtomicUsize,
    joins: AtomicUsize,
}
#[derive(Clone)]
struct Radio {
    data: Arc<Data>,
    block: Option<(mpsc::SyncSender<()>, Arc<Barrier>)>,
}
impl Backend for Radio {
    fn active(&self) -> bool {
        self.data.active.load(Ordering::Relaxed)
    }
    fn info(&self) -> Info {
        self.data.info.lock().unwrap().clone()
    }
    fn scan(&self) -> Result<Vec<String>, String> {
        self.data.scans.fetch_add(1, Ordering::Relaxed);
        if let Some((entered, release)) = &self.block {
            entered.send(()).unwrap();
            release.wait();
        }
        Ok(vec![
            "Home".into(),
            "Home".into(),
            "<b>not markup</b>".into(),
            "bad\0ssid".into(),
        ])
    }
    fn join(&self, ssid: &str, password: &str) -> Result<Ipv4Addr, String> {
        self.data.joins.fetch_add(1, Ordering::Relaxed);
        if password != "engineering" {
            return Err("Connection failed".into());
        }
        self.data.kv.set("ssid", ssid.as_bytes()).unwrap();
        self.data.kv.set("pass", password.as_bytes()).unwrap();
        let ip = Ipv4Addr::new(192, 168, 1, 42);
        let mut info = self.data.info.lock().unwrap();
        info.ssid = ssid.into();
        info.address = Some(ip);
        Ok(ip)
    }
    fn set_retry_minutes(&self, minutes: u16) -> Result<(), String> {
        self.data
            .kv
            .set("retry_min", &minutes.to_le_bytes())
            .unwrap();
        self.data.info.lock().unwrap().retry_minutes = minutes;
        Ok(())
    }
    fn close(&self) -> Result<(), String> {
        self.data.active.store(false, Ordering::Relaxed);
        Ok(())
    }
}
struct App;
impl Service for App {
    fn handle(&self, _: &Request<'_>, reply: &mut Reply<'_>) {
        reply.text(200, "text/plain", "normal app");
    }
    fn https_required(&self) -> bool {
        true
    }
    fn cors(&self, _: &str) -> Cors {
        Cors::Public
    }
    fn metrics(&self) -> Vec<(&'static str, f64)> {
        vec![("normal", 1.0)]
    }
}
type TestSite = Site<Portal<App, Radio>>;
fn radio() -> Radio {
    Radio {
        data: Arc::new(Data {
            active: AtomicBool::new(true),
            info: Mutex::new(Info {
                ssid: "old".into(),
                address: None,
                retry_minutes: 3,
            }),
            kv: MemKv::default(),
            scans: AtomicUsize::new(0),
            joins: AtomicUsize::new(0),
        }),
        block: None,
    }
}
fn setup(code: Option<&'static str>) -> (TestSite, Arc<Controller<Radio>>, Arc<Data>) {
    let backend = radio();
    let data = Arc::clone(&backend.data);
    let c = Arc::new(
        Controller::new(
            backend,
            Options {
                code,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let mut config = Config::new("test", "board.local");
    config.response_limit = 2048;
    config.assets = &[crate::web::Asset {
        path: "/index.html",
        mime: "text/html",
        raw: b"app index",
        gzip: &[],
        etag: "app",
        immutable: false,
    }];
    (
        Site::new(config, Portal::new(App, Arc::clone(&c)), DesktopPlatform),
        c,
        data,
    )
}
fn call(
    site: &TestSite,
    method: &str,
    path: &str,
    headers: &str,
    body: &str,
    secure: bool,
) -> (u16, String) {
    let raw = format!("{method} {path} HTTP/1.1\r\nHost: board.local\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len());
    let req = crate::http::parse(raw.as_bytes(), 16 * 1024)
        .unwrap()
        .unwrap();
    let mut scratch = Scratch::new(2048);
    let mut response = site.respond(&req, secure, &mut scratch);
    let mut bytes = Vec::new();
    while !response.next().is_empty() {
        let n = response.next().len();
        bytes.extend_from_slice(response.next());
        response.advance(n);
    }
    let text = String::from_utf8(bytes).unwrap();
    (
        text.split_whitespace().nth(1).unwrap().parse().unwrap(),
        text,
    )
}
const MARKER: &str = "X-Wifi-Setup-Request: 1\r\n";
const CODE: &str = "X-Wifi-Setup-Code: demo-code\r\nX-Wifi-Setup-Request: 1\r\n";
#[test]
fn portal_precedes_spa_and_only_setup_bypasses_https() {
    let (site, _, data) = setup(None);
    assert_eq!(call(&site, "GET", "/", "", "", false).0, 302);
    let (status, page) = call(&site, "GET", ROOT, "", "", false);
    assert_eq!(status, 200);
    assert!(page.contains("Connect to your Wi-Fi"));
    assert!(page.contains("</html>"));
    assert!(page.contains("Content-Security-Policy:"));
    assert_eq!(call(&site, "GET", "/api/normal", "", "", false).0, 403);
    assert_eq!(call(&site, "GET", "/api/normal", "", "", true).0, 200);
    assert_eq!(
        call(&site, "GET", "/hotspot-detect.html", "", "", false).0,
        302
    );
    assert!(call(&site, "GET", "/wifi-setup/setup.js", "", "", false)
        .1
        .contains("pagehide"));
    data.active.store(false, Ordering::Relaxed);
    assert_eq!(call(&site, "GET", ROOT, "", "", false).0, 404);
    assert!(call(&site, "GET", "/", "", "", true)
        .1
        .contains("app index"));
    assert_eq!(site.service.metrics(), vec![("normal", 1.0)]);
}
#[test]
fn code_gate_and_cross_origin_checks_apply_to_all_operations() {
    let (site, _, _) = setup(Some("demo-code"));
    let meta = call(&site, "GET", "/wifi-setup/api/meta", "", "", false);
    assert!(meta.1.contains("\"code_required\":true"));
    assert!(!meta.1.contains("demo-code"));
    assert_eq!(
        call(&site, "GET", "/wifi-setup/api/status", "", "", false).0,
        401
    );
    assert_eq!(
        call(&site, "POST", "/wifi-setup/api/scan", MARKER, "", false).0,
        401
    );
    assert_eq!(
        call(
            &site,
            "POST",
            "/wifi-setup/api/scan",
            "X-Wifi-Setup-Code: demo-code\r\n",
            "",
            false
        )
        .0,
        403
    );
    assert_eq!(
        call(
            &site,
            "GET",
            "/wifi-setup/api/status",
            &format!("{CODE}Origin: http://evil.example\r\n"),
            "",
            false
        )
        .0,
        403
    );
    assert_eq!(
        call(&site, "OPTIONS", "/wifi-setup/api/scan", CODE, "", false).0,
        405
    );
    assert_eq!(
        call(&site, "GET", "/wifi-setup/api/status", CODE, "", false).0,
        200
    );
}
#[test]
fn operations_are_bounded_and_only_worker_touches_radio() {
    let (site, c, data) = setup(None);
    assert_eq!(
        call(&site, "POST", "/wifi-setup/api/scan", MARKER, "", false).0,
        202
    );
    assert_eq!(data.scans.load(Ordering::Relaxed), 0);
    assert_eq!(
        call(&site, "POST", "/wifi-setup/api/scan", MARKER, "", false).0,
        409
    );
    assert_eq!(c.snapshot().phase, "scanning");
    assert!(c.run_once());
    assert_eq!(c.snapshot().networks, vec!["Home", "<b>not markup</b>"]);
    assert_eq!(data.scans.load(Ordering::Relaxed), 1);
    let join = r#"{"ssid":"Home","password":"engineering","retry_minutes":5}"#;
    assert_eq!(
        call(&site, "POST", "/wifi-setup/api/join", MARKER, join, false).0,
        202
    );
    assert_eq!(data.joins.load(Ordering::Relaxed), 0);
    c.run_once();
    let snapshot = c.snapshot();
    assert_eq!(snapshot.phase, "connected");
    assert_eq!(snapshot.address, "192.168.1.42");
    assert_eq!(snapshot.retry_minutes, 5);
    assert_eq!(data.kv.get("retry_min"), Some(5u16.to_le_bytes().to_vec()));
    assert_eq!(data.kv.get("ssid"), Some(b"Home".to_vec()));
    assert!(
        !call(&site, "GET", "/wifi-setup/api/status", MARKER, "", false)
            .1
            .contains("engineering")
    );
}
#[test]
fn bad_inputs_are_rejected_and_failed_join_preserves_credentials() {
    let (site, c, data) = setup(None);
    data.kv.set("ssid", b"old").unwrap();
    data.kv.set("pass", b"old-password").unwrap();
    for body in [
        r#"{"ssid":"","password":"engineering","retry_minutes":3}"#,
        r#"{"ssid":"Home","password":"short","retry_minutes":3}"#,
        r#"{"ssid":"Home","password":"engineering","retry_minutes":0}"#,
    ] {
        assert_eq!(
            call(&site, "POST", "/wifi-setup/api/join", MARKER, body, false).0,
            400
        );
    }
    assert_eq!(
        call(
            &site,
            "POST",
            "/wifi-setup/api/join",
            MARKER,
            &" ".repeat(513),
            false
        )
        .0,
        413
    );
    let wrong = r#"{"ssid":"new","password":"wrong-password","retry_minutes":3}"#;
    assert_eq!(
        call(&site, "POST", "/wifi-setup/api/join", MARKER, wrong, false).0,
        202
    );
    c.run_once();
    assert_eq!(c.snapshot().phase, "error");
    assert!(c.active());
    assert_eq!(data.kv.get("ssid"), Some(b"old".to_vec()));
    assert_eq!(data.kv.get("pass"), Some(b"old-password".to_vec()));
    assert_eq!(
        call(
            &site,
            "POST",
            "/wifi-setup/api/retry",
            MARKER,
            r#"{"retry_minutes":8}"#,
            false
        )
        .0,
        202
    );
    c.run_once();
    assert_eq!(data.kv.get("retry_min"), Some(8u16.to_le_bytes().to_vec()));
}
#[test]
fn status_remains_responsive_while_scan_is_blocked() {
    let mut backend = radio();
    let (entered, rx) = mpsc::sync_channel(1);
    let release = Arc::new(Barrier::new(2));
    backend.block = Some((entered, Arc::clone(&release)));
    let c = Arc::new(Controller::new(backend, Options::default()).unwrap());
    c.enqueue(Command::Scan).unwrap();
    let worker = Arc::clone(&c);
    let thread = std::thread::spawn(move || worker.run_once());
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let poller = Arc::clone(&c);
    let (tx, rx) = mpsc::sync_channel(1);
    let poll = std::thread::spawn(move || tx.send(poller.snapshot()).unwrap());
    let result = rx.recv_timeout(Duration::from_secs(2));
    release.wait();
    thread.join().unwrap();
    poll.join().unwrap();
    assert_eq!(result.unwrap().phase, "scanning");
}
#[test]
fn retry_window_defaults_bounds_and_backoff() {
    let window = RetryWindow::new(Options::default().retry_minutes).unwrap();
    assert_eq!(window.duration(), Duration::from_secs(180));
    assert!(!window.exhausted(Duration::from_secs(179)));
    assert!(window.exhausted(Duration::from_secs(180)));
    assert!(RetryWindow::new(0).is_err());
    assert!(RetryWindow::new(61).is_err());
    assert!(Options {
        code: Some(""),
        ..Default::default()
    }
    .validate()
    .is_err());
    assert_eq!(RetryWindow::backoff(0), Duration::from_secs(2));
    assert_eq!(RetryWindow::backoff(u32::MAX), Duration::from_secs(15));
}
