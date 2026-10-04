//! Transport and network events, for an app that keeps its own incident
//! history (NanaCoin's Board Health page does).
//!
//! The framework counts everything in [`crate::sys::STATS`] regardless. An
//! app that wants the individual events installs one observer with
//! [`observe`]; it runs on whichever task saw the event (the serving loop,
//! the TLS handshake task, Wi-Fi housekeeping), so it must be quick, must not
//! block and must not take a lock the serving loop holds. Appending to a
//! fixed-size ring is the intended use.
use std::sync::OnceLock;

/// Which framework task is reporting a [`Event::Turn`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    /// The TLS handshake task.
    Handshake,
    /// The connection multiplexer.
    Serve,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A task completed one turn of its loop. `tls`/`http` are its current
    /// connections (pending handshakes for [`Task::Handshake`]). Sent every
    /// turn: use it as a heartbeat, not a log line.
    Turn {
        task: Task,
        tls: u8,
        http: u8,
    },
    /// A TLS session was established after `ms`.
    Handshake {
        ms: u32,
    },
    /// esp-tls could not start a session (allocation or configuration).
    TlsInitFailed {
        code: i32,
    },
    TlsFailed {
        code: i32,
        ms: u32,
    },
    TlsTimeout {
        ms: u32,
    },
    /// A finished handshake found the hand-over queue full and was dropped.
    HandoffFull,
    /// Every slot of this kind was busy with a request in flight.
    AdmissionRejected {
        secure: bool,
    },
    /// A plain socket failed (TLS failures are reported as TLS events).
    SocketError {
        code: i32,
    },
    /// A request did not arrive completely within the deadline.
    RequestTimeout {
        ms: u32,
    },
    /// A response made no progress for the write-stall timeout.
    WriteStalled {
        ms: u32,
    },
    /// From dispatch to the last byte took at least [`SLOW_MS`].
    SlowRequest {
        status: u16,
        ms: u32,
    },
    /// Bad HTTP framing; the connection is answered and closed.
    InvalidRequest {
        status: u16,
    },
    /// A keep-alive connection sat idle too long and was closed.
    IdleExpired,
    /// Not enough memory to hold a response.
    AllocationFailed,
    /// Wi-Fi dropped; IDF reason code (2 auth expired, 15 handshake timeout,
    /// often a wrong password, 201 no AP found, 202 auth failed).
    WifiDown {
        reason: u16,
        rssi: i8,
    },
    WifiUp,
    /// Housekeeping found Wi-Fi down and is reconnecting.
    Reconnect,
    ReconnectFailed {
        code: i32,
    },
}

/// A request slower than this (dispatch to last byte) is an event.
pub const SLOW_MS: u32 = 500;

static OBSERVER: OnceLock<fn(&Event)> = OnceLock::new();

/// Installs the observer. Only the first call takes effect; returns whether
/// this one did.
pub fn observe(observer: fn(&Event)) -> bool {
    OBSERVER.set(observer).is_ok()
}

/// Reports an event to the observer, if any.
pub fn emit(event: Event) {
    if let Some(observer) = OBSERVER.get() {
        observer(&event);
    }
}

#[cfg(test)]
pub(crate) mod capture {
    //! Tests install one observer that records into a thread-local, so
    //! tests running in parallel each see only their own events.
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static SEEN: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    }

    fn record(event: &Event) {
        if !matches!(event, Event::Turn { .. }) {
            SEEN.with(|seen| seen.borrow_mut().push(*event));
        }
    }

    /// Starts capturing on this thread (clearing anything earlier).
    pub fn start() {
        observe(record);
        SEEN.with(|seen| seen.borrow_mut().clear());
    }

    pub fn take() -> Vec<Event> {
        SEEN.with(|seen| std::mem::take(&mut *seen.borrow_mut()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_reach_the_observer_on_the_reporting_thread() {
        capture::start();
        emit(Event::IdleExpired);
        emit(Event::Turn {
            task: Task::Serve,
            tls: 1,
            http: 0,
        });
        emit(Event::WifiDown {
            reason: 201,
            rssi: -80,
        });
        assert_eq!(
            capture::take(),
            vec![
                Event::IdleExpired,
                Event::WifiDown {
                    reason: 201,
                    rssi: -80
                }
            ]
        );
        // A second observer is refused: one owner, decided at startup.
        fn other(_: &Event) {}
        assert!(!observe(other));
    }
}
