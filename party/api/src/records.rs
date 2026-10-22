//! The state as one flash record per row (`spec/07-storage-limits.md`).
//!
//! | key | holds |
//! |---|---|
//! | `meta` | counters and settings ([`StoredMeta`]) |
//! | `p<id>` `h<id>` `g<id>` `l<id>` | one person, household, group (with its members), audit block |
//! | `e<id>` `r<id>` `q<id>` `n<id>` | one event, a block of an event's answers, a series, a person's notifications |
//! | `v<id>` `o<id>` `c<id>` | one invitation, outbound delivery, channel's settings |
//! | `s<id>` | one person's sessions |
//! | `ip` `ih` `ig` `il` `is` `ie` `ir` `iq` `in` `iv` `io` `ic` | which rows of that table exist ([`StoredIndex`]) |
//! | `tx` | journal of a commit that writes more than one record |
//! | `cnt` | commits ever made (written by housekeeping, not every commit) |
//!
//! A commit writes only the records that changed, and a table's index only
//! when a row came or went. One record is written directly (NVS replaces a
//! blob atomically). Several go into `tx` first, then into place, then `tx`
//! is removed; boot replays a `tx` it finds, so a multi-record change is
//! all-or-nothing across a power cut. This is Game Matcher's Sprint 10
//! store with one index per table, so adding a group never rewrites the
//! list of 2,000 people.
use crate::messages::{StoredIndex, StoredMeta, StoredSessions};
use crate::state::{Limits, Parts, State, SCHEMA};
use crate::table::{Row, Table};
use miniframework::kv::Kv;
use miniframework::wire::{self, Format, Message};
use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;

const TX_MAGIC: &[u8; 4] = b"PTX1";

/// One record to write, or to remove (`None`).
pub type Write = (String, Option<Vec<u8>>);

/// A table's prefix letter and index key.
#[derive(Clone, Copy)]
struct Shape {
    prefix: &'static str,
    index: &'static str,
}

const PEOPLE: Shape = Shape {
    prefix: "p",
    index: "ip",
};
const HOUSEHOLDS: Shape = Shape {
    prefix: "h",
    index: "ih",
};
const GROUPS: Shape = Shape {
    prefix: "g",
    index: "ig",
};
const AUDIT: Shape = Shape {
    prefix: "l",
    index: "il",
};
const SESSIONS: Shape = Shape {
    prefix: "s",
    index: "is",
};
const EVENTS: Shape = Shape {
    prefix: "e",
    index: "ie",
};
const RSVPS: Shape = Shape {
    prefix: "r",
    index: "ir",
};
const SERIES: Shape = Shape {
    prefix: "q",
    index: "iq",
};
const INBOXES: Shape = Shape {
    prefix: "n",
    index: "in",
};
const INVITES: Shape = Shape {
    prefix: "v",
    index: "iv",
};
const JOBS: Shape = Shape {
    prefix: "o",
    index: "io",
};
const CHANNELS: Shape = Shape {
    prefix: "c",
    index: "ic",
};

impl Shape {
    fn key(self, id: u32) -> String {
        format!("{}{id}", self.prefix)
    }
}

/// What [`RecordStore::open`] found.
pub struct Opened {
    pub state: State,
    pub store: RecordStore,
    /// Records that failed to load. They stay on flash and are reported.
    pub quarantined: Vec<String>,
}

/// One table as it is on flash: its ids in order.
#[derive(Clone, Default)]
struct OnFlash {
    ids: Vec<u32>,
}

pub struct RecordStore {
    kv: Arc<dyn Kv>,
    meta: Vec<u8>,
    tables: [OnFlash; 11],
    /// CRC of each person's `s<id>` record on flash.
    sessions: BTreeMap<u32, u32>,
    /// A journal was written but not fully applied (applied before the next).
    pending_tx: bool,
    saved_commits: u64,
    pub last_commit_bytes: u32,
    pub max_commit_bytes: u32,
}

/// The records one commit writes, and what flash will look like after.
pub struct Changes {
    pub writes: Vec<Write>,
    meta: Vec<u8>,
    tables: [OnFlash; 11],
    sessions: BTreeMap<u32, u32>,
}

fn encode<M: Message>(m: &M) -> io::Result<Vec<u8>> {
    wire::to_vec(Format::CborInt, m).map_err(|e| io::Error::other(e.to_string()))
}

fn bad(why: String) -> Box<dyn std::error::Error> {
    why.into()
}

/// Rows of one table that changed (written if present, removed if gone and
/// on flash), plus its index if the set of rows changed.
fn table_writes<T: Row + Message>(
    table: &Table<T>,
    shape: Shape,
    flash: &OnFlash,
    out: &mut Vec<Write>,
) -> io::Result<OnFlash> {
    for &id in table.dirty() {
        match table.get(id) {
            Some(row) => out.push((shape.key(id), Some(encode(row)?))),
            None if flash.ids.binary_search(&id).is_ok() => out.push((shape.key(id), None)),
            None => {}
        }
    }
    if flash.ids.iter().copied().eq(table.ids()) {
        return Ok(flash.clone());
    }
    let ids: Vec<u32> = table.ids().collect();
    out.push((
        shape.index.into(),
        Some(encode(&StoredIndex { ids: ids.clone() })?),
    ));
    Ok(OnFlash { ids })
}

fn encode_tx(writes: &[Write]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(writes.len() as u32).to_le_bytes());
    for (key, value) in writes {
        body.push(key.len() as u8);
        body.extend_from_slice(key.as_bytes());
        match value {
            Some(v) => {
                body.push(1);
                body.extend_from_slice(&(v.len() as u32).to_le_bytes());
                body.extend_from_slice(v);
            }
            None => body.push(0),
        }
    }
    let mut out = TX_MAGIC.to_vec();
    out.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn decode_tx(bytes: &[u8]) -> Result<Vec<Write>, &'static str> {
    let take = |at: &mut usize, n: usize| -> Result<&[u8], &'static str> {
        let s = bytes.get(*at..*at + n).ok_or("truncated")?;
        *at += n;
        Ok(s)
    };
    let mut at = 0;
    if take(&mut at, 4)? != TX_MAGIC {
        return Err("bad magic");
    }
    let crc = u32::from_le_bytes(take(&mut at, 4)?.try_into().unwrap());
    if crc32fast::hash(&bytes[at..]) != crc {
        return Err("checksum mismatch");
    }
    let n = u32::from_le_bytes(take(&mut at, 4)?.try_into().unwrap());
    let mut writes = Vec::new();
    for _ in 0..n {
        let len = take(&mut at, 1)?[0] as usize;
        let key = std::str::from_utf8(take(&mut at, len)?).map_err(|_| "bad key")?;
        let value = match take(&mut at, 1)?[0] {
            0 => None,
            _ => {
                let len = u32::from_le_bytes(take(&mut at, 4)?.try_into().unwrap()) as usize;
                Some(take(&mut at, len)?.to_vec())
            }
        };
        writes.push((key.to_string(), value));
    }
    Ok(writes)
}

impl RecordStore {
    /// Loads the state: replays a leftover journal, then reads every row
    /// its table's index names. An empty store is a fresh board.
    pub fn open(kv: Arc<dyn Kv>, limits: Limits) -> Result<Opened, Box<dyn std::error::Error>> {
        let mut store = RecordStore {
            kv: kv.clone(),
            meta: Vec::new(),
            tables: Default::default(),
            sessions: BTreeMap::new(),
            pending_tx: false,
            saved_commits: 0,
            last_commit_bytes: 0,
            max_commit_bytes: 0,
        };
        store.replay_tx()?;
        let Some(meta_bytes) = kv.get("meta") else {
            return Ok(Opened {
                state: State::new(limits),
                store,
                quarantined: Vec::new(),
            });
        };
        let meta: StoredMeta = wire::decode(Format::CborInt, &meta_bytes).map_err(|e| {
            bad(format!(
                "meta record unreadable ({e}); refusing to start empty"
            ))
        })?;
        if meta.schema > SCHEMA {
            return Err(bad(format!(
                "state was written by newer firmware (schema {}); refusing to misread it",
                meta.schema
            )));
        }
        let index = |shape: Shape| -> Result<Vec<u32>, Box<dyn std::error::Error>> {
            match kv.get(shape.index) {
                None => Ok(Vec::new()),
                Some(b) => wire::decode::<StoredIndex>(Format::CborInt, &b)
                    .map(|i| i.ids)
                    .map_err(|e| {
                        bad(format!(
                            "{} unreadable ({e}); refusing to start",
                            shape.index
                        ))
                    }),
            }
        };
        let mut quarantined = Vec::new();
        fn load<M: Message>(kv: &dyn Kv, key: String, quarantined: &mut Vec<String>) -> Option<M> {
            let decoded = kv
                .get(&key)
                .ok_or("missing".to_string())
                .and_then(|b| wire::decode(Format::CborInt, &b).map_err(|e| e.to_string()));
            match decoded {
                Ok(m) => Some(m),
                Err(why) => {
                    log::error!("state: record {key} {why}; skipped");
                    quarantined.push(format!("{key}: {why}"));
                    None
                }
            }
        }
        fn rows<M: Message>(
            kv: &dyn Kv,
            shape: Shape,
            ids: &[u32],
            quarantined: &mut Vec<String>,
        ) -> Vec<M> {
            ids.iter()
                .filter_map(|&id| load(kv, shape.key(id), quarantined))
                .collect()
        }
        let ids = [
            index(PEOPLE)?,
            index(HOUSEHOLDS)?,
            index(GROUPS)?,
            index(AUDIT)?,
            index(EVENTS)?,
            index(RSVPS)?,
            index(SERIES)?,
            index(INBOXES)?,
            index(INVITES)?,
            index(JOBS)?,
            index(CHANNELS)?,
        ];
        let k = &*kv;
        let mut parts = Parts {
            people: rows(k, PEOPLE, &ids[0], &mut quarantined),
            households: rows(k, HOUSEHOLDS, &ids[1], &mut quarantined),
            groups: rows(k, GROUPS, &ids[2], &mut quarantined),
            audit: rows(k, AUDIT, &ids[3], &mut quarantined),
            events: rows(k, EVENTS, &ids[4], &mut quarantined),
            rsvps: rows(k, RSVPS, &ids[5], &mut quarantined),
            series: rows(k, SERIES, &ids[6], &mut quarantined),
            inboxes: rows(k, INBOXES, &ids[7], &mut quarantined),
            invites: rows(k, INVITES, &ids[8], &mut quarantined),
            jobs: rows(k, JOBS, &ids[9], &mut quarantined),
            channels: rows(k, CHANNELS, &ids[10], &mut quarantined),
            commits: kv
                .get("cnt")
                .and_then(|b| Some(u64::from_le_bytes(b.as_slice().try_into().ok()?)))
                .unwrap_or(0),
            ..Default::default()
        };
        for id in index(SESSIONS)? {
            let key = SESSIONS.key(id);
            if let Some(bytes) = kv.get(&key) {
                store.sessions.insert(id, crc32fast::hash(&bytes));
            }
            if let Some(s) = load::<StoredSessions>(k, key, &mut quarantined) {
                parts.sessions.extend(s.sessions);
            }
        }
        store.saved_commits = parts.commits;
        store.meta = meta_bytes;
        store.tables = ids.map(|ids| OnFlash { ids });
        Ok(Opened {
            state: State::from_parts(meta, parts, limits),
            store,
            quarantined,
        })
    }

    /// Applies a journal left by a commit that didn't finish.
    fn replay_tx(&mut self) -> io::Result<()> {
        let Some(bytes) = self.kv.get("tx") else {
            self.pending_tx = false;
            return Ok(());
        };
        match decode_tx(&bytes) {
            Ok(writes) => {
                for (key, value) in &writes {
                    match value {
                        Some(v) => self.kv.set(key, v)?,
                        None => self.kv.remove(key)?,
                    }
                }
                log::warn!("state: replayed a journal of {} records", writes.len());
            }
            // NVS writes are atomic, so this is damage, not a torn write;
            // the records it named still hold the previous commit.
            Err(why) => log::error!("state: unreadable journal ({why}); dropped"),
        }
        self.kv.remove("tx")?;
        self.pending_tx = false;
        Ok(())
    }

    /// The records that differ between `state` and flash. Costs the rows
    /// that changed, plus a table's index when a row came or went.
    pub fn changes(&self, state: &State) -> io::Result<Changes> {
        let mut writes = Vec::new();
        let tables = [
            table_writes(&state.people, PEOPLE, &self.tables[0], &mut writes)?,
            table_writes(&state.households, HOUSEHOLDS, &self.tables[1], &mut writes)?,
            table_writes(&state.groups, GROUPS, &self.tables[2], &mut writes)?,
            table_writes(&state.audit, AUDIT, &self.tables[3], &mut writes)?,
            table_writes(&state.events, EVENTS, &self.tables[4], &mut writes)?,
            table_writes(&state.rsvps, RSVPS, &self.tables[5], &mut writes)?,
            table_writes(&state.series, SERIES, &self.tables[6], &mut writes)?,
            table_writes(&state.inboxes, INBOXES, &self.tables[7], &mut writes)?,
            table_writes(&state.invites, INVITES, &self.tables[8], &mut writes)?,
            table_writes(&state.jobs, JOBS, &self.tables[9], &mut writes)?,
            table_writes(&state.channels, CHANNELS, &self.tables[10], &mut writes)?,
        ];

        let mut sessions = self.sessions.clone();
        if state.sessions.dirty() || state.sessions_unsure {
            let grouped = state.stored_sessions();
            for (&id, list) in &grouped {
                let bytes = encode(&StoredSessions {
                    sessions: list.clone(),
                })?;
                let crc = crc32fast::hash(&bytes);
                if sessions.get(&id) != Some(&crc) {
                    sessions.insert(id, crc);
                    writes.push((SESSIONS.key(id), Some(bytes)));
                }
            }
            let gone: Vec<u32> = sessions
                .keys()
                .copied()
                .filter(|id| !grouped.contains_key(id))
                .collect();
            for id in gone {
                sessions.remove(&id);
                writes.push((SESSIONS.key(id), None));
            }
            if !sessions.keys().eq(self.sessions.keys()) {
                let ids = sessions.keys().copied().collect();
                writes.push((SESSIONS.index.into(), Some(encode(&StoredIndex { ids })?)));
            }
        }

        let meta = encode(&state.meta())?;
        if meta != self.meta {
            writes.push(("meta".into(), Some(meta.clone())));
        }
        Ok(Changes {
            writes,
            meta,
            tables,
            sessions,
        })
    }

    /// Writes `changes`. On `Err` flash is as it was before (the caller
    /// rolls RAM back). Once a journal is on flash the commit has happened:
    /// a failure applying it is retried before the next commit and at boot.
    pub fn write(&mut self, changes: Changes) -> io::Result<()> {
        if self.pending_tx {
            self.replay_tx()?;
        }
        let bytes = match changes.writes.as_slice() {
            [] => 0,
            [(key, value)] => {
                match value {
                    Some(v) => self.kv.set(key, v)?,
                    None => self.kv.remove(key)?,
                }
                value.as_ref().map_or(0, Vec::len)
            }
            writes => {
                let journal = encode_tx(writes);
                self.kv.set("tx", &journal)?;
                self.pending_tx = true;
                let mut applied = writes.iter().map(|(key, value)| match value {
                    Some(v) => self.kv.set(key, v),
                    None => self.kv.remove(key),
                });
                match applied.find(Result::is_err) {
                    Some(Err(e)) => log::error!("state: applying the journal: {e}; will retry"),
                    _ => {
                        if self.kv.remove("tx").is_ok() {
                            self.pending_tx = false;
                        }
                    }
                }
                journal.len()
                    + writes
                        .iter()
                        .map(|(_, v)| v.as_ref().map_or(0, Vec::len))
                        .sum::<usize>()
            }
        };
        self.last_commit_bytes = bytes as u32;
        self.max_commit_bytes = self.max_commit_bytes.max(bytes as u32);
        self.meta = changes.meta;
        self.tables = changes.tables;
        self.sessions = changes.sessions;
        Ok(())
    }

    /// Persists the commit count (housekeeping: at most once a minute).
    pub fn save_commits(&mut self, commits: u64) {
        if commits != self.saved_commits && self.kv.set("cnt", &commits.to_le_bytes()).is_ok() {
            self.saved_commits = commits;
        }
    }

    /// Records on flash (rows, sessions, meta, indexes).
    pub fn records(&self) -> u32 {
        let rows: usize = self.tables.iter().map(|t| t.ids.len()).sum();
        (rows + self.sessions.len() + 1 + 12) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_round_trip_and_damage() {
        let writes = vec![
            ("p1".to_string(), Some(b"one".to_vec())),
            ("g7".to_string(), None),
            ("meta".to_string(), Some(Vec::new())),
        ];
        let bytes = encode_tx(&writes);
        assert_eq!(decode_tx(&bytes).unwrap(), writes);
        let mut flipped = bytes.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert_eq!(decode_tx(&flipped), Err("checksum mismatch"));
        assert_eq!(
            decode_tx(&bytes[..bytes.len() - 1]),
            Err("checksum mismatch")
        );
        assert_eq!(decode_tx(b"nope"), Err("bad magic"));
    }
}
