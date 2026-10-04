//! Responses streamed straight from the store (and the synthetic benchmark
//! data) without building a message first.
//!
//! Each view writes exactly the fields its message declares (see
//! `messages.rs` for the tags), so clients decode them as ordinary
//! `SeriesData` / `Rows` / `RowColumns` messages.
use crate::tsdb::{Agg, Store};
use miniframework::wire::{Encode, Result, Writer};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Rows,
    Columns,
}

impl Shape {
    pub fn from_query(v: Option<&str>) -> Option<Shape> {
        match v.unwrap_or("rows") {
            "rows" | "" => Some(Shape::Rows),
            "columns" | "cols" => Some(Shape::Columns),
            _ => None,
        }
    }
}

pub enum Data {
    Raw { count: usize, next: Option<i64> },
    Buckets { step: i64, buckets: Vec<Agg> },
}

pub struct Item {
    pub id: u32,
    pub from: i64,
    pub to: i64,
    pub data: Data,
}

/// `QueryResult { series: [SeriesData] }`.
pub struct QueryView<'a> {
    pub store: &'a Store,
    pub items: Vec<Item>,
    pub shape: Shape,
}

fn u(t: i64) -> u64 {
    t.max(0) as u64
}

impl Encode for QueryView<'_> {
    fn encode<W: Writer>(&self, w: &mut W) -> Result<()> {
        w.begin(1)?;
        w.key(1, "series")?;
        w.list(self.items.len(), false)?;
        for item in &self.items {
            self.series(w, item)?;
        }
        w.end_list()?;
        w.end()
    }
}

impl QueryView<'_> {
    fn series<W: Writer>(&self, w: &mut W, item: &Item) -> Result<()> {
        let key = self.store.series(item.id).map_or("", |s| s.key.as_str());
        let (kind, step, data_fields) = match (&item.data, self.shape) {
            (Data::Raw { .. }, Shape::Rows) => ("raw", 0, 1),
            (Data::Raw { .. }, Shape::Columns) => ("raw", 0, 3),
            (Data::Buckets { step, .. }, Shape::Rows) => ("buckets", *step, 1),
            (Data::Buckets { step, .. }, Shape::Columns) => ("buckets", *step, 6),
        };
        let next = match item.data {
            Data::Raw { next, .. } => next,
            _ => None,
        };
        w.begin(6 + data_fields + next.is_some() as usize)?;
        w.key(1, "id")?;
        w.u64(item.id as u64)?;
        w.key(2, "key")?;
        w.str(key)?;
        w.key(3, "kind")?;
        w.str(kind)?;
        w.key(4, "from")?;
        w.u64(u(item.from))?;
        w.key(5, "to")?;
        w.u64(u(item.to))?;
        w.key(6, "step")?;
        w.u64(u(step))?;
        match &item.data {
            Data::Raw { count, .. } => {
                let points = || self.store.raw(item.id, item.from, item.to).take(*count);
                match self.shape {
                    Shape::Rows => {
                        w.key(7, "points")?;
                        w.list(*count, false)?;
                        for (t, v) in points() {
                            w.begin(2)?;
                            w.key(1, "t")?;
                            w.u64(u(t))?;
                            w.key(2, "v")?;
                            w.f64(v)?;
                            w.end()?;
                        }
                        w.end_list()?;
                    }
                    Shape::Columns => {
                        let t0 = points().next().map_or(item.from, |(t, _)| t);
                        w.key(9, "t0")?;
                        w.u64(u(t0))?;
                        w.key(10, "dt")?;
                        w.list(*count, true)?;
                        let mut prev = t0;
                        for (t, _) in points() {
                            w.u64(u(t - prev))?;
                            prev = t;
                        }
                        w.end_list()?;
                        w.key(11, "v")?;
                        w.list(*count, true)?;
                        for (_, v) in points() {
                            w.f64(v)?;
                        }
                        w.end_list()?;
                    }
                }
            }
            Data::Buckets { buckets, .. } => match self.shape {
                Shape::Rows => {
                    w.key(8, "buckets")?;
                    w.list(buckets.len(), false)?;
                    for b in buckets {
                        w.begin(5)?;
                        w.key(1, "t")?;
                        w.u64(u(b.t))?;
                        w.key(2, "min")?;
                        w.f64(b.min)?;
                        w.key(3, "max")?;
                        w.f64(b.max)?;
                        w.key(4, "avg")?;
                        w.f64(b.sum / b.n as f64)?;
                        w.key(5, "n")?;
                        w.u64(b.n as u64)?;
                        w.end()?;
                    }
                    w.end_list()?;
                }
                Shape::Columns => {
                    let t0 = buckets.first().map_or(item.from, |b| b.t);
                    w.key(9, "t0")?;
                    w.u64(u(t0))?;
                    w.key(10, "dt")?;
                    w.list(buckets.len(), true)?;
                    let mut prev = t0;
                    for b in buckets {
                        w.u64(u(b.t - prev))?;
                        prev = b.t;
                    }
                    w.end_list()?;
                    let column = |w: &mut W, tag, name, f: &dyn Fn(&Agg) -> f64| -> Result<()> {
                        w.key(tag, name)?;
                        w.list(buckets.len(), true)?;
                        for b in buckets {
                            w.f64(f(b))?;
                        }
                        w.end_list()
                    };
                    column(w, 12, "min", &|b| b.min)?;
                    column(w, 13, "max", &|b| b.max)?;
                    column(w, 14, "avg", &|b| b.sum / b.n as f64)?;
                    w.key(15, "n")?;
                    w.list(buckets.len(), true)?;
                    for b in buckets {
                        w.u64(b.n as u64)?;
                    }
                    w.end_list()?;
                }
            },
        }
        if let Some(next) = next {
            w.key(16, "next")?;
            w.u64(u(next))?;
        }
        w.end()
    }
}

const NAMES: [&str; 16] = [
    "attic",
    "kitchen",
    "garage",
    "porch",
    "cellar",
    "study",
    "hallway",
    "nursery",
    "greenhouse",
    "shed",
    "office",
    "pantry",
    "landing",
    "loft",
    "den",
    "laundry",
];
const KINDS: [&str; 4] = ["sensor", "switch", "meter", "camera"];

/// Deterministic synthetic rows: same `seed` and `i`, same row.
pub struct Synth {
    pub n: usize,
    pub seed: u64,
}

struct RowData {
    id: u32,
    name: &'static str,
    kind: &'static str,
    value: f64,
    count: u32,
    ok: bool,
    t: u64,
}

impl Synth {
    fn row(&self, i: usize) -> RowData {
        let mut x = self.seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let a = next();
        let b = next();
        RowData {
            id: i as u32 + 1,
            name: NAMES[(a % 16) as usize],
            kind: KINDS[(a >> 8) as usize % 4],
            // Two decimals, like most real readings.
            value: ((b % 100_000) as f64 - 20_000.0) / 100.0,
            count: (b >> 20) as u32 % 5000,
            ok: b & 1 == 0,
            t: 1_727_900_000_000 + i as u64 * 10_000,
        }
    }
}

/// `Rows { rows: [Row] }`.
pub struct RowsView(pub Synth);

impl Encode for RowsView {
    fn encode<W: Writer>(&self, w: &mut W) -> Result<()> {
        w.begin(1)?;
        w.key(1, "rows")?;
        w.list(self.0.n, false)?;
        let skip = w.skips_defaults();
        for i in 0..self.0.n {
            let r = self.0.row(i);
            // Protobuf leaves out zero/false, as a generated encoder would.
            let fields = 7
                - (skip && r.count == 0) as usize
                - (skip && !r.ok) as usize
                - (skip && r.value == 0.0) as usize;
            w.begin(fields)?;
            w.key(1, "id")?;
            w.u64(r.id as u64)?;
            w.key(2, "name")?;
            w.str(r.name)?;
            w.key(3, "kind")?;
            w.str(r.kind)?;
            if !(skip && r.value == 0.0) {
                w.key(4, "value")?;
                w.f64(r.value)?;
            }
            if !(skip && r.count == 0) {
                w.key(5, "count")?;
                w.u64(r.count as u64)?;
            }
            if !(skip && !r.ok) {
                w.key(6, "ok")?;
                w.bool(r.ok)?;
            }
            w.key(7, "t")?;
            w.u64(r.t)?;
            w.end()?;
        }
        w.end_list()?;
        w.end()
    }
}

/// `RowColumns`: the same data, one list per field.
pub struct ColumnsView(pub Synth);

impl Encode for ColumnsView {
    fn encode<W: Writer>(&self, w: &mut W) -> Result<()> {
        let n = self.0.n;
        let rows = || (0..n).map(|i| self.0.row(i));
        w.begin(7)?;
        w.key(1, "id")?;
        w.list(n, true)?;
        for r in rows() {
            w.u64(r.id as u64)?;
        }
        w.end_list()?;
        w.key(2, "name")?;
        w.list(n, false)?;
        for r in rows() {
            w.str(r.name)?;
        }
        w.end_list()?;
        w.key(3, "kind")?;
        w.list(n, false)?;
        for r in rows() {
            w.str(r.kind)?;
        }
        w.end_list()?;
        w.key(4, "value")?;
        w.list(n, true)?;
        for r in rows() {
            w.f64(r.value)?;
        }
        w.end_list()?;
        w.key(5, "count")?;
        w.list(n, true)?;
        for r in rows() {
            w.u64(r.count as u64)?;
        }
        w.end_list()?;
        w.key(6, "ok")?;
        w.list(n, true)?;
        for r in rows() {
            w.bool(r.ok)?;
        }
        w.end_list()?;
        w.key(7, "t")?;
        w.list(n, true)?;
        for r in rows() {
            w.u64(r.t)?;
        }
        w.end_list()?;
        w.end()
    }
}
