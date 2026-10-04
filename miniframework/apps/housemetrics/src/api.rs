//! The housemetrics HTTP API. Every route answers in the format the client
//! negotiates (`?fmt=` or `Accept`); see `API.md`.
use crate::auth::{Admin, Devices};
use crate::ingest;
use crate::messages::*;
use crate::scrape::Scrapes;
use crate::tsdb::Store;
use crate::views::{ColumnsView, Data, Item, QueryView, RowsView, Shape, Synth};
use miniframework::influx::Precision;
use miniframework::wire::{Message, Schema};
use miniframework::{wall_ms, ApiError, Reply, Request, Service};
use std::sync::{Arc, Mutex};

/// Size limits that differ between a board and a desktop.
#[derive(Clone, Debug)]
pub struct Tuning {
    /// Most raw points one query may return before paging.
    pub raw_page: usize,
    /// Most chart points (buckets) one series may return.
    pub max_points: usize,
    /// Most series in one query.
    pub max_ids: usize,
    /// Most synthetic rows.
    pub max_rows: usize,
}

pub struct App {
    pub store: Arc<Mutex<Store>>,
    pub devices: Devices,
    pub scrapes: Arc<Scrapes>,
    pub admin: Admin,
    pub tuning: Tuning,
}

fn now() -> u64 {
    wall_ms().unwrap_or(0)
}

/// A "nice" bucket width at least `min_ms` (1, 2, 5 × 10ⁿ seconds, or
/// whole minutes/hours), so chart buckets line up with clock time.
pub fn nice_step(min_ms: i64) -> i64 {
    const STEPS: [i64; 18] = [
        1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000,
        1_800_000, 3_600_000, 7_200_000, 10_800_000, 21_600_000, 43_200_000, 86_400_000,
    ];
    STEPS
        .iter()
        .copied()
        .find(|&s| s >= min_ms)
        .unwrap_or_else(|| (min_ms + 86_400_000 - 1) / 86_400_000 * 86_400_000)
}

impl App {
    fn series_list(&self) -> SeriesList {
        let store = self.store.lock().unwrap();
        let series = store
            .all()
            .map(|(id, s)| SeriesInfo {
                id,
                key: s.key.clone(),
                measurement: s.measurement.clone(),
                tags: s.tags.clone(),
                field: s.field.clone(),
                raw_points: store.raw_points(id),
                first_t: store.oldest(id).unwrap_or(0).max(0) as u64,
                last_t: s.last_t.max(0) as u64,
                last_v: s.last_v,
                raw_bytes: (store.raw_bits(id) / 8) as u32,
                total: s.total,
                raw_from: store.raw_start(id).unwrap_or(0).max(0) as u64,
            })
            .collect();
        SeriesList {
            series,
            store: stats(&store),
        }
    }

    fn query(&self, req: &Request<'_>, reply: &mut Reply<'_>) -> Result<(), ApiError> {
        let shape = Shape::from_query(req.param("shape").as_deref())
            .ok_or_else(|| ApiError::bad_request("shape must be rows or columns"))?;
        let ids: Vec<u32> = req
            .param("ids")
            .or_else(|| req.param("id"))
            .ok_or_else(|| ApiError::bad_request("ids=1,2,3 is required"))?
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|_| ApiError::bad_request("ids must be numbers"))?;
        if ids.is_empty() || ids.len() > self.tuning.max_ids {
            return Err(ApiError::bad_request(format!(
                "ask for 1 to {} series",
                self.tuning.max_ids
            )));
        }
        let store = self.store.lock().unwrap();
        let latest = ids
            .iter()
            .filter_map(|&id| store.series(id).map(|s| s.last_t))
            .max()
            .unwrap_or(0);
        let to = req
            .param_num::<i64>("to")?
            .unwrap_or_else(|| wall_ms().map_or(latest + 1, |t| t as i64).max(latest + 1));
        let from = req.param_num::<i64>("from")?.unwrap_or(to - 3_600_000);
        if from >= to {
            return Err(ApiError::bad_request("from must be before to"));
        }
        let max = req
            .param_num::<usize>("max")?
            .unwrap_or(1000)
            .clamp(1, self.tuning.max_points);
        let force_raw = req.param("raw").is_some_and(|v| v != "0");
        let limit = req
            .param_num::<usize>("limit")?
            .unwrap_or(self.tuning.raw_page)
            .clamp(1, self.tuning.raw_page);
        let mut items = Vec::with_capacity(ids.len());
        for id in ids {
            if store.series(id).is_none() {
                return Err(ApiError::not_found(format!("no series {id}")));
            }
            let count = store.raw_count(id, from, to);
            let data = if force_raw || (store.raw_covers(id, from) && count <= max) {
                let take = count.min(limit);
                let next = (count > take).then(|| {
                    store
                        .raw(id, from, to)
                        .nth(take - 1)
                        .map_or(to, |(t, _)| t + 1)
                });
                Data::Raw { count: take, next }
            } else {
                let step = nice_step((to - from + max as i64 - 1) / max as i64);
                let start = from.div_euclid(step) * step;
                Data::Buckets {
                    step,
                    buckets: store.buckets(id, start, to, step),
                }
            };
            items.push(Item { id, from, to, data });
        }
        reply.wire(
            req,
            &QueryView {
                store: &store,
                items,
                shape,
            },
        );
        Ok(())
    }

    fn write(&self, req: &Request<'_>, reply: &mut Reply<'_>) -> Result<(), ApiError> {
        self.devices.writer(req, &self.admin, now())?;
        let clock = wall_ms().map(|t| t as i64);
        // Wire formats by Content-Type; anything else (text/plain, curl's
        // form default, nothing at all) is Influx line protocol.
        let result = if miniframework::Format::from_mime(req.header("Content-Type")).is_none() {
            let precision = Precision::from_query(req.param("precision").as_deref())
                .ok_or_else(|| ApiError::bad_request("precision must be ns, us, ms or s"))?;
            let body = req.body_bytes()?;
            let text = std::str::from_utf8(&body)
                .map_err(|_| ApiError::bad_request("line protocol must be UTF-8"))?;
            ingest::write_lines(&mut self.store.lock().unwrap(), text, precision, clock, &[])
        } else {
            let batch: WriteBatch = req.decode()?;
            ingest::write_batch(&mut self.store.lock().unwrap(), &batch, clock)
        };
        let status = if result.accepted == 0 && result.rejected > 0 {
            400
        } else {
            200
        };
        reply.wire_status(req, status, &result);
        Ok(())
    }

    fn route(&self, req: &Request<'_>, reply: &mut Reply<'_>) -> Result<(), ApiError> {
        match (req.method, req.path) {
            ("GET", "/api/v1/series") => reply.wire(req, &self.series_list()),
            ("GET", "/api/v1/store") => reply.wire(req, &stats(&self.store.lock().unwrap())),
            ("GET", "/api/v1/query") => self.query(req, reply)?,
            ("POST", "/api/v1/write") => self.write(req, reply)?,
            ("GET", "/api/v1/admin") => {
                self.admin.check(req)?;
                reply.empty();
            }
            ("GET", "/api/v1/devices") => {
                self.admin.check(req)?;
                reply.wire(
                    req,
                    &DeviceList {
                        devices: self.devices.list(),
                    },
                );
            }
            ("POST", "/api/v1/devices") => {
                self.admin.check(req)?;
                let new: NewDevice = req.decode()?;
                let (device, token) = self.devices.create(&new.name, now())?;
                reply.wire_status(req, 201, &DeviceToken { device, token });
            }
            ("GET", "/api/v1/scrapes") => reply.wire(
                req,
                &TargetList {
                    targets: self.scrapes.list(),
                },
            ),
            ("POST", "/api/v1/scrapes") => {
                self.admin.check(req)?;
                let new: NewTarget = req.decode()?;
                let target = self.scrapes.add(&new)?;
                reply.wire_status(req, 201, &target);
            }
            ("GET", "/api/v1/bench/rows") => {
                let n = req.param_num::<usize>("n")?.unwrap_or(100);
                if n > self.tuning.max_rows {
                    return Err(ApiError::bad_request(format!(
                        "n is at most {}",
                        self.tuning.max_rows
                    )));
                }
                let synth = Synth {
                    n,
                    seed: req.param_num::<u64>("seed")?.unwrap_or(1).max(1),
                };
                match Shape::from_query(req.param("shape").as_deref()) {
                    Some(Shape::Rows) => reply.wire(req, &RowsView(synth)),
                    Some(Shape::Columns) => reply.wire(req, &ColumnsView(synth)),
                    None => return Err(ApiError::bad_request("shape must be rows or columns")),
                }
            }
            ("DELETE", _) => self.delete(req, reply)?,
            _ => {}
        }
        Ok(())
    }

    fn delete(&self, req: &Request<'_>, reply: &mut Reply<'_>) -> Result<(), ApiError> {
        let id = |parts: Vec<&str>| -> Result<u32, ApiError> {
            match parts.as_slice() {
                [id] => id
                    .parse()
                    .map_err(|_| ApiError::bad_request("id must be a number")),
                _ => Err(ApiError::not_found("No such API route")),
            }
        };
        let found = if let Some(parts) = req.rest("/api/v1/devices") {
            self.admin.check(req)?;
            self.devices.remove(id(parts)?)?
        } else if let Some(parts) = req.rest("/api/v1/scrapes") {
            self.admin.check(req)?;
            self.scrapes.remove(id(parts)?)?
        } else if let Some(parts) = req.rest("/api/v1/series") {
            self.admin.check(req)?;
            self.store.lock().unwrap().remove(id(parts)?)
        } else {
            return Ok(());
        };
        if found {
            reply.empty();
            Ok(())
        } else {
            Err(ApiError::not_found("Nothing with that id"))
        }
    }
}

pub fn stats(store: &Store) -> StoreStats {
    let (points, bits) = store
        .all()
        .map(|(id, _)| (store.raw_points(id), store.raw_bits(id)))
        .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    StoreStats {
        series: store.len() as u32,
        max_series: store.limits.max_series as u32,
        blocks_used: store.blocks_used() as u32,
        blocks_total: store.limits.blocks as u32,
        block_bytes: store.limits.block_bytes as u32,
        raw_points: points,
        bytes_per_point: if points == 0 {
            0.0
        } else {
            bits as f64 / 8.0 / points as f64
        },
        rollup_secs: store.limits.rollup_secs,
        rollup_slots: store.limits.rollup_slots as u32,
        accepted: store.counters.accepted,
        rejected: store.counters.rejected,
        evicted_blocks: store.counters.evicted_blocks,
        capacity_bytes: store.limits.bytes() as u32,
    }
}

impl Service for App {
    fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
        if let Err(e) = self.route(req, reply) {
            reply.fail(req, &e);
        }
    }

    fn schemas(&self) -> Vec<&'static Schema> {
        vec![
            SeriesList::SCHEMA,
            QueryResult::SCHEMA,
            WriteBatch::SCHEMA,
            WriteResult::SCHEMA,
            DeviceList::SCHEMA,
            NewDevice::SCHEMA,
            DeviceToken::SCHEMA,
            TargetList::SCHEMA,
            NewTarget::SCHEMA,
            Rows::SCHEMA,
            RowColumns::SCHEMA,
        ]
    }
}
