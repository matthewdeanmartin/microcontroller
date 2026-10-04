//! Protocol Buffers (proto3) wire encoding, driven by the same `Writer`
//! calls as the map formats.
//!
//! Integers are varints (signed ones zigzag: `sint64`), floats are fixed
//! width, repeated numeric scalars are packed. Default-valued fields are
//! omitted, as proto3 does.
//!
//! Nested messages and packed lists are length-prefixed. Instead of a
//! separate sizing pass, the writer reserves one byte for the length and,
//! if the finished body needs a longer varint, shifts the body right. Only
//! the bytes of that one submessage move, so a page of small row messages
//! costs nothing extra; one large outer list moves once.
use super::{Error, Key, Out, Reader, Result, Writer, MAX_DEPTH};
use std::borrow::Cow;

const VARINT: u8 = 0;
const FIXED64: u8 = 1;
const LEN: u8 = 2;
const FIXED32: u8 = 5;
/// No length placeholder (the top-level message, unpacked lists).
const NONE: usize = usize::MAX;

#[derive(Clone, Copy)]
enum Frame {
    Msg {
        len_at: usize,
    },
    List {
        tag: u32,
        packed: bool,
        len_at: usize,
    },
}

pub struct ProtoWriter<'a> {
    out: Out<'a>,
    pending: Option<u32>,
    stack: [Frame; MAX_DEPTH],
    depth: usize,
}

impl<'a> ProtoWriter<'a> {
    pub fn new(buf: &'a mut Vec<u8>, limit: usize) -> Self {
        Self {
            out: Out::new(buf, limit),
            pending: None,
            stack: [Frame::Msg { len_at: NONE }; MAX_DEPTH],
            depth: 0,
        }
    }

    fn varint(&mut self, mut v: u64) -> Result<()> {
        while v >= 0x80 {
            self.out.push(v as u8 | 0x80)?;
            v >>= 7;
        }
        self.out.push(v as u8)
    }

    /// Writes the key for a value of wire type `wt`, if this position needs
    /// one: a field's first value, or each element of an unpacked list.
    fn key_for(&mut self, wt: u8) -> Result<()> {
        if let Some(tag) = self.pending.take() {
            return self.varint((tag as u64) << 3 | wt as u64);
        }
        match self.depth.checked_sub(1).map(|i| self.stack[i]) {
            Some(Frame::List {
                tag, packed: false, ..
            }) => self.varint((tag as u64) << 3 | wt as u64),
            Some(Frame::List { packed: true, .. }) => Ok(()),
            _ => Err(Error::Invalid("protobuf value outside a field")),
        }
    }

    fn push(&mut self, frame: Frame) -> Result<()> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::Depth);
        }
        self.stack[self.depth] = frame;
        self.depth += 1;
        Ok(())
    }

    /// Reserves a one-byte length placeholder and returns its position.
    fn placeholder(&mut self) -> Result<usize> {
        let at = self.out.len();
        self.out.push(0)?;
        Ok(at)
    }

    fn patch(&mut self, len_at: usize) -> Result<()> {
        if len_at == NONE {
            return Ok(());
        }
        let len = (self.out.len() - len_at - 1) as u64;
        let mut size = 1;
        while len >> (7 * size) != 0 {
            size += 1;
        }
        if size > 1 {
            self.out.insert_zeros(len_at + 1, size - 1)?;
        }
        let bytes = self.out.bytes_mut();
        let mut v = len;
        for i in 0..size {
            bytes[len_at + i] = (v as u8 & 0x7f) | if i + 1 < size { 0x80 } else { 0 };
            v >>= 7;
        }
        Ok(())
    }
}

impl Writer for ProtoWriter<'_> {
    fn skips_defaults(&self) -> bool {
        true
    }

    fn begin(&mut self, _fields: usize) -> Result<()> {
        if self.depth == 0 && self.pending.is_none() {
            return self.push(Frame::Msg { len_at: NONE });
        }
        self.key_for(LEN)?;
        let len_at = self.placeholder()?;
        self.push(Frame::Msg { len_at })
    }

    fn end(&mut self) -> Result<()> {
        self.depth -= 1;
        match self.stack[self.depth] {
            Frame::Msg { len_at } => self.patch(len_at),
            Frame::List { .. } => Err(Error::Invalid("end() closes a list")),
        }
    }

    fn key(&mut self, tag: u32, _name: &'static str) -> Result<()> {
        self.pending = Some(tag);
        Ok(())
    }

    fn list(&mut self, len: usize, packed: bool) -> Result<()> {
        let tag = self
            .pending
            .take()
            .ok_or(Error::Invalid("protobuf lists must be field values"))?;
        if packed && len > 0 {
            self.varint((tag as u64) << 3 | LEN as u64)?;
            let len_at = self.placeholder()?;
            self.push(Frame::List {
                tag,
                packed: true,
                len_at,
            })
        } else {
            // Repeated messages/strings: each element carries the key.
            self.push(Frame::List {
                tag,
                packed: false,
                len_at: NONE,
            })
        }
    }

    fn end_list(&mut self) -> Result<()> {
        self.depth -= 1;
        match self.stack[self.depth] {
            Frame::List { len_at, .. } => self.patch(len_at),
            Frame::Msg { .. } => Err(Error::Invalid("end_list() closes a message")),
        }
    }

    fn u64(&mut self, v: u64) -> Result<()> {
        self.key_for(VARINT)?;
        self.varint(v)
    }

    fn i64(&mut self, v: i64) -> Result<()> {
        self.key_for(VARINT)?;
        self.varint(((v << 1) ^ (v >> 63)) as u64)
    }

    fn f64(&mut self, v: f64) -> Result<()> {
        self.key_for(FIXED64)?;
        self.out.extend(&v.to_le_bytes())
    }

    fn f32(&mut self, v: f32) -> Result<()> {
        self.key_for(FIXED32)?;
        self.out.extend(&v.to_le_bytes())
    }

    fn bool(&mut self, v: bool) -> Result<()> {
        self.key_for(VARINT)?;
        self.out.push(v as u8)
    }

    fn str(&mut self, v: &str) -> Result<()> {
        self.key_for(LEN)?;
        self.varint(v.len() as u64)?;
        self.out.extend(v.as_bytes())
    }

    fn null(&mut self) -> Result<()> {
        // Absent: drop the pending key.
        self.pending = None;
        Ok(())
    }
}

/// Wire type meaning "inside a packed run": each read uses the natural
/// encoding of the requested type.
const PACKED: u8 = 255;

pub struct ProtoReader<'de> {
    s: &'de [u8],
    pos: usize,
    /// End offset of each open message.
    limits: [usize; MAX_DEPTH],
    depth: usize,
    started: bool,
    wt: u8,
}

impl<'de> ProtoReader<'de> {
    pub fn new(s: &'de [u8]) -> Self {
        Self {
            s,
            pos: 0,
            limits: [0; MAX_DEPTH],
            depth: 0,
            started: false,
            wt: VARINT,
        }
    }

    fn take(&mut self, n: usize) -> Result<&'de [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::Eof)?;
        let limit = if self.depth > 0 {
            self.limits[self.depth - 1]
        } else {
            self.s.len()
        };
        if end > limit {
            return Err(Error::Eof);
        }
        let bytes = &self.s[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.take(1)?[0];
            v |= ((b & 0x7f) as u64) << shift;
            if b < 0x80 {
                return Ok(v);
            }
        }
        Err(Error::Invalid("varint too long"))
    }

    fn len(&mut self) -> Result<usize> {
        if self.wt != LEN {
            return Err(Error::Invalid("expected a length-delimited protobuf field"));
        }
        usize::try_from(self.varint()?).map_err(|_| Error::Eof)
    }

    fn fixed64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn fixed32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn varint_field(&mut self) -> Result<u64> {
        match self.wt {
            VARINT | PACKED => self.varint(),
            _ => Err(Error::Invalid("expected a varint protobuf field")),
        }
    }
}

impl<'de> Reader<'de> for ProtoReader<'de> {
    fn begin(&mut self) -> Result<()> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::Depth);
        }
        let end = if !self.started {
            self.started = true;
            self.s.len()
        } else {
            let len = self.len()?;
            self.pos.checked_add(len).ok_or(Error::Eof)?
        };
        if self.depth > 0 && end > self.limits[self.depth - 1] || end > self.s.len() {
            return Err(Error::Eof);
        }
        self.limits[self.depth] = end;
        self.depth += 1;
        Ok(())
    }

    fn next_key(&mut self) -> Result<Option<Key<'de>>> {
        if self.pos >= self.limits[self.depth - 1] {
            self.depth -= 1;
            return Ok(None);
        }
        let key = self.varint()?;
        self.wt = (key & 7) as u8;
        let tag = u32::try_from(key >> 3).map_err(|_| Error::Invalid("field number"))?;
        if tag == 0 {
            return Err(Error::Invalid("field number 0"));
        }
        Ok(Some(Key::Tag(tag)))
    }

    fn list<F: FnMut(&mut Self) -> Result<()>>(&mut self, packed: bool, mut each: F) -> Result<()> {
        if packed && self.wt == LEN {
            let len = self.len()?;
            let end = self.pos.checked_add(len).ok_or(Error::Eof)?;
            if self.depth == 0 || end > self.limits[self.depth - 1] {
                return Err(Error::Eof);
            }
            self.wt = PACKED;
            while self.pos < end {
                each(self)?;
            }
            if self.pos != end {
                return Err(Error::Invalid("packed run overran its length"));
            }
            Ok(())
        } else {
            each(self)
        }
    }

    fn null(&mut self) -> Result<bool> {
        Ok(false)
    }

    fn u64(&mut self) -> Result<u64> {
        self.varint_field()
    }

    fn i64(&mut self) -> Result<i64> {
        let v = self.varint_field()?;
        Ok((v >> 1) as i64 ^ -((v & 1) as i64))
    }

    fn f64(&mut self) -> Result<f64> {
        match self.wt {
            FIXED64 | PACKED => Ok(f64::from_bits(self.fixed64()?)),
            FIXED32 => Ok(f32::from_bits(self.fixed32()?) as f64),
            _ => Err(Error::Invalid("expected a double protobuf field")),
        }
    }

    fn f32(&mut self) -> Result<f32> {
        match self.wt {
            FIXED32 | PACKED => Ok(f32::from_bits(self.fixed32()?)),
            FIXED64 => Ok(f64::from_bits(self.fixed64()?) as f32),
            _ => Err(Error::Invalid("expected a float protobuf field")),
        }
    }

    fn bool(&mut self) -> Result<bool> {
        Ok(self.varint_field()? != 0)
    }

    fn str(&mut self) -> Result<Cow<'de, str>> {
        let len = self.len()?;
        std::str::from_utf8(self.take(len)?)
            .map(Cow::Borrowed)
            .map_err(|_| Error::Invalid("invalid UTF-8"))
    }

    fn skip(&mut self) -> Result<()> {
        match self.wt {
            VARINT => self.varint().map(drop),
            FIXED64 => self.take(8).map(drop),
            FIXED32 => self.take(4).map(drop),
            LEN => {
                let len = self.len()?;
                self.take(len).map(drop)
            }
            _ => Err(Error::Invalid("unsupported protobuf wire type")),
        }
    }
}
