//! A tiny persistent key-value store: NVS on the board, files on desktop.
//!
//! Keys are at most 15 characters of `[A-Za-z0-9_.-]` (the NVS limit), so
//! code that works on the desktop works on the board.
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

pub trait Kv: Send + Sync + 'static {
    fn get(&self, key: &str) -> Option<Vec<u8>>;
    fn set(&self, key: &str, value: &[u8]) -> io::Result<()>;
    fn remove(&self, key: &str) -> io::Result<()>;
}

pub fn check_key(key: &str) -> io::Result<()> {
    let ok = !key.is_empty()
        && key.len() <= 15
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("kv key {key:?} must be 1-15 characters of A-Z a-z 0-9 _ . -"),
        ))
    }
}

/// One file per key in a directory (desktop).
pub struct FileKv {
    dir: PathBuf,
}

impl FileKv {
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }
}

impl Kv for FileKv {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        check_key(key).ok()?;
        std::fs::read(self.dir.join(key)).ok()
    }
    fn set(&self, key: &str, value: &[u8]) -> io::Result<()> {
        check_key(key)?;
        let tmp = self.dir.join(format!("{key}.tmp"));
        std::fs::write(&tmp, value)?;
        std::fs::rename(tmp, self.dir.join(key))
    }
    fn remove(&self, key: &str) -> io::Result<()> {
        check_key(key)?;
        match std::fs::remove_file(self.dir.join(key)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

/// In memory only (tests).
#[derive(Default)]
pub struct MemKv(Mutex<HashMap<String, Vec<u8>>>);

impl Kv for MemKv {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: &[u8]) -> io::Result<()> {
        check_key(key)?;
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn remove(&self, key: &str) -> io::Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}
