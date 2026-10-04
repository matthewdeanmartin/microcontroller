//! Run a site on a PC: the same pipeline and connection loop as the board,
//! over plain HTTP. Use it with `ng serve` (proxy) while developing.
use crate::mux::{Limits, Mux};
use crate::site::{Service, Site};
use crate::sys::{Platform, SysInfo};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// Desktop "hardware": what the OS will tell us without extra crates.
pub struct DesktopPlatform;

impl Platform for DesktopPlatform {
    fn sysinfo(&self) -> SysInfo {
        SysInfo {
            platform: format!("{} desktop", std::env::consts::OS),
            chip: std::env::consts::ARCH.into(),
            cores: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            reset_reason: "process start".into(),
            sdk: format!("Rust std ({})", env!("CARGO_PKG_VERSION")),
            ..Default::default()
        }
    }
}

/// Serves until the process ends. `tick` runs between connection-loop
/// turns (about every millisecond when idle); keep it short.
pub fn serve<S: Service>(
    site: &Site<S>,
    address: &str,
    mut tick: impl FnMut(),
) -> std::io::Result<()> {
    crate::uptime_ms();
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    log::info!("{} listening on http://{address}", site.config.app);
    let mut mux: Mux<TcpStream> = Mux::new(
        Limits::desktop(),
        site.config.body_limit,
        site.config.response_limit,
    );
    // Windows rounds a 1 ms sleep up to its ~15 ms timer tick, which would
    // add that much to every new connection. Spin (yielding) for a moment
    // after activity, and only sleep once things have gone quiet.
    let mut last_busy = std::time::Instant::now();
    loop {
        let mut busy = false;
        while let Ok((stream, _)) = listener.accept() {
            stream.set_nonblocking(true)?;
            stream.set_nodelay(true)?;
            mux.add(stream);
            busy = true;
        }
        busy |= mux.turn(site);
        tick();
        if busy {
            last_busy = std::time::Instant::now();
        } else if last_busy.elapsed() < Duration::from_millis(250) {
            std::thread::yield_now();
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Log `info` and above to stderr and into the served log ring.
pub fn init_logging() {
    crate::logbuf::install(256 * 1024);
}
