//! Two snapshot slots over a [`Kv`] (NVS on the board, files on desktop,
//! memory in tests). See `spec/05-storage-limits.md`.
//!
//! A slot is a 24-byte header and the snapshot bytes:
//!
//! | bytes | field |
//! |---|---|
//! | 0..4 | magic `GMS1` |
//! | 4..6 | schema version (LE) |
//! | 6..8 | reserved |
//! | 8..16 | generation (LE) |
//! | 16..20 | payload length (LE) |
//! | 20..24 | CRC-32 of the payload (LE) |
//!
//! A commit writes the slot not holding the newest generation, so the
//! previous generation survives a torn or corrupt write. Boot takes the
//! newest valid slot. Two invalid slots are an error: the board refuses to
//! serve rather than start empty over someone's data.
use miniframework::kv::Kv;
use std::io;
use std::sync::Arc;

const MAGIC: &[u8; 4] = b"GMS1";
const HEADER: usize = 24;
const KEYS: [&str; 2] = ["snap0", "snap1"];

#[derive(Debug, PartialEq, Eq)]
pub struct Loaded {
    pub schema: u16,
    pub generation: u64,
    pub payload: Vec<u8>,
    /// Which slot it came from.
    pub slot: usize,
    /// The other slot existed but was invalid.
    pub fallback: bool,
}

#[derive(Debug)]
pub enum LoadError {
    /// Both slots exist and neither is valid.
    Corrupt(String),
    /// A slot was written by newer firmware.
    NewerSchema(u16),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Corrupt(why) => write!(
                f,
                "both state slots are unreadable ({why}); refusing to start empty"
            ),
            LoadError::NewerSchema(v) => write!(
                f,
                "state was written by newer firmware (schema {v}); refusing to misread it"
            ),
        }
    }
}

impl std::error::Error for LoadError {}

pub struct SlotStore {
    kv: Arc<dyn Kv>,
    /// Slot holding the newest generation, if any.
    current: Option<usize>,
    pub generation: u64,
}

fn encode(schema: u16, generation: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&schema.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// (schema, generation, payload) of a valid slot, or why it is invalid.
fn decode(bytes: &[u8]) -> Result<(u16, u64, &[u8]), &'static str> {
    if bytes.len() < HEADER || &bytes[..4] != MAGIC {
        return Err("bad header");
    }
    let schema = u16::from_le_bytes([bytes[4], bytes[5]]);
    let generation = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let len = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
    let crc = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
    let payload = bytes.get(HEADER..HEADER + len).ok_or("truncated")?;
    if bytes.len() != HEADER + len {
        return Err("length mismatch");
    }
    if crc32fast::hash(payload) != crc {
        return Err("checksum mismatch");
    }
    Ok((schema, generation, payload))
}

impl SlotStore {
    /// Opens the store and returns the newest valid snapshot, `None` for a
    /// board that has never committed.
    pub fn open(kv: Arc<dyn Kv>, max_schema: u16) -> Result<(Self, Option<Loaded>), LoadError> {
        let raw: Vec<Option<Vec<u8>>> = KEYS.iter().map(|k| kv.get(k)).collect();
        let mut valid = Vec::new();
        let mut errors = Vec::new();
        for (slot, bytes) in raw.iter().enumerate() {
            let Some(bytes) = bytes else { continue };
            match decode(bytes) {
                Ok((schema, generation, payload)) => {
                    valid.push((slot, schema, generation, payload))
                }
                Err(why) => errors.push(format!("{}: {why}", KEYS[slot])),
            }
        }
        let mut store = SlotStore {
            kv,
            current: None,
            generation: 0,
        };
        let Some(&(slot, schema, generation, payload)) =
            valid.iter().max_by_key(|(_, _, generation, _)| *generation)
        else {
            if errors.is_empty() {
                return Ok((store, None));
            }
            return Err(LoadError::Corrupt(errors.join(", ")));
        };
        if schema > max_schema {
            return Err(LoadError::NewerSchema(schema));
        }
        if !errors.is_empty() {
            log::warn!("state: using {} ({})", KEYS[slot], errors.join(", "));
        }
        store.current = Some(slot);
        store.generation = generation;
        Ok((
            store,
            Some(Loaded {
                schema,
                generation,
                payload: payload.to_vec(),
                slot,
                fallback: !errors.is_empty(),
            }),
        ))
    }

    /// Writes the next generation into the other slot.
    pub fn commit(&mut self, schema: u16, payload: &[u8]) -> io::Result<()> {
        let slot = match self.current {
            Some(s) => 1 - s,
            None => 0,
        };
        let generation = self.generation + 1;
        self.kv
            .set(KEYS[slot], &encode(schema, generation, payload))?;
        self.current = Some(slot);
        self.generation = generation;
        Ok(())
    }

    pub fn slot(&self) -> Option<usize> {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniframework::kv::MemKv;

    fn kv() -> Arc<dyn Kv> {
        Arc::new(MemKv::default())
    }

    #[test]
    fn empty_then_alternating_slots() {
        let kv = kv();
        let (mut store, loaded) = SlotStore::open(kv.clone(), 1).unwrap();
        assert!(loaded.is_none());
        store.commit(1, b"one").unwrap();
        store.commit(1, b"two").unwrap();
        store.commit(1, b"three").unwrap();
        assert!(kv.get("snap0").is_some() && kv.get("snap1").is_some());
        let (_, loaded) = SlotStore::open(kv, 1).unwrap();
        let loaded = loaded.unwrap();
        assert_eq!(loaded.payload, b"three");
        assert_eq!(loaded.generation, 3);
        assert!(!loaded.fallback);
    }

    #[test]
    fn torn_newest_slot_falls_back_one_generation() {
        let kv = kv();
        let (mut store, _) = SlotStore::open(kv.clone(), 1).unwrap();
        store.commit(1, b"older").unwrap();
        store.commit(1, b"newest").unwrap();
        let slot = KEYS[store.slot().unwrap()];
        let mut bytes = kv.get(slot).unwrap();
        bytes.truncate(bytes.len() - 2);
        kv.set(slot, &bytes).unwrap();
        let (mut store, loaded) = SlotStore::open(kv.clone(), 1).unwrap();
        let loaded = loaded.unwrap();
        assert_eq!(loaded.payload, b"older");
        assert!(loaded.fallback);
        // The next commit overwrites the torn slot, not the good one.
        store.commit(1, b"repaired").unwrap();
        let (_, loaded) = SlotStore::open(kv, 1).unwrap();
        assert_eq!(loaded.unwrap().payload, b"repaired");
    }

    #[test]
    fn flipped_bit_is_caught() {
        let kv = kv();
        let (mut store, _) = SlotStore::open(kv.clone(), 1).unwrap();
        store.commit(1, b"good").unwrap();
        store.commit(1, b"flip me").unwrap();
        let slot = KEYS[store.slot().unwrap()];
        let mut bytes = kv.get(slot).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        kv.set(slot, &bytes).unwrap();
        let (_, loaded) = SlotStore::open(kv, 1).unwrap();
        assert_eq!(loaded.unwrap().payload, b"good");
    }

    #[test]
    fn both_slots_bad_refuses() {
        let kv = kv();
        kv.set("snap0", b"junk").unwrap();
        kv.set("snap1", b"GMS1 more junk").unwrap();
        assert!(matches!(SlotStore::open(kv, 1), Err(LoadError::Corrupt(_))));
    }

    #[test]
    fn newer_schema_refuses() {
        let kv = kv();
        let (mut store, _) = SlotStore::open(kv.clone(), 5).unwrap();
        store.commit(5, b"future").unwrap();
        assert!(matches!(
            SlotStore::open(kv, 4),
            Err(LoadError::NewerSchema(5))
        ));
    }
}
