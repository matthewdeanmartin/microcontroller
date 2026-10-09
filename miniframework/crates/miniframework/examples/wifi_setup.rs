//! Local UI preview with a simulated radio; never touches physical Wi-Fi.
//! Optional compile-time WIFI_SETUP_CODE, runtime WIFI_SETUP_DEMO_DATA/ADDR.
use miniframework::{
    desktop::{self, DesktopPlatform},
    kv::{FileKv, Kv},
    wifi_setup::{Backend, Controller, Info, Options, Portal},
    Config, Reply, Request, Service, Site,
};
use std::{
    net::Ipv4Addr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

struct Radio {
    active: AtomicBool,
    info: Mutex<Info>,
    joined: Mutex<Option<Instant>>,
    kv: FileKv,
}
impl Backend for Radio {
    fn active(&self) -> bool {
        if self
            .joined
            .lock()
            .unwrap()
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(60))
        {
            self.active.store(false, Ordering::Relaxed);
        }
        self.active.load(Ordering::Relaxed)
    }
    fn info(&self) -> Info {
        self.info.lock().unwrap().clone()
    }
    fn scan(&self) -> Result<Vec<String>, String> {
        std::thread::sleep(Duration::from_millis(100));
        Ok(vec![
            "Engineering lab".into(),
            "Home Wi-Fi".into(),
            "Guest (open)".into(),
        ])
    }
    fn join(&self, ssid: &str, password: &str) -> Result<Ipv4Addr, String> {
        std::thread::sleep(Duration::from_millis(300));
        if ssid != "Guest (open)" && password != "engineering" {
            return Err("Could not connect. Demo password: engineering".into());
        }
        let ip = Ipv4Addr::LOCALHOST;
        let mut info = self.info.lock().unwrap();
        info.ssid = ssid.into();
        info.address = Some(ip);
        *self.joined.lock().unwrap() = Some(Instant::now());
        Ok(ip)
    }
    fn set_retry_minutes(&self, minutes: u16) -> Result<(), String> {
        self.kv
            .set("retry_min", &minutes.to_le_bytes())
            .map_err(|e| e.to_string())?;
        self.info.lock().unwrap().retry_minutes = minutes;
        Ok(())
    }
    fn close(&self) -> Result<(), String> {
        self.active.store(false, Ordering::Relaxed);
        Ok(())
    }
}
struct App;
impl Service for App {
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        if req.path == "/" || req.path == "/api/v1/demo" {
            reply.text(200, "text/plain", "App is running; Wi-Fi setup is closed.");
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options {
        code: option_env!("WIFI_SETUP_CODE"),
        ..Default::default()
    };
    let path = std::env::var_os("WIFI_SETUP_DEMO_DATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("miniframework-wifi-setup-demo"));
    let kv = FileKv::open(path)?;
    let saved = kv
        .get("retry_min")
        .filter(|v| v.len() == 2)
        .and_then(|v| {
            miniframework::wifi_setup::validate_minutes(u32::from(u16::from_le_bytes([v[0], v[1]])))
                .ok()
        })
        .unwrap_or(options.retry_minutes);
    let controller = Arc::new(Controller::new(
        Radio {
            active: AtomicBool::new(true),
            info: Mutex::new(Info {
                ssid: String::new(),
                address: None,
                retry_minutes: saved,
            }),
            joined: Mutex::new(None),
            kv,
        },
        options,
    )?);
    let worker = Arc::clone(&controller);
    std::thread::spawn(move || loop {
        worker.run_once();
        std::thread::sleep(Duration::from_millis(50));
    });
    let mut config = Config::new("wifi-setup-preview", "localhost");
    config.body_limit = 512;
    config.response_limit = 2048;
    let site = Site::new(config, Portal::new(App, controller), DesktopPlatform);
    let address = std::env::var("WIFI_SETUP_DEMO_ADDR").unwrap_or("127.0.0.1:18091".into());
    println!("Local Wi-Fi UI preview: http://{address}/wifi-setup (demo password: engineering)");
    desktop::serve(&site, &address, || {})?;
    Ok(())
}
