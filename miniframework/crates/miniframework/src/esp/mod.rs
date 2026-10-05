//! The ESP-IDF runner: Wi-Fi, SNTP, mDNS, HTTPS + HTTP listeners, NVS and
//! an HTTPS client, around the same [`Site`] the desktop runs.
//!
//! Task layout (from NanaCoin's two banks):
//! - a handshake task runs esp-tls server handshakes (~1 s of ECDHE each)
//!   and hands finished sessions over a bounded channel;
//! - single core (S2): the calling task (main) multiplexes every
//!   established connection and runs the app's `tick` between turns;
//! - two cores (S3, [`BoardConfig::app_core`] set): handshakes stay on the
//!   network core, the multiplexer gets its own task on the app core, and
//!   the calling task runs `tick` and housekeeping;
//! - Wi-Fi reconnection is checked every 10 s.
//!
//! On a single-core S2 both tasks share the CPU, so a new client's handshake
//! still slows everyone down; returning clients resume with TLS session
//! tickets (tens of milliseconds) and keep-alive avoids handshakes entirely.
//!
//! Storage is never erased by the framework. esp-idf-svc's convenience
//! constructors erase an NVS partition that is full or was written by a
//! newer IDF; here such a partition is an error the app reports, because
//! it may hold the only copy of someone's data.
#[cfg(feature = "tls")]
use crate::events::Task;
use crate::events::{self, Event};
use crate::fetch::{Fetch, Fetched};
use crate::kv::{check_key, Kv};
use crate::mux::{Conn, Limits, Mux};
use crate::site::{Service, Site};
use crate::status::{Stage, SIGNALS};
use crate::sys::{Heap, Partition, Platform, SysInfo, Wifi, STATS};
use esp_idf_svc::{
    eventloop::{EspSubscription, EspSystemEventLoop, System},
    hal::{
        cpu::Core,
        delay::FreeRtos,
        gpio::{Output, OutputPin, PinDriver},
        modem::Modem,
        reset::ResetReason,
        task::thread::{MallocCap, ThreadSpawnConfiguration},
        temp_sensor::{TempSensor, TempSensorConfig, TempSensorDriver},
    },
    http::{client::EspHttpConnection, Method},
    mdns::EspMdns,
    nvs::{EspDefaultNvsPartition, EspNvs, EspNvsPartition, NvsCustom, NvsDefault},
    sntp::{EspSntp, SntpConf},
    sys,
    tls::X509,
    wifi::{BlockingWifi, Configuration, EspWifi, WifiDeviceId},
};
use std::ffi::CStr;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(feature = "tls")]
use std::os::fd::AsRawFd;
#[cfg(feature = "tls")]
use std::ptr::NonNull;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

mod led;
mod wifi;
use wifi::Radio;
pub use wifi::WifiSetup;

// The connection loop waits in 1 ms steps (`FreeRtos::delay_ms(1)`). At the
// IDF default of 100 Hz each wait is 10 ms, and sending fell from ~175 to
// ~11 KiB/s on Minicloud's C6 (October 4, 2026). Set CONFIG_FREERTOS_HZ=1000
// in the app's sdkconfig.defaults.
const _: () = assert!(
    sys::CONFIG_FREERTOS_HZ >= 1000,
    "miniframework needs CONFIG_FREERTOS_HZ=1000 (sdkconfig.defaults); at 100 Hz the board sends ~15x slower"
);
mod logcap;

/// Lowest free stack seen (bytes) on the TLS and LED tasks, for the health log.
pub(crate) static TLS_STACK_FREE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);
pub(crate) static LED_STACK_FREE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

pub type Error = Box<dyn std::error::Error>;

/// What the board needs to come up. Certificates are PEM with a trailing
/// NUL (use `concat!(include_str!(...), "\0").as_bytes()`).
pub struct BoardConfig {
    pub ssid: &'static str,
    pub password: &'static str,
    /// mDNS host name without `.local`.
    pub hostname: &'static str,
    /// mDNS instance name (shown by service browsers).
    pub instance: &'static str,
    pub cert_pem: &'static [u8],
    pub key_pem: &'static [u8],
    /// The household CA (PEM + NUL), for the HTTPS client.
    pub ca_pem: Option<&'static [u8]>,
    pub limits: Limits,
    /// Put worker stacks (TLS handshakes, app workers) in PSRAM. Needed on
    /// the S2, whose internal RAM cannot hold them beside Wi-Fi. Never for a
    /// task that writes flash (NVS) — flash writes disable the PSRAM cache.
    pub psram_stacks: bool,
    /// Dual-core boards: the core for TLS handshakes (with Wi-Fi), and the
    /// core for the connection multiplexer. With `app_core` set, `serve`
    /// gives the multiplexer its own task there. `None`/`None` on the S2.
    pub network_core: Option<Core>,
    pub app_core: Option<Core>,
    /// Stack of the multiplexer task when it has its own (internal RAM).
    /// On a single-core board, `app_core: Some(Core::Core0)` also gives it
    /// its own task, leaving the calling task free for the app's loop.
    pub serve_stack: usize,
    /// Wait in [`start`] until Wi-Fi is up (true), or return at once and
    /// join in the background (an app with its own screen or buttons that
    /// must work without a network). Reconnects are always in the
    /// background: a lost access point never stalls the serving loop.
    pub wait_for_wifi: bool,
    pub ntp_server: Option<&'static str>,
    /// Wi-Fi chosen at runtime: the system-NVS namespace (max 15
    /// characters) holding the network a person picked (keys `ssid`,
    /// `pass`). It is tried before the built-in `ssid`/`password`, which
    /// may then be empty; built-in credentials that work are saved there.
    pub saved_wifi: Option<&'static str>,
    /// An open network to start when no network can be joined, so someone
    /// can choose one on the app's page ([`WifiSetup`]). Needs `saved_wifi`.
    pub setup_network: Option<&'static str>,
    /// Open the setup network at boot even with a saved network (a
    /// developer build for testing the setup pages).
    pub force_setup: bool,
    /// The `path` TXT record of the mDNS `_https` service.
    pub mdns_https_path: &'static str,
    /// The status light (POST, startup step codes, health, Morse), or
    /// `None` for a board without a usable LED.
    pub led: Option<Led>,
}

/// A plain (single-colour) status LED on a GPIO.
pub struct Led {
    pub(crate) driver: PinDriver<'static, Output>,
    /// The pin level that lights it (the S2 Mini's GPIO 15: high).
    pub active_high: bool,
    /// What it spells in Morse while healthy, e.g. the app's name.
    pub phrase: &'static str,
}

impl Led {
    /// The ESP32-S2 Mini's blue LED.
    pub fn s2_mini(
        pin: impl OutputPin + 'static,
        phrase: &'static str,
    ) -> Result<Self, sys::EspError> {
        Ok(Self {
            driver: PinDriver::output(pin)?,
            active_high: true,
            phrase,
        })
    }

    pub fn new(
        pin: impl OutputPin + 'static,
        active_high: bool,
        phrase: &'static str,
    ) -> Result<Self, sys::EspError> {
        Ok(Self {
            driver: PinDriver::output(pin)?,
            active_high,
            phrase,
        })
    }
}

impl BoardConfig {
    /// Defaults for an ESP32-S2 Mini; fill in Wi-Fi, names and certificates.
    pub fn s2(ssid: &'static str, password: &'static str, hostname: &'static str) -> Self {
        Self {
            ssid,
            password,
            hostname,
            instance: hostname,
            cert_pem: b"\0",
            key_pem: b"\0",
            ca_pem: None,
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
            led: None,
        }
    }

    /// Defaults for an ESP32-C6 without PSRAM (512 KiB internal RAM, one
    /// core): plain HTTP, three connections, the serving loop on its own
    /// task so the app's main loop stays free.
    pub fn c6(ssid: &'static str, password: &'static str, hostname: &'static str) -> Self {
        Self {
            limits: Limits {
                tls_clients: 0,
                http_clients: 3,
                h2_streams: 4,
                handshakes: 0,
                response_budget: 48 * 1024,
                idle: Duration::from_secs(30),
                request_deadline: Duration::from_secs(5),
            },
            psram_stacks: false,
            app_core: Some(Core::Core0),
            // Handlers that build JSON values need room (Minicloud ran its
            // HTTP thread with 32 KiB).
            serve_stack: 32 * 1024,
            ..Self::s2(ssid, password, hostname)
        }
    }

    /// Defaults for an ESP32-S3 N16R8: two cores, larger connection limits.
    #[cfg(mf_dual_core)]
    pub fn s3(ssid: &'static str, password: &'static str, hostname: &'static str) -> Self {
        Self {
            limits: Limits::large_board(),
            psram_stacks: false,
            network_core: Some(Core::Core0),
            app_core: Some(Core::Core1),
            ..Self::s2(ssid, password, hostname)
        }
    }
}

/// A board with Wi-Fi up, time and mDNS running.
pub struct Board {
    pub config: BoardConfig,
    pub nvs: EspDefaultNvsPartition,
    wifi: std::sync::Arc<Mutex<Radio>>,
    /// A background join in progress since then.
    joining: Option<Instant>,
    _sntp: EspSntp<'static>,
    mdns: Option<EspMdns>,
    _wifi_events: EspSubscription<'static, System>,
    partitions: Mutex<std::collections::BTreeMap<String, EspNvsPartition<NvsCustom>>>,
}

/// A background join that has not finished in this long is restarted.
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Names for the startup-failure page, set by [`start`].
static IDENTITY: OnceLock<(&'static str, &'static str)> = OnceLock::new();
static RADIO: Mutex<std::sync::Weak<Mutex<Radio>>> = Mutex::new(std::sync::Weak::new());

/// The first thing `main` calls: ESP-IDF patches, the uptime clock and the
/// served log (so every later line, and a crash, is kept). [`start`] calls
/// it too; an app that does work before `start` (its own status light)
/// calls it first. Idempotent.
pub fn init() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        sys::link_patches();
        crate::uptime_ms();
        logcap::install(16 * 1024);
        #[cfg(feature = "gzip")]
        crate::wire::gzip::set_runner(gzip_runner);
    });
}

/// Brings the board online. Retries Wi-Fi until it connects: a board that
/// boots before the router (after a power cut) must not give up. Takes
/// only the modem, so the app keeps the other peripherals (pins for its
/// own light or sensors): `start(config, Peripherals::take()?.modem)`.
pub fn start(mut config: BoardConfig, modem: Modem<'static>) -> Result<Board, Error> {
    init();
    let _ = IDENTITY.set((config.instance, config.hostname));
    // The light first, so POST and step codes show even if a later step fails.
    if let Some(led) = config.led.take() {
        led::start(led, config.psram_stacks);
    }
    SIGNALS.stage(Stage::System);
    let event_loop = EspSystemEventLoop::take()?;
    let wifi_events = event_loop.subscribe::<esp_idf_svc::wifi::WifiEvent, _>(|event| {
        // Runs on IDF's small system-event task: record only. Logging here
        // stalled the S2; housekeeping logs the reason later.
        if let esp_idf_svc::wifi::WifiEvent::StaDisconnected(info) = event {
            STATS.wifi_disconnects.fetch_add(1, Relaxed);
            STATS
                .wifi_last_reason
                .store(u32::from(info.reason()), Relaxed);
            SIGNALS.wifi(false);
            events::emit(Event::WifiDown {
                reason: info.reason(),
                rssi: info.rssi(),
            });
        }
    })?;
    SIGNALS.stage(Stage::Storage);
    let nvs = EspDefaultNvsPartition::take_with(false).map_err(|e| {
        format!("system NVS: {e} (not erased; it may hold data. Erase deliberately over USB if it is disposable)")
    })?;
    SIGNALS.stage(Stage::WifiDriver);
    let wifi = BlockingWifi::wrap(
        EspWifi::new(modem, event_loop.clone(), Some(nvs.clone()))?,
        event_loop,
    )?;
    let saved = match config.saved_wifi {
        Some(namespace) => Some(EspNvs::new(nvs.clone(), namespace, true)?),
        None => None,
    };
    let mut radio = Radio::new(
        wifi,
        saved,
        (config.ssid, config.password),
        config.setup_network.filter(|_| config.saved_wifi.is_some()),
    );
    let mut joining = None;
    if config.saved_wifi.is_some() {
        // Saved or built-in network, else the setup network.
        SIGNALS.stage(Stage::WifiJoin);
        radio.boot(config.force_setup)?;
    } else {
        let wifi = &mut radio.wifi;
        wifi.set_configuration(&Configuration::Client(wifi::client(
            config.ssid,
            config.password,
        )?))?;
        wifi.start()?;
        SIGNALS.stage(Stage::WifiJoin);
        if config.wait_for_wifi {
            loop {
                match wifi.connect().and_then(|_| wifi.wait_netif_up()) {
                    Ok(()) => break,
                    Err(e) => {
                        events::emit(Event::ReconnectFailed { code: e.code() });
                        log::warn!(
                            "Wi-Fi connect failed ({e}); last disconnect reason {}; retrying",
                            STATS.wifi_last_reason.load(Relaxed)
                        );
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }
            SIGNALS.wifi(true);
            events::emit(Event::WifiUp);
        } else {
            // The network stack is initialized; sockets can bind. Joining
            // continues in housekeeping.
            if let Err(e) = wifi.wifi_mut().connect() {
                log::warn!("Wi-Fi connect failed to start ({e}); retrying in the background");
            }
            joining = Some(Instant::now());
        }
    }
    SIGNALS.stage(Stage::Network);
    // Mains-powered server: modem sleep adds up to ~200 ms per packet.
    sys::esp!(unsafe { sys::esp_wifi_set_ps(sys::wifi_ps_type_t_WIFI_PS_NONE) })?;
    let mut sntp = SntpConf::default();
    if let Some(server) = config.ntp_server {
        sntp.servers.fill(server);
    }
    let sntp = EspSntp::new(&sntp)?;
    let mdns = (|| -> Result<EspMdns, sys::EspError> {
        let mut mdns = EspMdns::take()?;
        mdns.set_hostname(config.hostname)?;
        mdns.set_instance_name(config.instance)?;
        if config.limits.tls_clients > 0 {
            mdns.add_service(
                Some(config.instance),
                "_https",
                "_tcp",
                443,
                &[("path", config.mdns_https_path)],
            )?;
        }
        let http_path = if config.limits.tls_clients > 0 {
            "/trust"
        } else {
            "/"
        };
        mdns.add_service(
            Some(config.instance),
            "_http",
            "_tcp",
            80,
            &[("path", http_path)],
        )?;
        Ok(mdns)
    })()
    .map_err(|e| log::warn!("mDNS unavailable ({e}); reachable by IP only"))
    .ok();
    SIGNALS.mdns(mdns.is_some());
    SIGNALS.stage(Stage::App);
    let radio = std::sync::Arc::new(Mutex::new(radio));
    *RADIO.lock().unwrap_or_else(|e| e.into_inner()) = std::sync::Arc::downgrade(&radio);
    Ok(Board {
        config,
        nvs,
        wifi: radio,
        joining,
        _sntp: sntp,
        mdns,
        _wifi_events: wifi_events,
        partitions: Mutex::new(std::collections::BTreeMap::new()),
    })
}

/// The station's IPv4 address, or `None` while not connected.
pub fn station_ip() -> Option<String> {
    let radio = RADIO.lock().unwrap_or_else(|e| e.into_inner()).upgrade()?;
    let radio = radio.lock().unwrap_or_else(|e| e.into_inner());
    radio.sta_address().map(|ip| ip.to_string())
}

impl Board {
    /// The mDNS responder, to advertise more services (Minicloud's MQTT),
    /// or `None` if it failed to start.
    pub fn mdns(&mut self) -> Option<&mut EspMdns> {
        self.mdns.as_mut()
    }

    /// The setup network's controls, for a board configured with
    /// [`BoardConfig::setup_network`] (whether or not it is open now).
    pub fn wifi_setup(&self) -> Option<WifiSetup> {
        self.config
            .setup_network
            .filter(|_| self.config.saved_wifi.is_some())
            .map(|_| WifiSetup(std::sync::Arc::clone(&self.wifi)))
    }

    /// A key-value store in one NVS namespace (max 15 characters).
    pub fn kv(&self, namespace: &str) -> Result<NvsKv, Error> {
        Ok(NvsKv(Mutex::new(EspNvs::new(
            self.nvs.clone(),
            namespace,
            true,
        )?)))
    }

    /// A named NVS data partition (an app's own storage, such as
    /// NanaCoin's `ledger`). Initialized here so every error is returned:
    /// esp-idf-svc's constructor would erase a partition it cannot open.
    pub fn partition(&self, name: &str) -> Result<EspNvsPartition<NvsCustom>, Error> {
        // Serialize the preflight/take pair and retain an owner: another
        // caller dropping its clone cannot deinitialize the checked partition.
        let mut partitions = self.partitions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(partition) = partitions.get(name) {
            return Ok(partition.clone());
        }
        let c_name = std::ffi::CString::new(name)?;
        // SAFETY: valid NUL-terminated name. Initializing an initialized
        // partition is a no-op, so the take below never reaches its erase.
        sys::esp!(unsafe { sys::nvs_flash_init_partition(c_name.as_ptr()) })
            .map_err(|e| format!("NVS partition {name}: {e} (not erased)"))?;
        let partition = EspNvsPartition::<NvsCustom>::take(name)?;
        partitions.insert(name.to_owned(), partition.clone());
        Ok(partition)
    }

    /// An HTTP(S) client that trusts the household CA.
    pub fn fetch(&self) -> EspFetch {
        EspFetch {
            ca_pem: self.config.ca_pem,
        }
    }

    /// Transfers the temperature peripheral to a safe driver. A failed sensor
    /// remains absent from sysinfo; it never prevents the web service starting.
    pub fn platform(&self, sensor: TempSensor<'static>) -> EspPlatform {
        let mut config = TempSensorConfig::default();
        config.range_min = 10;
        config.range_max = 80;
        let temperature = TempSensorDriver::new(&config, sensor)
            .and_then(|mut driver| {
                driver.enable()?;
                Ok(driver)
            })
            .map_err(|e| log::warn!("Temperature sensor unavailable ({e})"))
            .ok();
        EspPlatform {
            temperature: Mutex::new(temperature),
            wifi: std::sync::Arc::clone(&self.wifi),
        }
    }

    /// Serves forever. `tick` runs often (between connection-loop turns on
    /// one core, every ~10 ms on two) and may write flash: it always runs on
    /// the calling task, whose stack is internal RAM. Keep it short.
    pub fn serve<S: Service>(self, site: Site<S>, mut tick: impl FnMut(&Site<S>)) -> ! {
        SIGNALS.stage(Stage::Listen);
        // A site lives until reboot; leaking it lets a second task share it.
        let site: &'static Site<S> = Box::leak(Box::new(site));
        let limits = self.config.limits.clone();
        let (ready, finished) = mpsc::sync_channel::<Socket>(2);
        #[cfg(feature = "tls")]
        let tls =
            TcpListener::bind(("0.0.0.0", 443)).and_then(|l| l.set_nonblocking(true).map(|_| l));
        let http =
            TcpListener::bind(("0.0.0.0", 80)).and_then(|l| l.set_nonblocking(true).map(|_| l));
        let http = match http {
            Ok(l) => l,
            Err(e) => fail(&format!("cannot listen on port 80: {e}")),
        };
        #[cfg(not(feature = "tls"))]
        {
            drop(ready);
            if limits.tls_clients > 0 {
                log::warn!("built without the `tls` feature: serving HTTP only");
            }
        }
        #[cfg(feature = "tls")]
        match tls {
            Ok(listener) if limits.tls_clients > 0 => {
                let cert = self.config.cert_pem;
                let key = self.config.key_pem;
                let pending = limits.handshakes.max(1);
                let spawned = spawn_task(
                    c"mf-tls",
                    24 * 1024,
                    self.config.psram_stacks,
                    self.config.network_core,
                    4,
                    move || handshakes(listener, ready, cert, key, pending),
                );
                if let Err(e) = spawned {
                    log::error!("TLS task failed to start: {e}; serving HTTP only");
                }
            }
            Ok(_) => {}
            Err(e) => log::error!("cannot listen on port 443: {e}; serving HTTP only"),
        }
        log::info!(
            "{} ready at https://{}.local/ ({} TLS + {} HTTP connections, {})",
            site.config.app,
            self.config.hostname,
            limits.tls_clients,
            limits.http_clients,
            if self.config.app_core.is_some() {
                "serving on its own core"
            } else {
                "single task"
            }
        );
        let mut mux: Mux<Socket> =
            Mux::new(limits, site.config.body_limit, site.config.response_limit);
        let mut turn = move || {
            SIGNALS.serve_beat();
            if let Ok(socket) = finished.try_recv() {
                mux.add(socket);
            }
            // Accept only with room: extra connections wait in the backlog.
            let accepted = if mux.has_room(false) {
                http.accept().ok()
            } else {
                None
            };
            if let Some(socket) = accepted.and_then(|(tcp, _)| Socket::plain(tcp).ok()) {
                mux.add(socket);
            }
            mux.turn(site)
        };
        let mut board = self;
        if let Some(core) = board.config.app_core {
            let spawned = spawn_task(
                c"mf-serve",
                board.config.serve_stack,
                false,
                Some(core),
                5,
                move || {
                    let mut rested = Instant::now();
                    loop {
                        let busy = turn();
                        if !busy || rested.elapsed() >= Duration::from_millis(20) {
                            FreeRtos::delay_ms(1);
                            rested = Instant::now();
                        } else {
                            std::thread::yield_now();
                        }
                    }
                },
            );
            if let Err(e) = spawned {
                fail(&format!("serving task failed to start: {e}"));
            }
            SIGNALS.ready();
            let mut house = Instant::now();
            let mut health = Instant::now() - Duration::from_secs(50);
            loop {
                tick(site);
                board.chores(&mut house, &mut health);
                FreeRtos::delay_ms(10);
            }
        }
        let mut house = Instant::now();
        let mut health = Instant::now() - Duration::from_secs(50);
        let mut rested = Instant::now();
        SIGNALS.ready();
        loop {
            let busy = turn();
            tick(site);
            board.chores(&mut house, &mut health);
            // Sleep a tick only when idle, or every 20 ms under load so the
            // idle task runs (and feeds the watchdog). Sleeping every turn
            // capped throughput at one turn per tick.
            if !busy || rested.elapsed() >= Duration::from_millis(20) {
                FreeRtos::delay_ms(1);
                rested = Instant::now();
            } else {
                std::thread::yield_now();
            }
        }
    }

    /// Wi-Fi every 10 s, the health line every minute.
    fn chores(&mut self, house: &mut Instant, health: &mut Instant) {
        if house.elapsed() >= Duration::from_secs(10) {
            *house = Instant::now();
            self.housekeeping();
        }
        if health.elapsed() >= Duration::from_secs(60) {
            *health = Instant::now();
            log_health();
        }
    }

    /// Wi-Fi without blocking: a lost or unfinished join is (re)started
    /// and checked on the next round, so the loop keeps serving meanwhile.
    fn housekeeping(&mut self) {
        let radio = std::sync::Arc::clone(&self.wifi);
        let mut radio = radio.lock().unwrap_or_else(|e| e.into_inner());
        if radio.in_setup() {
            radio.setup_chores();
            return;
        }
        let wifi = &mut radio.wifi;
        let up = wifi.is_connected().unwrap_or(false) && wifi.is_up().unwrap_or(false);
        if up {
            if self.joining.take().is_some() || !SIGNALS.health(false).wifi {
                let ip = wifi
                    .wifi()
                    .sta_netif()
                    .get_ip_info()
                    .ok()
                    .map(|info| info.ip.to_string())
                    .unwrap_or_default();
                log::info!("Wi-Fi up: {ip}");
                events::emit(Event::WifiUp);
            }
            SIGNALS.wifi(true);
            return;
        }
        SIGNALS.wifi(false);
        if self
            .joining
            .is_some_and(|since| since.elapsed() < JOIN_TIMEOUT)
        {
            return;
        }
        if self.joining.is_some() {
            events::emit(Event::ReconnectFailed { code: 0 });
            let _ = wifi.wifi_mut().disconnect();
        }
        events::emit(Event::Reconnect);
        log::warn!(
            "Wi-Fi down (last reason {}); reconnecting in the background",
            STATS.wifi_last_reason.load(Relaxed)
        );
        if let Err(e) = wifi.wifi_mut().connect() {
            events::emit(Event::ReconnectFailed { code: e.code() });
            log::warn!("Wi-Fi reconnect failed to start: {e}");
        }
        self.joining = Some(Instant::now());
    }
}

/// One line a minute: memory, stacks, TLS and connections. The trend
/// before a crash is in the served log (and its RTC copy).
fn log_health() {
    let (pf, pm, pl, _) = heap_info(sys::MALLOC_CAP_SPIRAM);
    let (inf, inm, inl, _) = heap_info(sys::MALLOC_CAP_INTERNAL | sys::MALLOC_CAP_8BIT);
    // SAFETY: queries the calling (serving) task's own stack.
    let main_stack = unsafe { sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut()) };
    let n = STATS.snapshot();
    log::info!(
        "health: psram free {pf} min {pm} block {pl} | internal free {inf} min {inm} block {inl} | stack free main {main_stack} tls {} led {} | tls {} ok {} failed, {} rejected | open {} tls {} http",
        TLS_STACK_FREE.load(Relaxed),
        LED_STACK_FREE.load(Relaxed),
        n.tls_handshakes,
        n.tls_failures,
        n.rejected,
        n.tls_open,
        n.http_open
    );
}

/// Startup failed: blink the failed step's number on the LED, repeat the
/// error in the log every 5 s (the S2's USB console attaches late), and
/// explain it as plain text on HTTP port 8080 (no TLS, almost no memory)
/// for anyone who can reach the board. Wi-Fi stays up if it got that far.
/// Never returns.
pub fn fail(error: &str) -> ! {
    use std::io::Write as _;
    SIGNALS.fatal();
    let (app, host) = IDENTITY.get().copied().unwrap_or(("board", "board"));
    let listener = TcpListener::bind(("0.0.0.0", 8080))
        .and_then(|l| l.set_nonblocking(true).map(|_| l))
        .ok();
    let mut said = Instant::now() - Duration::from_secs(5);
    loop {
        let (internal_free, _, internal_largest, _) =
            heap_info(sys::MALLOC_CAP_INTERNAL | sys::MALLOC_CAP_8BIT);
        let (psram_free, _, psram_largest, _) = heap_info(sys::MALLOC_CAP_SPIRAM);
        let heap = Heap {
            internal_free,
            internal_largest,
            psram_free,
            psram_largest,
            ..Default::default()
        };
        let text = crate::status::failure_report(
            app,
            &format!("{host}.local"),
            SIGNALS.current_stage(),
            error,
            &heap,
        );
        if said.elapsed() >= Duration::from_secs(5) {
            said = Instant::now();
            log::error!("{}", text.trim_end());
        }
        if let Some(Ok((mut stream, _))) = listener.as_ref().map(TcpListener::accept) {
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
            let _ = write!(
                stream,
                "HTTP/1.0 500 Startup failed\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n{text}"
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Free PSRAM a compression needs: ~230 KiB of compressor tables, the
/// output, and the helper's 96 KiB stack, with room to spare.
#[cfg(feature = "gzip")]
const GZIP_MIN_FREE: u32 = 448 * 1024;
#[cfg(feature = "gzip")]
const GZIP_MIN_BLOCK: u32 = 192 * 1024;

/// Runs deflate on a short-lived thread with a 96 KiB PSRAM stack:
/// miniz_oxide puts a 64 KiB buffer on the stack, more than the serving
/// task's 32 KiB. The serving task waits (it would have spent the time
/// compressing anyway). Skips compression when PSRAM is short.
#[cfg(feature = "gzip")]
fn gzip_runner(job: &mut (dyn FnMut() + Send)) -> bool {
    let (free, _, largest, _) = heap_info(sys::MALLOC_CAP_SPIRAM);
    if free < GZIP_MIN_FREE || largest < GZIP_MIN_BLOCK {
        log::warn!("gzip skipped: PSRAM free {free}, largest block {largest}");
        return false;
    }
    std::thread::scope(|scope| {
        let config = ThreadSpawnConfiguration {
            name: Some(c"mf-gzip"),
            stack_alloc_caps: MallocCap::Spiram | MallocCap::Cap8bit,
            ..Default::default()
        };
        if config.set().is_err() {
            return false;
        }
        let worker = std::thread::Builder::new()
            .stack_size(96 * 1024)
            .spawn_scoped(scope, || job());
        let _ = ThreadSpawnConfiguration::default().set();
        match worker {
            Ok(handle) => handle.join().is_ok(),
            Err(e) => {
                log::warn!("gzip skipped: no helper thread ({e})");
                false
            }
        }
    })
}

/// Spawns a thread with an IDF task name and optional PSRAM stack.
pub fn spawn_worker(
    name: &'static CStr,
    stack: usize,
    psram: bool,
    work: impl FnOnce() + Send + 'static,
) -> Result<std::thread::JoinHandle<()>, Error> {
    spawn_task(name, stack, psram, None, 0, work)
}

/// [`spawn_worker`] with a core and a FreeRTOS priority (0: IDF default).
/// Never give a task that writes flash a PSRAM stack.
pub fn spawn_task(
    name: &'static CStr,
    stack: usize,
    psram: bool,
    core: Option<Core>,
    priority: u8,
    work: impl FnOnce() + Send + 'static,
) -> Result<std::thread::JoinHandle<()>, Error> {
    let mut config = ThreadSpawnConfiguration {
        name: Some(name),
        pin_to_core: core,
        ..Default::default()
    };
    if priority > 0 {
        config.priority = priority;
    }
    if psram {
        config.stack_alloc_caps = MallocCap::Spiram | MallocCap::Cap8bit;
    }
    config.set()?;
    // Rust's pthread stack size overrides IDF's; set it on the Builder.
    let handle = std::thread::Builder::new().stack_size(stack).spawn(work);
    ThreadSpawnConfiguration::default().set()?;
    Ok(handle?)
}

/// A connection the loop owns: plain TCP, or TCP with a finished esp-tls
/// session. The TcpStream owns the descriptor and closes it once;
/// `esp_tls_server_session_delete` frees only the TLS context.
pub struct Socket {
    // Field order releases the TLS session before closing its TCP descriptor.
    #[cfg(feature = "tls")]
    tls: Option<TlsSession>,
    tcp: TcpStream,
    /// The handshake chose HTTP/2 by ALPN.
    #[cfg(feature = "http2")]
    h2: bool,
}

#[cfg(feature = "tls")]
struct TlsSession {
    raw: NonNull<sys::esp_tls_t>,
    write: crate::http::TlsWriteRetry,
}

// SAFETY: this private, non-Clone owner is the sole accessor of the context.
// IDF/mbedTLS contexts have no task affinity. A channel moves the owner after
// handshake completion, establishing synchronization; the producer never
// accesses it again. No Sync implementation permits concurrent access.
#[cfg(feature = "tls")]
unsafe impl Send for TlsSession {}

impl Socket {
    fn plain(tcp: TcpStream) -> io::Result<Self> {
        tcp.set_nodelay(true)?;
        tcp.set_nonblocking(true)?;
        Ok(Self {
            tcp,
            #[cfg(feature = "tls")]
            tls: None,
            #[cfg(feature = "http2")]
            h2: false,
        })
    }
}

#[cfg(feature = "tls")]
impl Drop for TlsSession {
    fn drop(&mut self) {
        // SAFETY: exclusively owned initialized context, including failed
        // partial setup. In IDF 5.5.3 this frees the context, not its socket fd.
        unsafe { sys::esp_tls_server_session_delete(self.raw.as_ptr()) };
    }
}

#[cfg(feature = "tls")]
fn tls_result(n: isize) -> io::Result<usize> {
    match n as i32 {
        sys::ESP_TLS_ERR_SSL_WANT_READ | sys::ESP_TLS_ERR_SSL_WANT_WRITE => {
            Err(io::ErrorKind::WouldBlock.into())
        }
        // Peer's TLS close: normal.
        n if n as i32 == sys::MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY => {
            Err(io::ErrorKind::ConnectionAborted.into())
        }
        n if n < 0 => {
            log::info!("TLS connection ended: {n} ({:#x})", -n);
            Err(io::ErrorKind::ConnectionAborted.into())
        }
        _ => Ok(n as usize),
    }
}

impl Read for Socket {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        #[cfg(feature = "tls")]
        if let Some(tls) = self.tls.as_mut() {
            // SAFETY: exclusively owned context, writable buffer.
            return tls_result(unsafe {
                sys::esp_tls_conn_read(tls.raw.as_ptr(), out.as_mut_ptr().cast(), out.len())
            });
        }
        self.tcp.read(out)
    }
}

impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        #[cfg(feature = "tls")]
        if let Some(tls) = self.tls.as_mut() {
            // SAFETY: exclusively owned context. The retry buffer preserves
            // pointer, length and contents after WANT_WRITE, as mbedTLS requires.
            let raw = tls.raw;
            return tls.write.write(bytes, |pending| {
                tls_result(unsafe {
                    sys::esp_tls_conn_write(raw.as_ptr(), pending.as_ptr().cast(), pending.len())
                })
            });
        }
        self.tcp.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Conn for Socket {
    fn secure(&self) -> bool {
        #[cfg(feature = "tls")]
        return self.tls.is_some();
        #[cfg(not(feature = "tls"))]
        false
    }
    #[cfg(feature = "http2")]
    fn h2(&self) -> bool {
        self.h2
    }
}

/// The handshake task. At most one handshake in flight on a single-core
/// board: two concurrent ECDHE computations would just take twice as long.
#[cfg(feature = "tls")]
fn handshakes(
    listener: TcpListener,
    ready: mpsc::SyncSender<Socket>,
    cert: &'static [u8],
    key: &'static [u8],
    pending_limit: usize,
) {
    // A full handshake is ~1 s of crypto on one core plus a few Wi-Fi round
    // trips, which spike to 200 ms+ on household networks: 1.3-3.5 s were
    // measured on an S2 (October 4, 2026). Pending handshakes are capped,
    // so a generous limit cannot pile up; a slow one beats a failed one.
    const TIMEOUT: Duration = Duration::from_secs(10);
    // The SDK copies/parses certificates into each session. Input slices are
    // static; configuration is only borrowed during session initialization.
    let (mut cfg, tickets_error) =
        crate::tls_config::optional_init::<sys::esp_tls_cfg_server_t, _>(|cfg| {
            // SAFETY: valid default config, exclusively owned by this task.
            sys::esp!(unsafe { sys::esp_tls_cfg_server_session_tickets_init(cfg) })
        });
    if let Some(error) = tickets_error {
        events::emit(Event::TlsInitFailed { code: error.code() });
        log::warn!("TLS session tickets unavailable; every visit pays a full handshake");
    }
    cfg.__bindgen_anon_3.servercert_buf = cert.as_ptr();
    cfg.__bindgen_anon_4.servercert_bytes = cert.len() as _;
    cfg.__bindgen_anon_5.serverkey_buf = key.as_ptr();
    cfg.__bindgen_anon_6.serverkey_bytes = key.len() as _;
    // mbedTLS retains the ALPN pointer array, including after task handoff.
    // Keep this 12-byte array (32-bit targets) for the firmware lifetime, even
    // if the handshake task unwinds. There is one server task per boot.
    #[cfg(feature = "http2")]
    let protocols = Box::leak(Box::new([
        c"h2".as_ptr(),
        c"http/1.1".as_ptr(),
        std::ptr::null(),
    ]));
    #[cfg(feature = "http2")]
    {
        cfg.alpn_protos = protocols.as_mut_ptr();
    }
    let mut pending: Vec<(Socket, Instant)> = Vec::with_capacity(pending_limit);
    let mut turns = 0u32;
    loop {
        SIGNALS.tls_beat();
        events::emit(Event::Turn {
            task: Task::Handshake,
            tls: pending.len() as u8,
            http: 0,
        });
        turns = turns.wrapping_add(1);
        if turns % 1000 == 0 {
            // SAFETY: this task's own stack.
            TLS_STACK_FREE.store(
                unsafe { sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut()) },
                Relaxed,
            );
        }
        // Accept only when the loop can take the finished session; a
        // handshake (about a second) for a connection that is then dropped
        // is the worst of both.
        if pending.len() < pending_limit && crate::mux::TLS_ROOM.load(Relaxed) {
            if let Ok((tcp, _)) = listener.accept() {
                if let Ok(mut socket) = Socket::plain(tcp) {
                    // SAFETY: init allocates an owned context; Socket's Drop
                    // releases it (and the fd) on every failure path.
                    if let Some(tls) = NonNull::new(unsafe { sys::esp_tls_init() }) {
                        socket.tls = Some(TlsSession {
                            raw: tls,
                            write: crate::http::TlsWriteRetry::new(),
                        });
                        let fd = socket.tcp.as_raw_fd();
                        let started =
                            unsafe { sys::esp_tls_server_session_init(&mut cfg, fd, tls.as_ptr()) };
                        if started == sys::ESP_OK {
                            pending.push((socket, Instant::now()));
                        } else {
                            STATS.tls_failures.fetch_add(1, Relaxed);
                            events::emit(Event::TlsInitFailed { code: started });
                            log::warn!("TLS session init failed: {started:#x}");
                        }
                    } else {
                        events::emit(Event::AllocationFailed);
                    }
                }
            }
        }
        let mut i = 0;
        while i < pending.len() {
            if pending[i].1.elapsed() >= TIMEOUT {
                STATS.tls_failures.fetch_add(1, Relaxed);
                events::emit(Event::TlsTimeout {
                    ms: TIMEOUT.as_millis() as u32,
                });
                log::warn!("TLS handshake timed out after {} ms", TIMEOUT.as_millis());
                pending.swap_remove(i);
                continue;
            }
            let tls = pending[i].0.tls.as_ref().unwrap().raw;
            // SAFETY: exclusively owned context; nonblocking socket.
            match unsafe { sys::esp_tls_server_session_continue_async(tls.as_ptr()) } {
                0 => {
                    #[allow(unused_mut)]
                    let (mut socket, began) = pending.swap_remove(i);
                    #[cfg(feature = "http2")]
                    {
                        socket.h2 = negotiated_h2(tls);
                    }
                    let ms = began.elapsed().as_millis() as u32;
                    STATS.handshake(ms);
                    events::emit(Event::Handshake { ms });
                    if ready.try_send(socket).is_err() {
                        STATS.rejected.fetch_add(1, Relaxed);
                        events::emit(Event::HandoffFull);
                    }
                }
                sys::ESP_TLS_ERR_SSL_WANT_READ | sys::ESP_TLS_ERR_SSL_WANT_WRITE => i += 1,
                code => {
                    STATS.tls_failures.fetch_add(1, Relaxed);
                    let (socket, began) = pending.swap_remove(i);
                    events::emit(Event::TlsFailed {
                        code,
                        ms: began.elapsed().as_millis() as u32,
                    });
                    let peer = socket
                        .tcp
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_default();
                    log::warn!(
                        "TLS handshake failed: {code} ({:#x}) after {} ms from {peer}",
                        -code,
                        began.elapsed().as_millis()
                    );
                }
            }
        }
        FreeRtos::delay_ms(1);
    }
}

/// Whether ALPN chose `h2` for a finished handshake.
#[cfg(feature = "http2")]
fn negotiated_h2(tls: NonNull<sys::esp_tls_t>) -> bool {
    // SAFETY: a live, exclusively owned context whose handshake finished;
    // the protocol string (if any) is owned by mbedTLS and NUL-terminated.
    unsafe {
        let ssl = sys::esp_tls_get_ssl_context(tls.as_ptr()).cast::<sys::mbedtls_ssl_context>();
        if ssl.is_null() {
            return false;
        }
        let chosen = sys::mbedtls_ssl_get_alpn_protocol(ssl);
        !chosen.is_null() && CStr::from_ptr(chosen).to_bytes() == b"h2"
    }
}

/// NVS-backed [`Kv`].
pub struct NvsKv(Mutex<EspNvs<NvsDefault>>);

impl Kv for NvsKv {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        check_key(key).ok()?;
        let nvs = self.0.lock().unwrap();
        let len = nvs.blob_len(key).ok()??;
        let mut buf = vec![0; len];
        let got = nvs.get_blob(key, &mut buf).ok()??.len();
        buf.truncate(got);
        Some(buf)
    }
    fn set(&self, key: &str, value: &[u8]) -> io::Result<()> {
        check_key(key)?;
        self.0
            .lock()
            .unwrap()
            .set_blob(key, value)
            .map_err(|e| io::Error::other(e.to_string()))
    }
    fn remove(&self, key: &str) -> io::Result<()> {
        check_key(key)?;
        self.0
            .lock()
            .unwrap()
            .remove(key)
            .map(drop)
            .map_err(|e| io::Error::other(e.to_string()))
    }
}

/// esp-idf's HTTP client. HTTPS verifies against the household CA.
pub struct EspFetch {
    ca_pem: Option<&'static [u8]>,
}

impl Fetch for EspFetch {
    fn get(&self, url: &str, accept: &str, limit: usize) -> io::Result<Fetched> {
        let err = |e: sys::EspError| io::Error::other(e.to_string());
        let mut conn = EspHttpConnection::new(&esp_idf_svc::http::client::Configuration {
            timeout: Some(Duration::from_secs(3)),
            server_certificate: self.ca_pem.map(X509::pem_until_nul),
            buffer_size: Some(1024),
            ..Default::default()
        })
        .map_err(err)?;
        conn.initiate_request(
            Method::Get,
            url,
            &[("Accept", accept), ("User-Agent", "miniframework")],
        )
        .map_err(err)?;
        conn.initiate_response().map_err(err)?;
        let status = conn.status();
        let content_type = conn.header("Content-Type").unwrap_or("").to_string();
        let mut body = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            let n = conn.read(&mut buf).map_err(err)?;
            if n == 0 {
                break;
            }
            if body.len() + n > limit {
                return Err(io::Error::other("response body too large"));
            }
            body.extend_from_slice(&buf[..n]);
        }
        Ok(Fetched {
            status,
            content_type,
            body,
        })
    }
}

pub struct EspPlatform {
    temperature: Mutex<Option<TempSensorDriver<'static>>>,
    wifi: std::sync::Arc<Mutex<Radio>>,
}

fn heap_info(caps: u32) -> (u32, u32, u32, u32) {
    let mut info = sys::multi_heap_info_t::default();
    // SAFETY: initialized output structure.
    unsafe { sys::heap_caps_get_info(&mut info, caps) };
    (
        info.total_free_bytes as u32,
        info.minimum_free_bytes as u32,
        info.largest_free_block as u32,
        (info.total_free_bytes + info.total_allocated_bytes) as u32,
    )
}

/// Why the chip last reset, in words.
#[allow(non_upper_case_globals)]
pub fn reset_reason() -> &'static str {
    match ResetReason::get() {
        ResetReason::PowerOn => "power on",
        ResetReason::ExternalPin => "reset pin",
        ResetReason::Software => "software restart",
        ResetReason::Panic => "crash",
        ResetReason::InterruptWatchdog | ResetReason::TaskWatchdog | ResetReason::Watchdog => {
            "watchdog"
        }
        ResetReason::Brownout => "brownout",
        ResetReason::USBPeripheral => "USB",
        _ => "other",
    }
}

#[allow(non_upper_case_globals)]
impl Platform for EspPlatform {
    fn sysinfo(&self) -> SysInfo {
        let mut flash: u32 = 0;
        let (ap, mac, ip) = {
            let radio = self.wifi.lock().unwrap_or_else(|e| e.into_inner());
            let wifi = radio.wifi.wifi();
            (
                wifi.get_ap_info().ok(),
                wifi.get_mac(WifiDeviceId::Sta).unwrap_or_default(),
                radio.sta_address(),
            )
        };
        // SAFETY: null selects the initialized default flash chip; output is
        // writable. IDF's version is an immutable static NUL-terminated string.
        let sdk = unsafe {
            if sys::esp_flash_get_size(std::ptr::null_mut(), &mut flash) != sys::ESP_OK {
                flash = 0;
            }
            CStr::from_ptr(sys::esp_get_idf_version())
                .to_string_lossy()
                .into_owned()
        };
        // The build target, not esp_chip_info(): its bindings are missing on
        // some targets' configurations (the C6's).
        let model = match CStr::from_bytes_until_nul(sys::CONFIG_IDF_TARGET)
            .ok()
            .and_then(|t| t.to_str().ok())
            .unwrap_or("")
        {
            "esp32" => "ESP32",
            "esp32s2" => "ESP32-S2",
            "esp32s3" => "ESP32-S3",
            "esp32c3" => "ESP32-C3",
            "esp32c6" => "ESP32-C6",
            _ => "ESP32 family",
        };
        let internal = heap_info(sys::MALLOC_CAP_INTERNAL | sys::MALLOC_CAP_8BIT);
        let psram = heap_info(sys::MALLOC_CAP_SPIRAM);
        SysInfo {
            platform: format!("{model} / ESP-IDF / Rust"),
            chip: model.into(),
            cores: sys::SOC_CPU_CORES_NUM,
            cpu_mhz: sys::CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ,
            reset_reason: reset_reason().into(),
            status: led::state_name().into(),
            flash_bytes: flash,
            temp_c: self
                .temperature
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .and_then(|sensor| sensor.get_celsius().ok())
                .filter(|v| v.is_finite()),
            sdk,
            heap: Heap {
                internal_free: internal.0,
                internal_min: internal.1,
                internal_largest: internal.2,
                internal_total: internal.3,
                psram_free: psram.0,
                psram_min: psram.1,
                psram_largest: psram.2,
                psram_total: psram.3,
            },
            wifi: Wifi {
                ssid: ap
                    .as_ref()
                    .map(|ap| ap.ssid.to_string())
                    .unwrap_or_default(),
                rssi: ap.as_ref().map_or(0, |ap| ap.signal_strength as i32),
                channel: ap.as_ref().map_or(0, |ap| ap.channel as u32),
                ip: ip.map(|ip| ip.to_string()).unwrap_or_default(),
                mac: mac
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(":"),
                // Filled from the event counters by `Site::sysinfo`.
                ..Default::default()
            },
            partitions: partitions(),
            ..Default::default()
        }
    }
}

/// The partition table, with the running app marked.
struct PartitionIterator(sys::esp_partition_iterator_t);

impl Drop for PartitionIterator {
    fn drop(&mut self) {
        // SAFETY: owned live iterator or null; next frees the exhausted
        // iterator and returns null, which we store before any Rust work.
        unsafe { sys::esp_partition_iterator_release(self.0) };
    }
}

fn partitions() -> Vec<Partition> {
    let mut found = Vec::new();
    // SAFETY: each partition pointer is read before the iterator advances;
    // `esp_partition_next` frees the iterator when it returns null. The
    // running partition pointer is static for the life of the firmware.
    unsafe {
        let running = sys::esp_ota_get_running_partition();
        let mut it = PartitionIterator(sys::esp_partition_find(
            sys::esp_partition_type_t_ESP_PARTITION_TYPE_ANY,
            sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_ANY,
            std::ptr::null(),
        ));
        while !it.0.is_null() {
            let p = sys::esp_partition_get(it.0);
            if let Some(part) = p.as_ref() {
                let label_bytes = part.label.map(|c| c as u8);
                let end = label_bytes
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(label_bytes.len());
                let label = String::from_utf8_lossy(&label_bytes[..end]);
                found.push(Partition {
                    name: label.into_owned(),
                    kind: if part.type_ == sys::esp_partition_type_t_ESP_PARTITION_TYPE_APP {
                        "app".into()
                    } else {
                        "data".into()
                    },
                    subtype: part.subtype as u32,
                    offset: part.address,
                    size: part.size,
                    running: std::ptr::eq(p, running),
                });
            }
            it.0 = sys::esp_partition_next(it.0);
        }
    }
    found.sort_by_key(|p| p.offset);
    found
}
