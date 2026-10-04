//! CBOR (RFC 8949) with either text keys (field names) or unsigned integer
//! keys (field tags). Floats use the shortest of float32/float64 that holds
//! the value exactly ("preferred serialization").
use super::{Error, Key, Out, Reader, Result, Writer, MAX_DEPTH};
use std::borrow::Cow;

pub struct CborWriter<'a> {
    out: Out<'a>,
    int_keys: bool,
}

impl<'a> CborWriter<'a> {
    pub fn new(buf: &'a mut Vec<u8>, limit: usize, int_keys: bool) -> Self {
        Self {
            out: Out::new(buf, limit),
            int_keys,
        }
    }

    fn head(&mut self, major: u8, v: u64) -> Result<()> {
        let m = major << 5;
        if v < 24 {
            self.out.push(m | v as u8)
        } else if v <= u8::MAX as u64 {
            self.out.extend(&[m | 24, v as u8])
        } else if v <= u16::MAX as u64 {
            self.out.push(m | 25)?;
            self.out.extend(&(v as u16).to_be_bytes())
        } else if v <= u32::MAX as u64 {
            self.out.push(m | 26)?;
            self.out.extend(&(v as u32).to_be_bytes())
        } else {
            self.out.push(m | 27)?;
            self.out.extend(&v.to_be_bytes())
        }
    }
}

impl Writer for CborWriter<'_> {
    fn begin(&mut self, fields: usize) -> Result<()> {
        self.head(5, fields as u64)
    }
    fn end(&mut self) -> Result<()> {
        Ok(())
    }
    fn key(&mut self, tag: u32, name: &'static str) -> Result<()> {
        if self.int_keys {
            self.head(0, tag as u64)
        } else {
            self.str(name)
        }
    }
    fn list(&mut self, len: usize, _packed: bool) -> Result<()> {
        self.head(4, len as u64)
    }
    fn end_list(&mut self) -> Result<()> {
        Ok(())
    }
    fn u64(&mut self, v: u64) -> Result<()> {
        self.head(0, v)
    }
    fn i64(&mut self, v: i64) -> Result<()> {
        if v >= 0 {
            self.head(0, v as u64)
        } else {
            self.head(1, !(v as u64))
        }
    }
    fn f64(&mut self, v: f64) -> Result<()> {
        let narrow = v as f32;
        if narrow as f64 == v || v.is_nan() {
            self.f32(narrow)
        } else {
            self.out.push(0xfb)?;
            self.out.extend(&v.to_be_bytes())
        }
    }
    fn f32(&mut self, v: f32) -> Result<()> {
        self.out.push(0xfa)?;
        self.out.extend(&v.to_be_bytes())
    }
    fn bool(&mut self, v: bool) -> Result<()> {
        self.out.push(if v { 0xf5 } else { 0xf4 })
    }
    fn str(&mut self, v: &str) -> Result<()> {
        self.head(3, v.len() as u64)?;
        self.out.extend(v.as_bytes())
    }
    fn null(&mut self) -> Result<()> {
        self.out.push(0xf6)
    }
}

/// Remaining map entries per level; `INDEFINITE` for `0xbf ... 0xff` maps.
const INDEFINITE: u64 = u64::MAX;

pub struct CborReader<'de> {
    s: &'de [u8],
    pos: usize,
    remaining: [u64; MAX_DEPTH],
    depth: usize,
}

impl<'de> CborReader<'de> {
    pub fn new(s: &'de [u8]) -> Self {
        Self {
            s,
            pos: 0,
            remaining: [0; MAX_DEPTH],
            depth: 0,
        }
    }

    fn take(&mut self, n: usize) -> Result<&'de [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::Eof)?;
        let bytes = self.s.get(self.pos..end).ok_or(Error::Eof)?;
        self.pos = end;
        Ok(bytes)
    }

    fn peek(&self) -> Result<u8> {
        self.s.get(self.pos).copied().ok_or(Error::Eof)
    }

    /// Reads a head; skips any semantic tags (major 6) in front of it.
    /// Returns (major, argument) with `INDEFINITE` for indefinite length.
    fn head(&mut self) -> Result<(u8, u64)> {
        loop {
            let b = self.take(1)?[0];
            let major = b >> 5;
            let info = b & 31;
            let arg = match info {
                0..=23 => info as u64,
                24 => self.take(1)?[0] as u64,
                25 => u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as u64,
                26 => u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64,
                27 => u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
                31 if matches!(major, 2..=5 | 7) => INDEFINITE,
                _ => return Err(Error::Invalid("reserved CBOR head")),
            };
            if major != 6 {
                return Ok((major, arg));
            }
        }
    }

    fn int(&mut self) -> Result<i128> {
        match self.head()? {
            (0, v) => Ok(v as i128),
            (1, v) => Ok(-1 - v as i128),
            _ => Err(Error::Invalid("expected a CBOR integer")),
        }
    }

    fn float_at(&mut self, info: u8) -> Result<f64> {
        Ok(match info {
            25 => half(u16::from_be_bytes(self.take(2)?.try_into().unwrap())),
            26 => f32::from_be_bytes(self.take(4)?.try_into().unwrap()) as f64,
            27 => f64::from_be_bytes(self.take(8)?.try_into().unwrap()),
            _ => return Err(Error::Invalid("expected a CBOR float")),
        })
    }

    fn skip_at(&mut self, level: usize) -> Result<()> {
        if level > MAX_DEPTH {
            return Err(Error::Depth);
        }
        let b = self.peek()?;
        if b >> 5 == 7 {
            self.pos += 1;
            return match b & 31 {
                24 => self.take(1).map(drop),
                25 => self.take(2).map(drop),
                26 => self.take(4).map(drop),
                27 => self.take(8).map(drop),
                _ => Ok(()),
            };
        }
        let (major, arg) = self.head()?;
        match (major, arg) {
            (0 | 1, _) => {}
            (2 | 3, INDEFINITE) => {
                while self.peek()? != 0xff {
                    self.skip_at(level + 1)?;
                }
                self.pos += 1;
            }
            (2 | 3, n) => {
                self.take(usize::try_from(n).map_err(|_| Error::Eof)?)?;
            }
            (4 | 5, INDEFINITE) => {
                while self.peek()? != 0xff {
                    self.skip_at(level + 1)?;
                }
                self.pos += 1;
            }
            (4, n) => {
                for _ in 0..n {
                    self.skip_at(level + 1)?;
                }
            }
            (5, n) => {
                for _ in 0..n {
                    self.skip_at(level + 1)?;
                    self.skip_at(level + 1)?;
                }
            }
            _ => return Err(Error::Invalid("unexpected CBOR item")),
        }
        Ok(())
    }
}

/// IEEE 754 half precision to f64.
fn half(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = (bits >> 10) & 0x1f;
    let mant = (bits & 0x3ff) as f64;
    sign * match exp {
        0 => mant * 2f64.powi(-24),
        31 if mant == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        e => (1.0 + mant / 1024.0) * 2f64.powi(e as i32 - 15),
    }
}

impl<'de> Reader<'de> for CborReader<'de> {
    fn begin(&mut self) -> Result<()> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::Depth);
        }
        match self.head()? {
            (5, n) => {
                self.remaining[self.depth] = n;
                self.depth += 1;
                Ok(())
            }
            _ => Err(Error::Invalid("expected a CBOR map")),
        }
    }

    fn next_key(&mut self) -> Result<Option<Key<'de>>> {
        let level = self.depth - 1;
        if self.remaining[level] == INDEFINITE {
            if self.peek()? == 0xff {
                self.pos += 1;
                self.depth -= 1;
                return Ok(None);
            }
        } else if self.remaining[level] == 0 {
            self.depth -= 1;
            return Ok(None);
        } else {
            self.remaining[level] -= 1;
        }
        match self.peek()? >> 5 {
            0 => Ok(Some(Key::Tag(
                u32::try_from(self.int()?).map_err(|_| Error::Invalid("key"))?,
            ))),
            _ => Ok(Some(Key::Name(self.str()?))),
        }
    }

    fn list<F: FnMut(&mut Self) -> Result<()>>(
        &mut self,
        _packed: bool,
        mut each: F,
    ) -> Result<()> {
        if self.null()? {
            return Ok(());
        }
        match self.head()? {
            (4, INDEFINITE) => {
                while self.peek()? != 0xff {
                    each(self)?;
                }
                self.pos += 1;
                Ok(())
            }
            (4, n) => {
                for _ in 0..n {
                    each(self)?;
                }
                Ok(())
            }
            _ => Err(Error::Invalid("expected a CBOR array")),
        }
    }

    fn null(&mut self) -> Result<bool> {
        // null (0xf6) and undefined (0xf7)
        if matches!(self.peek()?, 0xf6 | 0xf7) {
            self.pos += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn u64(&mut self) -> Result<u64> {
        u64::try_from(self.int()?).map_err(|_| Error::Invalid("expected an unsigned integer"))
    }

    fn i64(&mut self) -> Result<i64> {
        i64::try_from(self.int()?).map_err(|_| Error::Invalid("integer out of range"))
    }

    fn f64(&mut self) -> Result<f64> {
        let b = self.peek()?;
        match b {
            0xf9..=0xfb => {
                self.pos += 1;
                self.float_at(b & 31)
            }
            0xf6 | 0xf7 => {
                self.pos += 1;
                Ok(f64::NAN)
            }
            _ => Ok(self.int()? as f64),
        }
    }

    fn bool(&mut self) -> Result<bool> {
        match self.take(1)?[0] {
            0xf5 => Ok(true),
            0xf4 => Ok(false),
            _ => Err(Error::Invalid("expected a CBOR bool")),
        }
    }

    fn str(&mut self) -> Result<Cow<'de, str>> {
        match self.head()? {
            (3, INDEFINITE) => {
                let mut out = String::new();
                while self.peek()? != 0xff {
                    out.push_str(&self.str()?);
                }
                self.pos += 1;
                Ok(Cow::Owned(out))
            }
            (3, n) => {
                let bytes = self.take(usize::try_from(n).map_err(|_| Error::Eof)?)?;
                std::str::from_utf8(bytes)
                    .map(Cow::Borrowed)
                    .map_err(|_| Error::Invalid("invalid UTF-8"))
            }
            _ => Err(Error::Invalid("expected a CBOR text string")),
        }
    }

    fn skip(&mut self) -> Result<()> {
        self.skip_at(0)
    }
}
