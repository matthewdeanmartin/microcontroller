#[cfg(not(target_os = "espidf"))]
compile_error!(
    "build firmware with --target xtensa-esp32s3-espidf --no-default-features --features esp32"
);

use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{cpu::Core, peripherals::Peripherals},
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
    sync::{Arc, Mutex},
    time::Duration,
};

struct NvsJournal(EspNvs<NvsCustom>);

impl Journal for NvsJournal {
    fn read(&mut self, index: usize, frame: &mut [u8; FRAME_SIZE]) -> Result<bool, Error> {
        let key = format!("e{index:04x}");
        match self.0.get_blob(&key, frame).map_err(|_| Error::Storage)? {
            Some(data) if data.len() == FRAME_SIZE => Ok(true),
            Some(_) => Err(Error::CorruptJournal),
            None => Ok(false),
        }
    }
    fn append(&mut self, index: usize, frame: &[u8; FRAME_SIZE]) -> Result<(), Error> {
        // EspNvs::set_blob performs nvs_set_blob AND nvs_commit. The record is
        // the commit marker; no separately updated head can get out of sync.
        let key = format!("e{index:04x}");
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
    // Allocate response storage once. Requests never grow this buffer.
    let shared = Arc::new(Mutex::new((
        service,
        vec![0u8; api::RESPONSE_LIMIT].into_boxed_slice(),
    )));
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
        stack_size: 16 * 1024,
        max_open_sockets: 2,
        max_sessions: 2,
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
            let mut guard = shared.lock().unwrap();
            let (service, output) = &mut *guard;
            let (status, len) = if !allowed {
                api::error_response(Error::Forbidden, output)
            } else if method_name == "OPTIONS" {
                output[..2].copy_from_slice(b"{}");
                (200, 2)
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
                        api::handle_keyed(
                            service,
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
            req.into_response(status, None, &headers)?
                .write_all(&output[..len])?;
            Ok(())
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
    log::info!("Ready at https://nanacoin-rs.local; HTTP/ledger core 1, Wi-Fi/lwIP core 0");
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
