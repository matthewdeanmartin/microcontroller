//! Optional RAM job queue. Apps authenticate, validate and serialize jobs
//! before submitting; workers claim them outside the HTTP serving loop.
//! Jobs and results disappear on reboot. No threads are spawned implicitly.
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Random, unguessable job identifier. Apps choose its wire representation.
pub type JobId = [u8; 16];
/// Stable identity supplied by the app after authentication.
pub type Owner = [u8; 16];

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Includes pending, running and retained completed jobs.
    pub slots: usize,
    pub workers: usize,
    pub per_owner: usize,
    /// Lifetime from acceptance, including execution and result retention.
    /// An expired running job holds its slot until the worker releases it.
    pub lifetime: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Done { bytes: usize },
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Full,
    OwnerLimit,
    TooLarge,
    RandomUnavailable,
    Allocation,
    Expired,
    BufferTooSmall,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub pending: usize,
    pub running: usize,
    pub retained: usize,
}

struct Entry {
    id: JobId,
    owner: Owner,
    expires: Instant,
    sequence: u64,
    status: Option<Status>,
    data: Vec<u8>,
    len: usize,
}
struct State {
    entries: Vec<Entry>,
    sequence: u64,
}

/// Preallocates `slots * BYTES` payload/result bytes, plus slot metadata.
/// Each claimed worker owns at most another `BYTES` bytes. A slot reuses
/// its input buffer for the result; there is no growing request backlog.
pub struct Queue<const BYTES: usize> {
    limits: Limits,
    state: Mutex<State>,
}
impl<const BYTES: usize> Queue<BYTES> {
    pub fn new(limits: Limits) -> Result<Self, &'static str> {
        if BYTES == 0
            || limits.slots == 0
            || limits.workers == 0
            || limits.workers > limits.slots
            || limits.per_owner == 0
            || limits.per_owner > limits.slots
            || limits.lifetime.is_zero()
            || Instant::now().checked_add(limits.lifetime).is_none()
        {
            return Err("invalid queue limits");
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(limits.slots)
            .map_err(|_| "slot allocation failed")?;
        let now = Instant::now();
        for _ in 0..limits.slots {
            let mut data = Vec::new();
            data.try_reserve_exact(BYTES)
                .map_err(|_| "payload allocation failed")?;
            data.resize(BYTES, 0);
            entries.push(Entry {
                id: [0; 16],
                owner: [0; 16],
                expires: now,
                sequence: 0,
                status: None,
                data,
                len: 0,
            });
        }
        Ok(Self {
            limits,
            state: Mutex::new(State {
                entries,
                sequence: 0,
            }),
        })
    }
    fn reap(s: &mut State, now: Instant) {
        for e in &mut s.entries {
            if e.status.is_some() && e.expires <= now && e.status != Some(Status::Running) {
                Self::clear(e);
            }
        }
    }
    fn clear(e: &mut Entry) {
        e.status = None;
        e.len = 0;
        e.data.fill(0);
    }
    /// Call only after validation/authorization. Returns a token only after
    /// the bounded queue has accepted the owned payload.
    pub fn submit(&self, owner: Owner, payload: &[u8]) -> Result<JobId, Error> {
        self.submit_at(owner, payload, Instant::now())
    }
    fn submit_at(&self, owner: Owner, payload: &[u8], now: Instant) -> Result<JobId, Error> {
        if payload.len() > BYTES {
            return Err(Error::TooLarge);
        }
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::reap(&mut s, now);
        if s.entries
            .iter()
            .filter(|e| e.status.is_some() && e.owner == owner)
            .count()
            >= self.limits.per_owner
        {
            return Err(Error::OwnerLimit);
        }
        let index = s
            .entries
            .iter()
            .position(|e| e.status.is_none())
            .ok_or(Error::Full)?;
        let mut id = [0; 16];
        let mut unique = false;
        for _ in 0..4 {
            getrandom::getrandom(&mut id).map_err(|_| Error::RandomUnavailable)?;
            if !s.entries.iter().any(|e| e.status.is_some() && e.id == id) {
                unique = true;
                break;
            }
        }
        if !unique {
            return Err(Error::RandomUnavailable);
        }
        // Only pending order matters; rebase before a sequence wrap.
        if s.sequence == u64::MAX {
            s.entries.sort_unstable_by_key(|e| e.sequence);
            for (i, e) in s.entries.iter_mut().enumerate() {
                e.sequence = i as u64;
            }
            s.sequence = s.entries.len() as u64;
            // Sorting changed the free slot's index.
            return self.submit_rebased(s, owner, payload, now, id);
        }
        let seq = s.sequence;
        s.sequence += 1;
        Self::insert(
            &mut s.entries[index],
            owner,
            payload,
            now + self.limits.lifetime,
            id,
            seq,
        );
        Ok(id)
    }
    fn submit_rebased(
        &self,
        mut s: std::sync::MutexGuard<'_, State>,
        owner: Owner,
        payload: &[u8],
        now: Instant,
        id: JobId,
    ) -> Result<JobId, Error> {
        let seq = s.sequence;
        s.sequence += 1;
        let e = s
            .entries
            .iter_mut()
            .find(|e| e.status.is_none())
            .ok_or(Error::Full)?;
        Self::insert(e, owner, payload, now + self.limits.lifetime, id, seq);
        Ok(id)
    }
    fn insert(
        e: &mut Entry,
        owner: Owner,
        payload: &[u8],
        expires: Instant,
        id: JobId,
        sequence: u64,
    ) {
        e.id = id;
        e.owner = owner;
        e.expires = expires;
        e.sequence = sequence;
        e.data[..payload.len()].copy_from_slice(payload);
        e.len = payload.len();
        e.status = Some(Status::Pending);
    }
    /// FIFO claim. Call from an app worker task, not Service::handle.
    /// Dropping a claim without finishing marks it failed and releases the
    /// worker budget. The queue lock is never held during job execution.
    pub fn claim(&self) -> Result<Option<Job<'_, BYTES>>, Error> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::reap(&mut s, Instant::now());
        if s.entries
            .iter()
            .filter(|e| e.status == Some(Status::Running))
            .count()
            >= self.limits.workers
        {
            return Ok(None);
        }
        let Some(e) = s
            .entries
            .iter_mut()
            .filter(|e| e.status == Some(Status::Pending))
            .min_by_key(|e| e.sequence)
        else {
            return Ok(None);
        };
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(e.len)
            .map_err(|_| Error::Allocation)?;
        payload.extend_from_slice(&e.data[..e.len]);
        e.data.fill(0);
        e.len = 0;
        e.status = Some(Status::Running);
        Ok(Some(Job {
            queue: self,
            id: e.id,
            owner: e.owner,
            expires: e.expires,
            payload,
            finished: false,
        }))
    }
    /// Unknown, expired and another owner's jobs all return `None`.
    /// Copies a completed result into caller-owned bounded storage.
    pub fn poll(
        &self,
        owner: Owner,
        id: JobId,
        output: &mut [u8],
    ) -> Result<Option<Status>, Error> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        Self::reap(&mut s, now);
        let Some(e) = s
            .entries
            .iter()
            .find(|e| e.status.is_some() && e.owner == owner && e.id == id && e.expires > now)
        else {
            return Ok(None);
        };
        if matches!(e.status, Some(Status::Done { .. })) {
            if output.len() < e.len {
                return Err(Error::BufferTooSmall);
            }
            output[..e.len].copy_from_slice(&e.data[..e.len]);
        }
        Ok(e.status)
    }
    /// Release a completed/failed record after the caller retrieves it.
    pub fn forget(&self, owner: Owner, id: JobId) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = s.entries.iter_mut().find(|e| {
            e.id == id
                && e.owner == owner
                && matches!(e.status, Some(Status::Done { .. } | Status::Failed))
        }) {
            Self::clear(e);
            true
        } else {
            false
        }
    }
    pub fn stats(&self) -> Stats {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::reap(&mut s, Instant::now());
        let mut stats = Stats::default();
        for e in &s.entries {
            match e.status {
                Some(Status::Pending) => stats.pending += 1,
                Some(Status::Running) => stats.running += 1,
                Some(_) => stats.retained += 1,
                None => {}
            }
        }
        stats
    }
    fn complete(&self, id: JobId, result: Option<&[u8]>) -> Result<(), Error> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let e = s
            .entries
            .iter_mut()
            .find(|e| e.id == id && e.status == Some(Status::Running))
            .ok_or(Error::Expired)?;
        if e.expires <= Instant::now() {
            Self::clear(e);
            return Err(Error::Expired);
        }
        match result {
            Some(bytes) if bytes.len() <= BYTES => {
                e.data[..bytes.len()].copy_from_slice(bytes);
                e.len = bytes.len();
                e.status = Some(Status::Done { bytes: bytes.len() });
                Ok(())
            }
            Some(_) => {
                e.status = Some(Status::Failed);
                Err(Error::TooLarge)
            }
            None => {
                e.status = Some(Status::Failed);
                Ok(())
            }
        }
    }
}

pub struct Job<'a, const BYTES: usize> {
    queue: &'a Queue<BYTES>,
    id: JobId,
    owner: Owner,
    expires: Instant,
    payload: Vec<u8>,
    finished: bool,
}
impl<const BYTES: usize> Job<'_, BYTES> {
    pub fn id(&self) -> JobId {
        self.id
    }
    pub fn owner(&self) -> Owner {
        self.owner
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Cooperative deadline: expiry cannot interrupt application side effects.
    pub fn expired(&self) -> bool {
        Instant::now() >= self.expires
    }
    pub fn finish(mut self, result: &[u8]) -> Result<(), Error> {
        let result = self.queue.complete(self.id, Some(result));
        self.finished = true;
        result
    }
    pub fn fail(mut self) -> Result<(), Error> {
        let result = self.queue.complete(self.id, None);
        self.finished = true;
        result
    }
}
impl<const BYTES: usize> Drop for Job<'_, BYTES> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.queue.complete(self.id, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn queue() -> Queue<8> {
        Queue::new(Limits {
            slots: 2,
            workers: 1,
            per_owner: 1,
            lifetime: Duration::from_secs(60),
        })
        .unwrap()
    }
    #[test]
    fn bounded_fifo_and_ownership() {
        let q = queue();
        let first = q.submit([1; 16], b"one").unwrap();
        let second = q.submit([2; 16], b"two").unwrap();
        assert_eq!(q.submit([1; 16], b"x"), Err(Error::OwnerLimit));
        assert_eq!(q.submit([3; 16], b"x"), Err(Error::Full));
        assert_eq!(q.submit([3; 16], b"123456789"), Err(Error::TooLarge));
        let job = q.claim().unwrap().unwrap();
        assert_eq!(job.id(), first);
        assert_eq!(job.payload(), b"one");
        assert!(q.claim().unwrap().is_none());
        job.finish(b"result").unwrap();
        assert_eq!(q.poll([2; 16], first, &mut [0; 8]), Ok(None));
        assert_eq!(
            q.poll([1; 16], first, &mut [0; 1]),
            Err(Error::BufferTooSmall)
        );
        let mut output = [0; 8];
        assert_eq!(
            q.poll([1; 16], first, &mut output),
            Ok(Some(Status::Done { bytes: 6 }))
        );
        assert_eq!(&output[..6], b"result");
        assert!(q.forget([1; 16], first));
        assert_eq!(q.claim().unwrap().unwrap().id(), second);
        assert_eq!(q.stats().retained, 1); // Dropped second claim is failed.
    }
    #[test]
    fn expiry_releases_pending_but_keeps_running_budget() {
        let q = queue();
        q.submit_at([1; 16], b"old", Instant::now() - Duration::from_secs(61))
            .unwrap();
        assert!(q.claim().unwrap().is_none());
        let id = q.submit([1; 16], b"new").unwrap();
        let job = q.claim().unwrap().unwrap();
        q.state
            .lock()
            .unwrap()
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .unwrap()
            .expires = Instant::now();
        assert_eq!(q.poll([1; 16], id, &mut [0; 8]), Ok(None));
        assert_eq!(q.stats().running, 1);
        assert_eq!(job.finish(b"late"), Err(Error::Expired));
        assert_eq!(q.stats(), Stats::default());
    }
    #[test]
    fn oversized_result_releases_worker_and_records_failure() {
        let q = queue();
        let id = q.submit([1; 16], b"job").unwrap();
        assert_eq!(
            q.claim().unwrap().unwrap().finish(b"123456789"),
            Err(Error::TooLarge)
        );
        assert_eq!(q.poll([1; 16], id, &mut []), Ok(Some(Status::Failed)));
        assert_eq!(q.stats().running, 0);
    }
}
