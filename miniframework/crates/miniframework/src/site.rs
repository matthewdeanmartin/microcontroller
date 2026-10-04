//! The request pipeline an app plugs into.
//!
//! An app implements [`Service`]: one `handle` method that matches on
//! method and path and answers through [`Reply`]. The [`Site`] around it
//! adds what every board app needs: CORS, static files, `/ca` and `/trust`,
//! `/api/v1/sys`, `/api/v1/schema`, `/metrics`, format negotiation, optional
//! gzip, and `Server-Timing` on every API response.
use crate::http::{self, Body, Response};
use crate::sys::{self, Platform, SysInfo, STATS};
use crate::web::{self, Asset, Spa};
use crate::wire::{self, schema::SchemaDoc, Compression, Encode, Format, Message, Schema};
use crate::{message, uptime_ms, wall_ms};
use std::borrow::Cow;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Instant;

message! {
    /// Every API error has this body, in the negotiated format.
    pub struct ErrorBody {
        /// Stable machine-readable code, e.g. `not_found`.
        1 error: String,
        /// Human-readable explanation.
        2 message: String,
    }
}

/// An error a handler can return with `?` and send with [`Reply::fail`].
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(400, "bad_request", message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, "not_found", message)
    }
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(401, "unauthorized", message)
    }
}

impl From<wire::Error> for ApiError {
    fn from(e: wire::Error) -> Self {
        match e {
            wire::Error::Overflow => ApiError::new(413, "too_large", e.to_string()),
            _ => ApiError::new(400, "bad_body", e.to_string()),
        }
    }
}

/// One API request, borrowed from the connection's input buffer.
pub struct Request<'a> {
    pub method: &'a str,
    /// Path and query exactly as received.
    pub uri: &'a str,
    /// Path without the query string.
    pub path: &'a str,
    /// Raw query string (after `?`), possibly empty.
    pub query: &'a str,
    pub body: &'a [u8],
    /// Arrived over TLS.
    pub secure: bool,
    headers: &'a [(String, String)],
    /// Bounds a gunzipped body (only read with the `gzip` feature).
    #[cfg_attr(not(feature = "gzip"), allow(dead_code))]
    body_limit: usize,
}

impl<'a> Request<'a> {
    /// A request for tests and tools (no socket).
    pub fn new(
        method: &'a str,
        uri: &'a str,
        headers: &'a [(String, String)],
        body: &'a [u8],
        secure: bool,
    ) -> Self {
        let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
        Self {
            method,
            uri,
            path,
            query,
            body,
            secure,
            headers,
            body_limit: 64 * 1024,
        }
    }

    /// Header value, or "" if absent. Names are case-insensitive.
    pub fn header(&self, name: &str) -> &'a str {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }

    /// A query parameter, percent-decoded. `?flag` gives `Some("")`.
    pub fn param(&self, name: &str) -> Option<Cow<'a, str>> {
        self.params().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    /// A numeric query parameter; absent is `Ok(None)`, unparsable is a 400.
    pub fn param_num<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, ApiError> {
        match self.param(name) {
            None => Ok(None),
            Some(v) if v.is_empty() => Ok(None),
            Some(v) => v
                .parse()
                .map(Some)
                .map_err(|_| ApiError::bad_request(format!("{name} must be a number"))),
        }
    }

    pub fn params(&self) -> impl Iterator<Item = (Cow<'a, str>, Cow<'a, str>)> + 'a {
        self.query.split('&').filter(|p| !p.is_empty()).map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(k), percent_decode(v))
        })
    }

    /// Path segments after the given prefix: `/api/v1/devices/7` with
    /// prefix `/api/v1/devices` gives `["7"]`.
    pub fn rest(&self, prefix: &str) -> Option<Vec<&'a str>> {
        let rest = self.path.strip_prefix(prefix)?;
        if !(rest.is_empty() || rest.starts_with('/')) {
            return None;
        }
        Some(rest.split('/').filter(|s| !s.is_empty()).collect())
    }

    /// The response format the client asked for (`?fmt=`, then `Accept`).
    pub fn format(&self) -> Result<Format, ApiError> {
        wire::negotiate(self.header("Accept"), self.param("fmt").as_deref()).map_err(|_| {
            ApiError::new(
                406,
                "unknown_format",
                "fmt must be one of json, msgpack, cbor, cbor-int, protobuf",
            )
        })
    }

    /// The body, gunzipped if `Content-Encoding: gzip`.
    pub fn body_bytes(&self) -> Result<Cow<'a, [u8]>, ApiError> {
        match self.header("Content-Encoding").trim() {
            "" | "identity" => Ok(Cow::Borrowed(self.body)),
            #[cfg(feature = "gzip")]
            "gzip" => wire::gzip::decompress(self.body, self.body_limit * 8)
                .map(Cow::Owned)
                .map_err(ApiError::from),
            other => Err(ApiError::new(
                415,
                "unsupported_encoding",
                format!("Content-Encoding {other} is not supported"),
            )),
        }
    }

    /// Decodes the body as `M`, in the format its `Content-Type` names.
    pub fn decode<M: Message>(&self) -> Result<M, ApiError> {
        let format = Format::from_mime(self.header("Content-Type")).ok_or_else(|| {
            ApiError::new(
                415,
                "unsupported_media_type",
                "Content-Type must be application/json, application/msgpack, application/cbor (keys=int optional) or application/x-protobuf",
            )
        })?;
        Ok(wire::decode(format, &self.body_bytes()?)?)
    }
}

fn percent_decode(s: &str) -> Cow<'_, str> {
    if !s.contains(['%', '+']) {
        return Cow::Borrowed(s);
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

/// Reusable buffers for one connection loop. Allocated once, at the
/// response limit, so serving never grows the heap.
pub struct Scratch {
    body: Vec<u8>,
    spare: Vec<u8>,
    /// Address of the `body` allocation whose bytes up to the limit have
    /// all been written (see [`Reply::fill`]).
    zeroed: usize,
}

impl Scratch {
    pub fn new(response_limit: usize) -> Self {
        let mut body = vec![0; response_limit];
        body.clear();
        Self {
            zeroed: body.as_ptr() as usize,
            body,
            spare: Vec::new(),
        }
    }

    fn reply(&mut self, limit: usize) -> Reply<'_> {
        if self.body.as_ptr() as usize != self.zeroed || self.body.capacity() < limit {
            // A gzip reply swapped the buffers (or the limit grew): zero again.
            self.body.clear();
            self.body.resize(limit, 0);
            self.body.clear();
            self.zeroed = self.body.as_ptr() as usize;
        }
        let mut reply = Reply::new(&mut self.body, &mut self.spare, limit);
        reply.zeroed = true;
        reply
    }
}

/// How a handler answers. Start with [`Reply::wire`] for data.
pub struct Reply<'a> {
    status: u16,
    content_type: Cow<'static, str>,
    headers: Vec<(&'static str, String)>,
    body: &'a mut Vec<u8>,
    /// Compression output (only used with the `gzip` feature).
    #[cfg_attr(not(feature = "gzip"), allow(dead_code))]
    spare: &'a mut Vec<u8>,
    limit: usize,
    accepts_gzip: bool,
    enc_us: u64,
    gz_us: u64,
    raw_len: Option<usize>,
    format: Option<Format>,
    /// Every byte of `body`'s allocation up to `limit` has been written.
    zeroed: bool,
}

impl<'a> Reply<'a> {
    /// A reply over caller-owned buffers (tests, tools).
    pub fn new(body: &'a mut Vec<u8>, spare: &'a mut Vec<u8>, limit: usize) -> Self {
        body.clear();
        Self {
            status: 0,
            content_type: Cow::Borrowed("application/json"),
            headers: Vec::new(),
            body,
            spare,
            limit,
            accepts_gzip: true,
            enc_us: 0,
            gz_us: 0,
            raw_len: None,
            format: None,
            zeroed: false,
        }
    }

    /// 0 until something has been sent.
    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn body(&self) -> &[u8] {
        self.body
    }

    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    /// The response format chosen by [`Reply::wire`], if any.
    pub fn format(&self) -> Option<Format> {
        self.format
    }

    /// Encoding time in microseconds (0 if nothing was encoded).
    pub fn encode_us(&self) -> u64 {
        self.enc_us
    }

    /// Adds a response header. One the site sets by default
    /// (`Content-Type`, `Cache-Control`, `Vary`, `Server-Timing`) is
    /// replaced rather than repeated.
    pub fn header(&mut self, name: &'static str, value: impl Into<String>) {
        self.headers.push((name, value.into()));
    }

    /// Writes a pre-encoded body straight into the reply buffer, for an app
    /// whose handlers already write into a byte slice. `write` gets the
    /// whole response limit and returns the status and length it wrote.
    /// Nothing is copied or zeroed per request.
    pub fn fill(
        &mut self,
        content_type: &'static str,
        write: impl FnOnce(&mut [u8]) -> (u16, usize),
    ) {
        if self.zeroed && self.body.capacity() >= self.limit {
            // SAFETY: Scratch wrote every byte of this allocation up to the
            // limit (checked by address), and truncating a Vec<u8> never
            // de-initializes memory, so all `limit` bytes are initialized.
            unsafe { self.body.set_len(self.limit) };
        } else {
            self.body.clear();
            self.body.resize(self.limit, 0);
        }
        let (status, len) = write(&mut self.body[..]);
        self.body.truncate(len.min(self.limit));
        self.status = status;
        self.content_type = Cow::Borrowed(content_type);
    }

    /// Lets browsers revalidate a successful reply: a strong `ETag` from a
    /// hash of the bytes, `control` as `Cache-Control`, and 304 with no body
    /// when `If-None-Match` already has these bytes. Hashing the content
    /// (not a time or counter) stays correct after a data reset.
    pub fn revalidate(&mut self, req: &Request<'_>, control: &'static str) {
        use sha2::{Digest, Sha256};
        if self.status != 200 {
            return;
        }
        let etag = format!("\"{:x}\"", Sha256::digest(&self.body[..]));
        if web::etag_matches(req.header("If-None-Match"), &etag) {
            self.status = 304;
            self.body.clear();
        }
        self.header("Cache-Control", control);
        self.header("ETag", etag);
    }

    /// 200 with `value` in the negotiated format (gzipped if `?gz=`).
    pub fn wire<E: Encode + ?Sized>(&mut self, req: &Request<'_>, value: &E) {
        self.wire_status(req, 200, value)
    }

    pub fn wire_status<E: Encode + ?Sized>(&mut self, req: &Request<'_>, status: u16, value: &E) {
        let format = match req.format() {
            Ok(f) => f,
            Err(e) => {
                // Unknown ?fmt=: answer in JSON so the client can read why.
                self.encode_as(Format::Json, e.status, &body_of(&e));
                return;
            }
        };
        self.encode_as(format, status, value);
        if self.status >= 500 || self.status == 413 {
            return;
        }
        let Compression::Gzip(level) = Compression::from_query(req.param("gz").as_deref()) else {
            return;
        };
        #[cfg(feature = "gzip")]
        if self.accepts_gzip {
            let started = Instant::now();
            if wire::gzip::compress(self.body, level, self.spare) {
                self.raw_len = Some(self.body.len());
                std::mem::swap(self.body, self.spare);
                self.zeroed = false;
                self.gz_us = started.elapsed().as_micros() as u64;
            }
        }
        #[cfg(not(feature = "gzip"))]
        let _ = level;
    }

    fn encode_as<E: Encode + ?Sized>(&mut self, format: Format, status: u16, value: &E) {
        let started = Instant::now();
        match wire::encode(format, value, self.body, self.limit) {
            Ok(()) => {
                self.status = status;
                self.content_type = Cow::Borrowed(format.mime());
                self.format = Some(format);
            }
            Err(wire::Error::Overflow) => {
                let e = ApiError::new(
                    507,
                    "response_too_large",
                    format!(
                        "The response exceeds this board's {} KiB limit; ask for fewer items",
                        self.limit / 1024
                    ),
                );
                if wire::encode(format, &body_of(&e), self.body, self.limit).is_err() {
                    self.body.clear();
                }
                self.status = 507;
                self.content_type = Cow::Borrowed(format.mime());
                self.format = Some(format);
            }
            Err(e) => {
                self.text(
                    500,
                    "text/plain; charset=utf-8",
                    &format!("encode failed: {e}"),
                );
            }
        }
        self.enc_us = started.elapsed().as_micros() as u64;
    }

    /// An [`ErrorBody`] in the negotiated format.
    pub fn fail(&mut self, req: &Request<'_>, error: &ApiError) {
        self.wire_status(req, error.status, &body_of(error));
    }

    pub fn error(&mut self, req: &Request<'_>, status: u16, code: &'static str, message: &str) {
        self.fail(req, &ApiError::new(status, code, message));
    }

    pub fn text(&mut self, status: u16, content_type: &'static str, text: &str) {
        self.bytes(status, content_type, text.as_bytes());
    }

    pub fn bytes(&mut self, status: u16, content_type: &'static str, bytes: &[u8]) {
        self.body.clear();
        let n = bytes.len().min(self.limit);
        self.body.extend_from_slice(&bytes[..n]);
        self.status = status;
        self.content_type = Cow::Borrowed(content_type);
    }

    /// 204 No Content.
    pub fn empty(&mut self) {
        self.body.clear();
        self.status = 204;
    }
}

fn body_of(e: &ApiError) -> ErrorBody {
    ErrorBody {
        error: e.code.into(),
        message: e.message.clone(),
    }
}

/// An app: answers `/api/...` requests.
pub trait Service: Send + Sync + 'static {
    /// Leave `reply` untouched for "no such route" (the site sends a 404).
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>);
    /// While true, plain HTTP serves only `/trust`, `/ca` (and `/` as the
    /// trust page) and `/metrics`; everything else is 403. Asked on every
    /// plain-HTTP request, so it can change at runtime (NanaCoin's "require
    /// HTTPS" switch).
    fn https_required(&self) -> bool {
        false
    }
    /// Root messages of this API, published in `/api/v1/schema`.
    fn schemas(&self) -> Vec<&'static Schema> {
        Vec::new()
    }
}

/// Site-wide settings. `Config::new` fills sensible board defaults.
pub struct Config {
    /// App name shown in sysinfo and on the trust page.
    pub app: &'static str,
    pub version: &'static str,
    /// Build fingerprint (see `tools/` build scripts), or "dev".
    pub build: &'static str,
    /// `name.local` (mDNS) or `localhost:8080` on desktop.
    pub host: String,
    /// Extra origins allowed to call the API (same-origin always is),
    /// e.g. the Angular dev server `http://localhost:4200`.
    pub origins: Vec<String>,
    /// Largest accepted request body (before gunzip).
    pub body_limit: usize,
    /// Largest response the API may build (bytes, before gzip).
    pub response_limit: usize,
    pub assets: &'static [Asset],
    /// Which paths are Angular routes (served `/index.html`).
    pub spa: Spa,
    /// DER of the household CA, served at `/ca` with a `/trust` page.
    pub ca_der: Option<&'static [u8]>,
    /// File name browsers save `/ca` as.
    pub ca_filename: &'static str,
    /// The app's own `/trust` page; by default one is generated.
    pub trust_html: Option<&'static [u8]>,
    /// CORS: methods and request headers a browser may use cross-origin,
    /// and response headers its scripts may read. Apps with their own
    /// headers (`Idempotency-Key`, `ETag`, ...) extend these.
    pub cors_methods: &'static str,
    pub cors_headers: &'static str,
    pub expose_headers: &'static str,
    /// Extra Influx tags on `/metrics`, e.g. `("bank", "s3".into())`.
    pub influx_tags: Vec<(&'static str, String)>,
    /// protobuf package name in `/api/v1/schema.proto`.
    pub proto_package: &'static str,
}

impl Config {
    pub fn new(app: &'static str, host: impl Into<String>) -> Self {
        Self {
            app,
            version: "0.1.0",
            build: "dev",
            host: host.into(),
            origins: vec![
                "http://localhost:4200".into(),
                "http://127.0.0.1:4200".into(),
            ],
            body_limit: 16 * 1024,
            response_limit: 128 * 1024,
            assets: &[],
            spa: Spa::Extensionless,
            ca_der: None,
            ca_filename: "household-ca.crt",
            trust_html: None,
            cors_methods: "GET, POST, PUT, DELETE, OPTIONS",
            cors_headers: "Authorization, Content-Type, Content-Encoding, Accept",
            expose_headers: EXPOSED,
            influx_tags: Vec::new(),
            proto_package: "miniframework",
        }
    }
}

/// The pipeline: built-in routes, then the app, then static files.
pub struct Site<S: Service> {
    pub config: Config,
    pub service: S,
    platform: Box<dyn Platform>,
    trust_page: Option<&'static [u8]>,
    ca_disposition: &'static str,
}

const EXPOSED: &str =
    "Server-Timing, X-Raw-Length, X-Wire-Format, Content-Length, Content-Encoding";

impl<S: Service> Site<S> {
    pub fn new(config: Config, service: S, platform: impl Platform) -> Self {
        // Built once; a site lives until reboot, so leaking it is free.
        let trust_page = config.trust_html.or_else(|| {
            config.ca_der.map(|der| {
                &*Box::leak(
                    web::trust_page(config.app, &config.host, der)
                        .into_bytes()
                        .into_boxed_slice(),
                )
            })
        });
        let ca_disposition = Box::leak(
            format!(
                "attachment; filename=\"{}\"",
                config.ca_filename.replace(['"', '\\', '\r', '\n'], "")
            )
            .into_boxed_str(),
        );
        Self {
            config,
            service,
            platform: Box::new(platform),
            trust_page,
            ca_disposition,
        }
    }

    pub fn sysinfo(&self) -> SysInfo {
        let mut info = self.platform.sysinfo();
        info.app = self.config.app.into();
        info.version = self.config.version.into();
        info.build = self.config.build.into();
        info.host = self.config.host.clone();
        info.uptime_ms = uptime_ms();
        info.wall_ms = wall_ms().unwrap_or(0);
        info.net = STATS.snapshot();
        info.wifi.disconnects = STATS.wifi_disconnects.load(Relaxed);
        info.wifi.last_reason = STATS.wifi_last_reason.load(Relaxed);
        info
    }

    fn origin_allowed(&self, origin: &str, host: &str) -> bool {
        if origin.is_empty() {
            return true;
        }
        if origin.len() > 256 {
            return false;
        }
        let authority = origin.split_once("://").map_or("", |(_, a)| a);
        (!host.is_empty() && authority.eq_ignore_ascii_case(host))
            || self.config.origins.iter().any(|o| o == origin)
    }

    /// Answers one parsed HTTP request.
    pub fn respond(
        &self,
        request: &http::Request,
        secure: bool,
        scratch: &mut Scratch,
    ) -> Response {
        let began = Instant::now();
        STATS.requests.fetch_add(1, Relaxed);
        let method = request.method.as_str();
        let head = method == "HEAD";
        let (path, _) = request.uri.split_once('?').unwrap_or((&request.uri, ""));
        let is_api = path == "/api" || path.starts_with("/api/") || path == "/metrics";
        // Machine health stays scrapeable in every transport mode.
        let metrics = path == "/metrics" && (method == "GET" || head);

        if !secure && !metrics && self.service.https_required() {
            let reply = self
                .builtin_page(method, path, true)
                .unwrap_or(web::StaticReply {
                    status: 403,
                    bytes: b"This board requires HTTPS. Open /trust to set this device up.",
                    headers: vec![
                        ("Content-Type", "text/plain; charset=utf-8"),
                        ("Cache-Control", "no-store"),
                    ],
                });
            if reply.status >= 400 {
                STATS.errors.fetch_add(1, Relaxed);
            }
            return reply.into_response(head, request.close);
        }
        if !is_api {
            if let Some(reply) = self.builtin_page(method, path, false) {
                return reply.into_response(head, request.close);
            }
            if let Some(reply) = web::route(
                self.config.assets,
                self.config.spa,
                method,
                path,
                request.header("Accept-Encoding"),
                request.header("If-None-Match"),
            ) {
                if reply.status >= 400 {
                    STATS.errors.fetch_add(1, Relaxed);
                }
                STATS.sent(reply.bytes.len());
                return Response::new(
                    reply.status,
                    &reply.headers,
                    Body::Flash(reply.bytes),
                    head,
                    request.close,
                );
            }
        }

        let origin = request.header("Origin");
        let allowed = self.origin_allowed(origin, request.header("Host"));
        let req = Request {
            method: if head { "GET" } else { method },
            uri: &request.uri,
            path,
            query: request.uri.split_once('?').map_or("", |(_, q)| q),
            body: &request.body,
            secure,
            headers: &request.headers,
            body_limit: self.config.body_limit,
        };
        let mut reply = scratch.reply(self.config.response_limit);
        reply.accepts_gzip = web::accepts_gzip(request.header("Accept-Encoding"));

        if !allowed {
            reply.error(
                &req,
                403,
                "origin_not_allowed",
                "This origin may not call the API",
            );
        } else if method == "OPTIONS" {
            reply.empty();
        } else {
            self.builtin_api(&req, &mut reply);
            if reply.status == 0 {
                self.service.handle(&req, &mut reply);
            }
            if reply.status == 0 {
                reply.error(&req, 404, "not_found", "No such API route");
            }
        }

        let total_us = began.elapsed().as_micros() as u64;
        let app_us = total_us.saturating_sub(reply.enc_us + reply.gz_us);
        let mut timing = format!(
            "app;dur={:.3}, enc;dur={:.3}",
            app_us as f64 / 1000.0,
            reply.enc_us as f64 / 1000.0
        );
        if reply.raw_len.is_some() {
            timing.push_str(&format!(", gz;dur={:.3}", reply.gz_us as f64 / 1000.0));
        }
        let mut headers: Vec<(&str, String)> = vec![
            ("Content-Type", reply.content_type.to_string()),
            ("Cache-Control", "no-store".into()),
            ("Vary", "Accept, Origin".into()),
            ("Server-Timing", timing),
        ];
        // The app's own value for a default header replaces it.
        headers.retain(|(name, _)| {
            !reply
                .headers
                .iter()
                .any(|(set, _)| set.eq_ignore_ascii_case(name))
        });
        if let Some(format) = reply.format {
            headers.push(("X-Wire-Format", format.name().into()));
        }
        if let Some(raw) = reply.raw_len {
            headers.push(("Content-Encoding", "gzip".into()));
            headers.push(("X-Raw-Length", raw.to_string()));
        }
        if allowed && !origin.is_empty() {
            headers.push(("Access-Control-Allow-Origin", origin.into()));
            headers.push((
                "Access-Control-Allow-Methods",
                self.config.cors_methods.into(),
            ));
            headers.push((
                "Access-Control-Allow-Headers",
                self.config.cors_headers.into(),
            ));
            headers.push((
                "Access-Control-Expose-Headers",
                self.config.expose_headers.into(),
            ));
            headers.push(("Access-Control-Max-Age", "600".into()));
            headers.push(("Timing-Allow-Origin", origin.into()));
        }
        headers.append(&mut reply.headers);
        if reply.status >= 400 {
            STATS.errors.fetch_add(1, Relaxed);
        }
        // The response outlives the scratch buffer, so it gets its own copy.
        // Allocate fallibly: on a full heap answer 503, don't abort the board.
        let mut owned = Vec::new();
        if owned.try_reserve_exact(reply.body.len()).is_err() {
            STATS.errors.fetch_add(1, Relaxed);
            return Response::new(
                503,
                &[("Content-Type", "application/json"), ("Retry-After", "1")],
                Body::Flash(b"{\"error\":\"out_of_memory\",\"message\":\"The board is out of memory for this response; retry or ask for less\"}"),
                head,
                request.close,
            );
        }
        owned.extend_from_slice(reply.body);
        STATS.sent(owned.len());
        let header_refs: Vec<(&str, &str)> =
            headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        Response::new(
            reply.status,
            &header_refs,
            Body::Owned(owned),
            head,
            request.close,
        )
    }

    /// The answer to a request the HTTP layer refused (bad framing, a body
    /// or header over the limit) and the connection closes after it. When
    /// the headers can be read and the browser's origin is allowed, it
    /// carries CORS headers, so the page can tell its user *why* (a script
    /// cannot read a cross-origin error without them).
    pub fn refuse(&self, status: u16, raw: &[u8]) -> Response {
        let mut fields = [httparse::EMPTY_HEADER; 32];
        let mut parsed = httparse::Request::new(&mut fields);
        let complete = matches!(parsed.parse(raw), Ok(httparse::Status::Complete(_)));
        let header = |name: &str| -> &str {
            if !complete {
                return "";
            }
            parsed
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
                .and_then(|h| std::str::from_utf8(h.value).ok())
                .unwrap_or("")
        };
        let origin = header("Origin");
        let body: &'static [u8] = match status {
            413 => b"{\"error\":\"too_large\",\"message\":\"The request body is larger than this board accepts\"}",
            431 => b"{\"error\":\"headers_too_large\",\"message\":\"The request headers are larger than this board accepts\"}",
            _ => b"{\"error\":\"invalid_http_request\",\"message\":\"The request is not valid HTTP/1.1\"}",
        };
        let mut headers = vec![
            ("Content-Type", "application/json"),
            ("Cache-Control", "no-store"),
        ];
        if !origin.is_empty() && self.origin_allowed(origin, header("Host")) {
            headers.extend([
                ("Access-Control-Allow-Origin", origin),
                ("Access-Control-Expose-Headers", self.config.expose_headers),
                ("Vary", "Origin"),
            ]);
        }
        STATS.sent(body.len());
        Response::new(status, &headers, Body::Flash(body), false, true)
    }

    /// `/ca` and `/trust`; with `locked` (HTTPS required, plain HTTP)
    /// also `/` as the trust page.
    fn builtin_page(&self, method: &str, path: &str, locked: bool) -> Option<web::StaticReply> {
        let bytes: &'static [u8] = match path {
            "/ca" => self.config.ca_der?,
            "/trust" => self.trust_page?,
            "/" if locked => self.trust_page?,
            _ => return None,
        };
        let mut headers = vec![
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
            ("Referrer-Policy", "no-referrer"),
        ];
        if method != "GET" && method != "HEAD" {
            headers.push(("Allow", "GET, HEAD"));
            return Some(web::StaticReply {
                status: 405,
                bytes: b"",
                headers,
            });
        }
        if path == "/ca" {
            headers.push(("Content-Type", "application/x-x509-ca-cert"));
            headers.push(("Content-Disposition", self.ca_disposition));
        } else {
            headers.push(("Content-Type", "text/html; charset=utf-8"));
            headers.push((
                "Content-Security-Policy",
                "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            ));
        }
        Some(web::StaticReply {
            status: 200,
            bytes,
            headers,
        })
    }

    fn builtin_api(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        if req.method != "GET" {
            return;
        }
        match req.path {
            "/api/v1/sys" => reply.wire(req, &self.sysinfo()),
            "/metrics" => reply.text(
                200,
                "text/plain; version=0.0.4; charset=utf-8",
                &sys::influx_line(&self.sysinfo(), &self.config.influx_tags),
            ),
            "/api/v1/schema" => reply.wire(req, &self.schema()),
            "/api/v1/log" | "/api/v1/log.txt" => {
                let after = req.param_num::<u32>("after").ok().flatten().unwrap_or(0);
                let limit = req
                    .param_num::<usize>("limit")
                    .ok()
                    .flatten()
                    .unwrap_or(300)
                    .min(2000);
                let page = crate::logbuf::page(after, limit);
                if req.path.ends_with(".txt") {
                    reply.text(
                        200,
                        "text/plain; charset=utf-8",
                        &crate::logbuf::text(&page),
                    );
                } else {
                    reply.wire(req, &page);
                }
            }
            "/api/v1/schema.proto" => reply.text(
                200,
                "text/plain; charset=utf-8",
                &wire::proto_text(self.config.proto_package, &self.roots()),
            ),
            _ => {}
        }
    }

    fn roots(&self) -> Vec<&'static Schema> {
        let mut roots = self.service.schemas();
        roots.extend([
            SysInfo::SCHEMA,
            ErrorBody::SCHEMA,
            crate::logbuf::LogPage::SCHEMA,
        ]);
        roots
    }

    pub fn schema(&self) -> SchemaDoc {
        wire::schema_doc(&self.roots())
    }
}

impl web::StaticReply {
    fn into_response(self, head: bool, close: bool) -> Response {
        STATS.sent(self.bytes.len());
        Response::new(
            self.status,
            &self.headers,
            Body::Flash(self.bytes),
            head,
            close,
        )
    }
}

#[cfg(test)]
mod tests;
