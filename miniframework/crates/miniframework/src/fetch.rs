//! Outgoing HTTP: GET for scraping other boards, and [`Fetch::send`] for
//! anything else (OAuth token exchange, APIs with a bearer token). The board
//! implementation (esp-idf's client, HTTPS against the household CA or the
//! public bundle) is in `esp`; this module has the trait and the desktop
//! client: plain HTTP always, HTTPS with the `desktop-tls` feature.
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Fetched {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// A request beyond a plain GET.
#[derive(Debug, Clone, Copy)]
pub struct Outgoing<'a> {
    pub method: &'a str,
    pub url: &'a str,
    /// Extra headers (`Authorization`, `Content-Type`, `Accept`...).
    pub headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
    /// Most response body bytes accepted.
    pub limit: usize,
}

pub trait Fetch: Send + Sync + 'static {
    /// GET `url`, reading at most `limit` body bytes.
    fn get(&self, url: &str, accept: &str, limit: usize) -> io::Result<Fetched>;

    /// Any method, headers and body. Clients that only scrape need not
    /// implement it.
    fn send(&self, request: &Outgoing<'_>) -> io::Result<Fetched> {
        let _ = request;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this HTTP client only does GET",
        ))
    }
}

/// `http://host[:port]/path` → (host, port, path).
pub fn split_url(url: &str) -> io::Result<(bool, String, u16, String)> {
    let bad = || io::Error::new(io::ErrorKind::InvalidInput, format!("bad URL {url:?}"));
    let (secure, rest) = if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else {
        return Err(bad());
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| bad())?),
        None => (authority, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(bad());
    }
    Ok((secure, host.to_string(), port, path.to_string()))
}

/// The desktop client: plain HTTP over `std::net`, and HTTPS (rustls,
/// public web roots plus `roots`) with the `desktop-tls` feature.
pub struct PlainFetch {
    pub timeout: Duration,
    /// Extra trusted CA certificates (DER), e.g. the household CA.
    pub roots: Vec<Vec<u8>>,
}

impl Default for PlainFetch {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(3),
            roots: Vec::new(),
        }
    }
}

impl Fetch for PlainFetch {
    fn get(&self, url: &str, accept: &str, limit: usize) -> io::Result<Fetched> {
        self.send(&Outgoing {
            method: "GET",
            url,
            headers: &[("Accept", accept)],
            body: &[],
            limit,
        })
    }

    fn send(&self, request: &Outgoing<'_>) -> io::Result<Fetched> {
        let (secure, host, port, path) = split_url(request.url)?;
        let addr = (host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "host not found"))?;
        let mut stream = TcpStream::connect_timeout(&addr, self.timeout)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        if !secure {
            return exchange(&mut stream, &host, port, &path, request);
        }
        #[cfg(feature = "desktop-tls")]
        {
            let mut tls = desktop_client::connect(&host, stream, &self.roots)?;
            exchange(&mut tls, &host, port, &path, request)
        }
        #[cfg(not(feature = "desktop-tls"))]
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "https on the desktop needs the `desktop-tls` feature",
        ))
    }
}

/// HTTPS client connections for the desktop (rustls, ring).
#[cfg(feature = "desktop-tls")]
mod desktop_client {
    use std::io;
    use std::net::TcpStream;
    use std::sync::Arc;

    pub fn connect(
        host: &str,
        tcp: TcpStream,
        extra: &[Vec<u8>],
    ) -> io::Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for der in extra {
            roots
                .add(rustls::pki_types::CertificateDer::from(der.clone()))
                .map_err(|e| io::Error::other(format!("extra root: {e}")))?;
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let connection =
            rustls::ClientConnection::new(Arc::new(config), name).map_err(io::Error::other)?;
        Ok(rustls::StreamOwned::new(connection, tcp))
    }
}

/// Writes `request` and reads the whole response (Content-Length, chunked
/// or read-to-close) on any stream.
fn exchange<S: Read + Write>(
    stream: &mut S,
    host: &str,
    port: u16,
    path: &str,
    request: &Outgoing<'_>,
) -> io::Result<Fetched> {
    let authority = if port == 80 || port == 443 {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };
    let mut head = format!(
        "{} {path} HTTP/1.1\r\nHost: {authority}\r\nAccept-Encoding: identity\r\nUser-Agent: miniframework\r\nConnection: close\r\n",
        request.method
    );
    for (name, value) in request.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if !request.body.is_empty() || matches!(request.method, "POST" | "PUT" | "PATCH") {
        head.push_str(&format!("Content-Length: {}\r\n", request.body.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(request.body)?;
    stream.flush()?;
    let limit = request.limit;
    let mut raw = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = match stream.read(&mut buf) {
            Ok(n) => n,
            // rustls reports a peer that closed without close_notify.
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => 0,
            Err(e) => return Err(e),
        };
        if n == 0 {
            return parse_response(&raw, limit);
        }
        if raw.len() + n > limit + 8192 {
            return Err(io::Error::other("response body or headers too large"));
        }
        raw.extend_from_slice(&buf[..n]);
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut response = httparse::Response::new(&mut headers);
        let httparse::Status::Complete(start) = response
            .parse(&raw)
            .map_err(|_| io::Error::other("bad HTTP response"))?
        else {
            continue;
        };
        let chunked = response.headers.iter().any(|h| {
            h.name.eq_ignore_ascii_case("Transfer-Encoding")
                && h.value.eq_ignore_ascii_case(b"chunked")
        });
        if chunked {
            if let Ok(response) = parse_response(&raw, limit) {
                return Ok(response);
            }
        } else if let Some(length) = response
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("Content-Length"))
        {
            let length: usize = std::str::from_utf8(length.value)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .ok_or_else(|| io::Error::other("bad Content-Length"))?;
            if length > limit {
                return Err(io::Error::other("response body too large"));
            }
            if raw.len() - start >= length {
                return parse_response(&raw, limit);
            }
        }
    }
}

/// `application/x-www-form-urlencoded` body from pairs.
pub fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encodes everything but RFC 3986 unreserved characters.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Parses a complete HTTP/1.1 response (Content-Length, chunked, or
/// read-to-close).
pub fn parse_response(raw: &[u8], limit: usize) -> io::Result<Fetched> {
    let invalid = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_string());
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut response = httparse::Response::new(&mut headers);
    let start = match response
        .parse(raw)
        .map_err(|_| invalid("bad HTTP response"))?
    {
        httparse::Status::Complete(n) => n,
        httparse::Status::Partial => return Err(invalid("truncated HTTP response")),
    };
    let header = |name: &str| {
        response
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| String::from_utf8_lossy(h.value).trim().to_string())
            .unwrap_or_default()
    };
    let content_type = header("Content-Type");
    let rest = &raw[start..];
    let body = if header("Transfer-Encoding").eq_ignore_ascii_case("chunked") {
        let mut body = Vec::new();
        let mut pos = 0;
        loop {
            let line_end = rest[pos..]
                .windows(2)
                .position(|w| w == b"\r\n")
                .ok_or_else(|| invalid("bad chunk"))?;
            let size_text = std::str::from_utf8(&rest[pos..pos + line_end]).unwrap_or("");
            let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
                .map_err(|_| invalid("bad chunk size"))?;
            pos += line_end + 2;
            if size == 0 {
                break;
            }
            let chunk = rest
                .get(pos..pos + size)
                .ok_or_else(|| invalid("truncated chunk"))?;
            body.extend_from_slice(chunk);
            pos += size + 2;
        }
        body
    } else if let Ok(n) = header("Content-Length").parse::<usize>() {
        rest.get(..n)
            .ok_or_else(|| invalid("truncated body"))?
            .to_vec()
    } else {
        rest.to_vec()
    };
    if body.len() > limit {
        return Err(invalid("response body too large"));
    }
    Ok(Fetched {
        status: response.code.unwrap_or(0),
        content_type,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_body_does_not_wait_for_a_keepalive_peer_to_close() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release, released) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let n = socket.read(&mut request).unwrap();
            assert!(n > 0);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nx=1\n").unwrap();
            let _ = released.recv_timeout(Duration::from_secs(2));
        });
        let fetch = PlainFetch {
            timeout: Duration::from_millis(200),
            ..PlainFetch::default()
        };
        let reply = fetch
            .get(&format!("http://{address}/metrics"), "text/plain", 100)
            .unwrap();
        assert_eq!(reply.body, b"x=1\n");
        release.send(()).unwrap();
        worker.join().unwrap();
    }

    /// `MF_LIVE=1 cargo test --features desktop-tls live_https`: a real
    /// public HTTPS GET (read-only discovery document).
    #[cfg(feature = "desktop-tls")]
    #[test]
    fn live_https() {
        if std::env::var_os("MF_LIVE").is_none() {
            return;
        }
        let fetch = PlainFetch {
            timeout: Duration::from_secs(10),
            ..PlainFetch::default()
        };
        let r = fetch
            .get(
                "https://mastodon.social/.well-known/oauth-authorization-server",
                "application/json",
                64 * 1024,
            )
            .unwrap();
        assert_eq!(r.status, 200);
        assert!(String::from_utf8_lossy(&r.body).contains("token_endpoint"));
    }

    #[test]
    fn form_encoding() {
        assert_eq!(
            form(&[
                ("redirect_uri", "http://x.local/a b"),
                ("scope", "read:accounts")
            ]),
            "redirect_uri=http%3A%2F%2Fx.local%2Fa%20b&scope=read%3Aaccounts"
        );
    }

    #[test]
    fn post_with_headers_and_body() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            // Head and body may arrive in separate reads.
            let mut text = String::new();
            let mut buf = [0u8; 2048];
            while !text.ends_with("a=1") {
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0, "client closed early: {text}");
                text.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            socket
                .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
            text
        });
        let reply = PlainFetch::default()
            .send(&Outgoing {
                method: "POST",
                url: &format!("http://{address}/oauth/token"),
                headers: &[
                    ("Content-Type", "application/x-www-form-urlencoded"),
                    ("Authorization", "Bearer t"),
                ],
                body: b"a=1",
                limit: 100,
            })
            .unwrap();
        assert_eq!((reply.status, &reply.body[..]), (201, &b"ok"[..]));
        let sent = worker.join().unwrap();
        assert!(sent.starts_with("POST /oauth/token HTTP/1.1\r\n"), "{sent}");
        assert!(
            sent.contains("Content-Length: 3\r\n") && sent.contains("Authorization: Bearer t\r\n")
        );
        assert!(sent.ends_with("\r\n\r\na=1"), "{sent}");
    }

    #[test]
    fn urls_and_responses() {
        assert_eq!(
            split_url("http://nanacoin-s2.local/api/v1/diag").unwrap(),
            (false, "nanacoin-s2.local".into(), 80, "/api/v1/diag".into())
        );
        assert_eq!(split_url("https://h:8443").unwrap().2, 8443);
        assert!(split_url("ftp://x").is_err());
        let r = parse_response(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nab=1\r\n3\r\n x2\r\n0\r\n\r\n",
            100,
        )
        .unwrap();
        assert_eq!(r.body, b"ab=1 x2");
        assert_eq!(r.content_type, "text/plain");
        let r = parse_response(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\n\r\nnoEXTRA",
            100,
        )
        .unwrap();
        assert_eq!((r.status, &r.body[..]), (404, &b"no"[..]));
    }
}
