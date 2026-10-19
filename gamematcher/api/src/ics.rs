//! iCalendar (RFC 5545) for plans: a per-plan download and subscription
//! feeds (`spec/03-domain.md#calendar`). Times are UTC (`...Z`), so no time
//! zone database is needed. UIDs are stable and SEQUENCE is the plan's edit
//! count, so calendars update events instead of duplicating them.
use crate::messages::StoredPlan;
use crate::plans::{CANCELLED, DONE, ON, OPEN};
use crate::state::State;

/// Events in a feed: the newest this many plans.
pub const FEED_EVENTS: usize = 40;
/// A plan with no end time is shown this long, or as long as its longest
/// chosen game.
const DEFAULT_LENGTH: u64 = 3 * 3600;
/// Past plans stay in feeds this long (done) / so long (cancelled, so
/// subscribed calendars see the cancellation).
const KEEP_DONE: u64 = 60 * 86_400;
const KEEP_CANCELLED: u64 = 14 * 86_400;

/// Unix seconds → `YYYYMMDDTHHMMSSZ` (Howard Hinnant's civil-from-days).
pub fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
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

struct Event<'a> {
    uid: String,
    start: u64,
    end: u64,
    summary: String,
    description: String,
    location: &'a str,
    status: &'static str,
    sequence: u32,
    stamp: u64,
}

fn event(out: &mut String, e: &Event<'_>) {
    line(out, "BEGIN:VEVENT");
    line(out, &format!("UID:{}", e.uid));
    line(out, &format!("DTSTAMP:{}", utc(e.stamp)));
    line(out, &format!("DTSTART:{}", utc(e.start)));
    line(out, &format!("DTEND:{}", utc(e.end)));
    line(out, &format!("SUMMARY:{}", escape(&e.summary)));
    if !e.description.is_empty() {
        line(out, &format!("DESCRIPTION:{}", escape(&e.description)));
    }
    if !e.location.is_empty() {
        line(out, &format!("LOCATION:{}", escape(e.location)));
    }
    line(out, &format!("STATUS:{}", e.status));
    line(out, &format!("SEQUENCE:{}", e.sequence));
    line(out, "END:VEVENT");
}

fn calendar(name: &str, body: impl FnOnce(&mut String)) -> String {
    let mut out = String::new();
    line(&mut out, "BEGIN:VCALENDAR");
    line(&mut out, "VERSION:2.0");
    line(&mut out, "PRODID:-//gamematcher.local//Game Matcher//EN");
    line(&mut out, "CALSCALE:GREGORIAN");
    line(&mut out, "METHOD:PUBLISH");
    line(&mut out, &format!("X-WR-CALNAME:{}", escape(name)));
    line(&mut out, "REFRESH-INTERVAL;VALUE=DURATION:PT1H");
    line(&mut out, "X-PUBLISHED-TTL:PT1H");
    body(&mut out);
    line(&mut out, "END:VCALENDAR");
    out
}

fn title(state: &State, p: &StoredPlan) -> String {
    if !p.title.is_empty() {
        return p.title.clone();
    }
    let games: Vec<String> = p
        .games
        .iter()
        .filter_map(|&g| state.game(g).map(|g| g.title.clone()))
        .collect();
    if games.len() == 1 {
        games[0].clone()
    } else {
        format!("Game night: {}", games.join(", "))
    }
}

fn describe(state: &State, p: &StoredPlan) -> String {
    let names = |ids: &[u32]| -> String {
        ids.iter()
            .filter_map(|&g| state.game(g).map(|g| g.title.clone()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut lines = Vec::new();
    if !p.chosen_games.is_empty() {
        lines.push(format!("Playing: {}", names(&p.chosen_games)));
    } else {
        lines.push(format!("Games: {}", names(&p.games)));
    }
    let who: Vec<String> = p
        .responses
        .iter()
        .filter(|r| r.overall == "in")
        .map(|r| state.name(r.account))
        .collect();
    if !who.is_empty() {
        lines.push(format!("In: {}", who.join(", ")));
    }
    lines.push(format!("Host: {}", state.name(p.host)));
    if !p.notes.is_empty() {
        lines.push(String::new());
        lines.push(p.notes.clone());
    }
    lines.join("\n")
}

/// When a chosen time with no end finishes: its longest chosen game, else 3 h.
fn end_of(state: &State, p: &StoredPlan, start: u64, end: u64) -> u64 {
    if end > start {
        return end;
    }
    let longest = p
        .chosen_games
        .iter()
        .filter_map(|&g| state.game(g))
        .map(|g| u64::from(g.minutes) * 60)
        .max()
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_LENGTH);
    start + longest
}

/// The events one plan contributes: its chosen time when locked in (or
/// done/cancelled), else one tentative event per option.
fn plan_events(out: &mut String, state: &State, p: &StoredPlan) {
    let summary = title(state, p);
    let description = describe(state, p);
    if let Some(t) = p.times.iter().find(|t| Some(t.id) == p.chosen_time) {
        event(
            out,
            &Event {
                uid: format!("plan-{}@gamematcher.local", p.id),
                start: t.start,
                end: end_of(state, p, t.start, t.end),
                summary,
                description,
                location: &p.place,
                status: if p.status == CANCELLED {
                    "CANCELLED"
                } else {
                    "CONFIRMED"
                },
                sequence: p.edits,
                stamp: p.updated,
            },
        );
        return;
    }
    for t in &p.times {
        event(
            out,
            &Event {
                uid: format!("plan-{}-t{}@gamematcher.local", p.id, t.id),
                start: t.start,
                end: end_of(state, p, t.start, t.end),
                summary: format!("{summary} (maybe)"),
                description: description.clone(),
                location: &p.place,
                status: if p.status == CANCELLED {
                    "CANCELLED"
                } else {
                    "TENTATIVE"
                },
                sequence: p.edits,
                stamp: p.updated,
            },
        );
    }
}

/// One plan as a downloadable file.
pub fn plan_file(state: &State, p: &StoredPlan) -> String {
    calendar(&title(state, p), |out| plan_events(out, state, p))
}

/// Should `p` appear in a feed at `now`? Locked-in plans always; done and
/// cancelled ones for a while, so calendars keep history and see cancels.
fn in_feed(p: &StoredPlan, now: u64) -> bool {
    match p.status.as_str() {
        ON => true,
        DONE => now.saturating_sub(p.finished) < KEEP_DONE,
        CANCELLED => p.chosen_time.is_some() && now.saturating_sub(p.finished) < KEEP_CANCELLED,
        OPEN => false,
        _ => false,
    }
}

/// A subscription feed: every feed-worthy plan, or only those `account`
/// hosts or said in/maybe to.
pub fn feed(state: &State, account: Option<u32>, now: u64) -> String {
    let mut plans: Vec<&StoredPlan> = state
        .plans
        .iter()
        .filter(|p| in_feed(p, now))
        .filter(|p| {
            account.is_none_or(|a| {
                p.host == a
                    || p.responses
                        .iter()
                        .any(|r| r.account == a && r.overall != "out")
            })
        })
        .collect();
    plans.sort_by_key(|p| std::cmp::Reverse(p.updated));
    plans.truncate(FEED_EVENTS);
    let name = match account {
        Some(_) => "Game Matcher (mine)",
        None => "Game Matcher (household)",
    };
    calendar(name, |out| {
        for p in plans {
            plan_events(out, state, p);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{GameInput, LockIn, PlanInput, ResponseInput, TimeInput};
    use crate::state::Limits;

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "19700101T000000Z");
        assert_eq!(utc(951_782_400), "20000229T000000Z");
        assert_eq!(utc(1_792_254_810), "20261017T163330Z");
    }

    #[test]
    fn escaping_and_folding() {
        assert_eq!(escape("a;b,c\\d\ne"), "a\\;b\\,c\\\\d\\ne");
        let mut out = String::new();
        let long = format!("DESCRIPTION:{}", "é".repeat(60));
        line(&mut out, &long);
        for physical in out.split("\r\n").filter(|l| !l.is_empty()) {
            assert!(physical.len() <= 75, "{} octets", physical.len());
        }
        let unfolded = out.replace("\r\n ", "");
        assert_eq!(unfolded.trim_end(), long);
    }

    fn world() -> (State, u32, u32, u32) {
        let mut s = State::new(Limits {
            pin_rounds: 2,
            ..Limits::board()
        });
        let dad = s.setup("dad@h", "Dad", "1234", 0).unwrap();
        let kid = s.signup("kid@h", "Sam", "1234", 0).unwrap();
        let g = s
            .add_game(
                dad,
                &GameInput {
                    title: Some("Catan".into()),
                    minutes: Some(90),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        let p = s
            .create_plan(
                kid,
                &PlanInput {
                    title: Some("Saturday, finally".into()),
                    games: Some(vec![g]),
                    times: Some(vec![
                        TimeInput {
                            id: 0,
                            start: 1_793_000_000,
                            end: 0,
                        },
                        TimeInput {
                            id: 0,
                            start: 1_793_086_400,
                            end: 1_793_093_600,
                        },
                    ]),
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        (s, dad, kid, p)
    }

    #[test]
    fn open_plan_file_has_a_tentative_event_per_time() {
        let (s, _, _, p) = world();
        let ics = plan_file(&s, s.plan(p).unwrap());
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\n") && ics.ends_with("END:VCALENDAR\r\n"));
        assert_eq!(ics.matches("BEGIN:VEVENT").count(), 2);
        assert_eq!(ics.matches("STATUS:TENTATIVE").count(), 2);
        assert!(ics.contains("SUMMARY:Saturday\\, finally (maybe)"));
        assert!(
            !ics.contains('\n') || ics.split('\n').all(|l| l.is_empty() || l.ends_with('\r')),
            "CRLF only"
        );
    }

    #[test]
    fn feeds_follow_lock_in_and_people() {
        let (mut s, dad, kid, p) = world();
        assert!(
            !feed(&s, None, 20).contains("BEGIN:VEVENT"),
            "open plans stay out of feeds"
        );
        let t = s.plan(p).unwrap().times[0].id;
        let g = s.plan(p).unwrap().games[0];
        s.lock_plan(
            kid,
            p,
            &LockIn {
                time: t,
                games: vec![g],
            },
            30,
        )
        .unwrap();
        let house = feed(&s, None, 40);
        assert_eq!(house.matches("BEGIN:VEVENT").count(), 1);
        assert!(house.contains(&format!("UID:plan-{p}@gamematcher.local")));
        assert!(house.contains("DTEND:"), "{house}");
        assert!(
            house.contains(&format!("DTEND:{}", utc(1_793_000_000 + 90 * 60))),
            "game length"
        );
        assert!(feed(&s, Some(kid), 40).contains("BEGIN:VEVENT"), "the host");
        assert!(
            !feed(&s, Some(dad), 40).contains("BEGIN:VEVENT"),
            "dad hasn't answered"
        );
        s.respond(
            dad,
            p,
            &ResponseInput {
                overall: "maybe".into(),
                ..Default::default()
            },
            50,
        )
        .unwrap();
        assert!(feed(&s, Some(dad), 60).contains("BEGIN:VEVENT"));
        s.cancel_plan(kid, p, 70).unwrap();
        let cancelled = feed(&s, Some(kid), 80);
        assert!(cancelled.contains("STATUS:CANCELLED"));
        assert!(!feed(&s, Some(kid), 70 + KEEP_CANCELLED).contains("BEGIN:VEVENT"));
    }
}
