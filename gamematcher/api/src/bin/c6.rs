//! Game Matcher firmware for the Waveshare ESP32-C6-LCD-1.47. Build with
//! `make firmware`; read ../DEPLOY.md before flashing.
//!
//! Tasks: the framework's connection loop has its own task; this main task
//! runs housekeeping. Wi-Fi joins in the background. A failed startup is
//! explained at http://<board>:8080/ once Wi-Fi is up.
#[cfg(not(target_os = "espidf"))]
compile_error!(
    "build the firmware with `make firmware` (riscv32imac-esp-espidf, --features esp32)"
);

use esp_idf_svc::hal::peripherals::Peripherals;
use gamematcher::outbound::Outbox;
use gamematcher::{App, Limits};
use miniframework::esp::{self, BoardConfig};
use miniframework::oauth::{Config as OAuthConfig, OAuth};
use miniframework::status::SIGNALS;
use miniframework::{Config, Site};
use std::sync::Arc;

mod bundle {
    use miniframework::web::Asset;
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
    pub fn assets() -> &'static [Asset] {
        ASSETS
    }
}

const HOSTNAME: &str = match option_env!("GAMEMATCHER_HOSTNAME") {
    Some(h) => h,
    None => "gamematcher",
};

fn main() {
    if let Err(e) = run() {
        esp::fail(&e.to_string());
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    esp::init();
    let ssid = option_env!("GAMEMATCHER_WIFI_SSID")
        .ok_or("GAMEMATCHER_WIFI_SSID missing at build time")?;
    let password = option_env!("GAMEMATCHER_WIFI_PASSWORD")
        .ok_or("GAMEMATCHER_WIFI_PASSWORD missing at build time")?;
    let peripherals = Peripherals::take()?;

    let mut board_config = BoardConfig::c6(ssid, password, HOSTNAME);
    board_config.instance = "Game Matcher";
    // The household CA, for outbound HTTPS to household boards.
    board_config.ca_pem =
        Some(concat!(include_str!("../../certs/household-ca.crt"), "\0").as_bytes());
    // No PSRAM: one TLS session at a time (a handshake peaks around 45 KB;
    // minicloud saw two exhaust this board's heap). HTTP/2 needs only one.
    #[cfg(feature = "https")]
    {
        board_config.cert_pem =
            concat!(include_str!(concat!(env!("OUT_DIR"), "/server.crt")), "\0").as_bytes();
        board_config.key_pem =
            concat!(include_str!(concat!(env!("OUT_DIR"), "/server.key")), "\0").as_bytes();
        board_config.limits.tls_clients = 1;
        board_config.limits.handshakes = 1;
    }
    board_config.wait_for_wifi = false;
    // The screen first, so it shows something while Wi-Fi joins.
    #[cfg(feature = "display")]
    let mut screen = {
        SIGNALS.step(7, "screen");
        set_household_zone();
        let boot = esp_idf_svc::hal::gpio::PinDriver::input(
            peripherals.pins.gpio9,
            esp_idf_svc::hal::gpio::Pull::Up,
        )?;
        screen::Screen::new(boot)?
    };
    let board = esp::start(board_config, peripherals.modem)?;

    SIGNALS.step(8, "state partition");
    // The state's records (and the schema-1 snapshot slots they were
    // migrated from) live in their own NVS partition, apart from Wi-Fi and
    // framework settings. Never erased by firmware.
    let kv = Arc::new(board.partition_kv("gmstate", "gm")?);
    let limits = Limits::board();
    // What a PIN check costs on this chip (the rounds target ~100 ms).
    let started = std::time::Instant::now();
    gamematcher::pin::hash("0000", limits.pin_rounds)?;
    log::info!(
        "PIN hash: {} rounds in {} ms",
        limits.pin_rounds,
        started.elapsed().as_millis()
    );
    let mut app = App::open(kv.clone(), limits)?;
    #[cfg(feature = "display")]
    {
        app.utc_offset = household_offset;
    }
    // Mastodon sign-in: one client per trust anchor (esp-tls takes one or the
    // other), and the network steps on their own worker so a 1-3 s TLS
    // handshake to an instance never stalls serving.
    let oauth = Arc::new(OAuth::new(
        OAuthConfig {
            app_name: "Game Matcher",
            website: format!("http://{HOSTNAME}.local"),
            ..OAuthConfig::default()
        },
        kv.clone(),
        Arc::new(board.fetch()),
        Arc::new(board.fetch_public()),
    ));
    // The household Mastodon bot's queue: same worker, one TLS client at a time.
    let outbox = Arc::new(Outbox::open(
        kv.clone(),
        Arc::new(board.fetch()),
        Arc::new(board.fetch_public()),
    ));
    let public_outbox = Arc::new(Outbox::open_public(
        kv,
        Arc::new(board.fetch()),
        Arc::new(board.fetch_public()),
    ));
    let public_posts = public_outbox.clone();
    app.attach_public_outbox(public_outbox);
    let (worker, posts) = (oauth.clone(), outbox.clone());
    esp::spawn_worker(c"gm-oauth", 16 * 1024, false, move || loop {
        let now = miniframework::wall_ms().map_or(0, |ms| ms / 1000);
        // Posts wait for the clock: their retry times are wall-clock.
        let worked =
            worker.run_once() | (now != 0 && (posts.run_once(now) | public_posts.run_once(now)));
        if !worked {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    })?;
    app.attach_oauth(oauth);
    // Email from the build settings, unless the admin page set it up since.
    if let Some(key) = option_env!("GAMEMATCHER_BREVO_API_KEY") {
        let setup = gamematcher::outbound::MailSetup {
            api_key: key.into(),
            sender_email: option_env!("GAMEMATCHER_MAIL_FROM").unwrap_or("").into(),
            sender_name: "Game Matcher".into(),
            built_in: true,
        };
        if let Err(e) = outbox.begin_mail_setup(setup) {
            log::warn!("built-in email key: {}", e.message);
        }
    }
    app.attach_outbox(outbox);
    app.origin = format!("http://{HOSTNAME}.local");
    app.setup_code = option_env!("GAMEMATCHER_SETUP_CODE");
    app.build = option_env!("GAMEMATCHER_BUILD").unwrap_or("dev");

    let mut config = Config::new("gamematcher", format!("{HOSTNAME}.local"));
    config.body_limit = 4 * 1024;
    config.response_limit = 32 * 1024;
    config.assets = bundle::assets();
    config.ca_der = Some(include_bytes!("../../certs/household-ca.der"));
    config.proto_package = "gamematcher";
    // Calendar feeds (token URLs) are answered by the app, not static files.
    config.app_paths = &["/cal", "/auth"];
    // What /.well-known/miniframework says (homelab and friends read it).
    config.about = miniframework::about::AboutConfig {
        title: "Game Matcher",
        description: "Household game-night planner",
        board: "Waveshare ESP32-C6-LCD-1.47",
        hardware: &[
            "screen:320x172",
            "button:boot",
            "led:rgb",
            "sd-slot",
            "sensor:temperature",
        ],
        links: &[
            ("admin", "/admin"),
            ("screen", "/api/v1/display/frame.bmp"),
            ("deck", "/api/v1/display"),
        ],
    };
    config.build = app.build;
    app.dev_origins = config.origins.clone();

    #[cfg(feature = "gate")]
    start_gate(&board, &app)?;

    let site = Site::new(config, app, board.platform(peripherals.temp_sensor));
    log::info!("Game Matcher ready: http://{HOSTNAME}.local/ (joining Wi-Fi)");
    board.serve(site, move |site| {
        site.service.tick();
        #[cfg(feature = "display")]
        screen.tick(&site.service);
    })
}

/// Sprint 0 gate: a worker that fetches a household-CA host and a public-CA
/// host in turn while `POST /api/v1/gate` has it running.
#[cfg(feature = "gate")]
fn start_gate(board: &esp::Board, app: &App) -> Result<(), Box<dyn std::error::Error>> {
    use miniframework::fetch::Fetch;
    let household = option_env!("GAMEMATCHER_GATE_HOUSEHOLD_URL")
        .unwrap_or("https://housemetrics.local/api/v1/sys");
    let public = option_env!("GAMEMATCHER_GATE_PUBLIC_URL")
        .unwrap_or("https://mastodon.social/.well-known/oauth-authorization-server");
    app.gate
        .set_targets(&[("household-ca", household), ("public-ca", public)]);
    let gate = app.gate.clone();
    let household_fetch = board.fetch();
    let public_fetch = board.fetch_public();
    esp::spawn_worker(c"gm-gate", 16 * 1024, false, move || {
        let household = move |url: &str| {
            household_fetch
                .get(url, "application/json", 32 * 1024)
                .map(|f| (f.status, f.body.len()))
        };
        let public = move |url: &str| {
            public_fetch
                .get(url, "application/json", 32 * 1024)
                .map(|f| (f.status, f.body.len()))
        };
        loop {
            gate.run_once(&[&household, &public]);
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    })?;
    Ok(())
}

/// The household's zone (POSIX TZ string, `GAMEMATCHER_TZ` at build time),
/// for newlib's `localtime_r`.
#[cfg(feature = "display")]
fn set_household_zone() {
    let zone = option_env!("GAMEMATCHER_TZ").unwrap_or("EST5EDT,M3.2.0,M11.1.0");
    std::env::set_var("TZ", zone);
    // SAFETY: reads the TZ variable just set; no other thread uses time yet.
    unsafe { esp_idf_svc::sys::tzset() };
}

/// UTC offset (seconds) of the household zone at `utc`.
#[cfg(feature = "display")]
fn household_offset(utc: u64) -> i64 {
    use esp_idf_svc::sys;
    let Ok(t) = sys::time_t::try_from(utc) else {
        return 0;
    };
    let mut tm = sys::tm::default();
    // SAFETY: both pointers refer to live, aligned values for this call.
    // localtime_r writes only the caller's tm; TZ is fixed during startup.
    if unsafe { sys::localtime_r(&t, &mut tm) }.is_null() {
        return 0;
    }
    // Days from the civil date (Howard Hinnant), back to seconds.
    let (y, m, d) = (
        i64::from(tm.tm_year) + 1900,
        i64::from(tm.tm_mon) + 1,
        i64::from(tm.tm_mday),
    );
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let local = days * 86_400
        + i64::from(tm.tm_hour) * 3600
        + i64::from(tm.tm_min) * 60
        + i64::from(tm.tm_sec);
    local - utc as i64
}

/// The board's screen: the card rotation (`spec/07-display.md`).
#[cfg(feature = "display")]
mod screen {
    use esp_idf_svc::hal::gpio::{Input, PinDriver};
    use esp_idf_svc::sys;
    use gamematcher::messages::DisplayCard;
    use gamematcher::App;
    use std::hash::{Hash, Hasher};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};

    // The C driver owns global state and cannot be initialized or shared twice.
    static TAKEN: AtomicBool = AtomicBool::new(false);
    use std::time::{Duration, Instant};

    const TURN: Duration = Duration::from_secs(8);
    const PIN: Duration = Duration::from_secs(300);
    const LONG_PRESS: Duration = Duration::from_millis(900);
    const DIM_AFTER: Duration = Duration::from_secs(600);
    const BRIGHT: i32 = 40;
    const DIM: i32 = 15;

    pub struct Screen {
        boot: PinDriver<'static, Input>,
        // NonNull also keeps this owner !Send/!Sync; the C driver is task-local.
        pixels: NonNull<u16>,
        display_failed: bool,
        pressed_at: Option<Instant>,
        cards: Vec<DisplayCard>,
        deck_hash: u64,
        built: Instant,
        page: usize,
        turned: Instant,
        pinned_until: Option<Instant>,
        drawn: u64,
        changed: Instant,
        dimmed: bool,
    }

    fn hash(value: impl Hash) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut h);
        h.finish()
    }

    impl Screen {
        pub fn new(boot: PinDriver<'static, Input>) -> Result<Self, sys::EspError> {
            if TAKEN.swap(true, Ordering::AcqRel) {
                return Err(sys::EspError::from_infallible::<
                    { sys::ESP_ERR_INVALID_STATE },
                >());
            }
            // SAFETY: the singleton claim above permits initialization once.
            // Keep it claimed on failure: the C driver can be partially initialized.
            sys::esp!(unsafe { sys::gm_display_init() })?;
            // SAFETY: successful initialization allocated the fixed-size DMA buffer.
            let pixels = NonNull::new(unsafe { sys::gm_display_buffer() })
                .ok_or_else(sys::EspError::from_infallible::<{ sys::ESP_ERR_NO_MEM }>)?;
            let now = Instant::now();
            Ok(Self {
                boot,
                pixels,
                display_failed: false,
                pressed_at: None,
                cards: Vec::new(),
                deck_hash: 0,
                built: now - Duration::from_secs(60),
                page: 0,
                turned: now,
                pinned_until: None,
                drawn: 0,
                changed: now,
                dimmed: false,
            })
        }

        fn backlight(&mut self, dim: bool) {
            if dim != self.dimmed {
                // SAFETY: this singleton owns the initialized LEDC channel.
                match sys::esp!(unsafe {
                    sys::gm_display_backlight(if dim { DIM } else { BRIGHT })
                }) {
                    Ok(()) => self.dimmed = dim,
                    Err(e) => log::warn!("screen backlight: {e}"),
                }
            }
        }

        /// Called from the main loop every ~10 ms.
        pub fn tick(&mut self, app: &App) {
            if self.display_failed {
                return;
            }
            let now = Instant::now();
            // BOOT: short press = next card, long press = pin this one.
            let down = self.boot.is_low();
            match (down, self.pressed_at) {
                (true, None) => self.pressed_at = Some(now),
                (false, Some(at)) => {
                    self.pressed_at = None;
                    if now - at >= LONG_PRESS {
                        self.pinned_until = Some(now + PIN);
                    } else {
                        self.page += 1;
                        self.pinned_until = None;
                    }
                    self.turned = now;
                    self.changed = now;
                }
                _ => {}
            }
            if now - self.built >= Duration::from_secs(1) {
                self.built = now;
                self.cards = app.display_deck();
                let h = hash(format!("{:?}", self.cards));
                if h != self.deck_hash {
                    self.deck_hash = h;
                    self.changed = now;
                }
            }
            if self.cards.is_empty() {
                return;
            }
            let pinned = self.pinned_until.is_some_and(|until| now < until);
            if !pinned && now - self.turned >= TURN {
                self.page += 1;
                self.turned = now;
            }
            self.page %= self.cards.len();
            app.display_page
                .store(self.page, std::sync::atomic::Ordering::Relaxed);
            self.backlight(now - self.changed >= DIM_AFTER);

            // Redraw when anything visible changed: the deck, the card, the
            // minute (clocks and countdowns), or the network.
            let ip = miniframework::esp::station_ip();
            let minute = miniframework::wall_ms().unwrap_or(0) / 60_000;
            let wanted = hash((
                self.deck_hash,
                self.page,
                self.cards.len(),
                minute,
                ip.is_some(),
            ));
            if wanted == self.drawn {
                return;
            }
            self.drawn = wanted;
            let origin = ip.as_ref().map_or_else(
                || "http://gamematcher.local".to_string(),
                |ip| format!("http://{ip}"),
            );
            let card = self.cards[self.page].clone();
            let frame = app.display_frame(
                card,
                self.page,
                self.cards.len(),
                ip.is_some(),
                &origin,
                "gamematcher.local",
            );
            // SAFETY: init allocated and initialized 320 * 24 aligned u16 pixels.
            // This singleton is the only Rust owner, and the slice stays in this
            // draw call. Each flush waits for DMA; any failure disables all future
            // access, since a timed-out transfer may still be reading the buffer.
            let buf = unsafe { std::slice::from_raw_parts_mut(self.pixels.as_ptr(), 320 * 24) };
            let started = Instant::now();
            let result = frame.draw(buf, |top, rows, _| {
                // SAFETY: the strip was drawn into the component's buffer.
                sys::esp!(unsafe { sys::gm_display_flush(top, rows) })
            });
            match result {
                Ok(()) => log::info!(
                    "screen: card {}/{} drawn in {} ms",
                    self.page + 1,
                    self.cards.len(),
                    started.elapsed().as_millis()
                ),
                Err(e) => {
                    self.display_failed = true;
                    log::warn!("screen disabled until reboot after transfer failure: {e}");
                }
            }
        }
    }
}
