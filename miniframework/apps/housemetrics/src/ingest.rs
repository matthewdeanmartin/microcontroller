//! Turning incoming data into stored points: Influx lines, `WriteBatch`
//! messages, and flattened JSON from scraped boards.
use crate::messages::{WriteBatch, WriteResult};
use crate::tsdb::{canonical_tag_string, canonical_tags, Reject, Store};
use miniframework::influx::{self, Precision};

const MAX_ERRORS: usize = 5;

#[derive(Default)]
pub struct Tally(pub WriteResult);

impl Tally {
    fn ok(&mut self, created: bool) {
        self.0.accepted += 1;
        self.0.created += created as u32;
    }
    fn reject(&mut self, what: impl FnOnce() -> String) {
        self.0.rejected += 1;
        if self.0.errors.len() < MAX_ERRORS {
            self.0.errors.push(what());
        }
    }
}

/// Stores one value, creating its series if needed.
fn put(
    store: &mut Store,
    measurement: &str,
    tags: &str,
    field: &str,
    t: i64,
    v: f64,
) -> Result<bool, Reject> {
    let (id, created) = store.series_id(measurement, tags, field)?;
    store.insert(id, t, v)?;
    Ok(created)
}

/// Influx line protocol. Lines without a timestamp use `now_ms`; with no
/// clock (`None`) such lines are rejected.
pub fn write_lines(
    store: &mut Store,
    text: &str,
    precision: Precision,
    now_ms: Option<i64>,
    extra_tags: &[(&str, &str)],
) -> WriteResult {
    let mut tally = Tally::default();
    for (n, raw) in text.lines().enumerate() {
        let line = match influx::parse_line(raw) {
            Ok(Some(line)) => line,
            Ok(None) => continue,
            Err(why) => {
                tally.reject(|| format!("line {}: {why}", n + 1));
                continue;
            }
        };
        let t = match line.timestamp.map(|t| precision.to_ms(t)).or(now_ms) {
            Some(t) => t,
            None => {
                tally.reject(|| {
                    format!("line {}: no timestamp and the clock is not set yet", n + 1)
                });
                continue;
            }
        };
        let tags = canonical_tags(
            line.tags
                .iter()
                .map(|(k, v)| (k.as_ref(), v.as_ref()))
                .chain(extra_tags.iter().copied()),
        );
        for (field, value) in &line.fields {
            let Some(v) = value.as_f64() else {
                tally.reject(|| {
                    format!(
                        "line {}: field {field} is a string; only numbers are stored",
                        n + 1
                    )
                });
                continue;
            };
            match put(store, &line.measurement, &tags, field, t, v) {
                Ok(created) => tally.ok(created),
                Err(e) => tally.reject(|| format!("line {}: {field}: {}", n + 1, e.reason())),
            }
        }
    }
    tally.0
}

/// A decoded `WriteBatch` (any wire format).
pub fn write_batch(store: &mut Store, batch: &WriteBatch, now_ms: Option<i64>) -> WriteResult {
    let mut tally = Tally::default();
    for (n, s) in batch.samples.iter().enumerate() {
        let t = if s.t == 0 { now_ms } else { Some(s.t as i64) };
        let Some(t) = t else {
            tally.reject(|| format!("sample {n}: t is 0 and the clock is not set yet"));
            continue;
        };
        let tags = match canonical_tag_string(&s.tags) {
            Ok(tags) => tags,
            Err(_) => {
                tally.reject(|| format!("sample {n}: tags must look like k=v,k2=v2"));
                continue;
            }
        };
        let field = if s.f.is_empty() { "value" } else { &s.f };
        match put(store, &s.m, &tags, field, t, s.v) {
            Ok(created) => tally.ok(created),
            Err(e) => tally.reject(|| format!("sample {n}: {}", e.reason())),
        }
    }
    tally.0
}

/// Numeric leaves of a JSON document as `(path, value)`, objects joined
/// with `_`. Arrays, strings and nulls are skipped; booleans become 0/1.
pub fn flatten_json(text: &str, max_fields: usize) -> Result<Vec<(String, f64)>, &'static str> {
    let mut out = Vec::new();
    let mut p = Parser {
        s: text.as_bytes(),
        pos: 0,
    };
    p.value("", 0, &mut out, max_fields)?;
    Ok(out)
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while matches!(self.s.get(self.pos), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.pos).copied()
    }

    fn string(&mut self) -> Result<String, &'static str> {
        self.pos += 1; // opening quote
        let mut out = Vec::new();
        loop {
            match self.s.get(self.pos).copied().ok_or("unterminated string")? {
                b'"' => {
                    self.pos += 1;
                    return Ok(String::from_utf8_lossy(&out).into_owned());
                }
                b'\\' => {
                    self.pos += 2;
                    out.push(b'_');
                }
                b => {
                    out.push(b);
                    self.pos += 1;
                }
            }
        }
    }

    fn value(
        &mut self,
        path: &str,
        depth: usize,
        out: &mut Vec<(String, f64)>,
        max: usize,
    ) -> Result<(), &'static str> {
        if depth > 8 {
            return Err("JSON nested too deeply");
        }
        match self.peek().ok_or("unexpected end of JSON")? {
            b'{' => {
                self.pos += 1;
                if self.peek() == Some(b'}') {
                    self.pos += 1;
                    return Ok(());
                }
                loop {
                    if self.peek() != Some(b'"') {
                        return Err("expected a key");
                    }
                    let key = self.string()?;
                    if self.peek() != Some(b':') {
                        return Err("expected :");
                    }
                    self.pos += 1;
                    let child = if path.is_empty() {
                        key
                    } else {
                        format!("{path}_{key}")
                    };
                    self.value(&child, depth + 1, out, max)?;
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(());
                        }
                        _ => return Err("expected , or }"),
                    }
                }
            }
            b'[' => {
                // Skip arrays (lists of things don't map to one series).
                self.pos += 1;
                if self.peek() == Some(b']') {
                    self.pos += 1;
                    return Ok(());
                }
                let mut ignored = Vec::new();
                loop {
                    self.value("", depth + 1, &mut ignored, 0)?;
                    ignored.clear();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(());
                        }
                        _ => return Err("expected , or ]"),
                    }
                }
            }
            b'"' => self.string().map(drop),
            b't' | b'f' | b'n' => {
                let word: &[u8] = match self.s[self.pos] {
                    b't' => b"true",
                    b'f' => b"false",
                    _ => b"null",
                };
                if !self.s[self.pos..].starts_with(word) {
                    return Err("bad literal");
                }
                self.pos += word.len();
                if word != b"null" && out.len() < max && !path.is_empty() {
                    out.push((path.to_string(), (word == b"true") as u8 as f64));
                }
                Ok(())
            }
            _ => {
                let start = self.pos;
                while matches!(
                    self.s.get(self.pos),
                    Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                ) {
                    self.pos += 1;
                }
                let n: f64 = std::str::from_utf8(&self.s[start..self.pos])
                    .ok()
                    .and_then(|t| t.parse().ok())
                    .ok_or("bad number")?;
                if out.len() < max && !path.is_empty() && n.is_finite() {
                    out.push((path.to_string(), n));
                }
                Ok(())
            }
        }
    }
}

/// Field names become part of series keys: keep them tidy.
pub fn field_name(path: &str) -> String {
    path.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::Sample;
    use crate::tsdb::Limits;

    fn store() -> Store {
        Store::new(Limits {
            max_series: 16,
            block_bytes: 256,
            blocks: 32,
            rollup_secs: 60,
            rollup_slots: 10,
        })
    }

    #[test]
    fn influx_lines() {
        let mut s = store();
        let r = write_lines(
            &mut s,
            "temp,room=attic value=21.5,humidity=40i 1727900000\n\
             # comment\n\
             temp,room=attic value=21.6 1727900010\n\
             bad line\n\
             temp,room=attic note=\"hi\" 1727900020\n\
             temp,room=attic value=99 1727900005\n",
            Precision::S,
            None,
            &[],
        );
        assert_eq!(r.accepted, 3);
        assert_eq!(r.created, 2);
        assert_eq!(r.rejected, 3, "{:?}", r.errors);
        assert!(r.errors[0].starts_with("line 4"));
        let id = s.find("temp,room=attic value").unwrap();
        let points: Vec<_> = s.raw(id, 0, i64::MAX).collect();
        assert_eq!(
            points,
            [(1_727_900_000_000, 21.5), (1_727_900_010_000, 21.6)]
        );
        // No timestamp and no clock.
        let r = write_lines(&mut s, "x v=1", Precision::Ns, None, &[]);
        assert_eq!(r.rejected, 1);
        let r = write_lines(
            &mut s,
            "x v=1",
            Precision::Ns,
            Some(5),
            &[("target", "nanacoin")],
        );
        assert_eq!(r.accepted, 1);
        assert!(s.find("x,target=nanacoin v").is_some());
    }

    #[test]
    fn batches() {
        let mut s = store();
        let batch = WriteBatch {
            samples: vec![
                Sample {
                    m: "temp".into(),
                    tags: "room=attic,board=s2".into(),
                    f: String::new(),
                    t: 1_000,
                    v: 1.0,
                },
                Sample {
                    m: "temp".into(),
                    tags: "board=s2,room=attic".into(),
                    f: String::new(),
                    t: 2_000,
                    v: 2.0,
                },
                Sample {
                    m: String::new(),
                    tags: String::new(),
                    f: String::new(),
                    t: 0,
                    v: 2.0,
                },
            ],
        };
        let r = write_batch(&mut s, &batch, Some(9_000));
        assert_eq!((r.accepted, r.created, r.rejected), (2, 1, 1));
        assert!(s.find("temp,board=s2,room=attic value").is_some());
    }

    #[test]
    fn json_flattening() {
        let doc = r#"{"heap":{"free":1234,"largest":99.5},"ok":true,"name":"x","list":[1,{"a":2}],"none":null,"neg":-3e2}"#;
        let fields = flatten_json(doc, 64).unwrap();
        assert_eq!(
            fields,
            [
                ("heap_free".into(), 1234.0),
                ("heap_largest".into(), 99.5),
                ("ok".into(), 1.0),
                ("neg".into(), -300.0)
            ]
        );
        assert_eq!(flatten_json(doc, 2).unwrap().len(), 2);
        assert!(flatten_json("{\"a\":", 8).is_err());
        assert_eq!(field_name("heap free/x"), "heap_free_x");
    }
}
