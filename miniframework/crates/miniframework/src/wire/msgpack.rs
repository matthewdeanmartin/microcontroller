//! MessagePack with string keys. Integers and floats use the smallest
//! encoding that holds the value exactly (float32 when lossless).
use super::{Error, Key, Out, Reader, Result, Writer, MAX_DEPTH};
use std::borrow::Cow;

pub struct MsgPackWriter<'a> {
    out: Out<'a>,
}

impl<'a> MsgPackWriter<'a> {
    pub fn new(buf: &'a mut Vec<u8>, limit: usize) -> Self {
        Self {
            out: Out::new(buf, limit),
        }
    }

    fn header(&mut self, len: usize, fix: u8, fix_max: usize, m16: u8, m32: u8) -> Result<()> {
        if len <= fix_max {
            self.out.push(fix | len as u8)
        } else if len <= u16::MAX as usize {
            self.out.push(m16)?;
            self.out.extend(&(len as u16).to_be_bytes())
        } else {
            self.out.push(m32)?;
            self.out.extend(&(len as u32).to_be_bytes())
        }
    }
}

impl Writer for MsgPackWriter<'_> {
    fn begin(&mut self, fields: usize) -> Result<()> {
        self.header(fields, 0x80, 15, 0xde, 0xdf)
    }
    fn end(&mut self) -> Result<()> {
        Ok(())
    }
    fn key(&mut self, _tag: u32, name: &'static str) -> Result<()> {
        self.str(name)
    }
    fn list(&mut self, len: usize, _packed: bool) -> Result<()> {
        self.header(len, 0x90, 15, 0xdc, 0xdd)
    }
    fn end_list(&mut self) -> Result<()> {
        Ok(())
    }
    fn u64(&mut self, v: u64) -> Result<()> {
        if v < 128 {
            self.out.push(v as u8)
        } else if v <= u8::MAX as u64 {
            self.out.extend(&[0xcc, v as u8])
        } else if v <= u16::MAX as u64 {
            self.out.push(0xcd)?;
            self.out.extend(&(v as u16).to_be_bytes())
        } else if v <= u32::MAX as u64 {
            self.out.push(0xce)?;
            self.out.extend(&(v as u32).to_be_bytes())
        } else {
            self.out.push(0xcf)?;
            self.out.extend(&v.to_be_bytes())
        }
    }
    fn i64(&mut self, v: i64) -> Result<()> {
        if v >= 0 {
            self.u64(v as u64)
        } else if v >= -32 {
            self.out.push(v as i8 as u8)
        } else if v >= i8::MIN as i64 {
            self.out.extend(&[0xd0, v as i8 as u8])
        } else if v >= i16::MIN as i64 {
            self.out.push(0xd1)?;
            self.out.extend(&(v as i16).to_be_bytes())
        } else if v >= i32::MIN as i64 {
            self.out.push(0xd2)?;
            self.out.extend(&(v as i32).to_be_bytes())
        } else {
            self.out.push(0xd3)?;
            self.out.extend(&v.to_be_bytes())
        }
    }
    fn f64(&mut self, v: f64) -> Result<()> {
        let narrow = v as f32;
        if narrow as f64 == v || v.is_nan() {
            self.f32(narrow)
        } else {
            self.out.push(0xcb)?;
            self.out.extend(&v.to_be_bytes())
        }
    }
    fn f32(&mut self, v: f32) -> Result<()> {
        self.out.push(0xca)?;
        self.out.extend(&v.to_be_bytes())
    }
    fn bool(&mut self, v: bool) -> Result<()> {
        self.out.push(if v { 0xc3 } else { 0xc2 })
    }
    fn str(&mut self, v: &str) -> Result<()> {
        let len = v.len();
        if len < 32 {
            self.out.push(0xa0 | len as u8)?;
        } else if len <= u8::MAX as usize {
            self.out.extend(&[0xd9, len as u8])?;
        } else {
            self.header(len, 0, 0, 0xda, 0xdb)?;
        }
        self.out.extend(v.as_bytes())
    }
    fn null(&mut self) -> Result<()> {
        self.out.push(0xc0)
    }
}

pub struct MsgPackReader<'de> {
    s: &'de [u8],
    pos: usize,
    remaining: [usize; MAX_DEPTH],
    depth: usize,
}

impl<'de> MsgPackReader<'de> {
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

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn peek(&self) -> Result<u8> {
        self.s.get(self.pos).copied().ok_or(Error::Eof)
    }

    fn be(&mut self, n: usize) -> Result<u64> {
        Ok(self
            .take(n)?
            .iter()
            .fold(0u64, |acc, &b| acc << 8 | b as u64))
    }

    fn map_len(&mut self) -> Result<usize> {
        let b = self.byte()?;
        Ok(match b {
            0x80..=0x8f => (b & 15) as usize,
            0xde => self.be(2)? as usize,
            0xdf => self.be(4)? as usize,
            _ => return Err(Error::Invalid("expected a MessagePack map")),
        })
    }

    fn array_len(&mut self) -> Result<usize> {
        let b = self.byte()?;
        Ok(match b {
            0x90..=0x9f => (b & 15) as usize,
            0xdc => self.be(2)? as usize,
            0xdd => self.be(4)? as usize,
            _ => return Err(Error::Invalid("expected a MessagePack array")),
        })
    }

    /// Any integer, as i128 so u64 and i64 both fit.
    fn int(&mut self) -> Result<i128> {
        let b = self.byte()?;
        Ok(match b {
            0x00..=0x7f => b as i128,
            0xe0..=0xff => b as i8 as i128,
            0xcc => self.be(1)? as i128,
            0xcd => self.be(2)? as i128,
            0xce => self.be(4)? as i128,
            0xcf => self.be(8)? as i128,
            0xd0 => self.be(1)? as u8 as i8 as i128,
            0xd1 => self.be(2)? as u16 as i16 as i128,
            0xd2 => self.be(4)? as u32 as i32 as i128,
            0xd3 => self.be(8)? as i64 as i128,
            _ => return Err(Error::Invalid("expected a MessagePack integer")),
        })
    }

    fn bytes_len(&mut self, b: u8) -> Result<Option<usize>> {
        Ok(Some(match b {
            0xa0..=0xbf => (b & 31) as usize,
            0xd9 | 0xc4 => self.be(1)? as usize,
            0xda | 0xc5 => self.be(2)? as usize,
            0xdb | 0xc6 => self.be(4)? as usize,
            _ => return Ok(None),
        }))
    }
}

impl<'de> Reader<'de> for MsgPackReader<'de> {
    fn begin(&mut self) -> Result<()> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::Depth);
        }
        self.remaining[self.depth] = self.map_len()?;
        self.depth += 1;
        Ok(())
    }

    fn next_key(&mut self) -> Result<Option<Key<'de>>> {
        let left = &mut self.remaining[self.depth - 1];
        if *left == 0 {
            self.depth -= 1;
            return Ok(None);
        }
        *left -= 1;
        match self.peek()? {
            0x00..=0x7f | 0xcc..=0xcf => Ok(Some(Key::Tag(
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
        for _ in 0..self.array_len()? {
            each(self)?;
        }
        Ok(())
    }

    fn null(&mut self) -> Result<bool> {
        if self.peek()? == 0xc0 {
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
        match self.peek()? {
            0xca => {
                self.pos += 1;
                Ok(f32::from_bits(self.be(4)? as u32) as f64)
            }
            0xcb => {
                self.pos += 1;
                Ok(f64::from_bits(self.be(8)?))
            }
            0xc0 => {
                self.pos += 1;
                Ok(f64::NAN)
            }
            _ => Ok(self.int()? as f64),
        }
    }

    fn bool(&mut self) -> Result<bool> {
        match self.byte()? {
            0xc3 => Ok(true),
            0xc2 => Ok(false),
            _ => Err(Error::Invalid("expected a MessagePack bool")),
        }
    }

    fn str(&mut self) -> Result<Cow<'de, str>> {
        let b = self.byte()?;
        let len = match b {
            0xa0..=0xbf | 0xd9 | 0xda | 0xdb => self.bytes_len(b)?.unwrap_or(0),
            _ => return Err(Error::Invalid("expected a MessagePack string")),
        };
        std::str::from_utf8(self.take(len)?)
            .map(Cow::Borrowed)
            .map_err(|_| Error::Invalid("invalid UTF-8"))
    }

    fn skip(&mut self) -> Result<()> {
        self.skip_at(0)
    }
}

impl MsgPackReader<'_> {
    fn skip_at(&mut self, level: usize) -> Result<()> {
        if level > MAX_DEPTH {
            return Err(Error::Depth);
        }
        let b = self.peek()?;
        match b {
            0x80..=0x8f | 0xde | 0xdf => {
                let n = self.map_len()?;
                for _ in 0..n {
                    self.skip_at(level + 1)?;
                    self.skip_at(level + 1)?;
                }
            }
            0x90..=0x9f | 0xdc | 0xdd => {
                let n = self.array_len()?;
                for _ in 0..n {
                    self.skip_at(level + 1)?;
                }
            }
            0xc0 | 0xc2 | 0xc3 => self.pos += 1,
            0xca => {
                self.take(5)?;
            }
            0xcb => {
                self.take(9)?;
            }
            0xa0..=0xbf | 0xd9..=0xdb | 0xc4..=0xc6 => {
                self.pos += 1;
                let n = self.bytes_len(b)?.unwrap_or(0);
                self.take(n)?;
            }
            0xd4 => {
                self.take(3)?;
            }
            0xd5 => {
                self.take(4)?;
            }
            0xd6 => {
                self.take(6)?;
            }
            0xd7 => {
                self.take(10)?;
            }
            0xd8 => {
                self.take(18)?;
            }
            0xc7..=0xc9 => {
                self.pos += 1;
                let n = self.be(1 << (b - 0xc7))? as usize;
                self.take(n + 1)?;
            }
            _ => {
                self.int()?;
            }
        }
        Ok(())
    }
}
