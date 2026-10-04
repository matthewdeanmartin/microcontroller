//! Wire formats: one schema, five encodings.
//!
//! Every message is declared once with [`message!`](crate::message). That one
//! declaration gives the Rust struct, an encoder and decoder for each
//! [`Format`], and a [`Schema`] that the server publishes at
//! `/api/v1/schema` so browsers can decode protobuf and integer-keyed CBOR
//! without generated code.
//!
//! All encoders stream straight into one caller-owned buffer: no document
//! tree, no per-field allocation. That is deliberate. On a 2 MiB board the
//! cost that hurt was memory held by concurrent responses, not CPU spent on
//! encoding, so the formats are compared on equal (streaming) footing.
//!
//! Responses too big or too dynamic to build as a struct (a page of points
//! read straight out of a store) implement [`Encode`] by hand against the
//! [`Writer`] trait; see `docs/RECIPES.md`.
use std::borrow::Cow;

mod cbor;
mod json;
mod msgpack;
mod proto;
pub mod schema;

#[cfg(feature = "gzip")]
pub mod gzip;

pub use schema::{proto_text, schema_doc, FieldInfo, Schema, Ty};

/// The encodings a client can ask for. Gzip is not a format: it is a
/// transfer coding layered on any of these (see [`Compression`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Json,
    MsgPack,
    /// CBOR with field names as map keys (self-describing, like JSON).
    Cbor,
    /// CBOR with field tags as integer map keys: needs the schema to read.
    CborInt,
    Protobuf,
}

impl Format {
    pub const ALL: [Format; 5] = [
        Format::Json,
        Format::MsgPack,
        Format::Cbor,
        Format::CborInt,
        Format::Protobuf,
    ];

    /// The `?fmt=` name.
    pub fn name(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::MsgPack => "msgpack",
            Format::Cbor => "cbor",
            Format::CborInt => "cbor-int",
            Format::Protobuf => "protobuf",
        }
    }

    /// The `Content-Type` the server sends.
    pub fn mime(self) -> &'static str {
        match self {
            Format::Json => "application/json",
            Format::MsgPack => "application/msgpack",
            Format::Cbor => "application/cbor",
            Format::CborInt => "application/cbor; keys=int",
            Format::Protobuf => "application/x-protobuf",
        }
    }

    pub fn from_name(name: &str) -> Option<Format> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "json" => Format::Json,
            "msgpack" | "mp" | "messagepack" => Format::MsgPack,
            "cbor" => Format::Cbor,
            "cbor-int" | "cborint" | "cbor_int" => Format::CborInt,
            "protobuf" | "proto" | "pb" => Format::Protobuf,
            _ => return None,
        })
    }

    /// Reads a `Content-Type` or one `Accept` media range (parameters
    /// included). `application/cbor; keys=int` selects [`Format::CborInt`].
    pub fn from_mime(value: &str) -> Option<Format> {
        let mut parts = value.split(';');
        let base = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let int_keys = parts.any(|p| p.trim().eq_ignore_ascii_case("keys=int"));
        Some(match base.as_str() {
            "application/json" | "text/json" => Format::Json,
            "application/msgpack" | "application/x-msgpack" | "application/vnd.msgpack" => {
                Format::MsgPack
            }
            "application/cbor" if int_keys => Format::CborInt,
            "application/cbor" => Format::Cbor,
            "application/x-protobuf"
            | "application/protobuf"
            | "application/vnd.google.protobuf" => Format::Protobuf,
            _ => return None,
        })
    }
}

/// Picks the response format. `?fmt=` wins (handy in a browser address bar
/// and for benchmarks); otherwise the highest-q supported `Accept` range;
/// otherwise JSON. An unknown `?fmt=` is an error, an unknown `Accept` is not
/// (browsers send `text/html, */*` and still expect something readable).
pub fn negotiate(accept: &str, fmt: Option<&str>) -> Result<Format> {
    if let Some(name) = fmt {
        return Format::from_name(name).ok_or(Error::Invalid("unknown fmt"));
    }
    let mut best: Option<(f32, Format)> = None;
    for range in accept.split(',') {
        let mut q = 1.0f32;
        let mut media = String::new();
        for (i, part) in range.split(';').enumerate() {
            let part = part.trim();
            if i == 0 {
                media.push_str(part);
            } else if let Some(v) = part.strip_prefix("q=") {
                q = v.parse().unwrap_or(0.0);
            } else {
                media.push(';');
                media.push_str(part);
            }
        }
        if let Some(format) = Format::from_mime(&media) {
            if q > 0.0 && best.is_none_or(|(b, _)| q > b) {
                best = Some((q, format));
            }
        }
    }
    Ok(best.map_or(Format::Json, |(_, f)| f))
}

/// Optional gzip of the encoded body, asked for with `?gz=`.
///
/// Browsers send `Accept-Encoding: gzip` on every request, so honoring that
/// header alone would gzip everything and remove the experiment's control.
/// The server only compresses API responses when `?gz=` is present (and the
/// client accepts gzip), or when the app opts in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    /// Deflate level 1..=9.
    Gzip(u8),
}

impl Compression {
    /// `gz=1|on|true` → level 6, `gz=fast` → 1, `gz=best` → 9, `gz=0|off` → none.
    pub fn from_query(value: Option<&str>) -> Compression {
        match value.map(|v| v.trim().to_ascii_lowercase()) {
            None => Compression::None,
            Some(v) => match v.as_str() {
                "" | "1" | "on" | "true" | "yes" => Compression::Gzip(6),
                "fast" => Compression::Gzip(1),
                "best" => Compression::Gzip(9),
                _ => Compression::None,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The encoded value would exceed the buffer limit.
    Overflow,
    /// Input ended mid-value.
    Eof,
    /// Input is not valid for this format or schema.
    Invalid(&'static str),
    /// Nesting deeper than [`MAX_DEPTH`].
    Depth,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Overflow => f.write_str("response too large for the buffer"),
            Error::Eof => f.write_str("input ended early"),
            Error::Invalid(why) => write!(f, "invalid input: {why}"),
            Error::Depth => f.write_str("nesting too deep"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;

/// Readers and writers refuse deeper nesting (bounded stacks, no recursion
/// blow-ups from hostile input).
pub const MAX_DEPTH: usize = 16;

/// The streaming encoder interface every format implements.
///
/// Call order for a message: `begin(n)`, then for each field `key` followed by
/// exactly one value (a scalar, a nested `begin`..`end`, or a
/// `list`..`end_list`), then `end`. `n` is the number of fields that will be
/// written; formats with length-prefixed maps (MessagePack, CBOR) rely on it.
pub trait Writer {
    /// Protobuf omits fields holding default values; the self-describing
    /// formats always write every field (so `n` stays the schema's count).
    fn skips_defaults(&self) -> bool {
        false
    }
    fn begin(&mut self, fields: usize) -> Result<()>;
    fn end(&mut self) -> Result<()>;
    fn key(&mut self, tag: u32, name: &'static str) -> Result<()>;
    /// `packed`: elements are numeric scalars (protobuf packs them).
    fn list(&mut self, len: usize, packed: bool) -> Result<()>;
    fn end_list(&mut self) -> Result<()>;
    fn u64(&mut self, v: u64) -> Result<()>;
    fn i64(&mut self, v: i64) -> Result<()>;
    fn f64(&mut self, v: f64) -> Result<()>;
    fn f32(&mut self, v: f32) -> Result<()>;
    fn bool(&mut self, v: bool) -> Result<()>;
    fn str(&mut self, v: &str) -> Result<()>;
    /// An absent optional value (protobuf: field omitted).
    fn null(&mut self) -> Result<()>;
}

/// A map key as read from the wire: a name (JSON, MessagePack, CBOR) or a
/// tag (protobuf, integer-keyed CBOR).
#[derive(Debug, Clone, PartialEq)]
pub enum Key<'de> {
    Name(Cow<'de, str>),
    Tag(u32),
}

impl Key<'_> {
    /// The schema tag this key refers to, if the schema has it.
    pub fn tag(&self, schema: &Schema) -> Option<u32> {
        match self {
            Key::Tag(t) => Some(*t),
            Key::Name(n) => schema.fields.iter().find(|f| f.name == n).map(|f| f.tag),
        }
    }
}

/// The pull decoder interface every format implements.
pub trait Reader<'de> {
    /// Enter a message (map / length-delimited submessage).
    fn begin(&mut self) -> Result<()>;
    /// The next key in the current message, or `None` after its end.
    fn next_key(&mut self) -> Result<Option<Key<'de>>>;
    /// Array formats call `each` once per element. Protobuf calls it once per
    /// occurrence of a repeated field, or once per element of a packed run.
    fn list<F: FnMut(&mut Self) -> Result<()>>(&mut self, packed: bool, each: F) -> Result<()>;
    /// Consumes a null if one is next.
    fn null(&mut self) -> Result<bool>;
    fn u64(&mut self) -> Result<u64>;
    fn i64(&mut self) -> Result<i64>;
    fn f64(&mut self) -> Result<f64>;
    fn f32(&mut self) -> Result<f32> {
        self.f64().map(|v| v as f32)
    }
    fn bool(&mut self) -> Result<bool>;
    fn str(&mut self) -> Result<Cow<'de, str>>;
    /// Skip one value of any shape (an unknown field).
    fn skip(&mut self) -> Result<()>;
}

/// Anything that can stream itself into a [`Writer`].
pub trait Encode {
    fn encode<W: Writer>(&self, w: &mut W) -> Result<()>;
}

/// A declared message: encodable, decodable, and described by a schema.
pub trait Message: Encode + Default {
    const SCHEMA: &'static Schema;
    fn decode<'de, R: Reader<'de>>(r: &mut R) -> Result<Self>;
}

/// A value that can sit in a message field.
pub trait Field: Sized {
    fn ty() -> Ty;
    fn write<W: Writer>(&self, w: &mut W) -> Result<()>;
    fn read<'de, R: Reader<'de>>(r: &mut R) -> Result<Self>;
    /// Called for each occurrence of the field's key; lists append.
    fn merge<'de, R: Reader<'de>>(&mut self, r: &mut R) -> Result<()> {
        *self = Self::read(r)?;
        Ok(())
    }
    /// Whether protobuf may omit this value.
    fn is_default(&self) -> bool;
    /// Numeric scalars pack into one protobuf field when repeated.
    const PACKED: bool = false;
}

macro_rules! scalar {
    ($t:ty, $ty:ident, $w:ident, $r:ident, $as_w:ty, $packed:expr) => {
        impl Field for $t {
            fn ty() -> Ty {
                Ty::$ty
            }
            fn write<W: Writer>(&self, w: &mut W) -> Result<()> {
                w.$w(*self as $as_w)
            }
            fn read<'de, R: Reader<'de>>(r: &mut R) -> Result<Self> {
                let v = r.$r()?;
                <$t>::try_from(v).map_err(|_| Error::Invalid("integer out of range"))
            }
            fn is_default(&self) -> bool {
                *self == 0
            }
            const PACKED: bool = $packed;
        }
    };
}
scalar!(u64, U64, u64, u64, u64, true);
scalar!(u32, U32, u64, u64, u64, true);
scalar!(u16, U32, u64, u64, u64, true);
scalar!(u8, U32, u64, u64, u64, true);
scalar!(i64, I64, i64, i64, i64, true);
scalar!(i32, I32, i64, i64, i64, true);

impl Field for f64 {
    fn ty() -> Ty {
        Ty::F64
    }
    fn write<W: Writer>(&self, w: &mut W) -> Result<()> {
        w.f64(*self)
    }
    fn read<'de, R: Reader<'de>>(r: &mut R) -> Result<Self> {
        r.f64()
    }
    fn is_default(&self) -> bool {
        *self == 0.0 && self.is_sign_positive()
    }
    const PACKED: bool = true;
}

impl Field for f32 {
    fn ty() -> Ty {
        Ty::F32
    }
    fn write<W: Writer>(&self, w: &mut W) -> Result<()> {
        w.f32(*self)
    }
    fn read<'de, R: Reader<'de>>(r: &mut R) -> Result<Self> {
        r.f32()
    }
    fn is_default(&self) -> bool {
        *self == 0.0 && self.is_sign_positive()
    }
    const PACKED: bool = true;
}

impl Field for bool {
    fn ty() -> Ty {
        Ty::Bool
    }
    fn write<W: Writer>(&self, w: &mut W) -> Result<()> {
        w.bool(*self)
    }
    fn read<'de, R: Reader<'de>>(r: &mut R) -> Result<Self> {
        r.bool()
    }
    fn is_default(&self) -> bool {
        !*self
    }
    const PACKED: bool = true;
}

impl Field for String {
    fn ty() -> Ty {
        Ty::Str
    }
    fn write<W: Writer>(&self, w: &mut W) -> Result<()> {
        w.str(self)
    }
    fn read<'de, R: Reader<'de>>(r: &mut R) -> Result<Self> {
        Ok(r.str()?.into_owned())
    }
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

impl<T: Field> Field for Vec<T> {
    fn ty() -> Ty {
        Ty::List(Box::new(T::ty()))
    }
    fn write<W: Writer>(&self, w: &mut W) -> Result<()> {
        w.list(self.len(), T::PACKED)?;
        for item in self {
            item.write(w)?;
        }
        w.end_list()
    }
    fn read<'de, R: Reader<'de>>(r: &mut R) -> Result<Self> {
        let mut out = Vec::new();
        out.merge(r)?;
        Ok(out)
    }
    fn merge<'de, R: Reader<'de>>(&mut self, r: &mut R) -> Result<()> {
        r.list(T::PACKED, |r| {
            self.push(T::read(r)?);
            Ok(())
        })
    }
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

/// `None` is `null` in the self-describing formats and an absent field in
/// protobuf. `Some(0)` is still written by protobuf (explicit presence).
impl<T: Field> Field for Option<T> {
    fn ty() -> Ty {
        Ty::Opt(Box::new(T::ty()))
    }
    fn write<W: Writer>(&self, w: &mut W) -> Result<()> {
        match self {
            Some(v) => v.write(w),
            None => w.null(),
        }
    }
    fn read<'de, R: Reader<'de>>(r: &mut R) -> Result<Self> {
        if r.null()? {
            Ok(None)
        } else {
            T::read(r).map(Some)
        }
    }
    fn is_default(&self) -> bool {
        self.is_none()
    }
}

/// Declares a message: a plain struct plus its schema, encoders and
/// decoders. Tags are protobuf field numbers and integer CBOR keys: never
/// reuse or renumber one once clients exist.
///
/// ```
/// miniframework::message! {
///     /// One reading.
///     pub struct Reading {
///         /// Milliseconds since the Unix epoch.
///         1 t: u64,
///         2 v: f64,
///         3 unit: String,
///     }
/// }
/// ```
#[macro_export]
macro_rules! message {
    (
        $(#[doc = $doc:literal])*
        $vis:vis struct $name:ident {
            $( $(#[doc = $fdoc:literal])* $tag:literal $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[doc = $doc])*
        #[derive(Debug, Clone, Default, PartialEq)]
        $vis struct $name {
            $( $(#[doc = $fdoc])* pub $field: $ty, )*
        }

        impl $crate::wire::Encode for $name {
            fn encode<W: $crate::wire::Writer>(&self, w: &mut W) -> $crate::wire::Result<()> {
                let skip = w.skips_defaults();
                w.begin(0usize $( + { let _ = stringify!($field); 1usize } )*)?;
                $(
                    if !(skip && $crate::wire::Field::is_default(&self.$field)) {
                        w.key($tag, stringify!($field))?;
                        $crate::wire::Field::write(&self.$field, w)?;
                    }
                )*
                w.end()
            }
        }

        impl $crate::wire::Message for $name {
            const SCHEMA: &'static $crate::wire::Schema = &$crate::wire::Schema {
                name: stringify!($name),
                doc: concat!($($doc, "\n",)* ""),
                fields: &[
                    $( $crate::wire::FieldInfo {
                        tag: $tag,
                        name: stringify!($field),
                        doc: concat!($($fdoc, "\n",)* ""),
                        ty: <$ty as $crate::wire::Field>::ty,
                    }, )*
                ],
            };
            #[allow(unused_mut)]
            fn decode<'de, R: $crate::wire::Reader<'de>>(r: &mut R) -> $crate::wire::Result<Self> {
                let mut out = Self::default();
                r.begin()?;
                while let Some(key) = r.next_key()? {
                    match key.tag(<Self as $crate::wire::Message>::SCHEMA) {
                        $( Some($tag) => $crate::wire::Field::merge(&mut out.$field, r)?, )*
                        _ => r.skip()?,
                    }
                }
                Ok(out)
            }
        }

        impl $crate::wire::Field for $name {
            fn ty() -> $crate::wire::Ty {
                $crate::wire::Ty::Msg(|| <$name as $crate::wire::Message>::SCHEMA)
            }
            fn write<W: $crate::wire::Writer>(&self, w: &mut W) -> $crate::wire::Result<()> {
                $crate::wire::Encode::encode(self, w)
            }
            fn read<'de, R: $crate::wire::Reader<'de>>(r: &mut R) -> $crate::wire::Result<Self> {
                <$name as $crate::wire::Message>::decode(r)
            }
            fn is_default(&self) -> bool {
                false
            }
        }
    };
}

/// Encodes `value` into `out` (cleared first), refusing to grow past
/// `limit` bytes. Reuse one `out` across requests to avoid reallocation.
pub fn encode<E: Encode + ?Sized>(
    format: Format,
    value: &E,
    out: &mut Vec<u8>,
    limit: usize,
) -> Result<()> {
    out.clear();
    match format {
        Format::Json => value.encode(&mut json::JsonWriter::new(out, limit)),
        Format::MsgPack => value.encode(&mut msgpack::MsgPackWriter::new(out, limit)),
        Format::Cbor => value.encode(&mut cbor::CborWriter::new(out, limit, false)),
        Format::CborInt => value.encode(&mut cbor::CborWriter::new(out, limit, true)),
        Format::Protobuf => value.encode(&mut proto::ProtoWriter::new(out, limit)),
    }
}

/// Convenience: encode into a fresh vector with no practical limit.
pub fn to_vec<E: Encode + ?Sized>(format: Format, value: &E) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    encode(format, value, &mut out, usize::MAX)?;
    Ok(out)
}

/// Decodes one message. CBOR without integer keys and with integer keys use
/// the same reader: it accepts both kinds of key.
pub fn decode<M: Message>(format: Format, bytes: &[u8]) -> Result<M> {
    match format {
        Format::Json => {
            let mut r = json::JsonReader::new(bytes);
            let m = M::decode(&mut r)?;
            r.finish()?;
            Ok(m)
        }
        Format::MsgPack => M::decode(&mut msgpack::MsgPackReader::new(bytes)),
        Format::Cbor | Format::CborInt => M::decode(&mut cbor::CborReader::new(bytes)),
        Format::Protobuf => M::decode(&mut proto::ProtoReader::new(bytes)),
    }
}

/// The bounded output buffer shared by every writer.
pub(crate) struct Out<'a> {
    buf: &'a mut Vec<u8>,
    limit: usize,
}

impl<'a> Out<'a> {
    pub(crate) fn new(buf: &'a mut Vec<u8>, limit: usize) -> Self {
        Self { buf, limit }
    }
    #[inline]
    pub(crate) fn push(&mut self, b: u8) -> Result<()> {
        if self.buf.len() >= self.limit {
            return Err(Error::Overflow);
        }
        self.buf.push(b);
        Ok(())
    }
    #[inline]
    pub(crate) fn extend(&mut self, bytes: &[u8]) -> Result<()> {
        if self.buf.len() + bytes.len() > self.limit {
            return Err(Error::Overflow);
        }
        self.buf.extend_from_slice(bytes);
        Ok(())
    }
    pub(crate) fn len(&self) -> usize {
        self.buf.len()
    }
    /// Inserts `count` zero bytes at `at`, shifting the tail (protobuf
    /// length prefixes that turned out longer than one byte).
    pub(crate) fn insert_zeros(&mut self, at: usize, count: usize) -> Result<()> {
        if self.buf.len() + count > self.limit {
            return Err(Error::Overflow);
        }
        let old = self.buf.len();
        self.buf.resize(old + count, 0);
        self.buf.copy_within(at..old, at + count);
        Ok(())
    }
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        self.buf
    }
}

#[cfg(test)]
mod tests;
