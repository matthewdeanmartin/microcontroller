//! Bounded HTTP/1.1 framing for the nonblocking connection loop (from
//! NanaCoin's board transport). A partial request never blocks another
//! client. Invalid framing always closes the connection, so unread bytes
//! cannot become a second, ambiguous request.
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

/// Release stalled readers without truncating transfers that keep progressing.
pub const WRITE_STALL_TIMEOUT: Duration = Duration::from_secs(15);
// AES may need internal DMA bounce buffers even when TLS lives in PSRAM.
// Bound every write, including the coalesced HTTP prefix, below IDF's 1600-byte
// AES bounce chunk and leave each other client a turn between records.
pub const WRITE_CHUNK_LIMIT: usize = 1024;
/// Bytes one client may send per turn of the connection loop. With 1 KiB
/// per turn the board was capped near 60 KB/s (a loop turn costs a FreeRTOS
/// tick); 16 KiB keeps other clients' turns short while filling the link.
pub const TURN_BUDGET: usize = 16 * 1024;

pub const HEADER_LIMIT: usize = 4096;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub consumed: usize,
    pub close: bool,
}

impl Request {
    pub fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .unwrap_or("")
    }
}

pub fn parse(input: &[u8], body_limit: usize) -> Result<Option<Request>, u16> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut parsed = httparse::Request::new(&mut headers);
    let start = match parsed.parse(input).map_err(|_| 400u16)? {
        httparse::Status::Partial if input.len() >= HEADER_LIMIT => return Err(431),
        httparse::Status::Partial => return Ok(None),
        httparse::Status::Complete(n) if n > HEADER_LIMIT => return Err(431),
        httparse::Status::Complete(n) => n,
    };
    let method = parsed.method.ok_or(400u16)?;
    let uri = parsed.path.ok_or(400u16)?;
    if uri.len() > 1024 {
        return Err(414);
    }
    if !uri.starts_with('/') || uri.starts_with("//") {
        return Err(400);
    }
    let mut length = None;
    let mut host = false;
    let mut close = parsed.version != Some(1);
    for (index, header) in parsed.headers.iter().enumerate() {
        // Do not let different layers interpret duplicate security/framing
        // headers differently. Repeated headers are unnecessary for this API.
        if parsed.headers[..index]
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case(header.name))
        {
            return Err(400);
        }
        let value = std::str::from_utf8(header.value)
            .map_err(|_| 400u16)?
            .trim();
        if value.bytes().any(|b| b < 32 && b != b'\t') || value.contains('\x7f') {
            return Err(400);
        }
        if header.name.eq_ignore_ascii_case("Transfer-Encoding") {
            return Err(400);
        }
        if header.name.eq_ignore_ascii_case("Expect") {
            return Err(417);
        }
        if header.name.eq_ignore_ascii_case("Host") {
            host = !value.is_empty();
        }
        if header.name.eq_ignore_ascii_case("Connection")
            && value
                .split(',')
                .any(|v| v.trim().eq_ignore_ascii_case("close"))
        {
            close = true;
        }
        if header.name.eq_ignore_ascii_case("Content-Length") {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(400);
            }
            let n = value.parse::<usize>().map_err(|_| 413u16)?;
            if n > body_limit {
                return Err(413);
            }
            length = Some(n);
        }
    }
    if parsed.version == Some(1) && !host {
        return Err(400);
    }
    if matches!(method, "POST" | "PATCH" | "PUT") && length.is_none() {
        return Err(411);
    }
    let end = start + length.unwrap_or(0);
    if input.len() < end {
        return Ok(None);
    }
    Ok(Some(Request {
        method: method.to_owned(),
        uri: uri.to_owned(),
        headers: parsed
            .headers
            .iter()
            .map(|h| {
                (
                    h.name.to_owned(),
                    std::str::from_utf8(h.value).unwrap().trim().to_owned(),
                )
            })
            .collect(),
        body: input[start..end].to_vec(),
        consumed: end,
        close,
    }))
}

pub enum Body {
    Owned(Vec<u8>),
    Flash(&'static [u8]),
}

impl AsRef<[u8]> for Body {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Owned(b) => b,
            Self::Flash(b) => b,
        }
    }
}

/// Coalesce headers and small bodies into one write, retain large flash assets
/// by reference, and allow partial writes to resume on a later loop iteration.
pub struct Response {
    prefix: Vec<u8>,
    body: Body,
    skip: usize,
    sent: usize,
    last_progress: Instant,
    pub close: bool,
    pub status: u16,
}

impl Response {
    pub fn new(status: u16, headers: &[(&str, &str)], body: Body, head: bool, close: bool) -> Self {
        let mut prefix = format!("HTTP/1.1 {status} {}\r\n", reason(status)).into_bytes();
        for &(name, value) in headers {
            if name.eq_ignore_ascii_case("Content-Length")
                || name.eq_ignore_ascii_case("Transfer-Encoding")
                || name.eq_ignore_ascii_case("Connection")
                || name.contains(['\r', '\n'])
                || value.contains(['\r', '\n'])
            {
                continue;
            }
            prefix.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        // 304 has no message body; omit Content-Length (which otherwise would
        // have to describe the selected representation, not this empty reply).
        if status != 304 {
            prefix.extend_from_slice(
                format!("Content-Length: {}\r\n", body.as_ref().len()).as_bytes(),
            );
        }
        if close {
            prefix.extend_from_slice(b"Connection: close\r\n");
        }
        prefix.extend_from_slice(b"\r\n");
        let body = if head || status == 304 {
            Body::Flash(b"")
        } else {
            body
        };
        let skip = body.as_ref().len().min(WRITE_CHUNK_LIMIT);
        prefix.extend_from_slice(&body.as_ref()[..skip]);
        Self {
            prefix,
            body,
            skip,
            sent: 0,
            last_progress: Instant::now(),
            close,
            status,
        }
    }

    /// Nonblocking writes of up to [`TURN_BUDGET`] bytes (in
    /// [`WRITE_CHUNK_LIMIT`] chunks) per turn. The same slice is retried after
    /// WouldBlock (required by TLS); only accepted bytes advance the cursor.
    /// `now` is supplied by the caller so stalled/slow peers can be tested
    /// without sleeping. Returns true only when the complete reply was sent.
    pub fn send(&mut self, writer: &mut impl Write, now: Instant) -> io::Result<bool> {
        if self.next().is_empty() {
            return Ok(true);
        }
        if now.saturating_duration_since(self.last_progress) >= WRITE_STALL_TIMEOUT {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let mut budget = TURN_BUDGET;
        while budget > 0 && !self.next().is_empty() {
            match writer.write(self.next()) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    self.advance(n);
                    self.last_progress = now;
                    budget = budget.saturating_sub(n);
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    break
                }
                Err(e) => return Err(e),
            }
        }
        Ok(self.next().is_empty())
    }

    pub fn next(&self) -> &[u8] {
        if self.sent < self.prefix.len() {
            let rest = &self.prefix[self.sent..];
            &rest[..rest.len().min(WRITE_CHUNK_LIMIT)]
        } else {
            let rest = &self.body.as_ref()[self.skip + self.sent - self.prefix.len()..];
            &rest[..rest.len().min(WRITE_CHUNK_LIMIT)]
        }
    }

    pub fn advance(&mut self, count: usize) {
        self.sent += count;
    }

    pub fn retained_bytes(&self) -> usize {
        self.prefix.len()
            + match &self.body {
                Body::Owned(bytes) => bytes.len(),
                Body::Flash(_) => 0,
            }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        201 => "Created",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        417 => "Expectation Failed",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_body_and_pipelining_have_exact_boundaries() {
        let first =
            b"POST /api/v1/test HTTP/1.1\r\nHost: board.local\r\nContent-Length: 2\r\n\r\n{}";
        for end in 0..first.len() {
            assert!(parse(&first[..end], 1024).unwrap().is_none());
        }
        let mut data = first.to_vec();
        data.extend_from_slice(
            b"GET /api/v1/status HTTP/1.1\r\nHost: board.local\r\nConnection: close\r\n\r\n",
        );
        let a = parse(&data, 1024).unwrap().unwrap();
        assert_eq!(a.body, b"{}");
        assert_eq!(a.consumed, first.len());
        assert!(!a.close);
        let b = parse(&data[a.consumed..], 1024).unwrap().unwrap();
        assert!(b.close);
        assert_eq!(b.uri, "/api/v1/status");
    }

    #[test]
    fn reject_ambiguous_and_oversized_framing_before_body() {
        for extra in [
            "Content-Length: 0\r\nContent-Length: 1\r\n",
            "Transfer-Encoding: chunked\r\n",
            "Content-Length: +1\r\n",
            "Host: other\r\n",
        ] {
            assert_eq!(
                parse(
                    format!("GET / HTTP/1.1\r\nHost: n\r\n{extra}\r\n").as_bytes(),
                    1024
                )
                .unwrap_err(),
                400
            );
        }
        assert_eq!(
            parse(
                b"POST / HTTP/1.1\r\nHost: n\r\nContent-Length: 1025\r\n\r\n",
                1024
            )
            .unwrap_err(),
            413
        );
        assert_eq!(
            parse(b"POST / HTTP/1.1\r\nHost: n\r\n\r\n", 1024).unwrap_err(),
            411
        );
        assert_eq!(parse(b"GET / HTTP/1.1\r\n\r\n", 1024).unwrap_err(), 400);
        let huge = format!(
            "GET / HTTP/1.1\r\nHost: n\r\nX-Pad: {}",
            "x".repeat(HEADER_LIMIT)
        );
        assert_eq!(parse(huge.as_bytes(), 1024).unwrap_err(), 431);
    }

    fn wire(mut reply: Response) -> Vec<u8> {
        let mut out = Vec::new();
        while !reply.next().is_empty() {
            let count = reply.next().len().min(37);
            out.extend_from_slice(&reply.next()[..count]);
            reply.advance(count);
        }
        out
    }

    #[test]
    fn persistent_response_handles_partial_writes_and_large_bodies() {
        let body = vec![b'x'; 24000];
        let out = wire(Response::new(
            200,
            &[("Transfer-Encoding", "chunked"), ("Bad", "a\r\nb")],
            Body::Owned(body.clone()),
            false,
            false,
        ));
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let headers = std::str::from_utf8(&out[..split]).unwrap();
        assert!(headers.contains("Content-Length: 24000\r\n"));
        assert!(!headers.contains("Connection: close"));
        assert!(!headers.contains("Transfer-Encoding"));
        assert!(!headers.contains("Bad:"));
        assert_eq!(&out[split..], body);
    }

    struct SlowWriter {
        bytes: Vec<u8>,
        limit: usize,
        calls: usize,
        retry: Option<(usize, usize)>,
    }

    impl Write for SlowWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            assert!(bytes.len() <= WRITE_CHUNK_LIMIT);
            let slice = (bytes.as_ptr() as usize, bytes.len());
            if let Some(retry) = self.retry.take() {
                assert_eq!(slice, retry, "TLS retries must preserve the input slice");
            } else if self.calls.is_multiple_of(3) {
                self.calls += 1;
                self.retry = Some(slice);
                return Err(io::ErrorKind::WouldBlock.into());
            }
            self.calls += 1;
            let n = self.limit.min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn concurrent_large_responses_survive_slow_partial_writes_and_tls_retries() {
        let body: Vec<u8> = (0..273_005).map(|i| (i % 251) as u8).collect();
        let mut peers: Vec<_> = [113, 4096, 8192]
            .into_iter()
            .map(|limit| {
                (
                    Response::new(200, &[], Body::Owned(body.clone()), false, false),
                    SlowWriter {
                        bytes: Vec::new(),
                        limit,
                        calls: 0,
                        retry: None,
                    },
                    false,
                )
            })
            .collect();
        let start = Instant::now();
        let mut now = start;
        // Simulate a multiplexed loop: slow/WouldBlock clients yield to peers.
        while peers.iter().any(|(_, _, done)| !done) {
            for (response, writer, done) in &mut peers {
                if !*done {
                    *done = response.send(writer, now).unwrap();
                }
            }
            now += Duration::from_millis(100);
            assert!(now.duration_since(start) < Duration::from_secs(600));
        }
        assert!(now.duration_since(start) > Duration::from_secs(5));
        for (_, writer, _) in peers {
            let split = writer
                .bytes
                .windows(4)
                .position(|w| w == [13, 10, 13, 10])
                .unwrap()
                + 4;
            assert_eq!(&writer.bytes[split..], &body);
        }
    }

    #[test]
    fn stalled_response_expires_from_last_progress_not_start_or_retry() {
        /// Accepts `quota` bytes, then WouldBlock until topped up.
        struct Gate {
            quota: usize,
        }
        impl Write for Gate {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.quota == 0 {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let n = self.quota.min(bytes.len());
                self.quota -= n;
                Ok(n)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut response = Response::new(200, &[], Body::Flash(b"hello"), false, false);
        let start = response.last_progress;
        let mut gate = Gate { quota: 0 };
        // Blocked turns do not reset the clock...
        assert!(!response.send(&mut gate, start).unwrap());
        let progress = start + Duration::from_secs(14);
        gate.quota = 1;
        // ...progress does, and several turns of no progress are fine...
        assert!(!response.send(&mut gate, progress).unwrap());
        assert!(!response
            .send(&mut gate, progress + Duration::from_secs(14))
            .unwrap());
        // ...until the stall timeout passes since the last progress.
        assert_eq!(
            response
                .send(&mut gate, progress + WRITE_STALL_TIMEOUT)
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut,
        );
    }

    #[test]
    fn one_turn_sends_up_to_the_budget() {
        let body = vec![b'x'; 3 * TURN_BUDGET];
        let mut response = Response::new(200, &[], Body::Owned(body), false, false);
        let mut out = Vec::new();
        assert!(!response.send(&mut out, Instant::now()).unwrap());
        assert!(out.len() >= TURN_BUDGET && out.len() < TURN_BUDGET + WRITE_CHUNK_LIMIT);
        while !response.send(&mut out, Instant::now()).unwrap() {}
        assert!(out.ends_with(&[b'x'; 100]));
    }

    #[test]
    fn disconnected_writer_does_not_report_a_complete_response() {
        let mut response = Response::new(200, &[], Body::Flash(b"hello"), false, true);
        let mut writer = SlowWriter {
            bytes: Vec::new(),
            limit: 0,
            calls: 1,
            retry: None,
        };
        assert_eq!(
            response
                .send(&mut writer, Instant::now())
                .unwrap_err()
                .kind(),
            io::ErrorKind::WriteZero
        );
    }

    #[test]
    fn head_and_not_modified_have_no_body() {
        let head = wire(Response::new(200, &[], Body::Flash(b"hello"), true, false));
        assert!(head.ends_with(b"Content-Length: 5\r\n\r\n"));
        let cached = wire(Response::new(304, &[], Body::Flash(b"hello"), false, false));
        assert_eq!(cached, b"HTTP/1.1 304 Not Modified\r\n\r\n");
    }
}
