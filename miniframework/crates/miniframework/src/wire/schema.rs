//! Runtime description of declared messages, published at `/api/v1/schema`
//! (as a message, in any format) and `/api/v1/schema.proto` (proto3 text).
use super::{Encode, Message, Result, Writer};
use crate::message;

pub struct Schema {
    pub name: &'static str,
    pub doc: &'static str,
    pub fields: &'static [FieldInfo],
}

pub struct FieldInfo {
    pub tag: u32,
    pub name: &'static str,
    pub doc: &'static str,
    /// A function so generic field types can describe themselves.
    pub ty: fn() -> Ty,
}

#[derive(Clone)]
pub enum Ty {
    U32,
    U64,
    I32,
    I64,
    F32,
    F64,
    Bool,
    Str,
    Msg(fn() -> &'static Schema),
    List(Box<Ty>),
    Opt(Box<Ty>),
}

impl std::fmt::Debug for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::Msg(s) => write!(f, "Msg({})", s().name),
            Ty::List(t) => write!(f, "List({t:?})"),
            Ty::Opt(t) => write!(f, "Opt({t:?})"),
            other => f.write_str(other.scalar_name()),
        }
    }
}

impl Ty {
    /// The proto3 scalar name (`message` for nested messages).
    pub fn scalar_name(&self) -> &'static str {
        match self {
            Ty::U32 => "uint32",
            Ty::U64 => "uint64",
            Ty::I32 => "sint32",
            Ty::I64 => "sint64",
            Ty::F32 => "float",
            Ty::F64 => "double",
            Ty::Bool => "bool",
            Ty::Str => "string",
            Ty::Msg(_) => "message",
            Ty::List(t) | Ty::Opt(t) => t.scalar_name(),
        }
    }

    fn message(&self) -> Option<&'static Schema> {
        match self {
            Ty::Msg(s) => Some(s()),
            Ty::List(t) | Ty::Opt(t) => t.message(),
            _ => None,
        }
    }
}

message! {
    /// One field of a message.
    pub struct FieldDoc {
        /// Protobuf field number and integer CBOR key.
        1 tag: u32,
        2 name: String,
        /// proto3 scalar type, or "message".
        3 kind: String,
        /// Message type name when kind is "message".
        4 message: String,
        5 repeated: bool,
        6 optional: bool,
        7 doc: String,
    }
}

message! {
    pub struct MessageDoc {
        1 name: String,
        2 doc: String,
        3 fields: Vec<FieldDoc>,
    }
}

message! {
    /// Every message reachable from the API's response and request types.
    pub struct SchemaDoc {
        1 messages: Vec<MessageDoc>,
    }
}

/// [`SchemaDoc`] as `/api/v1/schema` sends it: every key whose value is the
/// default (an empty `message` or `doc`, `repeated`/`optional` false) is
/// left out. Decoders treat a missing key as its default, so this is the
/// same document at well under half the size: on a C6 the full JSON passed
/// its 32 KiB response limit at ~50 messages (Game Matcher, October 2026).
pub struct CompactSchema<'a>(pub &'a SchemaDoc);

impl Encode for CompactSchema<'_> {
    fn encode<W: Writer>(&self, w: &mut W) -> Result<()> {
        w.begin(1)?;
        w.key(1, "messages")?;
        w.list(self.0.messages.len(), false)?;
        for m in &self.0.messages {
            w.begin(2 + usize::from(!m.doc.is_empty()))?;
            w.key(1, "name")?;
            w.str(&m.name)?;
            if !m.doc.is_empty() {
                w.key(2, "doc")?;
                w.str(&m.doc)?;
            }
            w.key(3, "fields")?;
            w.list(m.fields.len(), false)?;
            for f in &m.fields {
                let count = 3
                    + usize::from(!f.message.is_empty())
                    + usize::from(f.repeated)
                    + usize::from(f.optional)
                    + usize::from(!f.doc.is_empty());
                w.begin(count)?;
                w.key(1, "tag")?;
                w.u64(f.tag as u64)?;
                w.key(2, "name")?;
                w.str(&f.name)?;
                w.key(3, "kind")?;
                w.str(&f.kind)?;
                if !f.message.is_empty() {
                    w.key(4, "message")?;
                    w.str(&f.message)?;
                }
                if f.repeated {
                    w.key(5, "repeated")?;
                    w.bool(true)?;
                }
                if f.optional {
                    w.key(6, "optional")?;
                    w.bool(true)?;
                }
                if !f.doc.is_empty() {
                    w.key(7, "doc")?;
                    w.str(&f.doc)?;
                }
                w.end()?;
            }
            w.end_list()?;
            w.end()?;
        }
        w.end_list()?;
        w.end()
    }
}

/// Every schema reachable from `roots`, each once, in discovery order.
pub fn reachable(roots: &[&'static Schema]) -> Vec<&'static Schema> {
    let mut seen: Vec<&'static Schema> = Vec::new();
    let mut stack: Vec<&'static Schema> = roots.iter().rev().copied().collect();
    while let Some(schema) = stack.pop() {
        if seen.iter().any(|s| s.name == schema.name) {
            continue;
        }
        seen.push(schema);
        for field in schema.fields.iter().rev() {
            if let Some(inner) = (field.ty)().message() {
                stack.push(inner);
            }
        }
    }
    seen
}

pub fn schema_doc(roots: &[&'static Schema]) -> SchemaDoc {
    let mut roots = roots.to_vec();
    roots.push(SchemaDoc::SCHEMA);
    SchemaDoc {
        messages: reachable(&roots)
            .into_iter()
            .map(|s| MessageDoc {
                name: s.name.into(),
                doc: s.doc.trim().into(),
                fields: s
                    .fields
                    .iter()
                    .map(|f| {
                        let ty = (f.ty)();
                        FieldDoc {
                            tag: f.tag,
                            name: f.name.into(),
                            kind: ty.scalar_name().into(),
                            message: ty.message().map(|m| m.name.into()).unwrap_or_default(),
                            repeated: matches!(ty, Ty::List(_)),
                            optional: matches!(ty, Ty::Opt(_)),
                            doc: f.doc.trim().into(),
                        }
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// A proto3 file equivalent to the schema, for `protoc` and other tooling.
pub fn proto_text(package: &str, roots: &[&'static Schema]) -> String {
    let mut out = format!(
        "// Generated by miniframework from the server's message! declarations.\n\
         syntax = \"proto3\";\npackage {package};\n"
    );
    for schema in reachable(roots) {
        out.push('\n');
        for line in schema.doc.lines().filter(|l| !l.trim().is_empty()) {
            out.push_str(&format!("//{line}\n"));
        }
        out.push_str(&format!("message {} {{\n", schema.name));
        for field in schema.fields {
            let ty = (field.ty)();
            for line in field.doc.lines().filter(|l| !l.trim().is_empty()) {
                out.push_str(&format!("  //{line}\n"));
            }
            let label = match ty {
                Ty::List(_) => "repeated ",
                Ty::Opt(_) => "optional ",
                _ => "",
            };
            let name = ty.message().map_or(ty.scalar_name(), |m| m.name);
            out.push_str(&format!(
                "  {label}{name} {} = {};\n",
                field.name, field.tag
            ));
        }
        out.push_str("}\n");
    }
    out
}
