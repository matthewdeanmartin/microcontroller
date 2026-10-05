//! The station, and the setup network for boards whose Wi-Fi is chosen by
//! a person rather than built in (from mastomini, spec/06 there).
//!
//! With [`BoardConfig::saved_wifi`](super::BoardConfig) the board joins the
//! network saved in that NVS namespace (keys `ssid`, `pass`), else the
//! built-in one, and saves built-in credentials that worked. With
//! [`BoardConfig::setup_network`](super::BoardConfig), a board that has no
//! network, or cannot join it, opens that open network instead: a DNS
//! responder answers every name with the board's address (phones show the
//! app's page as a captive portal), and the app's setup page uses
//! [`WifiSetup`] to list networks and join one. The page is the app's; the
//! framework serves no UI.
//!
//! While the setup network is open and nobody is on it, the saved network
//! is retried every minute, so a board that booted before its router (a
//! power cut) goes back to normal by itself. A joined board closes the
//! setup network when the app says so, or after 20 minutes.
use crate::captive;
use crate::events::{self, Event};
use crate::status::SIGNALS;
use esp_idf_svc::{
    nvs::{EspNvs, NvsDefault},
    sys,
    wifi::{
        AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration,
        EspWifi,
    },
};
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Joining from the setup page gives up after this long.
const JOIN_TIMEOUT: Duration = Duration::from_secs(20);
/// The setup network closes this long after the station joined, at the
/// latest (the app may close it sooner, e.g. once its owner is set up).
const CLOSE_AFTER_JOINED: Duration = Duration::from_secs(20 * 60);
/// While nobody uses the setup network, retry the saved network this often.
const RETRY_SAVED: Duration = Duration::from_secs(60);
/// Boot attempts on the saved network before opening the setup network.
const BOOT_ATTEMPTS: u32 = 3;

pub(super) fn client(ssid: &str, password: &str) -> Result<ClientConfiguration, String> {
    Ok(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| "The network name is too long")?,
        password: password
            .try_into()
            .map_err(|_| "The password is too long")?,
        auth_method: if password.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        },
        ..Default::default()
    })
}

fn access_point(ssid: &str) -> AccessPointConfiguration {
    AccessPointConfiguration {
        ssid: ssid.try_into().unwrap_or_default(),
        auth_method: AuthMethod::None,
        channel: 1,
        max_connections: 4,
        ..Default::default()
    }
}

/// The radio and what it is doing. Shared by housekeeping (the calling
/// task) and the app's setup page (the serving task).
pub(super) struct Radio {
    pub(super) wifi: BlockingWifi<EspWifi<'static>>,
    nvs: Option<EspNvs<NvsDefault>>,
    builtin: (&'static str, &'static str),
    setup_ssid: Option<&'static str>,
    /// The setup network is open.
    setup: bool,
    ssid: String,
    password: String,
    station_ip: Option<Ipv4Addr>,
    joined_at: Option<Instant>,
    retried_at: Option<Instant>,
    /// Why the setup network opened, for the page.
    reason: Option<String>,
    dns_started: bool,
}

impl Radio {
    pub(super) fn new(
        wifi: BlockingWifi<EspWifi<'static>>,
        nvs: Option<EspNvs<NvsDefault>>,
        builtin: (&'static str, &'static str),
        setup_ssid: Option<&'static str>,
    ) -> Self {
        Self {
            wifi,
            nvs,
            builtin,
            setup_ssid,
            setup: false,
            ssid: String::new(),
            password: String::new(),
            station_ip: None,
            joined_at: None,
            retried_at: None,
            reason: None,
            dns_started: false,
        }
    }

    pub(super) fn in_setup(&self) -> bool {
        self.setup
    }

    /// Saved credentials, else the built-in ones; `true` when saved.
    fn credentials(&self) -> Option<(String, String, bool)> {
        if let Some(nvs) = &self.nvs {
            let mut ssid = [0u8; 33];
            let mut pass = [0u8; 65];
            if let (Ok(Some(s)), Ok(p)) = (
                nvs.get_str("ssid", &mut ssid),
                nvs.get_str("pass", &mut pass),
            ) {
                return Some((s.to_string(), p.unwrap_or("").to_string(), true));
            }
        }
        let (ssid, password) = self.builtin;
        (!ssid.is_empty()).then(|| (ssid.to_string(), password.to_string(), false))
    }

    fn save(&self, ssid: &str, password: &str) -> Result<(), String> {
        let Some(nvs) = &self.nvs else {
            return Ok(());
        };
        nvs.set_str("ssid", ssid).map_err(|e| e.to_string())?;
        nvs.set_str("pass", password).map_err(|e| e.to_string())
    }

    pub(super) fn sta_address(&self) -> Option<Ipv4Addr> {
        let info = self.wifi.wifi().sta_netif().get_ip_info().ok()?;
        (!info.ip.is_unspecified()).then_some(info.ip)
    }

    /// Startup: the saved (or built-in) network, a few attempts; else the
    /// setup network when there is one. Err only when neither works.
    pub(super) fn boot(&mut self, force_setup: bool) -> Result<(), String> {
        let reason = match self.credentials() {
            _ if force_setup && self.setup_ssid.is_some() => {
                "setup forced by this build".to_string()
            }
            None => "No Wi-Fi network is saved yet.".to_string(),
            Some((ssid, password, saved)) => {
                let conf = client(&ssid, &password)?;
                self.wifi
                    .set_configuration(&Configuration::Client(conf))
                    .map_err(|e| e.to_string())?;
                self.wifi.start().map_err(|e| e.to_string())?;
                let mut last = String::new();
                for attempt in 1..=BOOT_ATTEMPTS {
                    match self.wifi.connect().and_then(|_| self.wifi.wait_netif_up()) {
                        Ok(()) => {
                            if !saved {
                                // Built-in credentials worked: keep them, so
                                // builds without them still find this network.
                                if let Err(e) = self.save(&ssid, &password) {
                                    log::warn!("Could not save Wi-Fi credentials: {e}");
                                }
                            }
                            self.ssid = ssid;
                            self.password = password;
                            self.station_ip = self.sta_address();
                            SIGNALS.wifi(true);
                            events::emit(Event::WifiUp);
                            return Ok(());
                        }
                        Err(e) => {
                            events::emit(Event::ReconnectFailed { code: e.code() });
                            log::warn!("Wi-Fi join {attempt}/{BOOT_ATTEMPTS} failed ({e})");
                            last = e.to_string();
                            let _ = self.wifi.disconnect();
                            std::thread::sleep(Duration::from_secs(2));
                        }
                    }
                }
                // Keep them for the background retries.
                self.ssid = ssid.clone();
                self.password = password;
                format!("Could not join {ssid} ({last}). Choose the network again.")
            }
        };
        if self.setup_ssid.is_none() {
            return Err(reason);
        }
        log::warn!("{reason}");
        self.open_setup(reason).map_err(|e| e.to_string())
    }

    /// Opens the setup network (the station idle until a network is chosen).
    fn open_setup(&mut self, reason: String) -> Result<(), sys::EspError> {
        let Some(ap) = self.setup_ssid else {
            return Ok(());
        };
        let _ = self.wifi.disconnect();
        self.wifi.set_configuration(&Configuration::Mixed(
            ClientConfiguration::default(),
            access_point(ap),
        ))?;
        if !self.wifi.is_started()? {
            self.wifi.start()?;
        }
        self.setup = true;
        self.reason = Some(reason);
        SIGNALS.setup(true);
        if !self.dns_started {
            self.dns_started = true;
            if let Err(e) = super::spawn_task(c"mf-dns", 6 * 1024, false, None, 3, dns_responder) {
                log::warn!("Captive DNS task failed to start: {e}");
            }
        }
        log::info!(
            "Setup network {ap} is up: join it and open http://{}/",
            captive::AP_IP
        );
        Ok(())
    }

    /// Joins `ssid` while keeping the setup network up; saves on success.
    fn join_from_setup(&mut self, ssid: &str, password: &str) -> Result<Ipv4Addr, String> {
        let ap = self.setup_ssid.ok_or("this board has no setup network")?;
        let conf = client(ssid, password)?;
        let _ = self.wifi.disconnect();
        self.wifi
            .set_configuration(&Configuration::Mixed(conf, access_point(ap)))
            .map_err(|e| e.to_string())?;
        // connect() waits for association; a wrong password fails here.
        if let Err(e) = self.wifi.connect() {
            let _ = self.wifi.disconnect();
            return Err(format!("could not connect ({e})"));
        }
        let deadline = Instant::now() + JOIN_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(ip) = self.sta_address() {
                self.save(ssid, password)?;
                self.joined(ssid, password, ip);
                return Ok(ip);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let _ = self.wifi.disconnect();
        Err("joined, but the router gave no address".into())
    }

    fn joined(&mut self, ssid: &str, password: &str, ip: Ipv4Addr) {
        self.ssid = ssid.to_string();
        self.password = password.to_string();
        self.station_ip = Some(ip);
        self.joined_at = Some(Instant::now());
        SIGNALS.wifi(true);
        events::emit(Event::WifiUp);
        log::info!("Ready at http://{ip}/ (Wi-Fi {ssid})");
    }

    /// Nearby network names, strongest first, without duplicates.
    fn scan(&mut self) -> Vec<String> {
        let mut found = self.wifi.scan().unwrap_or_default();
        found.sort_by_key(|ap| -i16::from(ap.signal_strength));
        let mut names: Vec<String> = Vec::new();
        for ap in found {
            let name = ap.ssid.to_string();
            if !name.is_empty() && Some(name.as_str()) != self.setup_ssid && !names.contains(&name)
            {
                names.push(name);
            }
        }
        names.truncate(20);
        names
    }

    /// Back to station only, on the network that worked.
    fn close_setup(&mut self) -> Result<(), String> {
        if !self.setup {
            return Ok(());
        }
        let conf = client(&self.ssid, &self.password)?;
        self.wifi
            .set_configuration(&Configuration::Client(conf))
            .map_err(|e| e.to_string())?;
        self.setup = false;
        SIGNALS.setup(false);
        log::info!("Setup network closed");
        if !self.wifi.is_connected().unwrap_or(false) {
            let _ = self.wifi.connect();
        }
        Ok(())
    }

    /// Housekeeping while the setup network is open (every 10 s).
    pub(super) fn setup_chores(&mut self) {
        if let Some(joined) = self.joined_at {
            if joined.elapsed() > CLOSE_AFTER_JOINED {
                if let Err(e) = self.close_setup() {
                    log::warn!("Could not close the setup network: {e}");
                }
            }
            return;
        }
        // A background retry of the saved network got an address.
        if self.retried_at.is_some() {
            if let Some(ip) = self.sta_address() {
                let (ssid, password) = (self.ssid.clone(), self.password.clone());
                self.joined(&ssid, &password, ip);
                if ap_clients() == 0 {
                    if let Err(e) = self.close_setup() {
                        log::warn!("Could not close the setup network: {e}");
                    }
                }
                return;
            }
        }
        let due = self.retried_at.is_none_or(|t| t.elapsed() >= RETRY_SAVED);
        if self.ssid.is_empty() || !due || ap_clients() > 0 {
            return;
        }
        let (Some(ap), Ok(conf)) = (self.setup_ssid, client(&self.ssid, &self.password)) else {
            return;
        };
        self.retried_at = Some(Instant::now());
        events::emit(Event::Reconnect);
        if self
            .wifi
            .set_configuration(&Configuration::Mixed(conf, access_point(ap)))
            .is_ok()
        {
            // Not waiting: the next round looks for an address.
            if let Err(e) = self.wifi.wifi_mut().connect() {
                events::emit(Event::ReconnectFailed { code: e.code() });
            }
        }
    }
}

/// Phones (stations) on the setup network.
fn ap_clients() -> usize {
    let mut list = sys::wifi_sta_list_t::default();
    // SAFETY: fills a local struct; fails harmlessly when the AP is off.
    if unsafe { sys::esp_wifi_ap_get_sta_list(&mut list) } == sys::ESP_OK {
        list.num as usize
    } else {
        0
    }
}

/// Answers every DNS `A` query with the setup address. Runs forever.
fn dns_responder() {
    let socket = match UdpSocket::bind("0.0.0.0:53") {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Captive DNS unavailable: {e}");
            return;
        }
    };
    let mut buf = [0u8; 512];
    loop {
        let Ok((len, peer)) = socket.recv_from(&mut buf) else {
            continue;
        };
        if let Some(reply) = captive::dns_reply(&buf[..len]) {
            let _ = socket.send_to(&reply, peer);
        }
    }
}

/// What an app's setup page needs (`Board::wifi_setup`). Cheap to clone;
/// calls lock the radio, and [`WifiSetup::join`] blocks up to 20 s, so
/// call it only from the setup page.
#[derive(Clone)]
pub struct WifiSetup(pub(super) Arc<Mutex<Radio>>);

impl WifiSetup {
    fn radio(&self) -> std::sync::MutexGuard<'_, Radio> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// The setup network is open.
    pub fn active(&self) -> bool {
        self.radio().setup
    }
    /// The station's address on the home network, once joined.
    pub fn station_ip(&self) -> Option<Ipv4Addr> {
        self.radio().station_ip
    }
    /// The home network's name (joined or being retried).
    pub fn ssid(&self) -> String {
        self.radio().ssid.clone()
    }
    /// Why the setup network opened.
    pub fn reason(&self) -> Option<String> {
        self.radio().reason.clone()
    }
    /// The setup network's own name.
    pub fn setup_ssid(&self) -> &'static str {
        self.radio().setup_ssid.unwrap_or("")
    }
    /// Nearby networks, strongest first (takes a couple of seconds).
    pub fn scan(&self) -> Vec<String> {
        self.radio().scan()
    }
    /// Joins and saves a network, keeping the setup network up so the
    /// phone can be told the board's new address.
    pub fn join(&self, ssid: &str, password: &str) -> Result<Ipv4Addr, String> {
        self.radio().join_from_setup(ssid, password)
    }
    /// How long ago the station joined from the setup network.
    pub fn joined_for(&self) -> Option<Duration> {
        self.radio().joined_at.map(|t| t.elapsed())
    }
    /// Closes the setup network (station only from now on).
    pub fn close(&self) -> Result<(), String> {
        self.radio().close_setup()
    }
}
