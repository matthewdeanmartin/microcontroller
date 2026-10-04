use serde::Serialize;
use std::{
    io,
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize)]
pub struct TimeSample {
    pub unix_ms: u64,
    pub round_trip_ms: u64,
    pub stratum: u8,
    pub server: SocketAddr,
}

/// One bounded UDP exchange. Numeric addresses deliberately avoid blocking DNS.
/// This samples the server transmit time; it does not discipline the local clock.
pub fn query(server: SocketAddr) -> Result<TimeSample, String> {
    let run = || -> io::Result<TimeSample> {
        let socket = UdpSocket::bind(if server.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })?;
        socket.connect(server)?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        socket.set_write_timeout(Some(Duration::from_secs(3)))?;
        let mut request = [0u8; 48];
        request[0] = 0x23; // NTP v4, client mode.
        getrandom::getrandom(&mut request[40..48]).map_err(|e| io::Error::other(e.to_string()))?;
        let started = Instant::now();
        socket.send(&request)?;
        let mut reply = [0u8; 512];
        let n = socket.recv(&mut reply)?;
        if n < 48
            || reply[0] & 7 != 4
            || !matches!((reply[0] >> 3) & 7, 3 | 4)
            || reply[0] >> 6 == 3
            || !(1..=15).contains(&reply[1])
            || reply[24..32] != request[40..48]
            || reply[40..48] == [0; 8]
        {
            return Err(io::Error::other(
                "invalid, unsynchronized, or rate-limited NTP reply",
            ));
        }
        let seconds = u32::from_be_bytes(reply[40..44].try_into().unwrap()) as u64;
        let fraction = u32::from_be_bytes(reply[44..48].try_into().unwrap()) as u64;
        // Era selection valid for 1970–2106, including the 2036 NTP rollover.
        let seconds = if seconds < 2_208_988_800 {
            seconds + (1u64 << 32)
        } else {
            seconds
        };
        Ok(TimeSample {
            unix_ms: (seconds - 2_208_988_800) * 1000 + ((fraction * 1000) >> 32),
            round_trip_ms: started.elapsed().as_millis() as u64,
            stratum: reply[1],
            server,
        })
    };
    run().map_err(|e| format!("NTP: {e}"))
}
