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
use crate::events::{self, Event, Task};
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
        modem::Modem,
        task::thread::{MallocCap, ThreadSpawnConfiguration},
    },
    http::{client::EspHttpConnection, Method},
    mdns::EspMdns,
    nvs::{EspDefaultNvsPartition, EspNvs, EspNvsPartition, NvsCustom, NvsDefault},
    sntp::{EspSntp, SntpConf},
    sys,
    tls::X509,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use std::ffi::CStr;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::ptr::NonNull;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

mod led;
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
    /// Stack of the multiplexer task on a dual-core board (internal RAM).
    pub serve_stack: usize,
    pub ntp_server: Option<&'static str>,
    /// The status light (POST, startup step codes, health, Morse), or
    /// `None` for a board without a usable LED.
    pub led: Option<Led>,
}

/// A plain (single-colour) status LED on a GPIO.
#[derive(Clone, Copy, Debug)]
pub struct Led {
    pub gpio: i32,
    /// The pin level that lights it (the S2 Mini's GPIO 15: high).
    pub active_high: bool,
    /// What it spells in Morse while healthy, e.g. the app's name.
    pub phrase: &'static str,
}

impl Led {
    /// The ESP32-S2 Mini's blue LED.
    pub const fn s2_mini(phrase: &'static str) -> Self {
        Self {
            gpio: 15,
            active_high: true,
            phrase,
        }
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
            ntp_server: None,
            led: None,
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
    wifi: BlockingWifi<EspWifi<'static>>,
    _sntp: EspSntp<'static>,
    _mdns: Option<EspMdns>,
    _wifi_events: EspSubscription<'static, System>,
}

/// Names for the startup-failure page, set by [`start`].
static IDENTITY: OnceLock<(&'static str, &'static str)> = OnceLock::new();

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
pub fn start(config: BoardConfig, modem: Modem<'static>) -> Result<Board, Error> {
    init();
    let _ = IDENTITY.set((config.instance, config.hostname));
    // The light first, so POST and step codes show even if a later step fails.
    if let Some(led) = config.led {
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
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, event_loop.clone(), Some(nvs.clone()))?,
        event_loop,
    )?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: config
            .ssid
            .try_into()
            .map_err(|_| "Wi-Fi SSID longer than 32 bytes")?,
        password: config
            .password
            .try_into()
            .map_err(|_| "Wi-Fi password longer than 64 bytes")?,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))?;
    wifi.start()?;
    SIGNALS.stage(Stage::WifiJoin);
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
        mdns.add_service(
            Some(config.instance),
            "_https",
            "_tcp",
            443,
            &[("path", "/")],
        )?;
        mdns.add_service(
            Some(config.instance),
            "_http",
            "_tcp",
            80,
            &[("path", "/trust")],
        )?;
        Ok(mdns)
    })()
    .map_err(|e| log::warn!("mDNS unavailable ({e}); reachable by IP only"))
    .ok();
    SIGNALS.mdns(mdns.is_some());
    SIGNALS.stage(Stage::App);
    Ok(Board {
        config,
        nvs,
        wifi,
        _sntp: sntp,
        _mdns: mdns,
        _wifi_events: wifi_events,
    })
}

impl Board {
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
        let c_name = std::ffi::CString::new(name)?;
        // SAFETY: valid NUL-terminated name. Initializing an initialized
        // partition is a no-op, so the take below never reaches its erase.
        sys::esp!(unsafe { sys::nvs_flash_init_partition(c_name.as_ptr()) })
            .map_err(|e| format!("NVS partition {name}: {e} (not erased)"))?;
        Ok(EspNvsPartition::<NvsCustom>::take(name)?)
    }

    /// An HTTP(S) client that trusts the household CA.
    pub fn fetch(&self) -> EspFetch {
        EspFetch {
            ca_pem: self.config.ca_pem,
        }
    }

    pub fn platform(&self) -> EspPlatform {
        EspPlatform {
            temperature: Mutex::new(Temperature::new()),
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
        let tls =
            TcpListener::bind(("0.0.0.0", 443)).and_then(|l| l.set_nonblocking(true).map(|_| l));
        let http =
            TcpListener::bind(("0.0.0.0", 80)).and_then(|l| l.set_nonblocking(true).map(|_| l));
        let http = match http {
            Ok(l) => l,
            Err(e) => fail(&format!("cannot listen on port 80: {e}")),
        };
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
            if let Ok((tcp, _)) = http.accept() {
                if let Ok(socket) = Socket::plain(tcp) {
                    mux.add(socket);
                }
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

    fn housekeeping(&mut self) {
        let up = self.wifi.is_connected().unwrap_or(false);
        SIGNALS.wifi(up);
        if !up {
            events::emit(Event::Reconnect);
            log::warn!(
                "Wi-Fi down (last reason {}); reconnecting",
                STATS.wifi_last_reason.load(Relaxed)
            );
            match self.wifi.connect().and_then(|_| self.wifi.wait_netif_up()) {
                Ok(()) => {
                    SIGNALS.wifi(true);
                    events::emit(Event::WifiUp);
                }
                Err(e) => {
                    events::emit(Event::ReconnectFailed { code: e.code() });
                    log::warn!("Wi-Fi reconnect failed: {e}");
                }
            }
        }
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
    tcp: TcpStream,
    tls: Option<NonNull<sys::esp_tls_t>>,
}

// SAFETY: one owner at a time. The handshake task hands a finished session
// through a channel and never touches it again.
unsafe impl Send for Socket {}

impl Socket {
    fn plain(tcp: TcpStream) -> io::Result<Self> {
        tcp.set_nodelay(true)?;
        tcp.set_nonblocking(true)?;
        Ok(Self { tcp, tls: None })
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(tls) = self.tls {
            // SAFETY: exclusively owned live context; TCP closes afterwards.
            unsafe { sys::esp_tls_server_session_delete(tls.as_ptr()) };
        }
    }
}

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
        match self.tls {
            // SAFETY: exclusively owned context, writable buffer.
            Some(tls) => tls_result(unsafe {
                sys::esp_tls_conn_read(tls.as_ptr(), out.as_mut_ptr().cast(), out.len())
            }),
            None => self.tcp.read(out),
        }
    }
}

impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.tls {
            // SAFETY: as above. After WANT_WRITE the caller retries the same
            // slice (http::Response guarantees it), as mbedTLS requires.
            Some(tls) => tls_result(unsafe {
                sys::esp_tls_conn_write(tls.as_ptr(), bytes.as_ptr().cast(), bytes.len())
            }),
            None => self.tcp.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Conn for Socket {
    fn secure(&self) -> bool {
        self.tls.is_some()
    }
}

/// The handshake task. At most one handshake in flight on a single-core
/// board: two concurrent ECDHE computations would just take twice as long.
fn handshakes(
    listener: TcpListener,
    ready: mpsc::SyncSender<Socket>,
    cert: &'static [u8],
    key: &'static [u8],
    pending_limit: usize,
) {
    const TIMEOUT: Duration = Duration::from_secs(4);
    // SAFETY: zero is IDF's documented default; PEM inputs are static and
    // NUL-terminated; cfg outlives every session created from it.
    let mut cfg: sys::esp_tls_cfg_server_t = unsafe { core::mem::zeroed() };
    cfg.__bindgen_anon_3.servercert_buf = cert.as_ptr();
    cfg.__bindgen_anon_4.servercert_bytes = cert.len() as _;
    cfg.__bindgen_anon_5.serverkey_buf = key.as_ptr();
    cfg.__bindgen_anon_6.serverkey_bytes = key.len() as _;
    // SAFETY: valid config owned by this task; ticket keys live until reboot.
    let tickets = unsafe { sys::esp_tls_cfg_server_session_tickets_init(&mut cfg) };
    if tickets != sys::ESP_OK {
        events::emit(Event::TlsInitFailed { code: tickets });
        log::warn!("TLS session tickets unavailable; every visit pays a full handshake");
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
        if pending.len() < pending_limit {
            if let Ok((tcp, _)) = listener.accept() {
                if let Ok(mut socket) = Socket::plain(tcp) {
                    // SAFETY: init allocates an owned context; Socket's Drop
                    // releases it (and the fd) on every failure path.
                    if let Some(tls) = NonNull::new(unsafe { sys::esp_tls_init() }) {
                        socket.tls = Some(tls);
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
            let tls = pending[i].0.tls.unwrap();
            // SAFETY: exclusively owned context; nonblocking socket.
            match unsafe { sys::esp_tls_server_session_continue_async(tls.as_ptr()) } {
                0 => {
                    let (socket, began) = pending.swap_remove(i);
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

struct Temperature(sys::temperature_sensor_handle_t);

// SAFETY: the handle is only used behind EspPlatform's mutex.
unsafe impl Send for Temperature {}

impl Temperature {
    fn new() -> Self {
        let mut handle = std::ptr::null_mut();
        let config = sys::temperature_sensor_config_t {
            range_min: 10,
            range_max: 80,
            ..Default::default()
        };
        // SAFETY: valid config and output pointer.
        unsafe {
            if sys::temperature_sensor_install(&config, &mut handle) != 0 {
                return Self(std::ptr::null_mut());
            }
            if sys::temperature_sensor_enable(handle) != 0 {
                sys::temperature_sensor_uninstall(handle);
                return Self(std::ptr::null_mut());
            }
        }
        Self(handle)
    }

    fn read(&self) -> Option<f32> {
        let mut value = 0.0;
        (!self.0.is_null()
            // SAFETY: enabled handle, used under the platform mutex.
            && unsafe { sys::temperature_sensor_get_celsius(self.0, &mut value) } == 0
            && value.is_finite())
        .then_some(value)
    }
}

pub struct EspPlatform {
    temperature: Mutex<Temperature>,
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
    use sys::*;
    // SAFETY: no arguments.
    match unsafe { esp_reset_reason() } {
        esp_reset_reason_t_ESP_RST_POWERON => "power on",
        esp_reset_reason_t_ESP_RST_EXT => "reset pin",
        esp_reset_reason_t_ESP_RST_SW => "software restart",
        esp_reset_reason_t_ESP_RST_PANIC => "crash",
        esp_reset_reason_t_ESP_RST_INT_WDT
        | esp_reset_reason_t_ESP_RST_TASK_WDT
        | esp_reset_reason_t_ESP_RST_WDT => "watchdog",
        esp_reset_reason_t_ESP_RST_BROWNOUT => "brownout",
        esp_reset_reason_t_ESP_RST_USB => "USB",
        _ => "other",
    }
}

#[allow(non_upper_case_globals)]
impl Platform for EspPlatform {
    fn sysinfo(&self) -> SysInfo {
        let mut chip = sys::esp_chip_info_t::default();
        let mut flash: u32 = 0;
        let mut ap = sys::wifi_ap_record_t::default();
        let mut mac = [0u8; 6];
        let mut ip = sys::esp_netif_ip_info_t::default();
        // SAFETY: IDF query functions with valid output pointers.
        let (connected, has_ip, sdk) = unsafe {
            sys::esp_chip_info(&mut chip);
            sys::esp_flash_get_size(std::ptr::null_mut(), &mut flash);
            sys::esp_wifi_get_mac(sys::wifi_interface_t_WIFI_IF_STA, mac.as_mut_ptr());
            let connected = sys::esp_wifi_sta_get_ap_info(&mut ap) == 0;
            let netif = sys::esp_netif_get_handle_from_ifkey(c"WIFI_STA_DEF".as_ptr());
            let has_ip = !netif.is_null() && sys::esp_netif_get_ip_info(netif, &mut ip) == 0;
            let sdk = CStr::from_ptr(sys::esp_get_idf_version())
                .to_string_lossy()
                .into_owned();
            (connected, has_ip, sdk)
        };
        let model = match chip.model {
            sys::esp_chip_model_t_CHIP_ESP32 => "ESP32",
            sys::esp_chip_model_t_CHIP_ESP32S2 => "ESP32-S2",
            sys::esp_chip_model_t_CHIP_ESP32S3 => "ESP32-S3",
            sys::esp_chip_model_t_CHIP_ESP32C3 => "ESP32-C3",
            sys::esp_chip_model_t_CHIP_ESP32C6 => "ESP32-C6",
            _ => "ESP32 family",
        };
        let internal = heap_info(sys::MALLOC_CAP_INTERNAL | sys::MALLOC_CAP_8BIT);
        let psram = heap_info(sys::MALLOC_CAP_SPIRAM);
        let ssid_len = ap
            .ssid
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(ap.ssid.len());
        SysInfo {
            platform: format!("{model} / ESP-IDF / Rust"),
            chip: format!(
                "{model} rev {}.{}",
                chip.revision / 100,
                chip.revision % 100
            ),
            cores: chip.cores as u32,
            cpu_mhz: sys::CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ,
            reset_reason: reset_reason().into(),
            status: led::state_name().into(),
            flash_bytes: flash,
            temp_c: self.temperature.lock().unwrap().read(),
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
                ssid: if connected {
                    String::from_utf8_lossy(&ap.ssid[..ssid_len]).into_owned()
                } else {
                    String::new()
                },
                rssi: if connected { ap.rssi as i32 } else { 0 },
                channel: if connected { ap.primary as u32 } else { 0 },
                ip: if has_ip {
                    let o = ip.ip.addr.to_ne_bytes();
                    format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3])
                } else {
                    String::new()
                },
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
fn partitions() -> Vec<Partition> {
    let mut found = Vec::new();
    // SAFETY: each partition pointer is read before the iterator advances;
    // `esp_partition_next` frees the iterator when it returns null. The
    // running partition pointer is static for the life of the firmware.
    unsafe {
        let running = sys::esp_ota_get_running_partition();
        let mut it = sys::esp_partition_find(
            sys::esp_partition_type_t_ESP_PARTITION_TYPE_ANY,
            sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_ANY,
            std::ptr::null(),
        );
        while !it.is_null() {
            let p = sys::esp_partition_get(it);
            if let Some(part) = p.as_ref() {
                let label = CStr::from_ptr(part.label.as_ptr()).to_string_lossy();
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
            it = sys::esp_partition_next(it);
        }
    }
    found.sort_by_key(|p| p.offset);
    found
}
