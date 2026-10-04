//! The in-memory metrics store.
//!
//! Raw points live in fixed-size Gorilla blocks drawn from one shared pool.
//! When the pool is full the globally oldest block is evicted, so raw
//! retention adapts to however many series are active. Every point also
//! updates a coarse rollup (min/max/sum/count per `rollup_secs`) kept in a
//! per-series ring, so older history survives eviction at lower resolution.
//!
//! RAM only, by design: a reboot starts empty (the board's own health
//! series refills within seconds).
use crate::gorilla::{self, EncState, WORST_POINT_BITS};
use miniframework::influx;
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_series: usize,
    pub block_bytes: usize,
    pub blocks: usize,
    pub rollup_secs: u32,
    pub rollup_slots: usize,
}

impl Limits {
    /// Bytes this configuration holds once full.
    pub fn bytes(&self) -> usize {
        self.blocks * (self.block_bytes + std::mem::size_of::<Meta>())
            + self.max_series * self.rollup_slots * std::mem::size_of::<Rollup>()
    }
}

/// Per-block bookkeeping (24 bytes). The encoder state of a series' newest
/// block lives on the series; closed blocks only need these counts.
#[derive(Clone, Copy, Default)]
struct Meta {
    first_t: i64,
    last_t: i64,
    count: u32,
    bits: u32,
}

/// One coarse bucket. 20 bytes: f32 is plenty for sensor aggregates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rollup {
    /// Unix seconds at the bucket start.
    pub start: u32,
    pub min: f32,
    pub max: f32,
    pub sum: f32,
    pub n: u16,
}

pub struct Series {
    pub key: String,
    pub measurement: String,
    /// Canonical `k=v,k2=v2` (sorted, Influx-escaped).
    pub tags: String,
    pub field: String,
    blocks: VecDeque<u32>,
    /// Encoder state of the newest block.
    enc: EncState,
    rollups: VecDeque<Rollup>,
    /// The first point ever accepted (raw may since have been evicted).
    pub first_t: i64,
    pub last_t: i64,
    pub last_v: f64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Not newer than the series' last point.
    OutOfOrder,
    NotFinite,
    TooManySeries,
    /// Every block belongs to some series' newest block.
    StoreFull,
    BadKey,
}

impl Reject {
    pub fn reason(self) -> &'static str {
        match self {
            Reject::OutOfOrder => "timestamp not newer than the series' last point",
            Reject::NotFinite => "value is NaN or infinite",
            Reject::TooManySeries => "series limit reached",
            Reject::StoreFull => "store full",
            Reject::BadKey => "empty measurement or field name",
        }
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Counters {
    pub accepted: u64,
    pub rejected: u64,
    pub evicted_blocks: u64,
    pub created_series: u64,
}

/// An aggregate over one query bucket.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Agg {
    pub t: i64,
    pub min: f64,
    pub max: f64,
    pub sum: f64,
    pub n: u32,
}

pub struct Store {
    pub limits: Limits,
    arena: Vec<u8>,
    metas: Vec<Meta>,
    free: Vec<u32>,
    series: Vec<Option<Series>>,
    index: HashMap<String, u32>,
    pub counters: Counters,
}

/// `measurement[,tags] field`, each part escaped, as the unique key.
pub fn series_key(measurement: &str, tags: &str, field: &str) -> String {
    let mut key = influx::escape_tag(measurement).into_owned();
    if !tags.is_empty() {
        key.push(',');
        key.push_str(tags);
    }
    key.push(' ');
    key.push_str(&influx::escape_tag(field));
    key
}

/// Sorts and escapes tag pairs into the canonical tag string.
pub fn canonical_tags<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut pairs: Vec<_> = pairs
        .into_iter()
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .collect();
    pairs.sort();
    pairs.dedup_by(|a, b| a.0 == b.0);
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", influx::escape_tag(k), influx::escape_tag(v)))
        .collect::<Vec<_>>()
        .join(",")
}

/// Parses `k=v,k2=v2` (Influx escapes allowed) into the canonical form.
pub fn canonical_tag_string(tags: &str) -> Result<String, Reject> {
    if tags.trim().is_empty() {
        return Ok(String::new());
    }
    // Reuse the line parser: "m,<tags> f=0".
    let line = format!("m,{tags} f=0");
    let parsed = influx::parse_line(&line)
        .ok()
        .flatten()
        .ok_or(Reject::BadKey)?;
    Ok(canonical_tags(
        parsed.tags.iter().map(|(k, v)| (k.as_ref(), v.as_ref())),
    ))
}

impl Store {
    pub fn new(limits: Limits) -> Self {
        assert!(limits.block_bytes * 8 > WORST_POINT_BITS * 2);
        assert!(limits.blocks >= 2 && limits.blocks < u32::MAX as usize);
        Self {
            arena: vec![0; limits.blocks * limits.block_bytes],
            metas: vec![Meta::default(); limits.blocks],
            free: (0..limits.blocks as u32).rev().collect(),
            series: Vec::new(),
            index: HashMap::new(),
            counters: Counters::default(),
            limits,
        }
    }

    pub fn series(&self, id: u32) -> Option<&Series> {
        self.series.get(id as usize)?.as_ref()
    }

    /// Live series with their ids.
    pub fn all(&self) -> impl Iterator<Item = (u32, &Series)> {
        self.series
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (i as u32, s)))
    }

    pub fn find(&self, key: &str) -> Option<u32> {
        self.index.get(key).copied()
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn blocks_used(&self) -> usize {
        self.limits.blocks - self.free.len()
    }

    fn block(&self, id: u32) -> &[u8] {
        let start = id as usize * self.limits.block_bytes;
        &self.arena[start..start + self.limits.block_bytes]
    }

    /// Takes a free block, evicting the oldest non-head block if needed.
    fn alloc(&mut self) -> Option<u32> {
        if let Some(id) = self.free.pop() {
            return Some(id);
        }
        let victim = self
            .series
            .iter_mut()
            .flatten()
            .filter(|s| s.blocks.len() >= 2)
            .min_by_key(|s| self.metas[s.blocks[0] as usize].last_t)?;
        let id = victim.blocks.pop_front()?;
        self.counters.evicted_blocks += 1;
        let start = id as usize * self.limits.block_bytes;
        self.arena[start..start + self.limits.block_bytes].fill(0);
        Some(id)
    }

    /// Finds or creates a series. `tags` must be canonical.
    pub fn series_id(
        &mut self,
        measurement: &str,
        tags: &str,
        field: &str,
    ) -> Result<(u32, bool), Reject> {
        if measurement.is_empty() || field.is_empty() {
            return Err(Reject::BadKey);
        }
        let key = series_key(measurement, tags, field);
        if let Some(&id) = self.index.get(&key) {
            return Ok((id, false));
        }
        if self.index.len() >= self.limits.max_series {
            return Err(Reject::TooManySeries);
        }
        let series = Series {
            key: key.clone(),
            measurement: measurement.into(),
            tags: tags.into(),
            field: field.into(),
            blocks: VecDeque::new(),
            enc: EncState::default(),
            rollups: VecDeque::with_capacity(self.limits.rollup_slots),
            first_t: i64::MAX,
            last_t: i64::MIN,
            last_v: f64::NAN,
            total: 0,
        };
        let id = match self.series.iter().position(Option::is_none) {
            Some(slot) => {
                self.series[slot] = Some(series);
                slot as u32
            }
            None => {
                self.series.push(Some(series));
                (self.series.len() - 1) as u32
            }
        };
        self.index.insert(key, id);
        self.counters.created_series += 1;
        Ok((id, true))
    }

    /// Appends one point (milliseconds since the Unix epoch).
    pub fn insert(&mut self, id: u32, t: i64, v: f64) -> Result<(), Reject> {
        let result = self.insert_inner(id, t, v);
        match result {
            Ok(()) => self.counters.accepted += 1,
            Err(_) => self.counters.rejected += 1,
        }
        result
    }

    fn insert_inner(&mut self, id: u32, t: i64, v: f64) -> Result<(), Reject> {
        if !v.is_finite() {
            return Err(Reject::NotFinite);
        }
        let block_bits = self.limits.block_bytes * 8;
        let (head, needs_block) = {
            let s = self.series(id).ok_or(Reject::BadKey)?;
            if t <= s.last_t {
                return Err(Reject::OutOfOrder);
            }
            match s.blocks.back() {
                Some(&b) => {
                    let full = s.enc.bits + WORST_POINT_BITS > block_bits;
                    (Some(b), full || !gorilla::fits_time(&s.enc, t))
                }
                None => (None, true),
            }
        };
        let block = if needs_block {
            let b = self.alloc().ok_or(Reject::StoreFull)?;
            self.metas[b as usize] = Meta {
                first_t: t,
                last_t: t,
                count: 0,
                bits: 0,
            };
            let s = self.series[id as usize].as_mut().unwrap();
            s.blocks.push_back(b);
            s.enc = EncState::default();
            b
        } else {
            head.unwrap()
        };
        let meta = &mut self.metas[block as usize];
        let start = block as usize * self.limits.block_bytes;
        let buf = &mut self.arena[start..start + self.limits.block_bytes];
        let enc = &mut self.series[id as usize].as_mut().unwrap().enc;
        gorilla::append(buf, enc, meta.first_t, t, v);
        meta.last_t = t;
        meta.count = enc.count;
        meta.bits = enc.bits as u32;

        let rollup_secs = self.limits.rollup_secs as i64;
        let slots = self.limits.rollup_slots;
        let s = self.series[id as usize].as_mut().unwrap();
        s.first_t = s.first_t.min(t);
        s.last_t = t;
        s.last_v = v;
        s.total += 1;
        let start = ((t.div_euclid(1000)).div_euclid(rollup_secs) * rollup_secs)
            .clamp(0, u32::MAX as i64) as u32;
        let value = v as f32;
        match s.rollups.back_mut() {
            Some(r) if r.start == start => {
                r.min = r.min.min(value);
                r.max = r.max.max(value);
                r.sum += value;
                r.n = r.n.saturating_add(1);
            }
            _ => {
                if s.rollups.len() >= slots {
                    s.rollups.pop_front();
                }
                s.rollups.push_back(Rollup {
                    start,
                    min: value,
                    max: value,
                    sum: value,
                    n: 1,
                });
            }
        }
        Ok(())
    }

    /// Removes a series and frees its blocks.
    pub fn remove(&mut self, id: u32) -> bool {
        let Some(series) = self.series.get_mut(id as usize).and_then(Option::take) else {
            return false;
        };
        self.index.remove(&series.key);
        for b in series.blocks {
            let start = b as usize * self.limits.block_bytes;
            self.arena[start..start + self.limits.block_bytes].fill(0);
            self.free.push(b);
        }
        true
    }

    /// Raw points in `[from, to)`, oldest first.
    pub fn raw(&self, id: u32, from: i64, to: i64) -> impl Iterator<Item = (i64, f64)> + '_ {
        let blocks = self.series(id).map(|s| &s.blocks);
        blocks
            .into_iter()
            .flatten()
            .filter(move |&&b| {
                let m = &self.metas[b as usize];
                m.last_t >= from && m.first_t < to
            })
            .flat_map(move |&b| {
                let m = &self.metas[b as usize];
                gorilla::points(self.block(b), m.bits as usize, m.first_t, m.count)
            })
            .filter(move |&(t, _)| t >= from && t < to)
    }

    /// Number of raw points in `[from, to)` (decodes only the edge blocks).
    pub fn raw_count(&self, id: u32, from: i64, to: i64) -> usize {
        let Some(s) = self.series(id) else { return 0 };
        s.blocks
            .iter()
            .map(|&b| {
                let m = &self.metas[b as usize];
                if m.last_t < from || m.first_t >= to {
                    0
                } else if m.first_t >= from && m.last_t < to {
                    m.count as usize
                } else {
                    gorilla::points(self.block(b), m.bits as usize, m.first_t, m.count)
                        .filter(|&(t, _)| t >= from && t < to)
                        .count()
                }
            })
            .sum()
    }

    /// Oldest raw timestamp still held for a series.
    pub fn raw_start(&self, id: u32) -> Option<i64> {
        let s = self.series(id)?;
        s.blocks.front().map(|&b| self.metas[b as usize].first_t)
    }

    pub fn raw_points(&self, id: u32) -> u64 {
        self.series(id).map_or(0, |s| {
            s.blocks
                .iter()
                .map(|&b| self.metas[b as usize].count as u64)
                .sum()
        })
    }

    pub fn raw_bits(&self, id: u32) -> u64 {
        self.series(id).map_or(0, |s| {
            s.blocks
                .iter()
                .map(|&b| self.metas[b as usize].bits as u64)
                .sum()
        })
    }

    /// Oldest time with any data: the first point, unless rollups have
    /// since dropped it too.
    pub fn oldest(&self, id: u32) -> Option<i64> {
        let s = self.series(id)?;
        if s.total == 0 {
            return None;
        }
        let rollup = s
            .rollups
            .front()
            .map_or(s.first_t, |r| r.start as i64 * 1000);
        Some(s.first_t.max(rollup))
    }

    /// Whether raw points still cover everything from `from` on.
    pub fn raw_covers(&self, id: u32, from: i64) -> bool {
        match (self.series(id), self.raw_start(id)) {
            (Some(s), Some(raw)) => raw <= from.max(s.first_t),
            _ => false,
        }
    }

    pub fn rollups(&self, id: u32) -> impl Iterator<Item = &Rollup> {
        self.series(id).into_iter().flat_map(|s| s.rollups.iter())
    }

    /// Aggregates `[from, to)` into buckets of `step` ms; empty buckets are
    /// left out. Raw points where the pool still has them, rollups before.
    pub fn buckets(&self, id: u32, from: i64, to: i64, step: i64) -> Vec<Agg> {
        let step = step.max(1);
        let n = ((to - from + step - 1) / step).clamp(0, 100_000) as usize;
        let mut out: Vec<Agg> = (0..n)
            .map(|i| Agg {
                t: from + i as i64 * step,
                min: f64::INFINITY,
                max: f64::NEG_INFINITY,
                sum: 0.0,
                n: 0,
            })
            .collect();
        let raw_start = self.raw_start(id).unwrap_or(i64::MAX);
        let rollup_ms = self.limits.rollup_secs as i64 * 1000;
        let mut add = |t: i64, min: f64, max: f64, sum: f64, count: u32| {
            if t < from || t >= to {
                return;
            }
            let a = &mut out[((t - from) / step) as usize];
            a.min = a.min.min(min);
            a.max = a.max.max(max);
            a.sum += sum;
            a.n += count;
        };
        // Rollups answer for time before `cutoff`, raw points from it on.
        // The cutoff is the first rollup boundary at or after the oldest raw
        // point, so the partly-evicted bucket comes whole from its rollup.
        let oldest_rollup = self
            .rollups(id)
            .next()
            .map_or(i64::MAX, |r| r.start as i64 * 1000);
        let cutoff = if raw_start == i64::MAX {
            i64::MAX
        } else if oldest_rollup <= raw_start {
            (raw_start + rollup_ms - 1).div_euclid(rollup_ms) * rollup_ms
        } else {
            raw_start
        };
        for r in self.rollups(id) {
            let t = r.start as i64 * 1000;
            if t < cutoff {
                add(t, r.min as f64, r.max as f64, r.sum as f64, r.n as u32);
            }
        }
        for (t, v) in self.raw(id, from.max(cutoff), to) {
            add(t, v, v, v, 1);
        }
        out.retain(|a| a.n > 0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Store {
        Store::new(Limits {
            max_series: 4,
            block_bytes: 64,
            blocks: 8,
            rollup_secs: 60,
            rollup_slots: 10,
        })
    }

    const T0: i64 = 1_727_900_000_000;

    #[test]
    fn insert_and_read_back() {
        let mut s = small();
        let (id, created) = s.series_id("temp", "room=attic", "value").unwrap();
        assert!(created);
        assert_eq!(s.series(id).unwrap().key, "temp,room=attic value");
        for i in 0..20 {
            s.insert(id, T0 + i * 10_000, 20.0 + i as f64).unwrap();
        }
        let points: Vec<_> = s.raw(id, 0, i64::MAX).collect();
        assert_eq!(points.len(), 20);
        assert!(s.raw_covers(id, 0), "nothing evicted: raw covers all time");
        assert_eq!(s.oldest(id), Some(T0));
        assert_eq!(points[19], (T0 + 190_000, 39.0));
        assert_eq!(s.raw_count(id, T0 + 50_000, T0 + 100_000), 5);
        assert_eq!(s.raw(id, T0 + 50_000, T0 + 100_000).count(), 5);
        assert_eq!(s.insert(id, T0, 1.0), Err(Reject::OutOfOrder));
        assert_eq!(
            s.insert(id, T0 + 1_000_000, f64::NAN),
            Err(Reject::NotFinite)
        );
        assert_eq!(s.counters.accepted, 20);
        assert_eq!(s.counters.rejected, 2);
    }

    #[test]
    fn eviction_keeps_newest_and_rollups_keep_history() {
        let mut s = small();
        let (a, _) = s.series_id("a", "", "v").unwrap();
        let (b, _) = s.series_id("b", "", "v").unwrap();
        let mut seed = 1u64;
        for i in 0..2000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let v = (seed >> 33) as f64 / 7.0;
            s.insert(a, T0 + i * 1000, v).unwrap();
            s.insert(b, T0 + i * 1000, v).unwrap();
        }
        assert!(s.counters.evicted_blocks > 0);
        // Both series still have their newest point, and raw points are a
        // contiguous recent window.
        for id in [a, b] {
            let points: Vec<_> = s.raw(id, 0, i64::MAX).collect();
            assert_eq!(points.last().unwrap().0, T0 + 1999 * 1000);
            assert!(points.windows(2).all(|w| w[1].0 == w[0].0 + 1000));
        }
        // Rollups: one per minute, last 10 kept.
        assert_eq!(s.rollups(a).count(), 10);
        let total: u32 = s.rollups(a).map(|r| r.n as u32).sum();
        assert!(total <= 600);
    }

    #[test]
    fn buckets_blend_rollups_and_raw() {
        let mut s = Store::new(Limits {
            max_series: 2,
            block_bytes: 128,
            blocks: 4,
            rollup_secs: 60,
            rollup_slots: 1000,
        });
        let (id, _) = s.series_id("m", "", "v").unwrap();
        // Minute-aligned, so each minute holds exactly the values 0..=59.
        const M0: i64 = 1_727_899_980_000;
        for i in 0..3600 {
            s.insert(id, M0 + i * 1000, (i % 60) as f64).unwrap();
        }
        let raw_start = s.raw_start(id).unwrap();
        assert!(raw_start > M0, "old raw blocks were evicted");
        assert!(!s.raw_covers(id, M0));
        assert!(s.raw_covers(id, raw_start));
        assert_eq!(s.oldest(id), Some(M0));
        let buckets = s.buckets(id, M0, M0 + 3_600_000, 600_000);
        assert_eq!(buckets.len(), 6);
        for b in &buckets {
            assert_eq!(b.min, 0.0);
            assert_eq!(b.max, 59.0);
            assert!((b.sum / b.n as f64 - 29.5).abs() < 0.01);
        }
        // Every point counted exactly once, from one source or the other.
        let n: u32 = buckets.iter().map(|b| b.n).sum();
        assert_eq!(n, 3600);
    }

    #[test]
    fn series_limits_and_removal() {
        let mut s = small();
        for m in ["a", "b", "c", "d"] {
            s.series_id(m, "", "v").unwrap();
        }
        assert_eq!(s.series_id("e", "", "v"), Err(Reject::TooManySeries));
        assert!(s.remove(1));
        let (id, created) = s.series_id("e", "", "v").unwrap();
        assert!(created);
        assert_eq!(id, 1, "slot reused");
        assert_eq!(s.series_id("", "", "v"), Err(Reject::BadKey));
    }

    #[test]
    fn tags_are_canonical() {
        assert_eq!(
            canonical_tag_string("room=attic,board=s2").unwrap(),
            "board=s2,room=attic"
        );
        assert_eq!(canonical_tag_string("").unwrap(), "");
        assert_eq!(canonical_tag_string(r"a=x\ y").unwrap(), r"a=x\ y");
        assert!(canonical_tag_string("novalue").is_err());
    }
}
