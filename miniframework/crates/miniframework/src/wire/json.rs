//! JSON: streaming writer and a small pull reader.
use super::{Error, Key, Out, Reader, Result, Writer, MAX_DEPTH};
use std::borrow::Cow;

pub struct JsonWriter<'a> {
    out: Out<'a>,
    /// The next item at the current level needs a separating comma. One flag
    /// suffices: `key` clears it so the value that follows gets none.
    comma: bool,
}

impl<'a> JsonWriter<'a> {
    pub fn new(buf: &'a mut Vec<u8>, limit: usize) -> Self {
        Self {
            out: Out::new(buf, limit),
            comma: false,
        }
    }

    #[inline]
    fn sep(&mut self) -> Result<()> {
        if self.comma {
            self.out.push(b',')?;
        }
        self.comma = true;
        Ok(())
    }
}

pub(crate) fn write_u64(out: &mut Out<'_>, mut v: u64) -> Result<()> {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    loop {
        i -= 1;
        digits[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out.extend(&digits[i..])
}

fn write_str(out: &mut Out<'_>, s: &str) -> Result<()> {
    out.push(b'"')?;
    let bytes = s.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let escape: &[u8] = match b {
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            0..=0x1f => b"",
            _ => continue,
        };
        out.extend(&bytes[start..i])?;
        if escape.is_empty() {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            out.extend(&[
                b'\\',
                b'u',
                b'0',
                b'0',
                HEX[(b >> 4) as usize],
                HEX[(b & 15) as usize],
            ])?;
        } else {
            out.extend(escape)?;
        }
        start = i + 1;
    }
    out.extend(&bytes[start..])?;
    out.push(b'"')
}

impl Writer for JsonWriter<'_> {
    fn begin(&mut self, _fields: usize) -> Result<()> {
        self.sep()?;
        self.comma = false;
        self.out.push(b'{')
    }
    fn end(&mut self) -> Result<()> {
        self.comma = true;
        self.out.push(b'}')
    }
    fn key(&mut self, _tag: u32, name: &'static str) -> Result<()> {
        self.sep()?;
        // Field names are Rust identifiers: no escaping needed.
        self.out.push(b'"')?;
        self.out.extend(name.as_bytes())?;
        self.out.extend(b"\":")?;
        self.comma = false;
        Ok(())
    }
    fn list(&mut self, _len: usize, _packed: bool) -> Result<()> {
        self.sep()?;
        self.comma = false;
        self.out.push(b'[')
    }
    fn end_list(&mut self) -> Result<()> {
        self.comma = true;
        self.out.push(b']')
    }
    fn u64(&mut self, v: u64) -> Result<()> {
        self.sep()?;
        write_u64(&mut self.out, v)
    }
    fn i64(&mut self, v: i64) -> Result<()> {
        self.sep()?;
        if v < 0 {
            self.out.push(b'-')?;
        }
        write_u64(&mut self.out, v.unsigned_abs())
    }
    fn f64(&mut self, v: f64) -> Result<()> {
        self.sep()?;
        if !v.is_finite() {
            return self.out.extend(b"null");
        }
        let mut buf = ryu::Buffer::new();
        let s = buf.format_finite(v);
        // ryu writes "21.0"; JSON readers accept it, but "21" is shorter
        // and what JSON.stringify produces.
        self.out
            .extend(s.strip_suffix(".0").unwrap_or(s).as_bytes())
    }
    fn f32(&mut self, v: f32) -> Result<()> {
        self.sep()?;
        if !v.is_finite() {
            return self.out.extend(b"null");
        }
        let mut buf = ryu::Buffer::new();
        let s = buf.format_finite(v);
        self.out
            .extend(s.strip_suffix(".0").unwrap_or(s).as_bytes())
    }
    fn bool(&mut self, v: bool) -> Result<()> {
        self.sep()?;
        self.out.extend(if v { b"true" } else { b"false" })
    }
    fn str(&mut self, v: &str) -> Result<()> {
        self.sep()?;
        write_str(&mut self.out, v)
    }
    fn null(&mut self) -> Result<()> {
        self.sep()?;
        self.out.extend(b"null")
    }
}

pub struct JsonReader<'de> {
    s: &'de [u8],
    pos: usize,
    /// Per open object/array: whether its first item is still to come.
    first: [bool; MAX_DEPTH],
    depth: usize,
}

impl<'de> JsonReader<'de> {
    pub fn new(s: &'de [u8]) -> Self {
        Self {
            s,
            pos: 0,
            first: [true; MAX_DEPTH],
            depth: 0,
        }
    }

    pub fn finish(&mut self) -> Result<()> {
        self.ws();
        if self.pos == self.s.len() {
            Ok(())
        } else {
            Err(Error::Invalid("trailing data after JSON value"))
        }
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\n' | b'\r' | b'\t') = self.s.get(self.pos) {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Result<u8> {
        self.ws();
        self.s.get(self.pos).copied().ok_or(Error::Eof)
    }

    fn expect(&mut self, b: u8) -> Result<()> {
        if self.peek()? == b {
            self.pos += 1;
            Ok(())
        } else {
            Err(Error::Invalid("unexpected JSON token"))
        }
    }

    fn push(&mut self) -> Result<()> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::Depth);
        }
        self.first[self.depth] = true;
        self.depth += 1;
        Ok(())
    }

    /// True when the container at the current depth has another item.
    fn more(&mut self, close: u8) -> Result<bool> {
        let b = self.peek()?;
        if b == close {
            self.pos += 1;
            self.depth -= 1;
            return Ok(false);
        }
        let first = &mut self.first[self.depth - 1];
        if !*first {
            if b != b',' {
                return Err(Error::Invalid("expected , in JSON"));
            }
            self.pos += 1;
        }
        *first = false;
        Ok(true)
    }

    fn number(&mut self) -> Result<&'de str> {
        self.ws();
        let start = self.pos;
        while let Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') = self.s.get(self.pos) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(Error::Invalid("expected a JSON number"));
        }
        std::str::from_utf8(&self.s[start..self.pos]).map_err(|_| Error::Invalid("number"))
    }

    fn literal(&mut self, word: &[u8]) -> Result<bool> {
        self.ws();
        if self.s[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn string(&mut self) -> Result<Cow<'de, str>> {
        self.expect(b'"')?;
        let start = self.pos;
        // Fast path: no escapes, borrow the input.
        loop {
            match self.s.get(self.pos) {
                None => return Err(Error::Eof),
                Some(b'"') => {
                    let s = std::str::from_utf8(&self.s[start..self.pos])
                        .map_err(|_| Error::Invalid("invalid UTF-8"))?;
                    self.pos += 1;
                    return Ok(Cow::Borrowed(s));
                }
                Some(b'\\') => break,
                Some(_) => self.pos += 1,
            }
        }
        let mut out: Vec<u8> = self.s[start..self.pos].to_vec();
        loop {
            let b = *self.s.get(self.pos).ok_or(Error::Eof)?;
            self.pos += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let e = *self.s.get(self.pos).ok_or(Error::Eof)?;
                    self.pos += 1;
                    match e {
                        b'"' | b'\\' | b'/' => out.push(e),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xd800..0xdc00).contains(&code) {
                                // Surrogate pair.
                                if self.s.get(self.pos..self.pos + 2) != Some(b"\\u") {
                                    return Err(Error::Invalid("lone surrogate"));
                                }
                                self.pos += 2;
                                let low = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&low) {
                                    return Err(Error::Invalid("bad surrogate pair"));
                                }
                                code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                            }
                            let c = char::from_u32(code).ok_or(Error::Invalid("bad \\u escape"))?;
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err(Error::Invalid("bad escape")),
                    }
                }
                _ => out.push(b),
            }
        }
        String::from_utf8(out)
            .map(Cow::Owned)
            .map_err(|_| Error::Invalid("invalid UTF-8"))
    }

    fn hex4(&mut self) -> Result<u32> {
        let digits = self.s.get(self.pos..self.pos + 4).ok_or(Error::Eof)?;
        self.pos += 4;
        let text = std::str::from_utf8(digits).map_err(|_| Error::Invalid("bad \\u escape"))?;
        u32::from_str_radix(text, 16).map_err(|_| Error::Invalid("bad \\u escape"))
    }
}

impl<'de> Reader<'de> for JsonReader<'de> {
    fn begin(&mut self) -> Result<()> {
        self.expect(b'{')?;
        self.push()
    }

    fn next_key(&mut self) -> Result<Option<Key<'de>>> {
        if !self.more(b'}')? {
            return Ok(None);
        }
        let key = self.string()?;
        self.expect(b':')?;
        Ok(Some(Key::Name(key)))
    }

    fn list<F: FnMut(&mut Self) -> Result<()>>(
        &mut self,
        _packed: bool,
        mut each: F,
    ) -> Result<()> {
        if self.literal(b"null")? {
            return Ok(());
        }
        self.expect(b'[')?;
        self.push()?;
        while self.more(b']')? {
            each(self)?;
        }
        Ok(())
    }

    fn null(&mut self) -> Result<bool> {
        self.literal(b"null")
    }

    fn u64(&mut self) -> Result<u64> {
        let n = self.number()?;
        n.parse::<u64>().or_else(|_| {
            // Accept integral floats ("1.0", "1e3") as JSON.stringify can emit them.
            let f: f64 = n.parse().map_err(|_| Error::Invalid("number"))?;
            if f >= 0.0 && f.fract() == 0.0 && f < 1.8446744073709552e19 {
                Ok(f as u64)
            } else {
                Err(Error::Invalid("expected an unsigned integer"))
            }
        })
    }

    fn i64(&mut self) -> Result<i64> {
        let n = self.number()?;
        n.parse::<i64>().or_else(|_| {
            let f: f64 = n.parse().map_err(|_| Error::Invalid("number"))?;
            if f.fract() == 0.0 && f.abs() < 9.2e18 {
                Ok(f as i64)
            } else {
                Err(Error::Invalid("expected an integer"))
            }
        })
    }

    fn f64(&mut self) -> Result<f64> {
        if self.literal(b"null")? {
            return Ok(f64::NAN);
        }
        self.number()?
            .parse()
            .map_err(|_| Error::Invalid("expected a number"))
    }

    fn bool(&mut self) -> Result<bool> {
        if self.literal(b"true")? {
            Ok(true)
        } else if self.literal(b"false")? {
            Ok(false)
        } else {
            Err(Error::Invalid("expected true or false"))
        }
    }

    fn str(&mut self) -> Result<Cow<'de, str>> {
        self.string()
    }

    fn skip(&mut self) -> Result<()> {
        match self.peek()? {
            b'"' => self.string().map(drop),
            b'{' => {
                self.begin()?;
                while self.next_key()?.is_some() {
                    self.skip()?;
                }
                Ok(())
            }
            b'[' => self.list(false, |r| r.skip()),
            b't' | b'f' => self.bool().map(drop),
            b'n' => {
                if self.null()? {
                    Ok(())
                } else {
                    Err(Error::Invalid("unexpected JSON token"))
                }
            }
            _ => self.number().map(drop),
        }
    }
}
