#[cfg(not(target_os = "espidf"))]
compile_error!(
    "build firmware with --target xtensa-esp32s3-espidf --no-default-features --features esp32"
);

use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{cpu::Core, peripherals::Peripherals, task::thread::ThreadSpawnConfiguration},
    http::{
        server::{Configuration as HttpConfiguration, EspHttpServer},
        Method,
    },
    io::{Read, Write},
    mdns::EspMdns,
    nvs::{EspDefaultNvsPartition, EspNvs, EspNvsPartition, NvsCustom},
    tls::X509,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use nanacoin::{
    api,
    domain::Error,
    journal::{Journal, Service, FRAME_SIZE},
};
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

/// Heap and uptime samples, published by the core 0 sampler and read by the
/// `/api/v1/diag` handler.
///
/// Diagnostics deliberately share nothing with the ledger. A request for
/// `/api/v1/diag` must be answerable while a write holds the service mutex —
/// that is the whole point of sampling on the other core — so the numbers
/// travel through plain atomics rather than anything that can block. Each
/// field is written by one task and read by another, so `Relaxed` is enough:
/// there is no invariant spanning two fields to tear.
#[derive(Default)]
struct Diagnostics {
    free: AtomicU32,
    largest: AtomicU32,
    minimum: AtomicU32,
    psram_free: AtomicU32,
    uptime: AtomicU32,
    samples: AtomicU32,
}

impl Diagnostics {
    fn sample(&self) {
        let internal = esp_idf_svc::sys::MALLOC_CAP_INTERNAL | esp_idf_svc::sys::MALLOC_CAP_8BIT;
        let spiram = esp_idf_svc::sys::MALLOC_CAP_SPIRAM;
        // SAFETY: ESP-IDF heap and timer queries take no pointer arguments.
        unsafe {
            self.free.store(
                esp_idf_svc::sys::heap_caps_get_free_size(internal) as u32,
                Ordering::Relaxed,
            );
            self.largest.store(
                esp_idf_svc::sys::heap_caps_get_largest_free_block(internal) as u32,
                Ordering::Relaxed,
            );
            self.minimum.store(
                esp_idf_svc::sys::heap_caps_get_minimum_free_size(internal) as u32,
                Ordering::Relaxed,
            );
            self.psram_free.store(
                esp_idf_svc::sys::heap_caps_get_free_size(spiram) as u32,
                Ordering::Relaxed,
            );
            self.uptime.store(
                (esp_idf_svc::sys::esp_timer_get_time() / 1_000_000) as u32,
                Ordering::Relaxed,
            );
        }
        self.samples.fetch_add(1, Ordering::Relaxed);
    }

    /// Serialises without allocating and without touching the ledger.
    fn write_json(&self, out: &mut [u8]) -> usize {
        use std::io::Write as _;
        let mut cursor = std::io::Cursor::new(out);
        let _ = write!(
            cursor,
            concat!(
                r#"{{"uptime_seconds":{},"free_heap":{},"largest_free_block":{},"#,
                r#""minimum_free_heap":{},"psram_free":{},"samples":{},"#,
                r#""sampler_core":0,"http_core":1}}"#
            ),
            self.uptime.load(Ordering::Relaxed),
            self.free.load(Ordering::Relaxed),
            self.largest.load(Ordering::Relaxed),
            self.minimum.load(Ordering::Relaxed),
            self.psram_free.load(Ordering::Relaxed),
            self.samples.load(Ordering::Relaxed),
        );
        cursor.position() as usize
    }
}

// Per-worker response storage.
//
// The handler closure is shared by every httpd worker, so a single buffer
// would have to live behind the service mutex and would keep that mutex held
// for the whole of serialisation and the socket write. One buffer per worker
// thread means the mutex covers only the ledger call itself.
//
// Allocated on first use by each worker and reused for that worker's life.
// PSRAM is what is plentiful here, so this trades 512 KiB per worker for a
// materially shorter critical section.
thread_local! {
    static RESPONSE: std::cell::RefCell<Box<[u8]>> =
        std::cell::RefCell::new(vec![0u8; api::RESPONSE_LIMIT].into_boxed_slice());
}

struct NvsJournal(EspNvs<NvsCustom>);

impl Journal for NvsJournal {
    fn read(&mut self, index: usize, frame: &mut [u8; FRAME_SIZE]) -> Result<bool, Error> {
        let mut key = heapless::String::<8>::new();
        core::fmt::Write::write_fmt(&mut key, format_args!("e{index:04x}"))
            .map_err(|_| Error::Capacity)?;
        match self.0.get_blob(&key, frame).map_err(|_| Error::Storage)? {
            Some(data) if data.len() == FRAME_SIZE => Ok(true),
            Some(_) => Err(Error::CorruptJournal),
            None => Ok(false),
        }
    }
    fn append(&mut self, index: usize, frame: &[u8; FRAME_SIZE]) -> Result<(), Error> {
        // EspNvs::set_blob performs nvs_set_blob AND nvs_commit. The record is
        // the commit marker; no separately updated head can get out of sync.
        let mut key = heapless::String::<8>::new();
        core::fmt::Write::write_fmt(&mut key, format_args!("e{index:04x}"))
            .map_err(|_| Error::Capacity)?;
        if self.0.blob_len(&key).map_err(|_| Error::Storage)?.is_some() {
            return Err(Error::Storage);
        }
        self.0.set_blob(&key, frame).map_err(|_| Error::Storage)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = Peripherals::take()?;
    let event_loop = EspSystemEventLoop::take()?;
    // Never auto-erase NVS on a version/full error: it may contain data.
    let system_nvs = EspDefaultNvsPartition::take_with(false)?;
    // The svc custom-partition convenience constructor auto-erases on some
    // init errors. Pre-initialize and propagate every error before calling it.
    // IDF initialization is idempotent; its second call sees an initialized
    // partition. Never reach the convenience constructor after a failed init.
    // SAFETY: the partition name is a static NUL-terminated C string.
    esp_idf_svc::sys::esp!(unsafe {
        esp_idf_svc::sys::nvs_flash_init_partition(c"ledger".as_ptr())
    })?;
    let partition = EspNvsPartition::<NvsCustom>::take("ledger")?;
    let journal = NvsJournal(EspNvs::new(partition, "nanacoin", true)?);
    let service = Service::open(journal).map_err(|e| format!("ledger startup: {e:?}"))?;
    // Response storage is per worker (see RESPONSE); the mutex guards only
    // the ledger service, so it is held for the domain call and nothing else.
    let shared = Arc::new(Mutex::new(service));
    let diagnostics = Arc::new(Diagnostics::default());
    diagnostics.sample();
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, event_loop.clone(), Some(system_nvs))?,
        event_loop,
    )?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: env!("NANACOIN_WIFI_SSID")
            .try_into()
            .map_err(|_| "SSID exceeds 32 bytes")?,
        password: env!("NANACOIN_WIFI_PASSWORD")
            .try_into()
            .map_err(|_| "Wi-Fi password exceeds 64 bytes")?,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))?;
    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;
    // Keep the SNTP service alive. Timed offers refuse writes until wall time
    // is valid; persisted deadlines must not restart when the board reboots.
    let mut time_config = esp_idf_svc::sntp::SntpConf::default();
    if let Some(server) = option_env!("NANACOIN_NTP_SERVER") {
        time_config.servers.fill(server);
    }
    let _sntp = esp_idf_svc::sntp::EspSntp::new(&time_config)?;

    let config = HttpConfiguration {
        https_port: 443,
        core: Some(Core::Core1),
        // Release parsing frames reach 7.5 KiB before the HTTP/TLS caller
        // frames. Keep headroom for the full call chain, not just one frame.
        stack_size: 24 * 1024,
        max_open_sockets: 4,
        max_sessions: 4,
        max_uri_handlers: 4,
        uri_match_wildcard: true,
        session_timeout: Duration::from_secs(10),
        server_certificate: Some(X509::pem_until_nul(
            concat!(include_str!("../../certs/server.crt"), "\0").as_bytes(),
        )),
        private_key: Some(X509::pem_until_nul(
            concat!(include_str!("../../certs/server.key"), "\0").as_bytes(),
        )),
        ..Default::default()
    };
    let mut server = EspHttpServer::new(&config)?;
    for (method, method_name) in [
        (Method::Get, "GET"),
        (Method::Post, "POST"),
        (Method::Patch, "PATCH"),
        (Method::Options, "OPTIONS"),
    ] {
        let shared = Arc::clone(&shared);
        let diagnostics = Arc::clone(&diagnostics);
        server.fn_handler::<esp_idf_svc::io::EspIOError, _>("/*", method, move |mut req| {
            let origin = heapless::String::<256>::try_from(req.header("Origin").unwrap_or(""));
            let allowed = origin.as_ref().is_ok_and(|o| {
                api::origin_allowed(
                    o,
                    option_env!("NANACOIN_ORIGINS").unwrap_or(api::DEFAULT_ORIGINS),
                )
            });
            let origin = origin.unwrap_or_default();
            let mut headers = heapless::Vec::<(&str, &str), 8>::new();
            for pair in [
                ("Content-Type", "application/json"),
                ("Cache-Control", "no-store"),
                ("Connection", "close"),
                ("Vary", "Origin"),
            ] {
                headers.push(pair).unwrap();
            }
            if allowed && !origin.is_empty() {
                headers
                    .push(("Access-Control-Allow-Origin", origin.as_str()))
                    .unwrap();
                headers
                    .push(("Access-Control-Allow-Methods", "GET, POST, PATCH, OPTIONS"))
                    .unwrap();
                headers
                    .push((
                        "Access-Control-Allow-Headers",
                        "Authorization, Content-Type, Idempotency-Key",
                    ))
                    .unwrap();
            }
            RESPONSE.with(|cell| {
                let mut borrowed = cell.borrow_mut();
                let output = &mut **borrowed;
                let (status, len) = if !allowed {
                    api::error_response(Error::Forbidden, output)
                } else if method_name == "OPTIONS" {
                    output[..2].copy_from_slice(b"{}");
                    (200, 2)
                } else if method_name == "GET"
                    && req.uri().split('?').next() == Some("/api/v1/diag")
                {
                    // Answered from the core 0 sampler's atomics. Never takes the
                    // service mutex, so it still responds while a write holds it.
                    (200, diagnostics.write_json(output))
                } else {
                    let length = if method_name == "POST" || method_name == "PATCH" {
                        req.header("Content-Length")
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(u64::MAX)
                    } else {
                        0
                    };
                    if length > api::BODY_LIMIT as u64 {
                        let (_, len) = api::error_response(Error::Capacity, output);
                        (413, len)
                    } else {
                        let mut body = [0; api::BODY_LIMIT];
                        if req.read_exact(&mut body[..length as usize]).is_err() {
                            api::error_response(Error::InvalidInput, output)
                        } else {
                            let mut service = shared.lock().unwrap();
                            api::handle_keyed(
                                &mut service,
                                method_name,
                                req.uri(),
                                req.header("Authorization").unwrap_or(""),
                                req.header("Idempotency-Key").unwrap_or(""),
                                &body[..length as usize],
                                output,
                            )
                        }
                    }
                };
                // Outside the mutex: serialising to the socket is the slow part
                // and no longer blocks other requests' ledger access.
                req.into_response(status, None, &headers)?
                    .write_all(&output[..len])?;
                Ok(())
            })
        })?;
    }
    let mut mdns = EspMdns::take()?;
    // Avoid the existing MicroPython nanacoin.local advertiser.
    mdns.set_hostname("nanacoin-rs")?;
    mdns.set_instance_name("NanaCoin Rust household ledger")?;
    mdns.add_service(
        Some("NanaCoin"),
        "_https",
        "_tcp",
        443,
        &[("path", "/api/v1/status")],
    )?;
    // Diagnostics sampler on core 0, beside Wi-Fi/lwIP and away from the
    // HTTPS workers and ledger on core 1. It only reads heap counters and
    // publishes atomics, so it never contends for the service mutex and keeps
    // /api/v1/diag answerable while a write is in flight.
    let sampler = Arc::clone(&diagnostics);
    // Applies to the next thread spawned on this task, then is restored so it
    // does not leak onto anything spawned later.
    ThreadSpawnConfiguration {
        name: Some(c"nanacoin-diag"),
        stack_size: 3072,
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;
    let _sampler = std::thread::Builder::new().spawn(move || loop {
        sampler.sample();
        std::thread::sleep(Duration::from_secs(2));
    })?;
    ThreadSpawnConfiguration::default().set()?;
    log::info!(
        "Ready at https://nanacoin-rs.local; HTTPS/ledger core 1 (4 sockets), Wi-Fi/lwIP/diag core 0"
    );
    loop {
        std::thread::sleep(Duration::from_secs(10));
        if !wifi.is_connected()? {
            log::warn!("Wi-Fi disconnected; reconnecting");
            if let Err(e) = wifi.connect().and_then(|_| wifi.wait_netif_up()) {
                log::warn!("Reconnect: {e}");
            }
        }
        // Internal largest-free-block matters more for TLS than total PSRAM.
        let caps = esp_idf_svc::sys::MALLOC_CAP_INTERNAL | esp_idf_svc::sys::MALLOC_CAP_8BIT;
        // SAFETY: ESP-IDF heap query functions have no pointer arguments.
        unsafe {
            log::info!(
                "internal heap: free={} largest={} minimum={}",
                esp_idf_svc::sys::heap_caps_get_free_size(caps),
                esp_idf_svc::sys::heap_caps_get_largest_free_block(caps),
                esp_idf_svc::sys::heap_caps_get_minimum_free_size(caps)
            );
        }
    }
}
