//! InfluxDB line protocol: the easiest format for a small board to send.
//!
//! ```text
//! temp,room=attic,board=s2 value=21.5,humidity=40i 1727900000000000000
//! └─┬┘ └──────┬──────────┘ └──────────┬─────────┘ └────────┬────────┘
//! measurement  tags (opt.)         fields         timestamp (opt., ns)
//! ```
//!
//! Escapes follow InfluxDB: `\,` `\ ` `\=` in names and tags; `\"` `\\` in
//! quoted string fields. Lines starting with `#` are comments.
use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Float(f64),
    Int(i64),
    UInt(u64),
    Bool(bool),
    Str(String),
}

impl Value {
    /// Numeric view for storing as a time series; strings have none.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(v) => Some(*v),
            Value::Int(v) => Some(*v as f64),
            Value::UInt(v) => Some(*v as f64),
            Value::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
            Value::Str(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line<'a> {
    pub measurement: Cow<'a, str>,
    pub tags: Vec<(Cow<'a, str>, Cow<'a, str>)>,
    pub fields: Vec<(Cow<'a, str>, Value)>,
    /// Raw timestamp in the request's precision, if the line had one.
    pub timestamp: Option<i64>,
}

/// Timestamp units, from `?precision=` (InfluxDB's default is ns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    Ns,
    Us,
    Ms,
    S,
}

impl Precision {
    pub fn from_query(value: Option<&str>) -> Option<Precision> {
        Some(match value.unwrap_or("ns") {
            "ns" | "n" => Precision::Ns,
            "us" | "u" | "µs" => Precision::Us,
            "ms" => Precision::Ms,
            "s" => Precision::S,
            _ => return None,
        })
    }

    pub fn to_ms(self, t: i64) -> i64 {
        match self {
            Precision::Ns => t / 1_000_000,
            Precision::Us => t / 1_000,
            Precision::Ms => t,
            Precision::S => t.saturating_mul(1000),
        }
    }
}

/// Splits on `sep` where it is neither escaped nor inside double quotes.
fn split_unescaped(s: &str, sep: u8, max: usize) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'"' => quoted = !quoted,
            b if b == sep && !quoted && parts.len() + 1 < max => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

fn unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('\\') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(e @ (',' | ' ' | '=' | '"' | '\\')) => out.push(e),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

fn value(raw: &str) -> Result<Value, &'static str> {
    if let Some(inner) = raw.strip_prefix('"') {
        let inner = inner.strip_suffix('"').ok_or("unterminated string field")?;
        return Ok(Value::Str(unescape(inner).into_owned()));
    }
    match raw {
        "t" | "T" | "true" | "True" | "TRUE" => return Ok(Value::Bool(true)),
        "f" | "F" | "false" | "False" | "FALSE" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if let Some(n) = raw.strip_suffix('i') {
        return n.parse().map(Value::Int).map_err(|_| "bad integer field");
    }
    if let Some(n) = raw.strip_suffix('u') {
        return n.parse().map(Value::UInt).map_err(|_| "bad unsigned field");
    }
    raw.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(Value::Float)
        .ok_or("bad float field")
}

/// Parses one line; `Ok(None)` for blank lines and comments.
pub fn parse_line(line: &str) -> Result<Option<Line<'_>>, &'static str> {
    let line = line.trim_matches(|c| c == ' ' || c == '\t' || c == '\r');
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let sections = split_unescaped(line, b' ', 3);
    if sections.len() < 2 {
        return Err("a line needs a measurement and at least one field");
    }
    let mut series = split_unescaped(sections[0], b',', usize::MAX).into_iter();
    let measurement = unescape(series.next().unwrap_or(""));
    if measurement.is_empty() {
        return Err("empty measurement");
    }
    let mut tags = Vec::new();
    for tag in series {
        let kv = split_unescaped(tag, b'=', 2);
        if kv.len() != 2 || kv[0].is_empty() || kv[1].is_empty() {
            return Err("tags must be key=value");
        }
        tags.push((unescape(kv[0]), unescape(kv[1])));
    }
    let mut fields = Vec::new();
    for field in split_unescaped(sections[1], b',', usize::MAX) {
        let kv = split_unescaped(field, b'=', 2);
        if kv.len() != 2 || kv[0].is_empty() {
            return Err("fields must be key=value");
        }
        fields.push((unescape(kv[0]), value(kv[1])?));
    }
    let timestamp = match sections.get(2).map(|t| t.trim()) {
        None | Some("") => None,
        Some(t) => Some(t.parse().map_err(|_| "bad timestamp")?),
    };
    Ok(Some(Line {
        measurement,
        tags,
        fields,
        timestamp,
    }))
}

/// Escapes a tag key/value or field key.
pub fn escape_tag(s: &str) -> Cow<'_, str> {
    if !s.contains([',', '=', ' ', '\\']) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if matches!(c, ',' | '=' | ' ' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    Cow::Owned(out)
}

/// A float field value in its shortest form (`21`, `21.5`).
pub fn number(v: f64) -> String {
    let mut buf = ryu::Buffer::new();
    let s = buf.format(v);
    s.strip_suffix(".0").unwrap_or(s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_examples() {
        let line =
            parse_line("temp,room=attic,board=s2 value=21.5,humidity=40i 1727900000000000000")
                .unwrap()
                .unwrap();
        assert_eq!(line.measurement, "temp");
        assert_eq!(line.tags.len(), 2);
        assert_eq!(line.tags[0], ("room".into(), "attic".into()));
        assert_eq!(line.fields[0], ("value".into(), Value::Float(21.5)));
        assert_eq!(line.fields[1], ("humidity".into(), Value::Int(40)));
        assert_eq!(line.timestamp, Some(1_727_900_000_000_000_000));
        assert_eq!(
            Precision::Ns.to_ms(line.timestamp.unwrap()),
            1_727_900_000_000
        );
    }

    #[test]
    fn escapes_quotes_and_odd_values() {
        let line = parse_line(
            r#"my\ room,place=up\,stairs,k\=x=v state="on, \"really\"",ok=t,n=-3u,big=1e3"#,
        );
        // -3u is invalid (unsigned), so the whole line is rejected.
        assert!(line.is_err());
        let line =
            parse_line(r#"my\ room,place=up\,stairs,k\=x=v state="on, \"really\"",ok=t,big=1e3"#)
                .unwrap()
                .unwrap();
        assert_eq!(line.measurement, "my room");
        assert_eq!(line.tags[0].1, "up,stairs");
        assert_eq!(line.tags[1].0, "k=x");
        assert_eq!(line.fields[0].1, Value::Str("on, \"really\"".into()));
        assert_eq!(line.fields[1].1, Value::Bool(true));
        assert_eq!(line.fields[2].1.as_f64(), Some(1000.0));
        assert_eq!(line.timestamp, None);
    }

    #[test]
    fn rejects_and_skips() {
        assert_eq!(parse_line("# comment").unwrap(), None);
        assert_eq!(parse_line("   ").unwrap(), None);
        assert!(parse_line("lonely").is_err());
        assert!(parse_line("m,tag value=1").is_err());
        assert!(parse_line("m value=abc").is_err());
        assert!(parse_line("m value=1 notatime").is_err());
        assert!(parse_line("m value=NaN").is_err());
    }

    #[test]
    fn escaping_round_trips() {
        let name = "up stairs,=x";
        let text = format!("m,k={} v=1", escape_tag(name));
        let line = parse_line(&text).unwrap().unwrap();
        assert_eq!(line.tags[0].1, name);
        assert_eq!(number(21.0), "21");
        assert_eq!(number(0.1), "0.1");
    }
}
