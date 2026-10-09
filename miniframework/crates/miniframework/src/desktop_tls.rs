//! HTTPS beside HTTP on the desktop runner (`desktop-tls`), with rustls in
//! place of the boards' esp-tls. The connection loop is the same one: a
//! TLS connection is just another [`Conn`] whose bytes rustls decrypts.
//! With `http2`, ALPN offers `h2` as the boards do.
use crate::mux::{Conn, Limits, Mux};
use crate::site::{Service, Site};
use rustls::{ServerConfig, ServerConnection};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The HTTPS side: where to listen, and the certificate chain and key
/// (PEM, as `tools/certs.sh` writes them).
pub struct Https {
    pub address: String,
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

impl Https {
    fn config(&self) -> io::Result<Arc<ServerConfig>> {
        let invalid = |e: String| io::Error::new(io::ErrorKind::InvalidData, e);
        let certs = rustls_pemfile::certs(&mut &self.cert_pem[..])
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| invalid(format!("certificate: {e}")))?;
        let key = rustls_pemfile::private_key(&mut &self.key_pem[..])
            .map_err(|e| invalid(format!("key: {e}")))?
            .ok_or_else(|| invalid("no private key in the key file".into()))?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| invalid(e.to_string()))?
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| invalid(e.to_string()))?;
        #[cfg(feature = "http2")]
        config.alpn_protocols.push(b"h2".to_vec());
        config.alpn_protocols.push(b"http/1.1".to_vec());
        Ok(Arc::new(config))
    }
}

/// A desktop connection: plain TCP, or TLS over TCP.
pub enum DesktopConn {
    Plain(TcpStream),
    Tls(Box<TlsConn>),
}

pub struct TlsConn {
    tls: ServerConnection,
    tcp: TcpStream,
}

impl TlsConn {
    /// Sends what rustls has queued; WouldBlock when the socket is full.
    fn flush_tls(&mut self) -> io::Result<()> {
        while self.tls.wants_write() {
            self.tls.write_tls(&mut self.tcp)?;
        }
        Ok(())
    }
}

impl Read for TlsConn {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.flush_tls() {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
            match self.tls.reader().read(out) {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                // The peer closed without close_notify: the end, as for TCP.
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(0),
                Err(e) => return Err(e),
            }
            if self.tls.read_tls(&mut self.tcp)? == 0 {
                return Ok(0);
            }
            if let Err(e) = self.tls.process_new_packets() {
                let _ = self.flush_tls(); // the alert, if any
                return Err(io::Error::new(io::ErrorKind::InvalidData, e));
            }
        }
    }
}

impl Write for TlsConn {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // Older ciphertext first, so rustls's buffer stays small.
        self.flush_tls()?;
        if self.tls.is_handshaking() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = self.tls.writer().write(bytes)?;
        match self.flush_tls() {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flush_tls()
    }
}

impl Drop for TlsConn {
    fn drop(&mut self) {
        self.tls.send_close_notify();
        let _ = self.flush_tls();
    }
}

impl Read for DesktopConn {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.read(out),
            Self::Tls(tls) => tls.read(out),
        }
    }
}

impl Write for DesktopConn {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.write(bytes),
            Self::Tls(tls) => tls.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(tcp) => tcp.flush(),
            Self::Tls(tls) => tls.flush(),
        }
    }
}

impl Conn for DesktopConn {
    fn secure(&self) -> bool {
        matches!(self, Self::Tls(_))
    }
    fn h2(&self) -> bool {
        match self {
            Self::Tls(tls) => tls.tls.alpn_protocol() == Some(b"h2"),
            Self::Plain(_) => false,
        }
    }
}

/// [`super::serve`] with HTTPS as well (when `https` is given). The TLS
/// handshake runs inside the connection loop: on a PC it takes about a
/// millisecond, so it needs no task of its own.
pub fn serve_https<S: Service>(
    site: &Site<S>,
    address: &str,
    https: Option<&Https>,
    mut tick: impl FnMut(),
) -> io::Result<()> {
    crate::uptime_ms();
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    log::info!("{} listening on http://{address}", site.config.app);
    let secure = match https {
        Some(https) => {
            let config = https.config()?;
            let listener = TcpListener::bind(&https.address)?;
            listener.set_nonblocking(true)?;
            log::info!("{} listening on https://{}", site.config.app, https.address);
            Some((listener, config))
        }
        None => None,
    };
    crate::incidents::start_volatile();
    let mut limits = Limits::desktop();
    limits.tls_clients = 64;
    let mut mux: Mux<DesktopConn> =
        Mux::new(limits, site.config.body_limit, site.config.response_limit);
    let mut last_busy = Instant::now();
    loop {
        let mut busy = false;
        while mux.has_room(false) {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            stream.set_nonblocking(true)?;
            stream.set_nodelay(true)?;
            mux.add(DesktopConn::Plain(stream));
            busy = true;
        }
        if let Some((listener, config)) = &secure {
            while mux.has_room(true) {
                let Ok((stream, _)) = listener.accept() else {
                    break;
                };
                stream.set_nonblocking(true)?;
                stream.set_nodelay(true)?;
                let tls = ServerConnection::new(Arc::clone(config)).map_err(io::Error::other)?;
                mux.add(DesktopConn::Tls(Box::new(TlsConn { tls, tcp: stream })));
                busy = true;
            }
        }
        busy |= mux.turn(site);
        tick();
        if busy {
            last_busy = Instant::now();
        } else if last_busy.elapsed() < Duration::from_millis(250) {
            std::thread::yield_now();
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
