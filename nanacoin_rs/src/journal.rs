use crate::domain::{Command, Error, Event, MemberId, Receipt, State, MAX_SEQUENCE};

pub const FRAME_SIZE: usize = 1024;
pub const MAX_RECORDS: usize = 4096;

/// A successful append means durable storage. An error may be ambiguous;
/// Service latches read-only until restart/replay instead of reusing the slot.
pub trait Journal {
    fn read(&mut self, index: usize, frame: &mut [u8; FRAME_SIZE]) -> Result<bool, Error>;
    fn append(&mut self, index: usize, frame: &[u8; FRAME_SIZE]) -> Result<(), Error>;
}

pub struct Service<J> {
    // Allocate fixed state once; returning/moving Service must not copy a
    // growing inline state through the small firmware startup stack.
    pub(crate) state: Box<State>,
    pub(crate) auth: crate::auth::Auth,
    journal: J,
    records: usize,
    storage_failed: bool,
    keyed: std::vec::Vec<KeyReceipt>,
    clock: fn() -> u64,
}

struct KeyReceipt {
    key: [u8; 32],
    actor: MemberId,
    index: usize,
}

impl<J: Journal> Service<J> {
    pub fn open(journal: J) -> Result<Self, Error> {
        Self::open_with_clock(journal, unix_time)
    }

    pub fn open_with_clock(mut journal: J, clock: fn() -> u64) -> Result<Self, Error> {
        let mut state = Box::new(State::default());
        let mut frame = [0; FRAME_SIZE];
        let mut records = 0;
        let mut keyed = std::vec::Vec::with_capacity(MAX_RECORDS);
        while records < MAX_RECORDS && journal.read(records, &mut frame)? {
            let event = decode(&frame)?;
            state.replay(&event)?;
            if let Some(key) = event.client_key {
                if keyed
                    .iter()
                    .any(|r: &KeyReceipt| r.actor == event.actor && r.key == key)
                {
                    return Err(Error::CorruptJournal);
                }
                keyed.push(KeyReceipt {
                    key,
                    actor: event.actor,
                    index: records,
                });
            }
            records += 1;
        }
        state.check_invariants()?;
        Ok(Self {
            state,
            auth: crate::auth::Auth::default(),
            journal,
            records,
            storage_failed: false,
            keyed,
            clock,
        })
    }

    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn storage_failed(&self) -> bool {
        self.storage_failed
    }

    /// A clock earlier than the last durable event is unavailable for timed deals.
    pub fn now(&self) -> u64 {
        let now = (self.clock)();
        if now < self.state.last_timestamp {
            0
        } else {
            now
        }
    }

    pub(crate) fn event(&mut self, sequence: u64) -> Result<Event, Error> {
        if self.storage_failed {
            return Err(Error::Storage);
        }
        if sequence == 0 || sequence > self.records as u64 {
            return Err(Error::NotFound);
        }
        let mut frame = [0; FRAME_SIZE];
        if !self.journal.read(sequence as usize - 1, &mut frame)? {
            return Err(Error::CorruptJournal);
        }
        decode(&frame)
    }

    pub fn execute(
        &mut self,
        actor: MemberId,
        request_id: u64,
        command: Command,
    ) -> Result<Receipt, Error> {
        self.commit(actor, request_id, command, None)
    }

    /// Legacy HTTP idempotency keys are retained for the whole bounded journal.
    /// On retry, compare with the durable original command, not a lossy cache.
    pub fn execute_keyed(
        &mut self,
        actor: MemberId,
        key: &str,
        command: Command,
    ) -> Result<Receipt, Error> {
        if self.storage_failed {
            return Err(Error::Storage);
        }
        if key.is_empty() || key.len() > 80 {
            return Err(Error::InvalidInput);
        }
        let key = crate::auth::digest(key);
        if let Some(receipt) = self.keyed.iter().find(|r| r.actor == actor && r.key == key) {
            let mut frame = [0; FRAME_SIZE];
            if !self.journal.read(receipt.index, &mut frame)? {
                return Err(Error::CorruptJournal);
            }
            let event = decode(&frame)?;
            if event.command != command {
                return Err(Error::Conflict);
            }
            return Ok(Receipt {
                sequence: event.sequence,
                replayed: true,
            });
        }
        let request_id = self.state.member(actor)?.last_request + 1;
        self.commit(actor, request_id, command, Some(key))
    }

    fn commit(
        &mut self,
        actor: MemberId,
        request_id: u64,
        command: Command,
        client_key: Option<[u8; 32]>,
    ) -> Result<Receipt, Error> {
        if self.storage_failed {
            return Err(Error::Storage);
        }
        if let Some(receipt) = self.state.retry(actor, request_id, &command)? {
            return Ok(receipt);
        }
        if self.records == MAX_RECORDS {
            return Err(Error::Capacity);
        }
        let now = self.now();
        self.state.validate_at(actor, &command, now)?;
        let sequence = self
            .state
            .sequence
            .checked_add(1)
            .filter(|s| *s <= MAX_SEQUENCE)
            .ok_or(Error::Overflow)?;
        let event = Event {
            timestamp: now.max(self.state.last_timestamp),
            client_key,
            version: 1,
            sequence,
            actor,
            request_id,
            command,
        };
        let frame = encode(&event)?;
        if self.journal.append(self.records, &frame).is_err() {
            self.storage_failed = true;
            return Err(Error::Storage);
        }
        self.state.apply(&event);
        if let Some(key) = client_key {
            self.keyed.push(KeyReceipt {
                key,
                actor,
                index: self.records,
            });
        }
        if let Command::UpdateMember {
            member,
            password,
            role,
            disabled,
            ..
        } = &event.command
        {
            if password.is_some() || role.is_some() || *disabled == Some(true) {
                self.auth.revoke_member(*member);
            }
        }
        self.records += 1;
        Ok(Receipt {
            sequence,
            replayed: false,
        })
    }
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn encode(event: &Event) -> Result<[u8; FRAME_SIZE], Error> {
    let mut frame = [0; FRAME_SIZE];
    frame[..4].copy_from_slice(b"NCR1");
    let len = serde_json_core::to_slice(event, &mut frame[12..]).map_err(|_| Error::Capacity)?;
    frame[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    let checksum = crc32fast::hash(&frame[12..12 + len]);
    frame[8..12].copy_from_slice(&checksum.to_le_bytes());
    Ok(frame)
}

pub fn decode(frame: &[u8; FRAME_SIZE]) -> Result<Event, Error> {
    let len = u32::from_le_bytes(frame[4..8].try_into().unwrap()) as usize;
    let checksum = u32::from_le_bytes(frame[8..12].try_into().unwrap());
    if &frame[..4] != b"NCR1"
        || len > FRAME_SIZE - 12
        || crc32fast::hash(&frame[12..12 + len]) != checksum
        || frame[12 + len..].iter().any(|b| *b != 0)
    {
        return Err(Error::CorruptJournal);
    }
    crate::json::decode(&frame[12..12 + len]).map_err(|_| Error::CorruptJournal)
}

#[cfg(feature = "desktop")]
pub mod file {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        io::{Read, Seek, SeekFrom, Write},
        path::Path,
    };

    pub struct FileJournal {
        file: File,
        records: usize,
    }
    impl FileJournal {
        pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)?;
            fs2::FileExt::try_lock_exclusive(&file)?;
            let len = file.metadata()?.len();
            if len > (MAX_RECORDS * FRAME_SIZE) as u64 {
                return Err(std::io::Error::other("journal exceeds capacity"));
            }
            // A partial final frame was never acknowledged as durable. Complete
            // frames with bad checksums fail closed; they are never discarded.
            let valid_len = len / FRAME_SIZE as u64 * FRAME_SIZE as u64;
            if valid_len != len {
                file.set_len(valid_len)?;
                file.sync_all()?;
            }
            Ok(Self {
                file,
                records: (valid_len / FRAME_SIZE as u64) as usize,
            })
        }
    }
    impl Journal for FileJournal {
        fn read(&mut self, index: usize, frame: &mut [u8; FRAME_SIZE]) -> Result<bool, Error> {
            if index >= self.records {
                return Ok(false);
            }
            self.file
                .seek(SeekFrom::Start((index * FRAME_SIZE) as u64))
                .map_err(|_| Error::Storage)?;
            self.file.read_exact(frame).map_err(|_| Error::Storage)?;
            Ok(true)
        }
        fn append(&mut self, index: usize, frame: &[u8; FRAME_SIZE]) -> Result<(), Error> {
            if index != self.records || index >= MAX_RECORDS {
                return Err(Error::Storage);
            }
            self.file
                .seek(SeekFrom::Start((index * FRAME_SIZE) as u64))
                .map_err(|_| Error::Storage)?;
            self.file.write_all(frame).map_err(|_| Error::Storage)?;
            self.file.sync_all().map_err(|_| Error::Storage)?;
            self.records += 1;
            Ok(())
        }
    }
}
