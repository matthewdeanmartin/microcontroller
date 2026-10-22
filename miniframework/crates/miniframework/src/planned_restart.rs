//! Bounded planned restart metadata. No heap, task, NVS write or free text.
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

const MAGIC: u32 = 0x52535400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Reason {
    Heap = 1,
    Maintenance = 2,
    Configuration = 3,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Heap => "planned: heap",
            Self::Maintenance => "planned: maintenance",
            Self::Configuration => "planned: configuration",
        }
    }

    fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::Heap),
            2 => Some(Self::Maintenance),
            3 => Some(Self::Configuration),
            _ => None,
        }
    }
}

pub struct Restart {
    pending: AtomicU32,
    previous: AtomicU32,
}

impl Default for Restart {
    fn default() -> Self {
        Self::new()
    }
}

impl Restart {
    pub const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
            previous: AtomicU32::new(0),
        }
    }

    /// First request wins; callers must quiesce app background writes first.
    pub fn request(&self, reason: Reason) -> bool {
        self.pending
            .compare_exchange(0, reason as u32, SeqCst, SeqCst)
            .is_ok()
    }

    pub fn pending(&self) -> Option<Reason> {
        Reason::from_code(self.pending.load(SeqCst))
    }

    pub fn cancel(&self) {
        self.pending.store(0, SeqCst);
    }

    /// Called only after connections drain, immediately before esp_restart.
    pub fn commit(&self, retained: &AtomicU32) {
        if let Some(reason) = self.pending() {
            retained.store(MAGIC | reason as u32, SeqCst);
        }
    }

    /// Consume on every boot, including crash and power-on, so a stale marker
    /// cannot relabel a later reset. Only software resets qualify as planned.
    pub fn boot(&self, retained: &AtomicU32, software_reset: bool) {
        let word = retained.swap(0, SeqCst);
        let reason = (software_reset && word & !0xff == MAGIC)
            .then(|| Reason::from_code(word & 0xff))
            .flatten();
        self.previous.store(reason.map_or(0, |r| r as u32), SeqCst);
        self.cancel();
    }

    pub fn previous(&self) -> Option<Reason> {
        Reason::from_code(self.previous.load(SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_committed_software_restart_is_planned_and_marker_is_consumed() {
        let rtc = AtomicU32::new(0);
        let run = Restart::new();
        assert!(run.request(Reason::Heap));
        assert!(!run.request(Reason::Maintenance));
        run.commit(&rtc);
        let next = Restart::new();
        next.boot(&rtc, true);
        assert_eq!(next.previous(), Some(Reason::Heap));
        next.boot(&rtc, true);
        assert_eq!(next.previous(), None);
        for software in [true, false] {
            rtc.store(0xdeadbeef, SeqCst);
            next.boot(&rtc, software);
            assert_eq!(next.previous(), None);
        }
        run.commit(&rtc);
        next.boot(&rtc, false);
        assert_eq!(next.previous(), None);
        assert_eq!(rtc.load(SeqCst), 0);
    }

    #[test]
    fn cancelled_or_uncommitted_requests_leave_no_reason() {
        let rtc = AtomicU32::new(0);
        let run = Restart::new();
        run.request(Reason::Heap);
        run.cancel();
        run.commit(&rtc);
        run.boot(&rtc, true);
        assert_eq!(run.previous(), None);
        assert!(run.request(Reason::Maintenance));
        run.boot(&rtc, true);
        assert_eq!(run.previous(), None);
    }
}
