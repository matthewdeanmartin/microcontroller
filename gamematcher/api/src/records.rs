//! The state as one flash record per row (`spec/sprint-10-storage.md`).
//!
//! | key | holds |
//! |---|---|
//! | `meta` | counters and settings ([`StoredMeta`]) |
//! | `idx` | which rows exist, in order ([`StoredIndex`]) |
//! | `a<id>` `g<id>` `p<id>` `v<id>` | one account, game, plan, provider |
//! | `s<id>` | one account's sessions |
//! | `tx` | journal of a commit that writes more than one record |
//! | `cnt` | commits ever made (written by housekeeping, not every commit) |
//!
//! A commit writes only the records that changed. One record is written
//! directly (NVS replaces a blob atomically). Several go into `tx` first,
//! then into place, then `tx` is removed; boot replays a `tx` it finds, so a
//! multi-record change is all-or-nothing across a power cut.
//!
//! The schema-1 snapshot slots (`snap0`/`snap1`) are read once, on the first
//! boot without `meta`, and left in place so older firmware can still boot.
use crate::messages::{StoredIndex, StoredMeta, StoredSessions, StoredState};
use crate::state::{Limits, Parts, State, SCHEMA};
use crate::store::SlotStore;
use crate::table::{Row, Table};
use miniframework::kv::Kv;
use miniframework::wire::{self, Format, Message};
use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;

const TX_MAGIC: &[u8; 4] = b"GMT1";

/// Keys of the rows (one letter + id).
pub fn account_key(id: u32) -> String {
    format!("a{id}")
}
pub fn game_key(id: u32) -> String {
    format!("g{id}")
}
pub fn plan_key(id: u32) -> String {
    format!("p{id}")
}
pub fn provider_key(id: u32) -> String {
    format!("v{id}")
}
pub fn sessions_key(id: u32) -> String {
    format!("s{id}")
}

/// One record to write, or to remove (`None`).
pub type Write = (String, Option<Vec<u8>>);

/// What [`RecordStore::open`] found.
pub struct Opened {
    pub state: State,
    pub store: RecordStore,
    /// Records that failed to load. They stay on flash and are reported.
    pub quarantined: Vec<String>,
    /// Converted from the schema-1 snapshot on this boot.
    pub migrated: bool,
}

pub struct RecordStore {
    kv: Arc<dyn Kv>,
    /// The `meta` and `idx` bytes on flash.
    meta: Vec<u8>,
    idx: StoredIndex,
    idx_bytes: Vec<u8>,
    /// CRC of each account's `s<id>` record on flash.
    sessions: BTreeMap<u32, u32>,
    /// A journal was written but not fully applied (applied before the next).
    pending_tx: bool,
    /// Commits count persisted in `cnt`.
    saved_commits: u64,
    pub last_commit_bytes: u32,
    pub max_commit_bytes: u32,
}

/// The records one commit writes, and what flash will look like after.
pub struct Changes {
    pub writes: Vec<Write>,
    meta: Vec<u8>,
    idx: StoredIndex,
    idx_bytes: Vec<u8>,
    sessions: BTreeMap<u32, u32>,
}

fn encode<M: Message>(m: &M) -> io::Result<Vec<u8>> {
    wire::to_vec(Format::CborInt, m).map_err(|e| io::Error::other(e.to_string()))
}

fn bad(why: String) -> Box<dyn std::error::Error> {
    why.into()
}

/// Rows of one table that changed: written if present, removed if gone
/// (and on flash).
fn table_writes<T: Row + Message>(
    table: &Table<T>,
    on_flash: &[u32],
    key: fn(u32) -> String,
    out: &mut Vec<Write>,
) -> io::Result<()> {
    for &id in table.dirty() {
        match table.get(id) {
            Some(row) => out.push((key(id), Some(encode(row)?))),
            None if on_flash.contains(&id) => out.push((key(id), None)),
            None => {}
        }
    }
    Ok(())
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
    /// Loads the state: replays a leftover journal, migrates the schema-1
    /// snapshot if there are no records yet, then reads every row.
    pub fn open(kv: Arc<dyn Kv>, limits: Limits) -> Result<Opened, Box<dyn std::error::Error>> {
        let mut store = RecordStore {
            kv: kv.clone(),
            meta: Vec::new(),
            idx: StoredIndex::default(),
            idx_bytes: Vec::new(),
            sessions: BTreeMap::new(),
            pending_tx: false,
            saved_commits: 0,
            last_commit_bytes: 0,
            max_commit_bytes: 0,
        };
        store.replay_tx()?;
        let Some(meta_bytes) = kv.get("meta") else {
            return store.migrate(limits);
        };
        let meta: StoredMeta = wire::decode(Format::CborInt, &meta_bytes).map_err(|e| {
            bad(format!(
                "meta record unreadable ({e}); refusing to start empty"
            ))
        })?;
        if meta.schema > SCHEMA as u32 {
            return Err(bad(format!(
                "state was written by newer firmware (schema {}); refusing to misread it",
                meta.schema
            )));
        }
        let idx_bytes = kv.get("idx").unwrap_or_default();
        let idx: StoredIndex = if idx_bytes.is_empty() {
            StoredIndex::default()
        } else {
            wire::decode(Format::CborInt, &idx_bytes)
                .map_err(|e| bad(format!("idx record unreadable ({e}); refusing to start")))?
        };
        let mut quarantined = Vec::new();
        let mut parts = Parts {
            commits: kv
                .get("cnt")
                .and_then(|b| Some(u64::from_le_bytes(b.as_slice().try_into().ok()?)))
                .unwrap_or(0),
            ..Default::default()
        };
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
        let kv = &*kv;
        parts.accounts = idx
            .accounts
            .iter()
            .filter_map(|&id| load(kv, account_key(id), &mut quarantined))
            .collect();
        parts.games = idx
            .games
            .iter()
            .filter_map(|&id| load(kv, game_key(id), &mut quarantined))
            .collect();
        parts.plans = idx
            .plans
            .iter()
            .filter_map(|&id| load(kv, plan_key(id), &mut quarantined))
            .collect();
        parts.providers = idx
            .providers
            .iter()
            .filter_map(|&id| load(kv, provider_key(id), &mut quarantined))
            .collect();
        for &id in &idx.sessions {
            let key = sessions_key(id);
            if let Some(bytes) = kv.get(&key) {
                store.sessions.insert(id, crc32fast::hash(&bytes));
            }
            if let Some(s) = load::<StoredSessions>(kv, key, &mut quarantined) {
                parts.sessions.extend(s.sessions);
            }
        }
        store.saved_commits = parts.commits;
        store.meta = meta_bytes;
        store.idx = idx;
        store.idx_bytes = idx_bytes;
        let state = State::from_parts(meta, parts, limits);
        Ok(Opened {
            state,
            store,
            quarantined,
            migrated: false,
        })
    }

    /// First boot of schema 2: write every row from the schema-1 snapshot,
    /// `meta` last (the commit point: a crash before it just migrates again).
    fn migrate(mut self, limits: Limits) -> Result<Opened, Box<dyn std::error::Error>> {
        let (_, loaded) = SlotStore::open(self.kv.clone(), 1)?;
        let Some(loaded) = loaded else {
            return Ok(Opened {
                state: State::new(limits),
                store: self,
                quarantined: Vec::new(),
                migrated: false,
            });
        };
        let stored: StoredState = wire::decode(Format::CborInt, &loaded.payload)
            .map_err(|e| bad(format!("state snapshot: {e}")))?;
        let mut state = State::from_stored(stored, limits);
        state.mark_all();
        let changes = self.changes(&state)?;
        let (mut last, mut rest): (Vec<Write>, Vec<Write>) = changes
            .writes
            .iter()
            .cloned()
            .partition(|(k, _)| k == "meta");
        rest.append(&mut last);
        for (key, value) in &rest {
            if let Some(v) = value {
                self.kv.set(key, v)?;
            }
        }
        self.kv.set("cnt", &state.commits.to_le_bytes())?;
        self.saved_commits = state.commits;
        self.adopt(changes);
        state.saved();
        log::info!(
            "state: migrated snapshot generation {} to {} records",
            loaded.generation,
            rest.len()
        );
        Ok(Opened {
            state,
            store: self,
            quarantined: Vec::new(),
            migrated: true,
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
    /// that changed, plus the id lists when a row came or went.
    pub fn changes(&self, state: &State) -> io::Result<Changes> {
        let mut writes = Vec::new();
        table_writes(
            &state.accounts,
            &self.idx.accounts,
            account_key,
            &mut writes,
        )?;
        table_writes(&state.games, &self.idx.games, game_key, &mut writes)?;
        table_writes(&state.plans, &self.idx.plans, plan_key, &mut writes)?;
        table_writes(
            &state.providers,
            &self.idx.providers,
            provider_key,
            &mut writes,
        )?;

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
                    writes.push((sessions_key(id), Some(bytes)));
                }
            }
            let gone: Vec<u32> = sessions
                .keys()
                .copied()
                .filter(|id| !grouped.contains_key(id))
                .collect();
            for id in gone {
                sessions.remove(&id);
                writes.push((sessions_key(id), None));
            }
        }

        let same = |a: &[u32], t: &mut dyn Iterator<Item = u32>| a.iter().copied().eq(t);
        let mut idx = self.idx.clone();
        let mut idx_bytes = self.idx_bytes.clone();
        let session_ids: Vec<u32> = sessions.keys().copied().collect();
        if !same(&idx.accounts, &mut state.accounts.ids())
            || !same(&idx.games, &mut state.games.ids())
            || !same(&idx.plans, &mut state.plans.ids())
            || !same(&idx.providers, &mut state.providers.ids())
            || idx.sessions != session_ids
        {
            idx = StoredIndex {
                accounts: state.accounts.ids().collect(),
                games: state.games.ids().collect(),
                plans: state.plans.ids().collect(),
                providers: state.providers.ids().collect(),
                sessions: session_ids,
            };
            idx_bytes = encode(&idx)?;
            writes.push(("idx".into(), Some(idx_bytes.clone())));
        }

        let meta = encode(&state.meta())?;
        if meta != self.meta {
            writes.push(("meta".into(), Some(meta.clone())));
        }
        Ok(Changes {
            writes,
            meta,
            idx,
            idx_bytes,
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
        self.adopt(changes);
        Ok(())
    }

    fn adopt(&mut self, changes: Changes) {
        self.meta = changes.meta;
        self.idx = changes.idx;
        self.idx_bytes = changes.idx_bytes;
        self.sessions = changes.sessions;
    }

    /// Persists the commit count (housekeeping: at most once a minute).
    pub fn save_commits(&mut self, commits: u64) {
        if commits != self.saved_commits && self.kv.set("cnt", &commits.to_le_bytes()).is_ok() {
            self.saved_commits = commits;
        }
    }

    /// Records on flash (rows, sessions, meta, index).
    pub fn records(&self) -> u32 {
        let i = &self.idx;
        (i.accounts.len() + i.games.len() + i.plans.len() + i.providers.len() + i.sessions.len())
            as u32
            + 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_round_trip_and_damage() {
        let writes = vec![
            ("a1".to_string(), Some(b"one".to_vec())),
            ("p7".to_string(), None),
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
