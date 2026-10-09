//! Opt-in local Wi-Fi setup portal, with a tiny offline vanilla-JS UI.
//! One pending operation and one worker; HTTP handlers never run the radio.
use crate::{
    wire::{self, Format, Schema},
    ApiError, Cors, Reply, Request, Service,
};
use std::{
    io::Cursor,
    net::Ipv4Addr,
    sync::{Arc, Mutex},
    time::Duration,
};

pub const DEFAULT_RETRY_MINUTES: u16 = 3;
pub const MAX_RETRY_MINUTES: u16 = 60;
pub const ROOT: &str = "/wifi-setup";
const INDEX: &[u8] = include_bytes!("wifi_setup/index.html");
const JS: &[u8] = include_bytes!("wifi_setup/setup.js");
const MAX_BODY: usize = 512;

/// Compiled app policy. `None` disables the lightweight setup-code gate.
/// The saved retry setting overrides this default on later boots.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub code: Option<&'static str>,
    pub retry_minutes: u16,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            code: None,
            retry_minutes: DEFAULT_RETRY_MINUTES,
        }
    }
}
impl Options {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_minutes(u32::from(self.retry_minutes))?;
        if self.code.is_some_and(|s| {
            s.is_empty() || s.len() > 64 || !s.bytes().all(|b| b.is_ascii_graphic())
        }) {
            return Err("setup code must be 1-64 printable ASCII characters, or None");
        }
        Ok(())
    }
}
pub fn validate_minutes(minutes: u32) -> Result<u16, &'static str> {
    if (1..=u32::from(MAX_RETRY_MINUTES)).contains(&minutes) {
        Ok(minutes as u16)
    } else {
        Err("retry minutes must be between 1 and 60")
    }
}
/// Boot/recovery policy shared with the board and exercised on desktop.
#[derive(Clone, Copy, Debug)]
pub struct RetryWindow {
    duration: Duration,
}
impl RetryWindow {
    pub fn new(minutes: u16) -> Result<Self, &'static str> {
        validate_minutes(u32::from(minutes))?;
        Ok(Self {
            duration: Duration::from_secs(u64::from(minutes) * 60),
        })
    }
    pub fn duration(&self) -> Duration {
        self.duration
    }
    pub fn exhausted(&self, elapsed: Duration) -> bool {
        elapsed >= self.duration
    }
    /// Bounded backoff between attempts, capped at 15 seconds.
    pub fn backoff(attempt: u32) -> Duration {
        Duration::from_secs((2u64 << attempt.min(3)).min(15))
    }
}

/// Backend snapshot. No passwords or setup code are exposed to the browser.
#[derive(Clone, Debug)]
pub struct Info {
    pub ssid: String,
    pub address: Option<Ipv4Addr>,
    pub retry_minutes: u16,
}
/// Implementations may block on radio operations: only the worker calls
/// them. `active()` must be a cheap, nonblocking read (e.g. an atomic flag).
pub trait Backend: Send + Sync + 'static {
    fn active(&self) -> bool;
    fn info(&self) -> Info;
    fn scan(&self) -> Result<Vec<String>, String>;
    /// Save credentials only after association and DHCP succeed.
    fn join(&self, ssid: &str, password: &str) -> Result<Ipv4Addr, String>;
    fn set_retry_minutes(&self, minutes: u16) -> Result<(), String>;
    fn close(&self) -> Result<(), String>;
}

crate::message! { pub struct Snapshot {
    1 active: bool,
    2 retry_minutes: u32,
    3 phase: String,
    4 ssid: String,
    5 address: String,
    6 error: String,
    7 networks: Vec<String>,
} }
crate::message! { struct JoinRequest { 1 ssid: String, 2 password: String, 3 retry_minutes: u32, } }
crate::message! { struct RetryRequest { 1 retry_minutes: u32, } }
crate::message! { struct Metadata { 1 active: bool, 2 code_required: bool, } }

enum Command {
    Scan,
    Join(JoinRequest),
    Retry(u16),
    Close,
}
struct State {
    snapshot: Snapshot,
    pending: Option<Command>,
    busy: bool,
}
/// Share with a `Portal` and one worker calling `run_once()` regularly.
pub struct Controller<B: Backend> {
    backend: B,
    options: Options,
    state: Mutex<State>,
}
impl<B: Backend> Controller<B> {
    pub fn new(backend: B, options: Options) -> Result<Self, &'static str> {
        options.validate()?;
        let info = backend.info();
        validate_minutes(u32::from(info.retry_minutes))?;
        Ok(Self {
            backend,
            options,
            state: Mutex::new(State {
                snapshot: Snapshot {
                    active: false,
                    retry_minutes: u32::from(info.retry_minutes),
                    phase: "ready".into(),
                    ssid: info.ssid,
                    address: info.address.map(|ip| ip.to_string()).unwrap_or_default(),
                    ..Default::default()
                },
                pending: None,
                busy: false,
            }),
        })
    }
    pub fn active(&self) -> bool {
        self.backend.active()
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot
            .clone();
        snapshot.active = self.active();
        snapshot
    }
    fn enqueue(&self, command: Command) -> Result<(), ApiError> {
        if !self.active() {
            return Err(ApiError::new(
                404,
                "setup_inactive",
                "Wi-Fi setup is closed",
            ));
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.busy {
            return Err(ApiError::new(
                409,
                "setup_busy",
                "A setup operation is already in progress",
            ));
        }
        state.snapshot.error.clear();
        state.snapshot.phase = match &command {
            Command::Scan => "scanning",
            Command::Join(_) => "joining",
            Command::Retry(_) => "saving",
            Command::Close => "closing",
        }
        .into();
        state.pending = Some(command);
        state.busy = true;
        Ok(())
    }
    /// Runs at most one queued operation, outside the HTTP loop. Concurrent
    /// calls cannot claim the same operation. No radio lock is held by polls.
    /// An app owns worker lifetime; the ESP helper uses an internal-RAM stack.
    pub fn run_once(&self) -> bool {
        let command = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .take();
        let Some(command) = command else {
            if self.state.lock().unwrap_or_else(|e| e.into_inner()).busy {
                return false;
            }
            let info = self.backend.info();
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.busy {
                state.snapshot.ssid = info.ssid;
                state.snapshot.address = info.address.map(|ip| ip.to_string()).unwrap_or_default();
                state.snapshot.retry_minutes = u32::from(info.retry_minutes);
                if info.address.is_some() && state.snapshot.phase != "error" {
                    state.snapshot.phase = "connected".into();
                }
            }
            return false;
        };
        let mut networks = None;
        let result = if !self.active() {
            Err("Wi-Fi setup closed before the operation began".into())
        } else {
            match command {
                Command::Scan => self.backend.scan().map(|mut names| {
                    names.retain(|s| !s.is_empty() && s.len() <= 32 && !s.contains('\0'));
                    let mut unique = Vec::with_capacity(20);
                    for name in names {
                        if !unique.contains(&name) {
                            unique.push(name);
                            if unique.len() == 20 {
                                break;
                            }
                        }
                    }
                    networks = Some(unique);
                }),
                Command::Join(request) => self
                    .backend
                    .set_retry_minutes(request.retry_minutes as u16)
                    .and_then(|_| {
                        self.backend
                            .join(&request.ssid, &request.password)
                            .map(|_| ())
                    }),
                Command::Retry(minutes) => self.backend.set_retry_minutes(minutes),
                Command::Close => {
                    std::thread::sleep(Duration::from_secs(1));
                    self.backend.close()
                }
            }
        };
        let info = self.backend.info();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(networks) = networks {
            state.snapshot.networks = networks;
        }
        state.snapshot.retry_minutes = u32::from(info.retry_minutes);
        state.snapshot.ssid = info.ssid;
        state.snapshot.address = info.address.map(|ip| ip.to_string()).unwrap_or_default();
        state.snapshot.phase = if result.is_err() {
            "error"
        } else if info.address.is_some() {
            "connected"
        } else {
            "ready"
        }
        .into();
        state.snapshot.error = result
            .err()
            .map(|s| s.chars().take(160).collect())
            .unwrap_or_default();
        state.busy = false;
        true
    }
}

/// Composable `Service` wrapper. Setup HTML/JS are served only while the
/// AP is active; all normal app routes and policies continue to delegate.
pub struct Portal<S, B: Backend> {
    inner: S,
    controller: Arc<Controller<B>>,
}
impl<S, B: Backend> Portal<S, B> {
    pub fn new(inner: S, controller: Arc<Controller<B>>) -> Self {
        Self { inner, controller }
    }
    pub fn inner(&self) -> &S {
        &self.inner
    }
    pub fn controller(&self) -> &Arc<Controller<B>> {
        &self.controller
    }
}
fn reserved(path: &str) -> bool {
    path == ROOT || path.starts_with("/wifi-setup/")
}
fn probe(path: &str) -> bool {
    matches!(
        path,
        "/" | "/generate_204"
            | "/gen_204"
            | "/hotspot-detect.html"
            | "/library/test/success.html"
            | "/connecttest.txt"
            | "/ncsi.txt"
            | "/canonical.html"
            | "/success.txt"
            | "/redirect"
            | "/fwlink"
    )
}
fn decode<M: crate::Message>(req: &Request<'_>) -> Result<M, ApiError> {
    if req.body.len() > MAX_BODY || req.length > MAX_BODY {
        return Err(ApiError::new(
            413,
            "setup_body_limit",
            "Setup requests must be at most 512 bytes",
        ));
    }
    if !matches!(req.header("Content-Encoding"), "" | "identity") {
        return Err(ApiError::new(
            415,
            "setup_encoding",
            "Compressed setup requests are not supported",
        ));
    }
    if req
        .header("Content-Type")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        != "application/json"
    {
        return Err(ApiError::new(
            415,
            "setup_content_type",
            "Use application/json",
        ));
    }
    wire::decode(Format::Json, req.body).map_err(ApiError::from)
}
fn credentials(request: &JoinRequest) -> Result<(), ApiError> {
    validate_minutes(request.retry_minutes).map_err(|e| ApiError::new(400, "retry_minutes", e))?;
    if request.ssid.is_empty() || request.ssid.len() > 32 || request.ssid.contains('\0') {
        return Err(ApiError::new(
            400,
            "invalid_ssid",
            "Network name must be 1-32 bytes without NUL",
        ));
    }
    let password = &request.password;
    if !password.is_empty()
        && !((8..=63).contains(&password.len()) && !password.contains('\0')
            || password.len() == 64 && password.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(ApiError::new(
            400,
            "invalid_password",
            "Use an empty password for an open network, 8-63 bytes, or a 64-character hex key",
        ));
    }
    Ok(())
}
impl<S: Service, B: Backend> Service for Portal<S, B> {
    fn setup_route(&self, path: &str) -> bool {
        reserved(path) || self.controller.active() && probe(path)
    }
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        if !self.setup_route(req.path) {
            self.inner.handle(req, reply);
            return;
        }
        reply.header("Cache-Control", "no-store");
        reply.header("X-Content-Type-Options", "nosniff");
        if !self.controller.active() {
            reply.error(req, 404, "setup_inactive", "Wi-Fi setup is closed");
            return;
        }
        if probe(req.path) {
            reply.text(302, "text/plain", "Open Wi-Fi setup");
            reply.header("Location", ROOT);
            return;
        }
        if req.method == "GET" && matches!(req.path, ROOT | "/wifi-setup/" | "/wifi-setup/setup.js")
        {
            reply.header("Content-Security-Policy", "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'");
            let (bytes, mime) = if req.path.ends_with(".js") {
                (JS, "text/javascript; charset=utf-8")
            } else {
                (INDEX, "text/html; charset=utf-8")
            };
            reply.stream(200, mime, bytes.len() as u64, Box::new(Cursor::new(bytes)));
            return;
        }
        if req.method == "GET" && req.path == "/wifi-setup/api/meta" {
            reply.wire(
                req,
                &Metadata {
                    active: true,
                    code_required: self.controller.options.code.is_some(),
                },
            );
            return;
        }
        let origin = req.header("Origin");
        if req.header("Sec-Fetch-Site") == "cross-site"
            || !origin.is_empty()
                && origin != format!("http://{}", req.header("Host"))
                && origin != format!("https://{}", req.header("Host"))
        {
            reply.error(
                req,
                403,
                "setup_origin",
                "Open setup directly on the device",
            );
            return;
        }
        if self
            .controller
            .options
            .code
            .is_some_and(|code| req.header("X-Wifi-Setup-Code") != code)
        {
            reply.error(req, 401, "setup_code", "Enter the setup code");
            return;
        }
        if req.method == "GET" && req.path == "/wifi-setup/api/status" {
            reply.wire(req, &self.controller.snapshot());
            return;
        }
        if req.method != "POST" {
            reply.error(
                req,
                405,
                "setup_method",
                "Use GET for status and POST for changes",
            );
            reply.header("Allow", "GET, POST");
            return;
        }
        if req.header("X-Wifi-Setup-Request") != "1" {
            reply.error(req, 403, "setup_request", "Use the device setup page");
            return;
        }
        let command = (|| -> Result<Command, ApiError> {
            match req.path {
                "/wifi-setup/api/scan" | "/wifi-setup/api/close" => {
                    if req.length != 0 {
                        return Err(ApiError::new(
                            400,
                            "unexpected_body",
                            "This operation takes no body",
                        ));
                    }
                    Ok(if req.path.ends_with("scan") {
                        Command::Scan
                    } else {
                        Command::Close
                    })
                }
                "/wifi-setup/api/join" => {
                    let request = decode::<JoinRequest>(req)?;
                    credentials(&request)?;
                    Ok(Command::Join(request))
                }
                "/wifi-setup/api/retry" => {
                    let request = decode::<RetryRequest>(req)?;
                    Ok(Command::Retry(
                        validate_minutes(request.retry_minutes)
                            .map_err(|e| ApiError::new(400, "retry_minutes", e))?,
                    ))
                }
                _ => Err(ApiError::new(404, "setup_route", "Unknown setup operation")),
            }
        })();
        match command.and_then(|command| self.controller.enqueue(command)) {
            Ok(()) => {
                reply.text(202, "application/json", "{\"accepted\":true}");
                reply.header("Retry-After", "1");
            }
            Err(error) => reply.fail(req, &error),
        }
    }
    fn streamed_body(&self, method: &str, path: &str) -> Option<usize> {
        if reserved(path) {
            None
        } else {
            self.inner.streamed_body(method, path)
        }
    }
    fn cors(&self, path: &str) -> Cors {
        if self.setup_route(path) {
            Cors::None
        } else {
            self.inner.cors(path)
        }
    }
    fn origin_allowed(&self, origin: &str) -> bool {
        self.inner.origin_allowed(origin)
    }
    fn https_required(&self) -> bool {
        self.inner.https_required()
    }
    fn schemas(&self) -> Vec<&'static Schema> {
        self.inner.schemas()
    }
    fn metrics(&self) -> Vec<(&'static str, f64)> {
        self.inner.metrics()
    }
}

#[cfg(test)]
mod tests;
