//! Outgoing HTTP GET, for scraping other boards. The board implementation
//! (esp-idf's client, HTTPS against the household CA) is in `esp`; this
//! module has the trait and a plain-HTTP desktop client.
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Fetched {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

pub trait Fetch: Send + Sync + 'static {
    /// GET `url`, reading at most `limit` body bytes.
    fn get(&self, url: &str, accept: &str, limit: usize) -> io::Result<Fetched>;
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

/// Plain HTTP over `std::net` (desktop; also fine on a board for http://).
pub struct PlainFetch {
    pub timeout: Duration,
}

impl Default for PlainFetch {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(3),
        }
    }
}

impl Fetch for PlainFetch {
    fn get(&self, url: &str, accept: &str, limit: usize) -> io::Result<Fetched> {
        let (secure, host, port, path) = split_url(url)?;
        if secure {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "https scraping needs the board client",
            ));
        }
        let addr = (host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "host not found"))?;
        let mut stream = TcpStream::connect_timeout(&addr, self.timeout)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: {accept}\r\nAccept-Encoding: identity\r\nUser-Agent: miniframework\r\nConnection: close\r\n\r\n"
        )?;
        let mut raw = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = stream.read(&mut buf)?;
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
        };
        let reply = fetch
            .get(&format!("http://{address}/metrics"), "text/plain", 100)
            .unwrap();
        assert_eq!(reply.body, b"x=1\n");
        release.send(()).unwrap();
        worker.join().unwrap();
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
