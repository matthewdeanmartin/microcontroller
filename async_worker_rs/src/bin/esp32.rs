use async_worker_rs::{Service, DEFAULT_NTP_ADDR};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::peripherals::Peripherals,
    http::{
        server::{Configuration as HttpConfiguration, EspHttpServer},
        Method,
    },
    io::Write,
    nvs::EspDefaultNvsPartition,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = Peripherals::take()?;
    let events = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take_with(false)?;
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, events.clone(), Some(nvs))?,
        events,
    )?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: env!("WORKER_WIFI_SSID")
            .try_into()
            .map_err(|_| "SSID too long")?,
        password: env!("WORKER_WIFI_PASSWORD")
            .try_into()
            .map_err(|_| "password too long")?,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))?;
    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;
    let jobs = Service::start(
        option_env!("WORKER_NTP_ADDR")
            .unwrap_or(DEFAULT_NTP_ADDR)
            .parse()?,
    )?;
    let mut server = EspHttpServer::new(&HttpConfiguration {
        stack_size: 12 * 1024,
        max_open_sockets: 3,
        max_sessions: 3,
        max_uri_handlers: 3,
        uri_match_wildcard: true,
        session_timeout: Duration::from_secs(3),
        ..Default::default()
    })?;
    for (method, name) in [
        (Method::Get, "GET"),
        (Method::Post, "POST"),
        (Method::Put, "PUT"),
    ] {
        let jobs = jobs.clone();
        server.fn_handler::<esp_idf_svc::io::EspIOError, _>("/*", method, move |req| {
            let reply = jobs.handle(name, req.uri());
            let mut headers = vec![
                ("Content-Type", reply.content_type),
                ("Cache-Control", "no-store"),
                ("Connection", "close"),
            ];
            if let Some(location) = reply.location.as_deref() {
                headers.push(("Location", location));
            }
            if let Some(retry) = reply.retry_after {
                headers.push(("Retry-After", retry));
            }
            req.into_response(reply.status, None, &headers)?
                .write_all(&reply.body)?;
            Ok(())
        })?;
    }
    println!(
        "Async worker ready: http://{}/",
        wifi.wifi().sta_netif().get_ip_info()?.ip
    );
    loop {
        std::thread::sleep(Duration::from_secs(5));
        if !wifi.is_connected().unwrap_or(false) {
            let _ = wifi.connect();
        }
    }
}
