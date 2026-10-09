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
#[cfg(feature = "wifi-setup")]
const WIFI: (&str, &str) = ("", "");
#[cfg(not(feature = "wifi-setup"))]
const WIFI: (&str, &str) = (
    env!("HOUSEMETRICS_WIFI_SSID"),
    env!("HOUSEMETRICS_WIFI_PASSWORD"),
);

/// tools/boardsafe refuses to write an image whose marker names another
/// board (see boards.py). build.rs makes it from HOUSEMETRICS_BOARD and
/// HOUSEMETRICS_HOSTNAME.
#[used]
static BOARD_MARKER: &str = env!("HOUSEMETRICS_MARKER");

fn main() {
    if let Err(e) = run() {
        // Blinks the failed startup step on the LED; logs it every 5 s.
        esp::fail(&e.to_string());
    }
}

fn run() -> Result<(), esp::Error> {
    esp::init();
    let peripherals = Peripherals::take()?;
    let board_config = BoardConfig {
        ssid: WIFI.0,
        password: WIFI.1,
        hostname: HOSTNAME,
        instance: "housemetrics",
        // certs/<hostname>.crt and .key, copied by build.rs.
        cert_pem: concat!(include_str!(concat!(env!("OUT_DIR"), "/server.crt")), "\0").as_bytes(),
        key_pem: concat!(include_str!(concat!(env!("OUT_DIR"), "/server.key")), "\0").as_bytes(),
        ca_pem: Some(concat!(include_str!("../../certs/household-ca.crt"), "\0").as_bytes()),
        limits: Limits::small_board(),
        psram_stacks: true,
        network_core: None,
        app_core: None,
        serve_stack: 32 * 1024,
        wait_for_wifi: true,
        ntp_server: None,
        saved_wifi: None,
        setup_network: None,
        force_setup: false,
        mdns_https_path: "/",
        // HOUSEMETRICS_STATUS_LED=off for a board without the S2 Mini's LED.
        led: match option_env!("HOUSEMETRICS_STATUS_LED") {
            Some("off") => None,
            _ => Some(Led::s2_mini(peripherals.pins.gpio15, "housemetrics")?),
        },
    };
    #[cfg(feature = "wifi-setup")]
    let board = {
        let mut config = board_config;
        // Provisioning builds take Wi-Fi from NVS/the phone, not baked-in
        // household credentials. A new board opens setup immediately.
        config.saved_wifi = Some("hm_wifi");
        config.setup_network = Some("housemetrics-setup");
        esp::start_with_wifi_setup(
            config,
            peripherals.modem,
            miniframework::wifi_setup::Options {
                code: option_env!("HOUSEMETRICS_WIFI_SETUP_CODE"),
                ..Default::default()
            },
        )?
    };
    #[cfg(not(feature = "wifi-setup"))]
    let board = esp::start(board_config, peripherals.modem)?;
    let profile = Profile::s2();
    let kv = Arc::new(board.kv("hm")?);
    let app = build(kv, env!("HOUSEMETRICS_ADMIN_PASSWORD"), false, &profile);
    configure_scrapes(&app.scrapes).map_err(|e| std::io::Error::other(e.message))?;
    let store = app.store.clone();
    let self_store = app.store.clone();
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
    #[cfg(feature = "wifi-setup")]
    let app = board.wifi_portal(app)?;
    let site = Site::new(config, app, board.platform(peripherals.temp_sensor));
    let mut last = Instant::now() - Duration::from_secs(60);
    board.serve(site, move |site| {
        if last.elapsed() >= Duration::from_secs(10) {
            last = Instant::now();
            record_self(&self_store, &site.sysinfo());
        }
    })
}
