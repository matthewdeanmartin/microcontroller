//! iCalendar (RFC 5545) for events: a per-event download and a personal
//! feed on the LAN (`spec/03-domain.md#calendars`). Times are UTC (`…Z`),
//! so no zone database is needed. UIDs are stable and SEQUENCE is the
//! event's edit count, so calendars update events instead of duplicating
//! them. The formatting helpers are Game Matcher's `ics.rs`.
use crate::events::{CANCELLED, DONE};
use crate::messages::StoredEvent;
use crate::state::State;

/// Events in a feed: at most this many, newest first.
pub const FEED_EVENTS: usize = 60;
/// Past events stay in feeds this long (done) / so long (cancelled, so
/// subscribed calendars see the cancellation).
const KEEP_DONE: u64 = 60 * 86_400;
const KEEP_CANCELLED: u64 = 14 * 86_400;

/// Unix seconds → `YYYYMMDDTHHMMSSZ`.
pub fn utc(secs: u64) -> String {
    let (y, m, d) = crate::tz::civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// TEXT value escaping (RFC 5545 §3.3.11).
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Appends a content line (for other modules' calendars).
pub fn push_line(out: &mut String, content: &str) {
    line(out, content);
}

/// Appends a content line, folded at 75 octets without splitting a UTF-8
/// character, ended with CRLF (§3.1).
fn line(out: &mut String, content: &str) {
    let mut width = 0;
    for c in content.chars() {
        let len = c.len_utf8();
        if width + len > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += len;
    }
    out.push_str("\r\n");
}

fn calendar(name: &str, body: impl FnOnce(&mut String)) -> String {
    let mut out = String::new();
    line(&mut out, "BEGIN:VCALENDAR");
    line(&mut out, "VERSION:2.0");
    line(&mut out, "PRODID:-//party.local//party//EN");
    line(&mut out, "CALSCALE:GREGORIAN");
    line(&mut out, "METHOD:PUBLISH");
    line(&mut out, &format!("X-WR-CALNAME:{}", escape(name)));
    line(&mut out, "REFRESH-INTERVAL;VALUE=DURATION:PT1H");
    line(&mut out, "X-PUBLISHED-TTL:PT1H");
    body(&mut out);
    line(&mut out, "END:VCALENDAR");
    out
}

fn location(state: &State, e: &StoredEvent) -> String {
    match e.where_kind.as_str() {
        "home" => {
            let home = format!("{}'s", state.household_name(e.home));
            if e.address.is_empty() {
                home
            } else {
                format!("{home}, {}", e.address)
            }
        }
        "place" => [e.place.as_str(), e.address.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

/// One VEVENT. `tentative` marks a "maybe" in a personal feed.
/// `public` leaves out what a published file must not carry (hosts, addresses).
fn vevent(out: &mut String, state: &State, e: &StoredEvent, tentative: bool, public: bool) {
    let group = state
        .group(e.group)
        .map(|g| g.name.clone())
        .unwrap_or_default();
    let mut description = if public {
        format!("{group}.")
    } else {
        format!(
            "{group}. Hosted by {}.",
            e.hosts
                .iter()
                .map(|h| state.name(*h))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if !e.about.is_empty() {
        description = format!("{}\n\n{description}", e.about);
    }
    if !e.cost.is_empty() {
        description.push_str(&format!("\nCost: {}", e.cost));
    }
    let status = if e.status == CANCELLED {
        "CANCELLED"
    } else if tentative {
        "TENTATIVE"
    } else {
        "CONFIRMED"
    };
    line(out, "BEGIN:VEVENT");
    line(out, &format!("UID:e{}@party.local", e.id));
    line(out, &format!("DTSTAMP:{}", utc(e.updated.max(e.created))));
    line(out, &format!("DTSTART:{}", utc(e.start)));
    line(out, &format!("DTEND:{}", utc(e.effective_end())));
    line(out, &format!("SUMMARY:{}", escape(&e.title)));
    line(out, &format!("DESCRIPTION:{}", escape(&description)));
    let place = match (public, e.where_kind.as_str()) {
        (true, "place") => e.place.clone(),
        (true, _) => String::new(),
        _ => location(state, e),
    };
    if !place.is_empty() {
        line(out, &format!("LOCATION:{}", escape(&place)));
    }
    if !e.map.is_empty() && !public {
        line(out, &format!("URL:{}", e.map));
    }
    line(out, &format!("STATUS:{status}"));
    line(out, &format!("SEQUENCE:{}", e.edits));
    line(out, "END:VEVENT");
}

/// A single event to download (a poll without a time has nothing to add).
pub fn event_file(state: &State, e: &StoredEvent) -> String {
    calendar(&e.title, |out| {
        if !e.polling() {
            vevent(out, state, e, false, false);
        }
    })
}

/// Everything `person` hosts, is invited to, or answered going/maybe,
/// recent and upcoming.
pub fn feed(state: &State, person: u32, now: u64) -> String {
    let mut events: Vec<(&StoredEvent, bool)> = state
        .events
        .iter()
        .filter(|e| !e.polling() && e.status != "draft")
        .filter(|e| match e.status.as_str() {
            DONE => e.effective_end() + KEEP_DONE > now,
            CANCELLED => e.updated + KEEP_CANCELLED > now,
            _ => true,
        })
        .filter(|e| state.event_access(Some(person), e).see)
        .filter_map(|e| {
            let r = state.rsvp(e.id, person);
            let mine = e.hosts.contains(&person)
                || e.invitees.contains(&person)
                || r.is_some_and(|r| {
                    r.status == "going" || r.status == "maybe" || r.status == "waitlist"
                });
            (mine && r.is_none_or(|r| r.status != "no"))
                .then(|| (e, r.is_some_and(|r| r.status != "going")))
        })
        .collect();
    events.sort_by_key(|(e, _)| std::cmp::Reverse(e.start));
    events.truncate(FEED_EVENTS);
    let name = format!("party · {}", state.name(person));
    calendar(&name, |out| {
        for (e, tentative) in events {
            vevent(out, state, e, tentative, false);
        }
    })
}

/// A public group's published feed: its public events, no hosts or addresses.
pub fn public_feed(state: &State, name: &str, events: &[&StoredEvent]) -> String {
    calendar(name, |out| {
        for e in events {
            vevent(out, state, e, false, true);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_escaping_and_folding() {
        assert_eq!(utc(0), "19700101T000000Z");
        assert_eq!(utc(1_793_000_000), "20261026T073320Z");
        assert_eq!(escape("a,b;c\\d\ne"), "a\\,b\\;c\\\\d\\ne");
        let mut out = String::new();
        line(&mut out, &format!("SUMMARY:{}", "é".repeat(60)));
        for l in out.split("\r\n").filter(|l| !l.is_empty()) {
            assert!(l.len() <= 75, "{} octets", l.len());
        }
        assert!(out.contains("\r\n "), "long lines are folded");
    }
}
