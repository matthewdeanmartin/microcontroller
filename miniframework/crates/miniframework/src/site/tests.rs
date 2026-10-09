use super::*;
use crate::desktop::DesktopPlatform;
use crate::wire::Message;

#[test]
fn fill_initializes_every_byte_across_capacity_and_limit_changes() {
    let mut scratch = Scratch::new(1);
    for limit in [2, 8, 3, 64, 0, 1, 64] {
        let mut reply = scratch.reply(limit);
        reply.fill("text/plain", |out| {
            assert_eq!(out, vec![0; limit]);
            out.fill(0xa5);
            (200, usize::MAX)
        });
        assert_eq!(reply.body(), vec![0xa5; limit]);
        reply.fill("text/plain", |out| {
            assert_eq!(out, vec![0; limit]);
            (200, 0)
        });
        assert!(reply.body().is_empty());
    }
}

#[test]
fn replacing_response_bodies_clears_stream_owned_and_compression_state() {
    let mut scratch = Scratch::new(64);
    let mut reply = scratch.reply(64);
    reply.stream(
        200,
        "text/plain",
        6,
        Box::new(std::io::Cursor::new(b"secret")),
    );
    reply.owned(200, "text/plain", b"owned".to_vec());
    assert!(reply.stream.is_none());
    reply.raw_len = Some(100);
    reply.gz_us = 99;
    reply.format = Some(Format::Cbor);
    reply.bytes(200, "text/plain", b"replacement");
    assert!(reply.owned.is_none());
    assert!(reply.raw_len.is_none());
    assert_eq!(reply.gz_us, 0);
    assert_eq!(reply.format(), None);
    assert_eq!(reply.body(), b"replacement");
    reply.stream(
        200,
        "text/plain",
        6,
        Box::new(std::io::Cursor::new(b"secret")),
    );
    reply.empty();
    assert!(reply.stream.is_none());
    assert!(reply.body().is_empty());
    assert_eq!(reply.status(), 204);
}

message! {
    pub struct Echo {
        1 text: String,
        2 n: u64,
    }
}

struct App;

impl Service for App {
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        match (req.method, req.path) {
            ("GET", "/api/v1/echo") => {
                let n = match req.param_num::<u64>("n") {
                    Ok(n) => n.unwrap_or(1),
                    Err(e) => return reply.fail(req, &e),
                };
                reply.wire(
                    req,
                    &Echo {
                        text: req.param("text").unwrap_or_default().into_owned(),
                        n,
                    },
                )
            }
            ("POST", "/api/v1/echo") => match req.decode::<Echo>() {
                Ok(echo) => reply.wire_status(req, 201, &echo),
                Err(e) => reply.fail(req, &e),
            },
            _ => {}
        }
    }
    fn schemas(&self) -> Vec<&'static Schema> {
        vec![Echo::SCHEMA]
    }
}

fn site() -> Site<App> {
    let mut config = Config::new("test", "board.local");
    config.ca_der = Some(b"not really DER");
    Site::new(config, App, DesktopPlatform)
}

struct Wire {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Wire {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }
}

fn render(mut response: Response) -> Wire {
    let mut out = Vec::new();
    while !response.next().is_empty() {
        let n = response.next().len();
        out.extend_from_slice(response.next());
        response.advance(n);
    }
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let head = std::str::from_utf8(&out[..split]).unwrap();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Wire {
        status,
        headers,
        body: out[split..].to_vec(),
    }
}

fn call(site: &Site<App>, raw: &[u8]) -> Wire {
    let request = crate::http::parse(raw, 16 * 1024).unwrap().unwrap();
    let mut scratch = Scratch::new(64 * 1024);
    render(site.respond(&request, false, &mut scratch))
}

fn get(site: &Site<App>, uri: &str, extra: &str) -> Wire {
    call(
        site,
        format!("GET {uri} HTTP/1.1\r\nHost: board.local\r\n{extra}\r\n").as_bytes(),
    )
}

#[test]
fn every_format_answers_with_timing() {
    let site = site();
    for format in Format::ALL {
        let uri = format!("/api/v1/echo?text=hi%20there&n=7&fmt={}", format.name());
        let w = get(&site, &uri, "");
        assert_eq!(w.status, 200);
        assert_eq!(w.header("Content-Type"), format.mime());
        assert_eq!(w.header("X-Wire-Format"), format.name());
        assert!(w.header("Server-Timing").starts_with("app;dur="));
        let echo: Echo = wire::decode(format, &w.body).unwrap();
        assert_eq!(
            echo,
            Echo {
                text: "hi there".into(),
                n: 7
            }
        );
    }
    let w = get(
        &site,
        "/api/v1/echo",
        "Accept: application/cbor; keys=int\r\n",
    );
    assert_eq!(w.header("X-Wire-Format"), "cbor-int");
}

#[cfg(feature = "gzip")]
#[test]
fn gzip_only_when_asked_and_accepted() {
    let site = site();
    let text = "a".repeat(60);
    let w = get(
        &site,
        &format!("/api/v1/echo?text={text}"),
        "Accept-Encoding: gzip\r\n",
    );
    assert_eq!(w.header("Content-Encoding"), "");
    let w = get(
        &site,
        &format!("/api/v1/echo?text={text}&gz=1"),
        "Accept-Encoding: gzip\r\n",
    );
    assert_eq!(w.header("Content-Encoding"), "gzip");
    let raw: usize = w.header("X-Raw-Length").parse().unwrap();
    let body = wire::gzip::decompress(&w.body, 1 << 20).unwrap();
    assert_eq!(body.len(), raw);
    assert!(w.header("Server-Timing").contains("gz;dur="));
    let w = get(&site, "/api/v1/echo?gz=1", "Accept-Encoding: identity\r\n");
    assert_eq!(w.header("Content-Encoding"), "");
}

#[cfg(feature = "gzip")]
fn post(site: &Site<App>, content_type: &str, extra: &str, body: &[u8]) -> Wire {
    let mut raw = format!(
        "POST /api/v1/echo HTTP/1.1\r\nHost: board.local\r\nContent-Type: {content_type}\r\n{extra}Content-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    call(site, &raw)
}

#[cfg(feature = "gzip")]
#[test]
fn post_bodies_in_any_format_and_gzip() {
    let site = site();
    let echo = Echo {
        text: "posted".into(),
        n: 3,
    };
    for format in Format::ALL {
        let body = wire::to_vec(format, &echo).unwrap();
        let w = post(&site, format.mime(), "", &body);
        assert_eq!(w.status, 201, "{format:?}");
        assert_eq!(wire::decode::<Echo>(Format::Json, &w.body).unwrap(), echo);
    }
    let mut gz = Vec::new();
    wire::gzip::compress(&wire::to_vec(Format::Json, &echo).unwrap(), 6, &mut gz);
    let w = post(&site, "application/json", "Content-Encoding: gzip\r\n", &gz);
    assert_eq!(w.status, 201);
    let w = post(&site, "text/xml", "", b"<>");
    assert_eq!(w.status, 415);
    assert!(String::from_utf8_lossy(&w.body).contains("unsupported_media_type"));
}

#[test]
fn errors_are_structured_in_the_negotiated_format() {
    let site = site();
    let w = get(&site, "/api/v1/nope?fmt=cbor", "");
    assert_eq!(w.status, 404);
    let e: ErrorBody = wire::decode(Format::Cbor, &w.body).unwrap();
    assert_eq!(e.error, "not_found");
    assert_eq!(get(&site, "/api/v1/echo?n=abc", "").status, 400);
    let w = get(&site, "/api/v1/echo?fmt=xml", "");
    assert_eq!(w.status, 406);
    assert_eq!(w.header("Content-Type"), "application/json");
}

#[test]
fn cors_same_origin_dev_origin_and_strangers() {
    let site = site();
    let w = get(&site, "/api/v1/echo", "Origin: https://board.local\r\n");
    assert_eq!(w.status, 200);
    let w = get(&site, "/api/v1/echo", "Origin: http://localhost:4200\r\n");
    assert_eq!(
        w.header("Access-Control-Allow-Origin"),
        "http://localhost:4200"
    );
    assert!(w
        .header("Access-Control-Expose-Headers")
        .contains("Server-Timing"));
    let w = get(&site, "/api/v1/echo", "Origin: http://evil.example\r\n");
    assert_eq!(w.status, 403);
    let w = call(
        &site,
        b"OPTIONS /api/v1/echo HTTP/1.1\r\nHost: board.local\r\nOrigin: http://localhost:4200\r\n\r\n",
    );
    assert_eq!(w.status, 204);
    assert_eq!(w.header("Access-Control-Allow-Private-Network"), "");
}

#[test]
fn private_network_preflights_are_granted_to_allowed_origins_only() {
    let site = site();
    let w = call(
        &site,
        b"OPTIONS /api/v1/echo HTTP/1.1\r\nHost: board.local\r\nOrigin: http://localhost:4200\r\nAccess-Control-Request-Method: GET\r\nAccess-Control-Request-Private-Network: true\r\n\r\n",
    );
    assert_eq!(w.status, 204);
    assert_eq!(w.header("Access-Control-Allow-Private-Network"), "true");
    let w = call(
        &site,
        b"OPTIONS /api/v1/echo HTTP/1.1\r\nHost: board.local\r\nOrigin: http://evil.example\r\nAccess-Control-Request-Private-Network: true\r\n\r\n",
    );
    assert_eq!(w.header("Access-Control-Allow-Private-Network"), "");
}

#[test]
fn builtins() {
    let site = site();
    let w = get(&site, "/api/v1/sys?fmt=msgpack", "");
    let info: SysInfo = wire::decode(Format::MsgPack, &w.body).unwrap();
    assert_eq!(info.app, "test");
    assert_eq!(info.host, "board.local");
    assert!(info.net.requests >= 1);
    let w = get(&site, "/api/v1/schema", "");
    let schema: SchemaDoc = wire::decode(Format::Json, &w.body).unwrap();
    assert!(schema.messages.iter().any(|m| m.name == "Echo"));
    assert!(schema.messages.iter().any(|m| m.name == "SysInfo"));
    assert!(schema.messages.iter().any(|m| m.name == "Incidents"));
    let w = get(&site, "/.well-known/incidents", "");
    assert_eq!(w.status, 200);
    let incidents: crate::incidents::Incidents = wire::decode(Format::Json, &w.body).unwrap();
    assert_eq!(incidents.counters.len(), 28, "every kind, zero or not");
    assert!(incidents.volatile);
    let w = get(&site, "/api/v1/schema.proto", "");
    assert!(String::from_utf8_lossy(&w.body).contains("message Echo {"));
    let w = get(&site, "/metrics", "");
    let text = String::from_utf8(w.body).unwrap();
    let line = crate::influx::parse_line(text.trim()).unwrap().unwrap();
    assert_eq!(line.measurement, "board");
    let w = get(&site, "/trust", "");
    assert!(String::from_utf8_lossy(&w.body).contains("board.local"));
    assert_eq!(get(&site, "/ca", "").body, b"not really DER");
}

#[test]
fn oversized_responses_become_507() {
    let mut config = Config::new("test", "board.local");
    config.response_limit = 64;
    let small = Site::new(config, App, DesktopPlatform);
    let uri = format!("/api/v1/echo?text={}", "x".repeat(200));
    let request = crate::http::parse(
        format!("GET {uri} HTTP/1.1\r\nHost: board.local\r\n\r\n").as_bytes(),
        1024,
    )
    .unwrap()
    .unwrap();
    let mut scratch = Scratch::new(64);
    let w = render(small.respond(&request, false, &mut scratch));
    assert_eq!(w.status, 507);
}

/// An app shaped like NanaCoin: handlers write JSON into a byte slice,
/// public reads revalidate, and HTTPS can be required at runtime.
struct Bank {
    https_only: std::sync::atomic::AtomicBool,
}

impl Service for Bank {
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        match (req.method, req.path) {
            ("GET", "/api/v1/status") => {
                reply.fill("application/json", |out| {
                    let body = format!("{{\"uri\":\"{}\"}}", req.uri);
                    out[..body.len()].copy_from_slice(body.as_bytes());
                    (200, body.len())
                });
                reply.revalidate(req, "public, no-cache");
                reply.header("X-Generation", "7");
            }
            ("GET", "/api/v1/me") => {
                reply.fill("application/json", |out| {
                    out[..2].copy_from_slice(b"{}");
                    (200, 2)
                });
                reply.header("Cache-Control", "private, no-store");
            }
            ("GET", "/api/v1/missing") => reply.fill("application/json", |out| {
                out[..12].copy_from_slice(b"{\"error\":40}");
                (404, 12)
            }),
            ("GET", "/api/v1/whole") => reply.fill("text/plain", |out| {
                let len = out.len();
                out.fill(b'z');
                (200, len + 10)
            }),
            _ => {}
        }
    }
    fn https_required(&self) -> bool {
        self.https_only.load(Relaxed)
    }
}

fn bank() -> Site<Bank> {
    let mut config = Config::new("nanacoin", "nanacoin.local");
    config.ca_der = Some(b"DER");
    config.ca_filename = "NanaCoin-Home-CA.crt";
    config.trust_html = Some(b"<p>trust me</p>");
    config.cors_methods = "GET, POST, PATCH, OPTIONS";
    config.cors_headers = "Authorization, Content-Type, Idempotency-Key, If-None-Match";
    config.expose_headers = "X-Generation, Server-Timing, ETag";
    config.influx_tags = vec![("bank", "s3".into())];
    config.response_limit = 4096;
    Site::new(
        config,
        Bank {
            https_only: std::sync::atomic::AtomicBool::new(false),
        },
        DesktopPlatform,
    )
}

fn call_on<S: Service>(site: &Site<S>, raw: &[u8], secure: bool, scratch: &mut Scratch) -> Wire {
    let request = crate::http::parse(raw, 1024).unwrap().unwrap();
    render(site.respond(&request, secure, scratch))
}

fn get_on<S: Service>(site: &Site<S>, uri: &str, extra: &str, secure: bool) -> Wire {
    let mut scratch = Scratch::new(site.config.response_limit);
    call_on(
        site,
        format!("GET {uri} HTTP/1.1\r\nHost: nanacoin.local\r\n{extra}\r\n").as_bytes(),
        secure,
        &mut scratch,
    )
}

#[test]
fn fill_writes_in_place_and_keeps_the_raw_uri() {
    let site = bank();
    let w = get_on(&site, "/api/v1/status?limit=5&x=%20", "", true);
    assert_eq!(w.status, 200);
    assert_eq!(w.body, br#"{"uri":"/api/v1/status?limit=5&x=%20"}"#);
    assert_eq!(w.header("Content-Type"), "application/json");
    let w = get_on(&site, "/api/v1/missing", "", true);
    assert_eq!((w.status, &w.body[..]), (404, &b"{\"error\":40}"[..]));
    // A handler claiming more than the limit is cut to the limit.
    let w = get_on(&site, "/api/v1/whole", "", true);
    assert_eq!(w.body.len(), 4096);
}

#[cfg(feature = "gzip")]
#[test]
fn fill_after_a_gzip_reply_still_sees_initialized_memory() {
    // ?gz=1 swaps the scratch buffers; the next fill must not trust the
    // swapped-in allocation.
    struct Both;
    impl Service for Both {
        fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
            if req.path == "/api/big" {
                reply.wire(
                    req,
                    &Echo {
                        text: "y".repeat(2000),
                        n: 1,
                    },
                );
            } else {
                reply.fill("text/plain", |out| {
                    assert_eq!(out.len(), 4096);
                    out[..2].copy_from_slice(b"ok");
                    (200, 2)
                });
            }
        }
    }
    let mut config = Config::new("test", "board.local");
    config.response_limit = 4096;
    let site = Site::new(config, Both, DesktopPlatform);
    let mut scratch = Scratch::new(4096);
    let gz = call_on(
        &site,
        b"GET /api/big?gz=1 HTTP/1.1\r\nHost: b\r\nAccept-Encoding: gzip\r\n\r\n",
        false,
        &mut scratch,
    );
    assert_eq!(gz.header("Content-Encoding"), "gzip");
    for _ in 0..3 {
        let w = call_on(
            &site,
            b"GET /api/small HTTP/1.1\r\nHost: b\r\n\r\n",
            false,
            &mut scratch,
        );
        assert_eq!(w.body, b"ok");
    }
}

#[test]
fn revalidation_gives_a_content_etag_and_304() {
    let site = bank();
    let first = get_on(&site, "/api/v1/status", "", true);
    let etag = first.header("ETag").to_string();
    assert!(etag.starts_with('"') && etag.len() == 66, "{etag}");
    assert_eq!(first.header("Cache-Control"), "public, no-cache");
    assert_eq!(first.header("X-Generation"), "7");
    for validator in [etag.clone(), format!("\"other\", W/{etag}"), "*".into()] {
        let w = get_on(
            &site,
            "/api/v1/status",
            &format!("If-None-Match: {validator}\r\n"),
            true,
        );
        assert_eq!(w.status, 304, "{validator}");
        assert!(w.body.is_empty());
        assert_eq!(w.header("ETag"), etag);
    }
    // Different bytes, different tag.
    let other = get_on(
        &site,
        "/api/v1/status?changed",
        &format!("If-None-Match: {etag}\r\n"),
        true,
    );
    assert_eq!(other.status, 200);
    assert_ne!(other.header("ETag"), etag);
    // Errors never get a validator.
    let w = get_on(&site, "/api/v1/missing", "If-None-Match: *\r\n", true);
    assert_eq!((w.status, w.header("ETag")), (404, ""));
    assert_eq!(w.header("Cache-Control"), "no-store");
}

#[test]
fn an_app_header_replaces_the_default_instead_of_repeating_it() {
    let site = bank();
    for path in ["/api/v1/me", "/api/v1/status"] {
        let w = get_on(&site, path, "", true);
        let controls: Vec<_> = w
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("Cache-Control"))
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(controls.len(), 1, "{path}: {controls:?}");
        assert_ne!(controls[0], "no-store", "{path}");
    }
}

#[test]
fn cors_lists_come_from_the_config() {
    let site = bank();
    let w = get_on(
        &site,
        "/api/v1/status",
        "Origin: http://localhost:4200\r\n",
        true,
    );
    assert_eq!(
        w.header("Access-Control-Allow-Methods"),
        "GET, POST, PATCH, OPTIONS"
    );
    assert!(w
        .header("Access-Control-Allow-Headers")
        .contains("Idempotency-Key"));
    assert_eq!(
        w.header("Access-Control-Expose-Headers"),
        "X-Generation, Server-Timing, ETag"
    );
    let long = format!("Origin: http://localhost:4200/{}\r\n", "a".repeat(300));
    assert_eq!(get_on(&site, "/api/v1/status", &long, true).status, 403);
}

#[test]
fn https_required_locks_plain_http_down_to_trust_and_metrics() {
    let site = bank();
    // Open: plain HTTP reaches the API.
    assert_eq!(get_on(&site, "/api/v1/status", "", false).status, 200);
    site.service.https_only.store(true, Relaxed);
    for path in [
        "/api/v1/status",
        "/api/v1/sys",
        "/api/v1/log",
        "/.well-known/incidents",
        "/index.html",
        "/x",
    ] {
        let w = get_on(&site, path, "", false);
        assert_eq!(w.status, 403, "{path}");
        assert_eq!(w.header("Cache-Control"), "no-store");
    }
    assert_eq!(get_on(&site, "/", "", false).body, b"<p>trust me</p>");
    assert_eq!(
        get_on(&site, "/trust?x=1", "", false).body,
        b"<p>trust me</p>"
    );
    let ca = get_on(&site, "/ca", "", false);
    assert_eq!(ca.body, b"DER");
    assert_eq!(
        ca.header("Content-Disposition"),
        "attachment; filename=\"NanaCoin-Home-CA.crt\""
    );
    let mut scratch = Scratch::new(4096);
    let post = call_on(
        &site,
        b"POST /trust HTTP/1.1\r\nHost: n\r\nContent-Length: 0\r\n\r\n",
        false,
        &mut scratch,
    );
    assert_eq!(post.status, 405);
    let metrics = get_on(&site, "/metrics", "", false);
    assert_eq!(metrics.status, 200);
    let text = String::from_utf8(metrics.body).unwrap();
    assert!(
        text.starts_with("board,host=nanacoin.local,app=nanacoin,bank=s3 "),
        "{text}"
    );
    // HTTPS is unaffected.
    assert_eq!(get_on(&site, "/api/v1/status", "", true).status, 200);
    let trust = get_on(&site, "/trust", "", true);
    assert!(trust
        .header("Content-Security-Policy")
        .contains("default-src 'none'"));
    assert_eq!(trust.header("Referrer-Policy"), "no-referrer");
}

#[test]
fn listed_spa_routes_make_unknown_paths_404() {
    static ASSETS: &[Asset] = &[Asset {
        path: "/index.html",
        mime: "text/html",
        raw: b"html",
        gzip: b"gz",
        etag: "\"i\"",
        immutable: false,
    }];
    let mut config = Config::new("test", "board.local");
    config.assets = ASSETS;
    config.spa = Spa::Routes(&["", "/market"]);
    let site = Site::new(config, App, DesktopPlatform);
    assert_eq!(get(&site, "/market", "").body, b"html");
    assert_eq!(get(&site, "/secrets", "").status, 404);
    // HEAD of a page: headers, no body.
    let w = call(&site, b"HEAD / HTTP/1.1\r\nHost: board.local\r\n\r\n");
    assert_eq!(w.status, 200);
    assert!(w.body.is_empty());
}

#[test]
fn refused_requests_are_readable_by_an_allowed_origin() {
    let site = bank();
    let raw = b"POST /api/v1/x HTTP/1.1\r\nHost: nanacoin.local\r\nOrigin: http://localhost:4200\r\nContent-Length: 99999\r\n\r\n";
    let w = render(site.refuse(413, raw));
    assert_eq!(w.status, 413);
    assert_eq!(
        w.header("Access-Control-Allow-Origin"),
        "http://localhost:4200"
    );
    assert_eq!(w.header("Connection"), "close");
    let body: ErrorBody = wire::decode(Format::Json, &w.body).unwrap();
    assert_eq!(body.error, "too_large");
    // A stranger, or headers that cannot be read, get no CORS grant.
    let stranger =
        b"POST /api/v1/x HTTP/1.1\r\nHost: nanacoin.local\r\nOrigin: http://evil.example\r\n\r\n";
    assert_eq!(
        render(site.refuse(413, stranger)).header("Access-Control-Allow-Origin"),
        ""
    );
    let broken = render(site.refuse(
        400,
        b"GARBAGE \x01\x02 HTTP/9\r\nOrigin: http://localhost:4200",
    ));
    assert_eq!(broken.status, 400);
    assert_eq!(broken.header("Access-Control-Allow-Origin"), "");
}

#[test]
fn heads_the_parser_rejects_still_get_cors() {
    let site = bank();
    // Too many headers for the parser, but every line is complete.
    let mut many = String::from(
        "GET /api/v1/x HTTP/1.1\r\nHost: nanacoin.local\r\nOrigin: http://localhost:4200\r\n",
    );
    for i in 0..crate::http::MAX_HEADERS {
        many.push_str(&format!("X-H{i}: v\r\n"));
    }
    many.push_str("\r\n");
    let w = render(site.refuse(431, many.as_bytes()));
    assert_eq!(w.status, 431);
    assert_eq!(
        w.header("Access-Control-Allow-Origin"),
        "http://localhost:4200"
    );
    // Cut off at the size limit mid-header: complete lines still count.
    let cut = format!(
        "GET /api/v1/x?q=1 HTTP/1.1\r\nOrigin: http://localhost:4200\r\nHost: nanacoin.local\r\nCookie: {}",
        "a".repeat(5000)
    );
    let w = render(site.refuse(431, cut.as_bytes()));
    assert_eq!(
        w.header("Access-Control-Allow-Origin"),
        "http://localhost:4200"
    );
    // The Origin line itself cut off: no grant.
    let partial =
        b"GET /api/v1/x HTTP/1.1\r\nHost: nanacoin.local\r\nOrigin: http://localhost:4200";
    assert_eq!(
        render(site.refuse(431, partial)).header("Access-Control-Allow-Origin"),
        ""
    );
    // Two Origins are ambiguous: no grant.
    let twice = b"GET /api/v1/x HTTP/1.1\r\nHost: nanacoin.local\r\nOrigin: http://localhost:4200\r\nOrigin: http://evil.example\r\n\r\n";
    assert_eq!(
        render(site.refuse(400, twice)).header("Access-Control-Allow-Origin"),
        ""
    );
}

/// Shaped like Minicloud: files under /blobs, CORS origins from data.
struct Files {
    trusted: Vec<String>,
}

impl Service for Files {
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        match (req.method, req.path) {
            ("GET", "/blobs/photo.jpg") => {
                let data: Vec<u8> = (0..9000u32).map(|i| i as u8).collect();
                reply.stream(
                    200,
                    "image/jpeg",
                    9000,
                    Box::new(std::io::Cursor::new(data)),
                );
                reply.header("ETag", "\"abc\"");
            }
            ("PUT", "/blobs/echo") => {
                let mut bytes = Vec::new();
                req.body_reader().read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes.len(), req.length);
                reply.bytes(201, "application/octet-stream", &bytes);
            }
            ("GET", "/api/status") => reply.text(200, "application/json", "{}"),
            _ => {}
        }
    }
    fn origin_allowed(&self, origin: &str) -> bool {
        self.trusted.iter().any(|o| o == origin)
    }
}

fn files() -> Site<Files> {
    static ASSETS: &[Asset] = &[Asset {
        path: "/index.html",
        mime: "text/html",
        raw: b"",
        gzip: b"gz",
        etag: "\"i\"",
        immutable: false,
    }];
    let mut config = Config::new("minicloud", "minicloud.local");
    config.assets = ASSETS;
    config.app_paths = &["/blobs"];
    Site::new(
        config,
        Files {
            trusted: vec!["https://nanacoin.local".into()],
        },
        DesktopPlatform,
    )
}

#[test]
fn files_stream_from_app_paths_with_their_own_headers() {
    let site = files();
    let w = get_on(&site, "/blobs/photo.jpg", "", false);
    assert_eq!(w.status, 200);
    assert_eq!(w.header("Content-Type"), "image/jpeg");
    assert_eq!(w.header("Content-Length"), "9000");
    assert_eq!(w.header("ETag"), "\"abc\"");
    assert_eq!(w.body.len(), 9000);
    assert_eq!(w.body[8999], (8999u32 as u8));
    // Not the static site: an unknown file there is the app's 404.
    let missing = get_on(&site, "/blobs/nothing.jpg", "", false);
    assert_eq!(missing.status, 404);
    assert_eq!(missing.header("Content-Type"), "application/json");
    // A prefix only matches whole segments.
    assert_eq!(get_on(&site, "/blobsy", "", false).body, b"gz");
}

#[test]
fn a_streamed_request_body_reaches_the_handler_through_a_reader() {
    let site = files();
    let raw = b"PUT /blobs/echo HTTP/1.1\r\nHost: minicloud.local\r\nContent-Length: 5\r\n\r\n";
    // A 1-byte buffer limit makes this body a stream.
    let request = crate::http::parse_streaming(raw, 1, &|_, _| Some(64))
        .unwrap()
        .unwrap();
    assert_eq!(request.streamed, Some(5));
    let mut source: &[u8] = b"hello";
    let mut scratch = Scratch::new(4096);
    let w = render(site.respond_with(&request, false, &mut scratch, Some(&mut source)));
    assert_eq!((w.status, &w.body[..]), (201, &b"hello"[..]));
}

#[test]
fn data_driven_origins_get_cors_and_others_do_not() {
    let site = files();
    let w = get_on(
        &site,
        "/api/status",
        "Origin: https://nanacoin.local\r\n",
        false,
    );
    assert_eq!(
        w.header("Access-Control-Allow-Origin"),
        "https://nanacoin.local"
    );
    let stranger = get_on(
        &site,
        "/api/status",
        "Origin: https://evil.example\r\n",
        false,
    );
    assert_eq!(stranger.status, 403);
}

/// Shaped like mastomini: its own router owns every path, a public API
/// any origin may call, same-origin pages without CORS.
struct Router;

impl Service for Router {
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        match (req.method, req.path) {
            ("OPTIONS", "/api/v1/statuses") => {
                reply.owned(200, "text/plain", Vec::new());
                reply.header("Access-Control-Max-Age", "86400");
            }
            ("GET", "/api/v1/timeline") => {
                // Larger than the response limit: owned bodies are not cut.
                reply.owned(200, "application/json", vec![b'x'; 5000]);
                reply.header("Access-Control-Expose-Headers", "Link");
                reply.header("Link", "<a>; rel=\"next\"");
            }
            ("GET", "/oauth/authorize") => reply.text(200, "text/html", "<form>"),
            ("GET", "/@alice") => reply.text(200, "text/html", "alice"),
            _ => {}
        }
    }
    fn cors(&self, path: &str) -> Cors {
        if path.starts_with("/api/") {
            Cors::Public
        } else {
            Cors::None
        }
    }
    fn metrics(&self) -> Vec<(&'static str, f64)> {
        vec![("store_entries_used", 12.0)]
    }
}

fn router() -> Site<Router> {
    let mut config = Config::new("mastomini", "mastomini.local");
    config.app_paths = &["/"];
    config.response_limit = 1024;
    config.ca_der = Some(b"der");
    config.expose_headers = "Link, Server-Timing";
    Site::new(config, Router, DesktopPlatform)
}

#[test]
fn an_app_path_of_slash_gives_the_app_every_path_after_the_built_ins() {
    let site = router();
    assert_eq!(get_on(&site, "/@alice", "", false).body, b"alice");
    // Built-in pages still answer first.
    assert_eq!(get_on(&site, "/ca", "", false).body, b"der");
    assert!(get_on(&site, "/metrics", "", false)
        .body
        .starts_with(b"board,"));
    // Unknown paths are the app's to refuse (here: the site's API 404).
    assert_eq!(get_on(&site, "/nothing", "", false).status, 404);
}

#[test]
fn public_cors_lets_any_origin_call_and_says_star() {
    let site = router();
    let w = get_on(
        &site,
        "/api/v1/timeline",
        "Origin: https://elk.zone\r\n",
        false,
    );
    assert_eq!(w.status, 200);
    assert_eq!(w.header("Access-Control-Allow-Origin"), "*");
    // The app's value replaces the site's, never repeats it.
    let exposed: Vec<_> = w
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("Access-Control-Expose-Headers"))
        .collect();
    assert_eq!(exposed.len(), 1);
    assert_eq!(exposed[0].1, "Link");
    assert_eq!(w.body.len(), 5000, "an owned body is sent whole");
    // Without an Origin header too (the board probe asks that way).
    assert_eq!(
        get_on(&site, "/api/v1/timeline", "", false).header("Access-Control-Allow-Origin"),
        "*"
    );
}

#[test]
fn public_preflights_go_to_the_app_then_default_to_204() {
    let site = router();
    let mut scratch = Scratch::new(1024);
    let answered = call_on(
        &site,
        b"OPTIONS /api/v1/statuses HTTP/1.1\r\nHost: x\r\nOrigin: https://elk.zone\r\n\r\n",
        false,
        &mut scratch,
    );
    assert_eq!(answered.status, 200);
    assert_eq!(answered.header("Access-Control-Max-Age"), "86400");
    assert_eq!(answered.header("Access-Control-Allow-Origin"), "*");
    let defaulted = call_on(
        &site,
        b"OPTIONS /api/v1/other HTTP/1.1\r\nHost: x\r\nOrigin: https://elk.zone\r\n\r\n",
        false,
        &mut scratch,
    );
    assert_eq!(defaulted.status, 204);
    assert_eq!(defaulted.header("Access-Control-Allow-Origin"), "*");
}

#[test]
fn no_cors_pages_neither_grant_nor_refuse_origins() {
    let site = router();
    let w = get_on(
        &site,
        "/oauth/authorize",
        "Origin: https://evil.example\r\n",
        false,
    );
    assert_eq!(w.status, 200, "not refused: the app decides");
    assert_eq!(w.header("Access-Control-Allow-Origin"), "");
    let mut scratch = Scratch::new(1024);
    let options = call_on(
        &site,
        b"OPTIONS /oauth/authorize HTTP/1.1\r\nHost: x\r\n\r\n",
        false,
        &mut scratch,
    );
    assert_eq!(options.status, 404, "OPTIONS is the app's to answer");
}

#[test]
fn refusals_on_public_paths_are_readable_by_any_origin() {
    let site = router();
    let raw = b"POST /api/v1/media HTTP/1.1\r\nHost: x\r\nOrigin: https://elk.zone\r\nContent-Length: 99999\r\n\r\n";
    let w = render(site.refuse(413, raw));
    assert_eq!(w.header("Access-Control-Allow-Origin"), "*");
    let page = b"POST /oauth/authorize HTTP/1.1\r\nHost: x\r\nOrigin: https://elk.zone\r\nContent-Length: 99999\r\n\r\n";
    assert_eq!(
        render(site.refuse(413, page)).header("Access-Control-Allow-Origin"),
        ""
    );
}

#[test]
fn metrics_carry_the_apps_fields() {
    let site = router();
    let w = get_on(&site, "/metrics", "", false);
    let line = String::from_utf8(w.body).unwrap();
    assert!(line.contains(",store_entries_used=12"), "{line}");
}
