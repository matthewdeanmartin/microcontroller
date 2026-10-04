//! housemetrics firmware for the ESP32-S2 Mini. Build with
//! `make firmware`; read DEPLOY_S2.md before flashing.
#[cfg(not(target_os = "espidf"))]
compile_error!("build the firmware with `make firmware` (Xtensa target, --features esp32)");

use esp_idf_svc::hal::peripherals::Peripherals;
use housemetrics::{build, configure_scrapes, record_self, Profile};
use miniframework::esp::{self, BoardConfig, Led};
use miniframework::mux::Limits;
use miniframework::{Config, Site};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod bundle {
    use miniframework::web::Asset;
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
    pub fn assets() -> &'static [Asset] {
        ASSETS
    }
}

const HOSTNAME: &str = match option_env!("HOUSEMETRICS_HOSTNAME") {
    Some(h) => h,
    None => "housemetrics",
};

/// tools/boardsafe refuses to write an image whose marker names another
/// board (see boards.py). A build with another HOUSEMETRICS_HOSTNAME needs
/// its own registry entry and marker.
#[used]
static BOARD_MARKER: &str = "HOUSEMETRICS-BOARD:s2:housemetrics.local;";

fn main() {
    if let Err(e) = run() {
        // Blinks the failed startup step on the LED; logs it every 5 s.
        esp::fail(&e.to_string());
    }
}

fn run() -> Result<(), esp::Error> {
    esp::init();
    let peripherals = Peripherals::take()?;
    let board = esp::start(
        BoardConfig {
            ssid: env!("HOUSEMETRICS_WIFI_SSID"),
            password: env!("HOUSEMETRICS_WIFI_PASSWORD"),
            hostname: HOSTNAME,
            instance: "housemetrics",
            cert_pem: concat!(include_str!("../../certs/housemetrics.crt"), "\0").as_bytes(),
            key_pem: concat!(include_str!("../../certs/housemetrics.key"), "\0").as_bytes(),
            ca_pem: Some(concat!(include_str!("../../certs/household-ca.crt"), "\0").as_bytes()),
            limits: Limits::small_board(),
            psram_stacks: true,
            network_core: None,
            app_core: None,
            serve_stack: 32 * 1024,
            ntp_server: None,
            // HOUSEMETRICS_STATUS_LED=off for a board without the S2 Mini's LED.
            led: match option_env!("HOUSEMETRICS_STATUS_LED") {
                Some("off") => None,
                _ => Some(Led::s2_mini("housemetrics")),
            },
        },
        peripherals.modem,
    )?;
    let profile = Profile::s2();
    let kv = Arc::new(board.kv("hm")?);
    let app = build(kv, env!("HOUSEMETRICS_ADMIN_PASSWORD"), false, &profile);
    configure_scrapes(&app.scrapes).map_err(|e| std::io::Error::other(e.message))?;
    let store = app.store.clone();
    let scrapes = app.scrapes.clone();
    let fetch = board.fetch();
    esp::spawn_worker(c"hm-scrape", 16 * 1024, true, move || loop {
        scrapes.run_due(&fetch, &store);
        std::thread::sleep(Duration::from_secs(1));
    })?;
    let mut config = Config::new("housemetrics", format!("{HOSTNAME}.local"));
    config.body_limit = profile.body_limit;
    config.response_limit = profile.response_limit;
    config.assets = bundle::assets();
    config.ca_der = Some(include_bytes!("../../certs/household-ca.der"));
    config.proto_package = "housemetrics";
    config.build = option_env!("HOUSEMETRICS_BUILD").unwrap_or("dev");
    if let Some(extra) = option_env!("HOUSEMETRICS_ORIGINS") {
        config.origins.extend(extra.split(',').map(str::to_string));
    }
    let site = Site::new(config, app, board.platform());
    let mut last = Instant::now() - Duration::from_secs(60);
    board.serve(site, move |site| {
        if last.elapsed() >= Duration::from_secs(10) {
            last = Instant::now();
            record_self(&site.service.store, &site.sysinfo());
        }
    })
}
