//! party firmware for an ESP32-S3 N16R8. Build with `make firmware`; read
//! ../DEPLOY.md before flashing.
//!
//! Tasks: the framework's connection loop runs on its own core; this main
//! task runs housekeeping. Wi-Fi joins in the background. A failed startup
//! is explained at http://<board>:8080/ once Wi-Fi is up.
#[cfg(not(target_os = "espidf"))]
compile_error!("build the firmware with `make firmware` (xtensa-esp32s3-espidf, --features esp32)");

use esp_idf_svc::hal::peripherals::Peripherals;
use miniframework::esp::{self, BoardConfig, Led};
use miniframework::oauth::{Config as OAuthConfig, OAuth};
use miniframework::status::SIGNALS;
use miniframework::{Config, Site};
use party::work::Fetchers;
use party::{App, Limits, Shared};
use std::sync::Arc;

mod bundle {
    use miniframework::web::Asset;
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
    pub fn assets() -> &'static [Asset] {
        ASSETS
    }
}

const HOSTNAME: &str = match option_env!("PARTY_HOSTNAME") {
    Some(h) => h,
    None => "party",
};

fn main() {
    if let Err(e) = run() {
        esp::fail(&e.to_string());
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    esp::init();
    let ssid = option_env!("PARTY_WIFI_SSID").ok_or("PARTY_WIFI_SSID missing at build time")?;
    let password =
        option_env!("PARTY_WIFI_PASSWORD").ok_or("PARTY_WIFI_PASSWORD missing at build time")?;
    let peripherals = Peripherals::take()?;

    let mut board_config = BoardConfig::s3(ssid, password, HOSTNAME);
    board_config.instance = "party";
    // The household CA, for outbound HTTPS to household boards.
    board_config.ca_pem =
        Some(concat!(include_str!("../../certs/household-ca.crt"), "\0").as_bytes());
    #[cfg(feature = "https")]
    {
        board_config.cert_pem =
            concat!(include_str!(concat!(env!("OUT_DIR"), "/server.crt")), "\0").as_bytes();
        board_config.key_pem =
            concat!(include_str!(concat!(env!("OUT_DIR"), "/server.key")), "\0").as_bytes();
    }
    board_config.wait_for_wifi = false;
    // The N16R8's WS2812 pixel (GPIO 48, where mastomini drove it on this
    // board): the power-on and startup codes, then Morse while healthy.
    board_config.led = Some(Led::ws2812(
        peripherals.pins.gpio48,
        [0, 0, 24],
        "It is party time!",
    ));
    let board = esp::start(board_config, peripherals.modem)?;

    SIGNALS.step(8, "state partition");
    // The record store lives in its own NVS partition, apart from Wi-Fi and
    // framework settings. Never erased by firmware.
    let kv = Arc::new(board.partition_kv("partydata", "party")?);
    let limits = Limits::board();
    // What a PIN check costs on this chip (the rounds target ~100 ms).
    let started = std::time::Instant::now();
    party::pin::hash("0000", limits.pin_rounds)?;
    log::info!(
        "PIN hash: {} rounds in {} ms",
        limits.pin_rounds,
        started.elapsed().as_millis()
    );
    let mut app = App::open(kv.clone(), limits)?;
    app.setup_code = option_env!("PARTY_SETUP_CODE");
    app.build = option_env!("PARTY_BUILD").unwrap_or("dev");
    // Mastodon sign-in: one client per trust anchor (esp-tls takes one or the
    // other); the network steps run on the worker below.
    let oauth = Arc::new(OAuth::new(
        OAuthConfig {
            app_name: "party",
            website: format!("http://{HOSTNAME}.local"),
            ..OAuthConfig::default()
        },
        kv,
        Arc::new(board.fetch()),
        Arc::new(board.fetch_public()),
    ));
    app.attach_oauth(oauth);
    app.set_origin(&format!("http://{HOSTNAME}.local"));

    let mut config = Config::new("party", format!("{HOSTNAME}.local"));
    config.body_limit = 8 * 1024;
    config.assets = bundle::assets();
    config.ca_der = Some(include_bytes!("../../certs/household-ca.der"));
    config.proto_package = "party";
    // Calendar feeds (token URLs) and the sign-in callback are answered by
    // the app, not static files.
    config.app_paths = &["/cal", "/auth"];
    config.about = miniframework::about::AboutConfig {
        title: "party",
        description: "Clubs and parties for the household and the people around it",
        board: "ESP32-S3 N16R8",
        hardware: &["led:rgb"],
        links: &[("admin", "/admin")],
    };
    config.build = app.build;
    app.dev_origins = config.origins.clone();

    #[cfg(feature = "gate")]
    start_gate(&board, &app)?;

    // Outbound work (sign-in steps, email, the bot, publication): one TLS
    // client at a time, never on the serving loop.
    let app = Arc::new(app);
    let worker = app.clone();
    let (household, public) = (board.fetch(), board.fetch_public());
    esp::spawn_worker(c"party-out", 20 * 1024, false, move || loop {
        let f = Fetchers {
            household: &household,
            public: &public,
        };
        if !worker.work(&f) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    })?;

    let site = Site::new(config, Shared(app), board.platform(peripherals.temp_sensor));
    log::info!("party ready: http://{HOSTNAME}.local/ (joining Wi-Fi)");
    board.serve(site, move |site| site.service.0.tick())
}

/// Sprint 0 gate: a worker for the storage and crypto runs (in a scratch
/// namespace of partydata) and the outbound-HTTPS round robin.
#[cfg(feature = "gate")]
fn start_gate(board: &esp::Board, app: &App) -> Result<(), Box<dyn std::error::Error>> {
    use miniframework::fetch::Fetch;
    app.gate.set_targets(&[
        (
            "mastodon.social",
            "https://mastodon.social/.well-known/oauth-authorization-server",
        ),
        ("api.github.com", "https://api.github.com/zen"),
        ("api.brevo.com", "https://api.brevo.com/v3/"),
    ]);
    let gate = app.gate.clone();
    let scratch = board.partition_kv("partydata", "gate")?;
    let fetch = board.fetch_public();
    esp::spawn_worker(c"party-gate", 16 * 1024, false, move || {
        let get = move |url: &str| {
            fetch
                .get(url, "application/json", 32 * 1024)
                .map(|f| (f.status, f.body.len()))
        };
        loop {
            gate.run_once(&scratch, &[&get, &get, &get]);
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    })?;
    Ok(())
}
