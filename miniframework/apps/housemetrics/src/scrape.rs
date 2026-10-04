//! Pulling metrics from other boards on a schedule.
//!
//! A target URL may return Influx lines (`text/plain`, e.g. another
//! miniframework board's `/metrics`) or JSON (e.g. NanaCoin's
//! `/api/v1/diag`), whose numeric fields become series of a measurement
//! named after the target.
use crate::ingest::{field_name, flatten_json, write_lines};
use crate::messages::{NewTarget, StoredTargets, Target};
use crate::tsdb::Store;
use miniframework::fetch::Fetch;
use miniframework::influx::Precision;
use miniframework::kv::Kv;
use miniframework::wire::{self, Format};
use miniframework::{wall_ms, ApiError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const KEY: &str = "scrapes";
pub const MAX_TARGETS: usize = 16;
const BODY_LIMIT: usize = 32 * 1024;
const MAX_FIELDS: usize = 48;

#[derive(Clone)]
struct State {
    target: Target,
    due: Instant,
}

pub struct Scrapes {
    kv: Arc<dyn Kv>,
    inner: Mutex<(Vec<State>, u32, Vec<u32>)>,
}

impl Scrapes {
    pub fn load(kv: Arc<dyn Kv>) -> Self {
        let stored: StoredTargets = kv
            .get(KEY)
            .and_then(|b| wire::decode(Format::Protobuf, &b).ok())
            .unwrap_or_default();
        let states = stored
            .targets
            .into_iter()
            .zip(stored.ids)
            .map(|(t, id)| State {
                target: Target {
                    id,
                    name: t.name,
                    url: t.url,
                    every_s: t.every_s,
                    ..Default::default()
                },
                due: Instant::now(),
            })
            .collect();
        Self {
            kv,
            inner: Mutex::new((states, stored.next_id.max(1), stored.managed_ids)),
        }
    }

    fn save(&self, states: &[State], next_id: u32, managed_ids: &[u32]) -> Result<(), ApiError> {
        let stored = StoredTargets {
            targets: states
                .iter()
                .map(|s| NewTarget {
                    name: s.target.name.clone(),
                    url: s.target.url.clone(),
                    every_s: s.target.every_s,
                })
                .collect(),
            ids: states.iter().map(|s| s.target.id).collect(),
            next_id,
            managed_ids: managed_ids.to_vec(),
        };
        let bytes = wire::to_vec(Format::Protobuf, &stored).map_err(ApiError::from)?;
        self.kv
            .set(KEY, &bytes)
            .map_err(|e| ApiError::new(500, "storage_failed", e.to_string()))
    }

    pub fn list(&self) -> Vec<Target> {
        self.inner
            .lock()
            .unwrap()
            .0
            .iter()
            .map(|s| s.target.clone())
            .collect()
    }

    pub fn add(&self, new: &NewTarget) -> Result<Target, ApiError> {
        let name = field_name(new.name.trim());
        if name.is_empty() {
            return Err(ApiError::bad_request("Name the target"));
        }
        miniframework::fetch::split_url(&new.url).map_err(|_| {
            ApiError::bad_request("url must be http://host[:port]/path or https://...")
        })?;
        let every_s = if new.every_s == 0 {
            10
        } else {
            new.every_s.clamp(5, 3600)
        };
        let mut inner = self.inner.lock().unwrap();
        if inner.0.len() >= MAX_TARGETS {
            return Err(ApiError::new(
                409,
                "too_many_targets",
                format!("At most {MAX_TARGETS} targets"),
            ));
        }
        let target = Target {
            id: inner.1,
            name,
            url: new.url.trim().into(),
            every_s,
            ..Default::default()
        };
        inner.0.push(State {
            target: target.clone(),
            due: Instant::now(),
        });
        inner.1 += 1;
        let (states, next, managed) = &*inner;
        if let Err(e) = self.save(states, *next, managed) {
            inner.0.pop();
            return Err(e);
        }
        Ok(target)
    }

    pub fn remove(&self, id: u32) -> Result<bool, ApiError> {
        let mut inner = self.inner.lock().unwrap();
        let mut states = inner.0.clone();
        states.retain(|s| s.target.id != id);
        if states.len() == inner.0.len() {
            return Ok(false);
        }
        self.save(&states, inner.1, &inner.2)?;
        inner.0 = states;
        Ok(true)
    }

    /// Reconcile file-owned targets atomically, retaining unrelated UI targets.
    /// Matching URLs adopt already-entered targets without duplicate scrapes.
    pub fn configure(&self, targets: &[crate::deployment::ScrapeTarget]) -> Result<(), ApiError> {
        let mut inner = self.inner.lock().unwrap();
        let mut states = inner.0.clone();
        states.retain(|s| {
            !inner.2.contains(&s.target.id)
                || targets
                    .iter()
                    .any(|t| t.name == s.target.name || t.url == s.target.url)
        });
        let mut next = inner.1;
        let mut managed = Vec::new();
        for target in targets {
            let id = if let Some(state) = states
                .iter_mut()
                .find(|s| s.target.name == target.name || s.target.url == target.url)
            {
                state.target.name = target.name.clone();
                state.target.url = target.url.clone();
                state.target.every_s = target.every_s;
                state.target.id
            } else {
                let id = next;
                next += 1;
                states.push(State {
                    target: Target {
                        id,
                        name: target.name.clone(),
                        url: target.url.clone(),
                        every_s: target.every_s,
                        ..Default::default()
                    },
                    due: Instant::now(),
                });
                id
            };
            managed.push(id);
        }
        if states.len() > MAX_TARGETS {
            return Err(ApiError::bad_request(
                "configured and manual targets exceed the 16-target limit",
            ));
        }
        let unchanged = inner.1 == next
            && inner.2 == managed
            && inner.0.len() == states.len()
            && inner.0.iter().zip(&states).all(|(a, b)| {
                a.target.id == b.target.id
                    && a.target.name == b.target.name
                    && a.target.url == b.target.url
                    && a.target.every_s == b.target.every_s
            });
        if !unchanged {
            self.save(&states, next, &managed)?;
            *inner = (states, next, managed);
        }
        Ok(())
    }

    /// Scrapes every due target once. Network I/O happens with no lock
    /// held; only the parsed points take the store lock.
    pub fn run_due(&self, fetch: &dyn Fetch, store: &Mutex<Store>) {
        let due: Vec<Target> = {
            let mut inner = self.inner.lock().unwrap();
            let now = Instant::now();
            inner
                .0
                .iter_mut()
                .filter(|s| s.due <= now)
                .map(|s| {
                    s.due = now + Duration::from_secs(s.target.every_s as u64);
                    s.target.clone()
                })
                .collect()
        };
        for target in due {
            let started = Instant::now();
            let outcome = scrape(fetch, &target, store);
            let ms = started.elapsed().as_millis() as u32;
            let mut inner = self.inner.lock().unwrap();
            if let Some(s) = inner.0.iter_mut().find(|s| s.target.id == target.id) {
                s.target.last_ms = ms;
                match outcome {
                    Ok(n) => {
                        s.target.last_ok = wall_ms().unwrap_or(0);
                        s.target.samples += n;
                        s.target.last_error.clear();
                    }
                    Err(e) => s.target.last_error = e,
                }
            }
        }
    }
}

fn scrape(fetch: &dyn Fetch, target: &Target, store: &Mutex<Store>) -> Result<u32, String> {
    let response = fetch
        .get(
            &target.url,
            "text/plain, application/json;q=0.9",
            BODY_LIMIT,
        )
        .map_err(|e| e.to_string())?;
    if response.status != 200 {
        return Err(format!("HTTP {}", response.status));
    }
    let text = String::from_utf8_lossy(&response.body);
    let now = wall_ms().map(|t| t as i64);
    let result = if response.content_type.starts_with("application/json") {
        let fields = flatten_json(&text, MAX_FIELDS).map_err(str::to_string)?;
        let Some(t) = now else {
            return Err("clock not set yet".into());
        };
        let mut lines = String::new();
        for (path, v) in fields {
            lines.push_str(&format!(
                "{} {}={}\n",
                miniframework::influx::escape_tag(&target.name),
                field_name(&path),
                miniframework::influx::number(v)
            ));
        }
        write_lines(
            &mut store.lock().unwrap(),
            &lines,
            Precision::Ms,
            Some(t),
            &[("source", &target.name)],
        )
    } else {
        write_lines(
            &mut store.lock().unwrap(),
            &text,
            Precision::Ns,
            now,
            &[("source", &target.name)],
        )
    };
    if result.accepted == 0 && result.rejected > 0 {
        return Err(result.errors.first().cloned().unwrap_or_default());
    }
    Ok(result.accepted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tsdb::Limits;
    use miniframework::fetch::Fetched;
    use miniframework::kv::MemKv;

    struct Canned(&'static str, &'static str);

    impl Fetch for Canned {
        fn get(&self, _url: &str, _accept: &str, _limit: usize) -> std::io::Result<Fetched> {
            Ok(Fetched {
                status: 200,
                content_type: self.0.into(),
                body: self.1.as_bytes().to_vec(),
            })
        }
    }

    fn store() -> Mutex<Store> {
        Mutex::new(Store::new(Limits {
            max_series: 64,
            block_bytes: 256,
            blocks: 64,
            rollup_secs: 60,
            rollup_slots: 4,
        }))
    }

    #[test]
    fn deployment_reconciles_without_duplicates_and_preserves_manual_targets() {
        let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
        let scrapes = Scrapes::load(kv.clone());
        let peer = scrapes
            .add(&NewTarget {
                name: "entered peer".into(),
                url: "http://peer.local/metrics".into(),
                every_s: 10,
            })
            .unwrap();
        scrapes
            .add(&NewTarget {
                name: "manual".into(),
                url: "http://manual.local/metrics".into(),
                every_s: 10,
            })
            .unwrap();
        let targets = vec![crate::deployment::ScrapeTarget {
            name: "peer".into(),
            url: "http://peer.local/metrics".into(),
            every_s: 30,
        }];
        scrapes.configure(&targets).unwrap();
        assert_eq!(scrapes.list().len(), 2);
        assert_eq!(scrapes.list()[0].id, peer.id);
        assert_eq!(scrapes.list()[0].every_s, 30);
        let before = kv.get(KEY).unwrap();
        let reloaded = Scrapes::load(kv.clone());
        reloaded.configure(&targets).unwrap();
        assert_eq!(kv.get(KEY).unwrap(), before);
        reloaded.configure(&[]).unwrap();
        assert_eq!(reloaded.list().len(), 1);
        assert_eq!(reloaded.list()[0].name, "manual");
    }

    #[test]
    fn json_and_influx_targets() {
        let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
        let scrapes = Scrapes::load(kv.clone());
        scrapes
            .add(&NewTarget {
                name: "nanacoin s2".into(),
                url: "http://nanacoin-s2.local/api/v1/diag".into(),
                every_s: 0,
            })
            .unwrap();
        assert_eq!(Scrapes::load(kv).list()[0].every_s, 10);
        let store = store();
        scrapes.run_due(
            &Canned(
                "application/json",
                r#"{"free_heap":100,"psram":{"free":7}}"#,
            ),
            &store,
        );
        let target = &scrapes.list()[0];
        assert_eq!(target.samples, 2, "{}", target.last_error);
        assert!(store
            .lock()
            .unwrap()
            .find("nanacoin_s2,source=nanacoin_s2 psram_free")
            .is_some());
        // Not due again yet.
        scrapes.run_due(&Canned("application/json", r#"{"x":1}"#), &store);
        assert_eq!(scrapes.list()[0].samples, 2);

        let s2 = Scrapes::load(Arc::new(MemKv::default()));
        s2.add(&NewTarget {
            name: "peer".into(),
            url: "http://peer.local/metrics".into(),
            every_s: 30,
        })
        .unwrap();
        s2.run_due(
            &Canned(
                "text/plain",
                "board,host=peer.local rssi=-60,uptime_s=5 1727900000000000000\n",
            ),
            &store,
        );
        assert_eq!(s2.list()[0].samples, 2);
        assert!(s2
            .add(&NewTarget {
                name: "x".into(),
                url: "ftp://x".into(),
                every_s: 1
            })
            .is_err());
    }
}
