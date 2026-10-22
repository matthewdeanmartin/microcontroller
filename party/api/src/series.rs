//! Recurring events (`spec/03-domain.md#recurring-events`): a series holds
//! the rule and a template, and its occurrences are ordinary events made on
//! a rolling window (the next few), so an open-ended series costs bounded
//! storage. Times are wall-clock in the instance zone, so "first Friday at
//! 19:00" stays 19:00 across daylight-saving changes.
//!
//! Each occurrence is made exactly once (`made` counts them). Editing one
//! changes only it; editing with `scope=following` also changes the
//! template and every later occurrence already made.
use crate::events::SCHEDULED;
use crate::messages::{EventInput, SeriesInput, SeriesView, StoredEvent, StoredSeries};
use crate::state::{full, State};
use crate::tz::{days_in_month, nth_weekday, Zone};
use miniframework::ApiError;

pub const ACTIVE_SERIES: &str = "active";
pub const ENDED: &str = "ended";
/// Occurrences kept ahead when the rule doesn't say.
pub const DEFAULT_WINDOW: u32 = 4;

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// The start of occurrence `n` (0 = the first), or None past the rule's end.
pub fn occurrence(s: &StoredSeries, zone: &Zone, n: u32) -> Option<u64> {
    if s.count > 0 && n >= s.count {
        return None;
    }
    let first = zone.local(s.first);
    let time = first.time();
    let n64 = i64::from(n);
    let days = match s.rule.as_str() {
        "weekly" => first.days() + 7 * i64::from(s.every.max(1)) * n64,
        "monthly-nth" | "monthly-date" => {
            let month = first.year * 12 + i64::from(first.month) - 1 + n64;
            let (year, m) = (month.div_euclid(12), (month.rem_euclid(12) + 1) as u32);
            if s.rule == "monthly-nth" {
                nth_weekday(year, m, s.nth, first.weekday)?
            } else {
                crate::tz::days_from_civil(year, m, first.day.min(days_in_month(year, m)))
            }
        }
        _ => return None,
    };
    let start = zone.to_utc(days, time);
    (s.until == 0 || start <= s.until).then_some(start)
}

/// "Every 2 weeks on Friday at 19:00", "The last Sunday of each month at 10:30".
pub fn describe(s: &StoredSeries, zone: &Zone) -> String {
    let l = zone.local(s.first);
    let at = format!("at {:02}:{:02}", l.hour, l.minute);
    let day = WEEKDAYS[l.weekday as usize];
    match s.rule.as_str() {
        "weekly" if s.every <= 1 => format!("Every {day} {at}"),
        "weekly" => format!("Every {} weeks on {day} {at}", s.every),
        "monthly-nth" => {
            let nth = match s.nth {
                1 => "first",
                2 => "second",
                3 => "third",
                4 => "fourth",
                _ => "last",
            };
            format!("The {nth} {day} of each month {at}")
        }
        _ => format!("Day {} of each month {at}", l.day),
    }
}

impl State {
    pub fn series_ref(&self, id: u32) -> Result<&StoredSeries, ApiError> {
        self.series
            .get(id)
            .ok_or_else(|| ApiError::not_found("No such series"))
    }

    fn can_edit_series(&self, actor: u32, s: &StoredSeries) -> bool {
        let host = s.template.hosts.contains(&actor);
        host || self
            .group(s.group)
            .is_some_and(|g| self.access(Some(actor), g).manage)
    }

    fn check_rule(
        rule: &str,
        every: u32,
        nth: i32,
        window: u32,
        count: u32,
    ) -> Result<(), ApiError> {
        if !["weekly", "monthly-nth", "monthly-date"].contains(&rule) {
            return Err(ApiError::bad_request(
                "Repeat weekly, monthly-nth or monthly-date",
            ));
        }
        if rule == "weekly" && !(1..=4).contains(&every) {
            return Err(ApiError::bad_request("Every 1 to 4 weeks"));
        }
        if rule == "monthly-nth" && !matches!(nth, 1..=4 | -1) {
            return Err(ApiError::bad_request(
                "The 1st to 4th, or the last, weekday of the month",
            ));
        }
        if !(1..=8).contains(&window) {
            return Err(ApiError::bad_request("Keep 1 to 8 occurrences ahead"));
        }
        if count > 104 {
            return Err(ApiError::bad_request(
                "At most 104 occurrences (or open-ended)",
            ));
        }
        Ok(())
    }

    /// A new series; its first occurrences are made straight away.
    pub fn create_series(
        &mut self,
        actor: u32,
        group: u32,
        input: &SeriesInput,
        now: u64,
    ) -> Result<u32, ApiError> {
        self.can_create_event(actor, group)?;
        if self.series.len() >= self.limits.series {
            return Err(full("series", self.limits.series));
        }
        let event = input
            .event
            .as_ref()
            .ok_or_else(|| ApiError::bad_request("A series needs its first event"))?;
        let template = self.build_event(actor, group, event, now)?;
        if template.polling() || template.start <= now {
            return Err(ApiError::bad_request(
                "A series starts at a fixed time in the future",
            ));
        }
        let rule = input.rule.clone().unwrap_or_else(|| "weekly".into());
        let every = input.every.unwrap_or(1);
        let nth = match input.nth {
            Some(n) => n,
            // The first occurrence's place in its month: 1st..4th, or last.
            None => {
                let l = self.zone.local(template.start);
                if l.day > 28 {
                    -1
                } else {
                    (l.day as i32 - 1) / 7 + 1
                }
            }
        };
        let window = input.window.unwrap_or(DEFAULT_WINDOW);
        let count = input.count.unwrap_or(0);
        Self::check_rule(&rule, every, nth, window, count)?;
        if rule == "monthly-nth" {
            let l = self.zone.local(template.start);
            let want = nth_weekday(l.year, l.month, nth, l.weekday);
            if want != Some(l.days()) {
                return Err(ApiError::bad_request(
                    "The first date doesn't match that rule",
                ));
            }
        }
        let id = self.next_series;
        self.next_series += 1;
        let length = template.end.saturating_sub(template.start);
        self.series.push(StoredSeries {
            id,
            group,
            rule,
            every,
            nth,
            until: input.until.unwrap_or(0),
            count,
            window,
            first: template.start,
            length: if template.end == 0 { 0 } else { length },
            template: StoredEvent {
                series: id,
                ..template
            },
            regulars: Vec::new(),
            made: 0,
            status: ACTIVE_SERIES.into(),
            created_by: actor,
        });
        self.log(
            actor,
            "series.create",
            format!("series {id} in group {group}"),
            now,
        );
        self.materialize(id, now)?;
        Ok(id)
    }

    /// Makes occurrences until `window` are ahead (or the rule ends).
    /// Returns whether any were made. Occurrences already in the past (the
    /// board was off) are skipped, not made late.
    pub fn materialize(&mut self, id: u32, now: u64) -> Result<bool, ApiError> {
        let mut made_any = false;
        for _ in 0..64 {
            let s = self.series_ref(id)?;
            if s.status != ACTIVE_SERIES {
                break;
            }
            let ahead = self
                .events
                .iter()
                .filter(|e| e.series == id && e.active() && e.start > now)
                .count() as u32;
            if ahead >= s.window {
                break;
            }
            let n = s.made;
            let Some(start) = occurrence(s, &self.zone, n) else {
                self.series.get_mut(id).expect("exists").status = ENDED.into();
                break;
            };
            let s = self.series.get_mut(id).expect("exists");
            s.made += 1;
            if start <= now {
                continue;
            }
            if self.upcoming_count() >= self.limits.upcoming_events {
                return Err(full("upcoming events", self.limits.upcoming_events));
            }
            let s = self.series_ref(id)?;
            let regulars = s.regulars.clone();
            let length = s.length;
            let template = s.template.clone();
            let event_id = self.next_event;
            self.next_event += 1;
            // The deadline keeps its distance from the start.
            let rsvp_by = if template.rsvp_by == 0 {
                0
            } else {
                start.saturating_sub(template.start.saturating_sub(template.rsvp_by))
            };
            let e = StoredEvent {
                id: event_id,
                start,
                end: if length == 0 { 0 } else { start + length },
                occurrence: n,
                rev: 1,
                created: now,
                updated: now,
                rsvp_by,
                ..template
            };
            self.events.push(e);
            for person in regulars {
                if self.eligible(person, self.event(event_id).expect("just made")) {
                    self.set_going(event_id, person, now);
                }
            }
            made_any = true;
        }
        Ok(made_any)
    }

    /// Housekeeping: every active series' window.
    pub fn materialize_all(&mut self, now: u64) -> bool {
        let ids: Vec<u32> = self
            .series
            .iter()
            .filter(|s| s.status == ACTIVE_SERIES)
            .map(|s| s.id)
            .collect();
        let mut changed = false;
        for id in ids {
            match self.materialize(id, now) {
                Ok(made) => changed |= made,
                Err(e) => log::warn!("series {id}: {}", e.message),
            }
        }
        changed
    }

    /// "This and following": changes occurrence `event` and every later one
    /// already made, and the template future ones copy. A new start time
    /// moves the series' time of day (and, given an end, its length).
    pub fn update_following(
        &mut self,
        actor: u32,
        event: u32,
        input: &EventInput,
        now: u64,
    ) -> Result<(), ApiError> {
        let (e, access) = self.visible_event(Some(actor), event)?;
        if !access.edit {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only the hosts can change this event",
            ));
        }
        if e.series == 0 {
            return Err(ApiError::bad_request("This event isn't part of a series"));
        }
        let (sid, from) = (e.series, e.occurrence);
        if let Some(r) = input.rev {
            if r != e.rev {
                return Err(ApiError::new(
                    409,
                    "stale",
                    "Someone changed this event meanwhile; reload and try again",
                ));
            }
        }
        // The template: everything but per-occurrence times.
        let mut s = self.series_ref(sid)?.clone();
        if let Some(start) = input.start {
            let l = self.zone.local(start);
            let first = self.zone.local(s.first);
            s.first = self.zone.to_utc(first.days(), l.time());
            let end = input.end.unwrap_or(0);
            if end != 0 {
                s.length = end.saturating_sub(start);
            } else if input.end.is_some() {
                s.length = 0;
            }
        }
        let template_input = EventInput {
            start: None,
            end: None,
            rev: None,
            status: None,
            ..input.clone()
        };
        let mut template = s.template.clone();
        template.start = s.first;
        template.end = if s.length == 0 { 0 } else { s.first + s.length };
        self.apply_event_input(&mut template, &template_input, false)?;
        s.template = template;
        // Every occurrence from this one on.
        let later: Vec<(u32, u32)> = self
            .events
            .iter()
            .filter(|o| o.series == sid && o.occurrence >= from && o.active())
            .map(|o| (o.id, o.occurrence))
            .collect();
        *self.series.get_mut(sid).expect("exists") = s;
        for (id, n) in later {
            let s = self.series_ref(sid)?;
            let mut each = EventInput {
                rev: None,
                ..template_input.clone()
            };
            if input.start.is_some() || input.end.is_some() {
                if let Some(start) = occurrence(s, &self.zone, n) {
                    each.start = Some(start);
                    each.end = Some(if s.length == 0 { 0 } else { start + s.length });
                }
            }
            self.update_event(actor, id, &each, now)?;
        }
        self.log(
            actor,
            "series.update",
            format!("series {sid} from occurrence {from}"),
            now,
        );
        Ok(())
    }

    /// Changes a series' end or window.
    pub fn update_series(
        &mut self,
        actor: u32,
        id: u32,
        input: &SeriesInput,
        now: u64,
    ) -> Result<(), ApiError> {
        let s = self.series_ref(id)?.clone();
        if !self.can_edit_series(actor, &s) {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only the hosts can change this series",
            ));
        }
        let mut next = s.clone();
        if let Some(u) = input.until {
            next.until = u;
        }
        if let Some(c) = input.count {
            next.count = c;
        }
        if let Some(w) = input.window {
            next.window = w;
        }
        Self::check_rule(&next.rule, next.every, next.nth, next.window, next.count)?;
        if next.status == ENDED
            && (next.until == 0 || next.until > now)
            && (next.count == 0 || next.made < next.count)
        {
            next.status = ACTIVE_SERIES.into();
        }
        if next != s {
            *self.series.get_mut(id).expect("exists") = next;
            self.log(actor, "series.update", format!("series {id}"), now);
            self.materialize(id, now)?;
        }
        Ok(())
    }

    /// Stops a series; with `cancel_future`, its upcoming occurrences are cancelled too.
    pub fn end_series(
        &mut self,
        actor: u32,
        id: u32,
        cancel_future: bool,
        now: u64,
    ) -> Result<(), ApiError> {
        let s = self.series_ref(id)?.clone();
        if !self.can_edit_series(actor, &s) {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only the hosts can end this series",
            ));
        }
        self.series.get_mut(id).expect("exists").status = ENDED.into();
        if cancel_future {
            let future: Vec<u32> = self
                .events
                .iter()
                .filter(|e| e.series == id && e.active() && e.start > now)
                .map(|e| e.id)
                .collect();
            for e in future {
                self.cancel_event(actor, e, now)?;
            }
        }
        self.log(actor, "series.end", format!("series {id}"), now);
        Ok(())
    }

    /// "I'm a regular": a standing going for new occurrences, and for the
    /// upcoming ones the person hasn't answered.
    pub fn set_regular(&mut self, actor: u32, id: u32, on: bool, now: u64) -> Result<(), ApiError> {
        let s = self.series_ref(id)?;
        let upcoming: Vec<u32> = self
            .events
            .iter()
            .filter(|e| e.series == id && e.status == SCHEDULED && !e.locked(now))
            .map(|e| e.id)
            .collect();
        let can = upcoming
            .iter()
            .any(|&e| self.eligible(actor, self.event(e).expect("listed")))
            || self
                .group(s.group)
                .and_then(|g| self.membership(g, actor))
                .is_some_and(|m| m.status == crate::state::ACTIVE);
        if !can {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Join the group to be a regular",
            ));
        }
        let s = self.series.get_mut(id).expect("exists");
        let present = s.regulars.contains(&actor);
        if on == present {
            return Ok(());
        }
        if on {
            s.regulars.push(actor);
            for e in upcoming {
                if self.rsvp(e, actor).is_none()
                    && self.eligible(actor, self.event(e).expect("listed"))
                {
                    self.set_going(e, actor, now);
                }
            }
        } else {
            s.regulars.retain(|&p| p != actor);
        }
        Ok(())
    }

    pub fn series_view(&self, viewer: u32, id: u32, now: u64) -> Result<SeriesView, ApiError> {
        let s = self.series_ref(id)?;
        let g = self
            .group(s.group)
            .ok_or_else(|| ApiError::not_found("No such series"))?;
        if !self.access(Some(viewer), g).see {
            return Err(ApiError::not_found("No such series"));
        }
        let mut upcoming: Vec<&StoredEvent> = self
            .events
            .iter()
            .filter(|e| e.series == id && e.active() && e.effective_end() > now)
            .filter(|e| self.event_access(Some(viewer), e).see)
            .collect();
        upcoming.sort_by_key(|e| e.start);
        Ok(SeriesView {
            id,
            group: s.group,
            rule: s.rule.clone(),
            every: s.every,
            nth: s.nth,
            until: s.until,
            count: s.count,
            window: s.window,
            status: s.status.clone(),
            summary: describe(s, &self.zone),
            regular: s.regulars.contains(&viewer),
            regulars: s.regulars.len() as u32,
            upcoming: upcoming
                .into_iter()
                .map(|e| self.event_summary(Some(viewer), e))
                .collect(),
            can_edit: self.can_edit_series(viewer, s),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tz::{days_from_civil, DEFAULT};

    fn series(rule: &str, first: u64) -> StoredSeries {
        StoredSeries {
            rule: rule.into(),
            every: 1,
            nth: 1,
            first,
            ..Default::default()
        }
    }

    fn at(zone: &Zone, y: i64, m: u32, d: u32, h: i64) -> u64 {
        zone.to_utc(days_from_civil(y, m, d), h * 3600)
    }

    #[test]
    fn first_friday_at_seven_survives_dst() {
        let z = Zone::parse(DEFAULT).unwrap();
        // Fri Oct 2 2026, 19:00 EDT; the first Friday of November is after
        // the change back to EST.
        let s = series("monthly-nth", at(&z, 2026, 10, 2, 19));
        let nov = occurrence(&s, &z, 1).unwrap();
        assert_eq!(nov, at(&z, 2026, 11, 6, 19));
        let l = z.local(nov);
        assert_eq!((l.month, l.day, l.hour, l.weekday), (11, 6, 19, 5));
        let mar = occurrence(&s, &z, 5).unwrap();
        assert_eq!(z.local(mar).day, 5, "March 5 2027 is the first Friday");
        assert_eq!(z.local(mar).hour, 19);
        assert_eq!(describe(&s, &z), "The first Friday of each month at 19:00");
    }

    #[test]
    fn weekly_monthly_and_bounds() {
        let z = Zone::parse(DEFAULT).unwrap();
        let mut s = series("weekly", at(&z, 2026, 10, 30, 18));
        s.every = 2;
        assert_eq!(
            z.local(occurrence(&s, &z, 1).unwrap()).day,
            13,
            "two weeks later, Nov 13"
        );
        assert_eq!(z.local(occurrence(&s, &z, 1).unwrap()).hour, 18);
        assert_eq!(describe(&s, &z), "Every 2 weeks on Friday at 18:00");
        let mut d = series("monthly-date", at(&z, 2027, 1, 31, 12));
        assert_eq!(
            z.local(occurrence(&d, &z, 1).unwrap()).day,
            28,
            "Feb 28: clamped"
        );
        assert_eq!(
            z.local(occurrence(&d, &z, 2).unwrap()).day,
            31,
            "back to the 31st in March"
        );
        d.count = 2;
        assert!(occurrence(&d, &z, 2).is_none());
        let mut last = series("monthly-nth", at(&z, 2026, 11, 29, 10));
        last.nth = -1;
        assert_eq!(
            z.local(occurrence(&last, &z, 1).unwrap()).day,
            27,
            "last Sunday of December"
        );
        last.until = at(&z, 2026, 12, 1, 0);
        assert!(occurrence(&last, &z, 1).is_none());
    }
}
