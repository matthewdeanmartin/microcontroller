//! Gorilla time-series compression (Pelkonen et al., VLDB 2015), adapted
//! to millisecond timestamps.
//!
//! Each block is self-contained: its first timestamp lives in the block's
//! metadata, so evicting the oldest block never breaks a newer one.
//!
//! Timestamps store the delta-of-delta. Buckets are tuned for boards that
//! report about every 10 s with some jitter:
//!
//! | prefix | payload | range of delta-of-delta (ms) |
//! |--------|---------|------------------------------|
//! | `0`    | -       | 0                            |
//! | `10`   | 8 bits  | -128 ..= 127                 |
//! | `110`  | 12 bits | -2048 ..= 2047               |
//! | `1110` | 20 bits | -524288 ..= 524287           |
//! | `1111` | 32 bits | anything that fits i32       |
//!
//! Values store the XOR with the previous value: `0` if equal, otherwise the
//! meaningful bits, reusing the previous leading/trailing-zero window when
//! it fits.

/// Bits one point can need at worst: 4 + 32 (time) + 2 + 5 + 6 + 64 (value).
pub const WORST_POINT_BITS: usize = 113;

#[derive(Clone, Copy, Debug, Default)]
pub struct EncState {
    pub prev_t: i64,
    pub prev_delta: i64,
    pub prev_v: u64,
    pub leading: u8,
    pub trailing: u8,
    pub count: u32,
    /// Bits written so far.
    pub bits: usize,
}

struct BitWriter<'a> {
    buf: &'a mut [u8],
    bits: usize,
}

impl BitWriter<'_> {
    fn put(&mut self, value: u64, n: u32) {
        for i in (0..n).rev() {
            if value >> i & 1 == 1 {
                self.buf[self.bits / 8] |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }
    }
}

pub struct BitReader<'a> {
    buf: &'a [u8],
    pos: usize,
    end: usize,
}

impl BitReader<'_> {
    fn bit(&mut self) -> Option<bool> {
        if self.pos >= self.end {
            return None;
        }
        let b = self.buf[self.pos / 8] & (0x80 >> (self.pos % 8)) != 0;
        self.pos += 1;
        Some(b)
    }
    fn get(&mut self, n: u32) -> Option<u64> {
        let mut v = 0u64;
        for _ in 0..n {
            v = v << 1 | self.bit()? as u64;
        }
        Some(v)
    }
}

fn sign_extend(v: u64, bits: u32) -> i64 {
    let shift = 64 - bits;
    ((v << shift) as i64) >> shift
}

/// Appends a point. `buf` must be zeroed beyond `state.bits`. The caller
/// guarantees space (`WORST_POINT_BITS`) and that `t` is newer than the
/// previous point and within i32 milliseconds of the previous delta.
pub fn append(buf: &mut [u8], state: &mut EncState, first_t: i64, t: i64, v: f64) {
    let mut w = BitWriter {
        buf,
        bits: state.bits,
    };
    let bits = v.to_bits();
    if state.count == 0 {
        // First point: time is the block's first_t; value raw.
        debug_assert_eq!(t, first_t);
        w.put(bits, 64);
        state.prev_t = t;
        state.prev_delta = 0;
        state.prev_v = bits;
        state.leading = u8::MAX;
    } else {
        let delta = t - state.prev_t;
        let dod = delta - state.prev_delta;
        match dod {
            0 => w.put(0, 1),
            -128..=127 => {
                w.put(0b10, 2);
                w.put(dod as u64 & 0xff, 8);
            }
            -2048..=2047 => {
                w.put(0b110, 3);
                w.put(dod as u64 & 0xfff, 12);
            }
            -524_288..=524_287 => {
                w.put(0b1110, 4);
                w.put(dod as u64 & 0xf_ffff, 20);
            }
            _ => {
                w.put(0b1111, 4);
                w.put(dod as u64 & 0xffff_ffff, 32);
            }
        }
        let xor = bits ^ state.prev_v;
        if xor == 0 {
            w.put(0, 1);
        } else {
            w.put(1, 1);
            let leading = (xor.leading_zeros() as u8).min(31);
            let trailing = xor.trailing_zeros() as u8;
            if state.leading != u8::MAX && leading >= state.leading && trailing >= state.trailing {
                w.put(0, 1);
                let meaningful = 64 - state.leading as u32 - state.trailing as u32;
                w.put(xor >> state.trailing, meaningful);
            } else {
                w.put(1, 1);
                let meaningful = 64 - leading as u32 - trailing as u32;
                w.put(leading as u64, 5);
                // 64 meaningful bits is stored as 0 (6 bits hold 0..=63).
                w.put(meaningful as u64 & 63, 6);
                w.put(xor >> trailing, meaningful);
                state.leading = leading;
                state.trailing = trailing;
            }
        }
        state.prev_delta = delta;
        state.prev_t = t;
        state.prev_v = bits;
    }
    state.bits = w.bits;
    state.count += 1;
}

/// Whether `t` can follow the block's last point without overflowing the
/// widest timestamp bucket.
pub fn fits_time(state: &EncState, t: i64) -> bool {
    if state.count == 0 {
        return true;
    }
    let dod = (t - state.prev_t) - state.prev_delta;
    i32::try_from(dod).is_ok()
}

/// Iterates the points of one block.
pub struct Points<'a> {
    r: BitReader<'a>,
    first_t: i64,
    remaining: u32,
    started: bool,
    t: i64,
    delta: i64,
    v: u64,
    leading: u32,
    trailing: u32,
}

pub fn points(buf: &[u8], bits: usize, first_t: i64, count: u32) -> Points<'_> {
    Points {
        r: BitReader {
            buf,
            pos: 0,
            end: bits,
        },
        first_t,
        remaining: count,
        started: false,
        t: 0,
        delta: 0,
        v: 0,
        leading: 0,
        trailing: 0,
    }
}

impl Iterator for Points<'_> {
    type Item = (i64, f64);

    fn next(&mut self) -> Option<(i64, f64)> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        if !self.started {
            self.started = true;
            self.t = self.first_t;
            self.v = self.r.get(64)?;
            return Some((self.t, f64::from_bits(self.v)));
        }
        let dod = if !self.r.bit()? {
            0
        } else if !self.r.bit()? {
            sign_extend(self.r.get(8)?, 8)
        } else if !self.r.bit()? {
            sign_extend(self.r.get(12)?, 12)
        } else if !self.r.bit()? {
            sign_extend(self.r.get(20)?, 20)
        } else {
            sign_extend(self.r.get(32)?, 32)
        };
        self.delta += dod;
        self.t += self.delta;
        if self.r.bit()? {
            if self.r.bit()? {
                self.leading = self.r.get(5)? as u32;
                let mut meaningful = self.r.get(6)? as u32;
                if meaningful == 0 {
                    meaningful = 64;
                }
                self.trailing = 64 - self.leading - meaningful;
            }
            let meaningful = 64 - self.leading - self.trailing;
            let xor = self.r.get(meaningful)? << self.trailing;
            self.v ^= xor;
        }
        Some((self.t, f64::from_bits(self.v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(data: &[(i64, f64)]) -> usize {
        let mut buf = vec![0u8; 64 * 1024];
        let mut state = EncState::default();
        for &(t, v) in data {
            assert!(fits_time(&state, t));
            append(&mut buf, &mut state, data[0].0, t, v);
        }
        let back: Vec<_> = points(&buf, state.bits, data[0].0, state.count).collect();
        assert_eq!(back.len(), data.len());
        for (a, b) in back.iter().zip(data) {
            assert_eq!(a.0, b.0);
            assert_eq!(a.1.to_bits(), b.1.to_bits());
        }
        state.bits
    }

    #[test]
    fn regular_series_compress_well() {
        let start = 1_727_900_000_000;
        // Integer-valued counter every 10 s exactly: ~2 bits per point.
        let counter: Vec<_> = (0..1000).map(|i| (start + i * 10_000, 5.0)).collect();
        let bits = roundtrip(&counter);
        assert!(bits < 64 + 1000 * 3, "{bits}");
        // Jittered temperature with one decimal.
        let mut seed = 7u64;
        let temps: Vec<_> = (0..1000)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let jitter = (seed % 200) as i64 - 100;
                let temp = (215 + (seed % 7) as i64 - 3) as f64 / 10.0;
                (start + i * 10_000 + jitter, temp)
            })
            .collect();
        let bits = roundtrip(&temps);
        let per_point = bits as f64 / 8.0 / 1000.0;
        assert!(per_point < 8.0, "{per_point} bytes per point");
    }

    #[test]
    fn extremes_round_trip() {
        let data = vec![
            (0, 0.0),
            (1, -0.0),
            (2, f64::MAX),
            (1_000_000, f64::MIN_POSITIVE),
            (1_000_001, f64::NAN),
            (1_000_000_000, 1e-300),
            (1_000_000_001, 42.0),
            (1_000_000_002, 42.0),
        ];
        roundtrip(&data);
    }

    #[test]
    fn bucket_edges_round_trip() {
        // Delta-of-delta exactly at each bucket's limits.
        let mut t = 0i64;
        let mut delta = 10_000i64;
        let mut data = vec![(0, 1.0)];
        for dod in [
            127, -128, 128, 2047, -2048, 2048, 524_287, -524_288, 524_288, -524_289,
        ] {
            delta += dod;
            t += delta;
            data.push((t, 1.0));
        }
        roundtrip(&data);
    }

    #[test]
    fn huge_gaps_are_refused_not_corrupted() {
        let mut buf = vec![0u8; 256];
        let mut state = EncState::default();
        append(&mut buf, &mut state, 0, 0, 1.0);
        append(&mut buf, &mut state, 0, 10, 1.0);
        assert!(!fits_time(&state, 10 + 10 + i32::MAX as i64 + 1));
        assert!(fits_time(&state, 10 + 10 + i32::MAX as i64));
    }
}
