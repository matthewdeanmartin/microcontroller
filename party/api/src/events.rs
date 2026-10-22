//! Events: a small core plus a fixed menu of typed modules, and kinds that
//! are presets of modules (`spec/03-domain.md#events`).
//!
//! Who may do what with an event:
//!
//! | | see | answer for themselves | edit, cancel, see every detail |
//! |---|---|---|---|
//! | host, group organizer or co-organizer, admin | ✓ | ✓ | ✓ |
//! | active group member | ✓ (not drafts, not others' invite-only events) | ✓ | |
//! | invitee | ✓ (even without seeing the group) | ✓ | |
//! | can see the group | ✓ for `group`/`public` events | join first | |
use crate::groups::ARCHIVED;
use crate::messages::{
    EventInput, EventSummary, PersonView, SlotCount, StoredEvent, StoredOption, StoredQuestion,
    StoredShift, StoredSlot,
};
use crate::state::{full, State, ACTIVE, DISABLED};
use miniframework::ApiError;

pub const DRAFT: &str = "draft";
pub const SCHEDULED: &str = "scheduled";
pub const CANCELLED: &str = "cancelled";
pub const DONE: &str = "done";

pub const MODULES: &[&str] = &[
    "headcount",
    "bring",
    "diet",
    "rides",
    "when",
    "shifts",
    "questions",
    "cost",
    "link",
];
pub const KINDS: &[&str] = &[
    "generic", "party", "dinner", "potluck", "outing", "meeting", "workday", "poll",
];

/// An event with no end time counts as this long (for "done" and calendars).
pub const DEFAULT_LENGTH: u64 = 3 * 3600;
/// Most guests anyone may bring when plus-ones are unlimited.
pub const MAX_GUESTS: u32 = 20;

/// What one person may do with one event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventAccess {
    pub see: bool,
    pub answer: bool,
    pub edit: bool,
}

/// A kind's modules and defaults (`spec/03-domain.md#event-kinds-presets`).
pub struct Preset {
    pub modules: &'static [&'static str],
    pub plus_ones: &'static str,
    pub plus_max: u32,
    pub reminder_secs: u64,
    /// Answers close this long before the start (0 = they don't).
    pub rsvp_before: u64,
}

pub fn preset(kind: &str) -> Option<Preset> {
    let p = |modules, plus_ones, plus_max, reminder_secs, rsvp_before| Preset {
        modules,
        plus_ones,
        plus_max,
        reminder_secs,
        rsvp_before,
    };
    Some(match kind {
        "generic" => p(&["headcount"], "unlimited", 0, 0, 0),
        "party" => p(&["headcount", "bring", "diet"], "limited", 2, 86_400, 0),
        "dinner" => p(&["headcount", "diet"], "none", 0, 86_400, 2 * 86_400),
        "potluck" => p(&["headcount", "bring", "diet"], "limited", 2, 86_400, 0),
        "outing" => p(&["headcount", "rides", "cost"], "limited", 2, 86_400, 0),
        "meeting" => p(&["headcount", "questions"], "unlimited", 0, 3_600, 0),
        "workday" => p(&["headcount", "shifts", "bring"], "unlimited", 0, 86_400, 0),
        "poll" => p(&["headcount", "when"], "unlimited", 0, 0, 0),
        _ => return None,
    })
}

fn text(raw: &str, max: usize, what: &str) -> Result<String, ApiError> {
    let t = raw.trim();
    if t.chars().count() > max || t.chars().any(|c| c.is_control() && c != '\n') {
        return Err(ApiError::bad_request(format!(
            "{what}: at most {max} characters"
        )));
    }
    Ok(t.to_string())
}

fn pick(raw: &str, allowed: &[&str], what: &str) -> Result<String, ApiError> {
    if allowed.contains(&raw) {
        Ok(raw.to_string())
    } else {
        Err(ApiError::bad_request(format!(
            "{what} must be one of: {}",
            allowed.join(", ")
        )))
    }
}

fn stale() -> ApiError {
    ApiError::new(
        409,
        "stale",
        "Someone changed this event meanwhile; reload and try again",
    )
}

impl StoredEvent {
    pub fn has(&self, module: &str) -> bool {
        self.modules.iter().any(|m| m == module)
    }

    pub fn active(&self) -> bool {
        self.status == DRAFT || self.status == SCHEDULED
    }

    /// The end used for "done" and calendars.
    pub fn effective_end(&self) -> u64 {
        if self.end > self.start {
            self.end
        } else {
            self.start + DEFAULT_LENGTH
        }
    }

    /// A time poll that hasn't been locked in (no fixed start yet).
    pub fn polling(&self) -> bool {
        self.has("when") && self.start == 0
    }

    /// Answers can no longer change (except by hosts while it's scheduled).
    pub fn locked(&self, now: u64) -> bool {
        self.status != SCHEDULED
            || (self.rsvp_by != 0 && now > self.rsvp_by)
            || (self.start != 0 && now >= self.start)
    }
}

/// Gives new list items (id 0) fresh ids; keeps the ids of existing ones.
fn number<T>(items: &mut [T], next: &mut u32, id: impl Fn(&mut T) -> &mut u32) {
    for item in items {
        let slot = id(item);
        if *slot == 0 {
            *slot = *next;
            *next += 1;
        }
    }
}

impl State {
    pub fn event(&self, id: u32) -> Option<&StoredEvent> {
        self.events.get(id)
    }

    fn event_ref(&self, id: u32) -> Result<&StoredEvent, ApiError> {
        self.event(id)
            .ok_or_else(|| ApiError::not_found("No such event"))
    }

    pub fn event_access(&self, viewer: Option<u32>, e: &StoredEvent) -> EventAccess {
        let Some(g) = self.group(e.group) else {
            return EventAccess::default();
        };
        let ga = self.access(viewer, g);
        let host = viewer.is_some_and(|v| e.hosts.contains(&v));
        let edit = ga.manage || host;
        if edit {
            return EventAccess {
                see: true,
                answer: e.status == SCHEDULED,
                edit,
            };
        }
        if e.status == DRAFT {
            return EventAccess::default();
        }
        let invitee = viewer.is_some_and(|v| e.invitees.contains(&v));
        let member = viewer
            .and_then(|v| self.membership(g, v))
            .is_some_and(|m| m.status == ACTIVE);
        let see = invitee || (ga.see && e.visibility != "invite-only");
        EventAccess {
            see,
            answer: see && (member || invitee) && e.status == SCHEDULED,
            edit: false,
        }
    }

    /// The event if `viewer` may see it (404 otherwise, as for groups).
    pub fn visible_event(
        &self,
        viewer: Option<u32>,
        id: u32,
    ) -> Result<(&StoredEvent, EventAccess), ApiError> {
        let e = self.event_ref(id)?;
        let a = self.event_access(viewer, e);
        if a.see {
            Ok((e, a))
        } else {
            Err(ApiError::not_found("No such event"))
        }
    }

    /// May `actor` create events in group `group`?
    pub fn can_create_event(&self, actor: u32, group: u32) -> Result<(), ApiError> {
        let g = self
            .group(group)
            .ok_or_else(|| ApiError::not_found("No such group"))?;
        let access = self.access(Some(actor), g);
        if !access.see {
            return Err(ApiError::not_found("No such group"));
        }
        if g.status == ARCHIVED {
            return Err(ApiError::new(409, "archived", "This group is archived"));
        }
        let member = self
            .membership(g, actor)
            .is_some_and(|m| m.status == ACTIVE);
        if access.manage || (g.members_create_events && member) {
            Ok(())
        } else {
            Err(ApiError::new(
                403,
                "not_allowed",
                "Only the group's organizers can create events here",
            ))
        }
    }

    pub fn upcoming_count(&self) -> usize {
        self.events.iter().filter(|e| e.active()).count()
    }

    /// Applies `input` to `e` (validating everything), as hosts editing or
    /// creating it. Returns whether answers are affected (a `rev` bump).
    pub fn apply_event_input(
        &self,
        e: &mut StoredEvent,
        input: &EventInput,
        creating: bool,
    ) -> Result<bool, ApiError> {
        let before = e.clone();
        if let Some(t) = &input.title {
            let t = text(t, 80, "Title")?;
            if t.is_empty() {
                return Err(ApiError::bad_request("An event needs a title"));
            }
            e.title = t;
        }
        if let Some(a) = &input.about {
            e.about = text(a, 4_000, "About")?;
        }
        if let Some(hosts) = &input.hosts {
            let mut hosts = hosts.clone();
            hosts.dedup();
            if hosts.is_empty() || hosts.len() > 4 {
                return Err(ApiError::bad_request("An event has 1 to 4 hosts"));
            }
            if hosts
                .iter()
                .any(|h| !self.person(*h).is_some_and(|p| p.status == ACTIVE))
            {
                return Err(ApiError::bad_request("Hosts must be people on this board"));
            }
            e.hosts = hosts;
        }
        if let Some(s) = input.start {
            e.start = s;
        }
        if let Some(s) = input.end {
            e.end = s;
        }
        if let Some(w) = &input.where_kind {
            e.where_kind = pick(w, &["place", "home", "tbd"], "where_kind")?;
        }
        if let Some(p) = &input.place {
            e.place = text(p, 120, "Place")?;
        }
        if let Some(a) = &input.address {
            e.address = text(a, 200, "Address")?;
        }
        if let Some(m) = &input.map {
            e.map = text(m, 300, "Map link")?;
        }
        if let Some(h) = input.home {
            if h != 0 && self.households.get(h).is_none() {
                return Err(ApiError::not_found("No such household"));
            }
            e.home = h;
        }
        if let Some(v) = &input.visibility {
            e.visibility = pick(v, &["group", "invite-only", "public"], "visibility")?;
        }
        if let Some(inv) = &input.invitees {
            let mut inv = inv.clone();
            inv.sort_unstable();
            inv.dedup();
            if inv.len() > self.limits.rsvps_per_event {
                return Err(full("invitees for one event", self.limits.rsvps_per_event));
            }
            if inv
                .iter()
                .any(|p| !self.person(*p).is_some_and(|p| p.status != DISABLED))
            {
                return Err(ApiError::bad_request(
                    "Invitees must be people on this board",
                ));
            }
            e.invitees = inv;
        }
        if let Some(c) = input.capacity {
            if c > 2_000 {
                return Err(ApiError::bad_request("Capacity: at most 2,000"));
            }
            e.capacity = c;
        }
        if let Some(r) = input.rsvp_by {
            e.rsvp_by = r;
        }
        if let Some(p) = &input.plus_ones {
            e.plus_ones = pick(p, &["none", "limited", "unlimited"], "plus_ones")?;
        }
        if let Some(m) = input.plus_max {
            if m > 9 {
                return Err(ApiError::bad_request("Guests per person: at most 9"));
            }
            e.plus_max = m;
        }
        if let Some(s) = &input.status {
            let s = pick(s, &[DRAFT, SCHEDULED], "status")?;
            if s == DRAFT && !creating && e.status == SCHEDULED {
                return Err(ApiError::new(
                    409,
                    "already_scheduled",
                    "A scheduled event can't go back to draft; cancel it instead",
                ));
            }
            if e.active() {
                e.status = s;
            }
        }
        if let Some(mods) = &input.modules {
            let mut mods: Vec<String> = mods
                .iter()
                .map(|m| pick(m, MODULES, "module"))
                .collect::<Result<_, _>>()?;
            if !mods.iter().any(|m| m == "headcount") {
                mods.insert(0, "headcount".into());
            }
            mods.dedup();
            e.modules = mods;
        }
        let mut next = e.next_item.max(1);
        if let Some(slots) = &input.bring {
            let mut slots: Vec<StoredSlot> = slots.clone();
            if slots.len() > 24 {
                return Err(ApiError::bad_request("A bring list has at most 24 items"));
            }
            for s in &mut slots {
                s.name = text(&s.name, 40, "Bring-list item")?;
                if s.name.is_empty() || !(1..=99).contains(&s.count) {
                    return Err(ApiError::bad_request(
                        "Each item needs a name and a count of 1 to 99",
                    ));
                }
            }
            number(&mut slots, &mut next, |s| &mut s.id);
            e.bring = slots;
        }
        if let Some(d) = &input.diet_prompt {
            e.diet_prompt = text(d, 120, "Diet question")?;
        }
        if let Some(qs) = &input.questions {
            let mut qs: Vec<StoredQuestion> = qs.clone();
            if qs.len() > 4 {
                return Err(ApiError::bad_request("At most 4 questions"));
            }
            for q in &mut qs {
                q.kind = pick(&q.kind, &["yesno", "choice", "text"], "question kind")?;
                q.prompt = text(&q.prompt, 120, "Question")?;
                if q.prompt.is_empty() {
                    return Err(ApiError::bad_request("Each question needs a prompt"));
                }
                q.choices = q
                    .choices
                    .iter()
                    .map(|c| text(c, 40, "Choice"))
                    .collect::<Result<_, _>>()?;
                q.choices.retain(|c| !c.is_empty());
                if q.kind == "choice" && !(2..=6).contains(&q.choices.len()) {
                    return Err(ApiError::bad_request(
                        "A choice question has 2 to 6 choices",
                    ));
                }
                if q.kind != "choice" {
                    q.choices.clear();
                }
            }
            number(&mut qs, &mut next, |q| &mut q.id);
            e.questions = qs;
        }
        if let Some(c) = &input.cost {
            e.cost = text(c, 200, "Cost")?;
        }
        if let Some(r) = &input.rides_note {
            e.rides_note = text(r, 200, "Rides note")?;
        }
        if let Some(opts) = &input.when {
            let mut opts: Vec<StoredOption> = opts.clone();
            if opts.len() > 6 {
                return Err(ApiError::bad_request("A time poll has at most 6 times"));
            }
            if opts
                .iter()
                .any(|o| o.start == 0 || (o.end != 0 && o.end < o.start))
            {
                return Err(ApiError::bad_request(
                    "Each time needs a start, and an end after it",
                ));
            }
            number(&mut opts, &mut next, |o| &mut o.id);
            e.when = opts;
        }
        if let Some(shifts) = &input.shifts {
            let mut shifts: Vec<StoredShift> = shifts.clone();
            if shifts.len() > 16 {
                return Err(ApiError::bad_request("At most 16 shifts"));
            }
            for s in &mut shifts {
                s.name = text(&s.name, 40, "Shift")?;
                if s.name.is_empty()
                    || !(1..=99).contains(&s.count)
                    || (s.end != 0 && s.end < s.start)
                {
                    return Err(ApiError::bad_request(
                        "Each shift needs a name, a count of 1 to 99, and an end after its start",
                    ));
                }
            }
            number(&mut shifts, &mut next, |s| &mut s.id);
            e.shifts = shifts;
        }
        if let Some(l) = &input.link {
            e.link = text(l, 300, "Link")?;
        }
        if let Some(r) = input.reminder_secs {
            if r > 14 * 86_400 {
                return Err(ApiError::bad_request("Reminders: at most two weeks ahead"));
            }
            e.reminder_secs = r;
            e.reminded = false;
        }
        e.next_item = next;

        // The whole event must make sense, whatever was changed.
        if e.polling() {
            if e.when.len() < 2 {
                return Err(ApiError::bad_request("A time poll needs at least 2 times"));
            }
        } else if e.start == 0 {
            return Err(ApiError::bad_request("An event needs a start time"));
        }
        if e.end != 0 && e.end < e.start {
            return Err(ApiError::bad_request("The end is before the start"));
        }
        if e.visibility == "public"
            && self
                .group(e.group)
                .is_some_and(|g| g.visibility != "public")
        {
            return Err(ApiError::bad_request(
                "Only events of public groups can be public",
            ));
        }
        if e.where_kind == "home" && e.home == 0 {
            return Err(ApiError::bad_request("Whose home? Pick a household"));
        }

        let affects_answers = before.start != e.start
            || before.end != e.end
            || before.where_kind != e.where_kind
            || before.place != e.place
            || before.address != e.address
            || before.home != e.home
            || before.modules != e.modules
            || before.bring != e.bring
            || before.when != e.when
            || before.shifts != e.shifts
            || before.questions != e.questions;
        if affects_answers && !creating {
            e.rev += 1;
        }
        if *e != before {
            e.edits += 1;
        }
        Ok(affects_answers)
    }

    /// A new event from its kind's preset and `input`.
    pub fn build_event(
        &self,
        actor: u32,
        group: u32,
        input: &EventInput,
        now: u64,
    ) -> Result<StoredEvent, ApiError> {
        let kind = pick(input.kind.as_deref().unwrap_or("generic"), KINDS, "kind")?;
        let p = preset(&kind).expect("checked above");
        let mut e = StoredEvent {
            id: 0,
            group,
            kind,
            hosts: vec![actor],
            where_kind: if p.modules.contains(&"rides") {
                "place"
            } else {
                "tbd"
            }
            .into(),
            visibility: "group".into(),
            plus_ones: p.plus_ones.into(),
            plus_max: p.plus_max,
            status: SCHEDULED.into(),
            rev: 1,
            created: now,
            updated: now,
            created_by: actor,
            modules: p.modules.iter().map(|m| m.to_string()).collect(),
            reminder_secs: p.reminder_secs,
            next_item: 1,
            ..Default::default()
        };
        self.apply_event_input(&mut e, input, true)?;
        if input.rsvp_by.is_none() && p.rsvp_before > 0 && e.start > p.rsvp_before {
            e.rsvp_by = e.start - p.rsvp_before;
        }
        Ok(e)
    }

    pub fn create_event(
        &mut self,
        actor: u32,
        group: u32,
        input: &EventInput,
        now: u64,
    ) -> Result<u32, ApiError> {
        self.can_create_event(actor, group)?;
        if self.upcoming_count() >= self.limits.upcoming_events {
            return Err(full("upcoming events", self.limits.upcoming_events));
        }
        let mut e = self.build_event(actor, group, input, now)?;
        if e.status == SCHEDULED && !e.polling() && e.start <= now {
            return Err(ApiError::bad_request("Pick a time in the future"));
        }
        let id = self.next_event;
        self.next_event += 1;
        e.id = id;
        let invitees = e.invitees.clone();
        let status = e.status.clone();
        self.events.push(e);
        if status == SCHEDULED {
            for p in invitees {
                self.notify(p, "invited", id, actor, now);
            }
        }
        self.log(
            actor,
            "event.create",
            format!("event {id} in group {group}"),
            now,
        );
        Ok(id)
    }

    /// A host's edit. Returns whether answers were affected.
    pub fn update_event(
        &mut self,
        actor: u32,
        id: u32,
        input: &EventInput,
        now: u64,
    ) -> Result<bool, ApiError> {
        let (e, access) = self.visible_event(Some(actor), id)?;
        if !access.edit {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only the hosts can change this event",
            ));
        }
        if !e.active() {
            return Err(ApiError::new(
                409,
                "not_active",
                "This event is over or cancelled",
            ));
        }
        if input.rev.is_some_and(|r| r != e.rev) {
            return Err(stale());
        }
        let before = e.clone();
        let mut next = e.clone();
        let affects = self.apply_event_input(&mut next, input, false)?;
        if next == before {
            return Ok(false);
        }
        next.updated = now;
        let was_draft = before.status == DRAFT && next.status == SCHEDULED;
        let new_invitees: Vec<u32> = next
            .invitees
            .iter()
            .copied()
            .filter(|p| was_draft || !before.invitees.contains(p))
            .collect();
        *self.events.get_mut(id).expect("checked above") = next;
        let now_scheduled = self.event(id).is_some_and(|e| e.status == SCHEDULED);
        if now_scheduled {
            for p in new_invitees {
                self.notify(p, "invited", id, actor, now);
            }
            if affects && !was_draft {
                for p in self.engaged(id) {
                    self.notify(p, "changed", id, actor, now);
                }
            }
        }
        // More room, or different plus-one rules: let the waitlist in.
        self.promote_waitlist(id, now);
        self.log(actor, "event.update", format!("event {id}"), now);
        Ok(affects)
    }

    pub fn cancel_event(&mut self, actor: u32, id: u32, now: u64) -> Result<(), ApiError> {
        let (e, access) = self.visible_event(Some(actor), id)?;
        if !access.edit {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only the hosts can cancel this event",
            ));
        }
        if !e.active() {
            return Ok(());
        }
        let was_scheduled = e.status == SCHEDULED;
        let mut tell: Vec<u32> = self.all_rsvps(id).map(|r| r.person).collect();
        tell.extend(e.invitees.iter().copied());
        tell.sort_unstable();
        tell.dedup();
        let e = self.events.get_mut(id).expect("checked above");
        e.status = CANCELLED.into();
        e.updated = now;
        e.edits += 1;
        if was_scheduled {
            for p in tell {
                self.notify(p, "cancelled", id, actor, now);
            }
        }
        self.log(actor, "event.cancel", format!("event {id}"), now);
        Ok(())
    }

    /// Deletes an event and its answers (hosts; meant for mistakes and drafts).
    pub fn delete_event(&mut self, actor: u32, id: u32, now: u64) -> Result<(), ApiError> {
        let (_, access) = self.visible_event(Some(actor), id)?;
        if !access.edit {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only the hosts can delete this event",
            ));
        }
        self.drop_event(id);
        self.log(actor, "event.delete", format!("event {id}"), now);
        Ok(())
    }

    fn drop_event(&mut self, id: u32) {
        if let Some(i) = self.events.position(id) {
            self.events.remove(i);
        }
        self.remove_rsvps(id);
    }

    /// Picks the winning time of a poll: the event gets that time, and
    /// everyone who said it works and was going or maybe is now going.
    pub fn lock_in(&mut self, actor: u32, id: u32, option: u32, now: u64) -> Result<(), ApiError> {
        let (e, access) = self.visible_event(Some(actor), id)?;
        if !access.edit {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only the hosts can pick the time",
            ));
        }
        if !e.polling() || !e.active() {
            return Err(ApiError::new(
                409,
                "not_polling",
                "This event's time is already set",
            ));
        }
        let o = e
            .when
            .iter()
            .find(|o| o.id == option)
            .cloned()
            .ok_or_else(|| ApiError::not_found("No such time"))?;
        let mut modules = e.modules.clone();
        modules.retain(|m| m != "when");
        let input = EventInput {
            start: Some(o.start),
            end: Some(o.end),
            modules: Some(modules),
            ..Default::default()
        };
        self.update_event(actor, id, &input, now)?;
        let winners: Vec<u32> = self
            .all_rsvps(id)
            .filter(|r| r.when.contains(&option) && r.status != "no")
            .map(|r| r.person)
            .collect();
        for person in winners {
            self.set_going(id, person, now);
        }
        self.log(
            actor,
            "event.lock",
            format!("event {id} option {option}"),
            now,
        );
        Ok(())
    }

    /// People to tell about a change: everyone going or maybe, and hosts.
    pub fn engaged(&self, id: u32) -> Vec<u32> {
        let mut out: Vec<u32> = self
            .all_rsvps(id)
            .filter(|r| r.status == "going" || r.status == "maybe" || r.status == "waitlist")
            .map(|r| r.person)
            .collect();
        if let Some(e) = self.event(id) {
            out.extend(e.hosts.iter().copied());
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Housekeeping: scheduled events whose end passed become done, and the
    /// past ring drops the oldest. Returns whether anything changed.
    pub fn advance_events(&mut self, now: u64) -> bool {
        let mut changed = false;
        for i in 0..self.events.len() {
            let e = &self.events[i];
            if e.status == SCHEDULED && !e.polling() && now >= e.effective_end() {
                self.events[i].status = DONE.into();
                changed = true;
            }
        }
        let past: Vec<(u64, u32)> = self
            .events
            .iter()
            .filter(|e| !e.active())
            .map(|e| (e.effective_end().max(e.updated), e.id))
            .collect();
        if past.len() > self.limits.past_events {
            let mut past = past;
            past.sort_unstable();
            for (_, id) in past.iter().take(past.len() - self.limits.past_events) {
                self.drop_event(*id);
            }
            changed = true;
        }
        changed
    }

    /// Reminders that are due (the event's lead time before its start).
    pub fn send_reminders(&mut self, now: u64) -> bool {
        let due: Vec<u32> = self
            .events
            .iter()
            .filter(|e| {
                e.status == SCHEDULED
                    && e.reminder_secs > 0
                    && !e.reminded
                    && e.start > now
                    && now + e.reminder_secs >= e.start
            })
            .map(|e| e.id)
            .collect();
        for &id in &due {
            let tell: Vec<u32> = self
                .all_rsvps(id)
                .filter(|r| r.status == "going" || r.status == "maybe")
                .map(|r| r.person)
                .collect();
            for p in tell {
                self.notify(p, "reminder", id, 0, now);
            }
            if let Some(e) = self.events.get_mut(id) {
                e.reminded = true;
            }
        }
        !due.is_empty()
    }

    // ---- views ------------------------------------------------------------------

    pub fn heads(&self, id: u32) -> u32 {
        self.all_rsvps(id)
            .filter(|r| r.status == "going")
            .map(|r| 1 + r.plus)
            .sum()
    }

    /// Counts of claims per item: bring slots, poll options or shifts.
    fn counts(
        &self,
        id: u32,
        items: &[u32],
        pick: impl Fn(&crate::messages::StoredRsvp) -> &[u32],
    ) -> Vec<SlotCount> {
        items
            .iter()
            .map(|&item| {
                let names: Vec<String> = self
                    .all_rsvps(id)
                    .filter(|r| r.status != "no" && pick(r).contains(&item))
                    .map(|r| self.name(r.person))
                    .collect();
                SlotCount {
                    id: item,
                    count: names.len() as u32,
                    names,
                }
            })
            .collect()
    }

    pub fn event_summary(&self, viewer: Option<u32>, e: &StoredEvent) -> EventSummary {
        EventSummary {
            id: e.id,
            group: e.group,
            group_name: self
                .group(e.group)
                .map(|g| g.name.clone())
                .unwrap_or_default(),
            title: e.title.clone(),
            kind: e.kind.clone(),
            start: e.start,
            end: e.end,
            status: e.status.clone(),
            heads: self.heads(e.id),
            capacity: e.capacity,
            my_status: viewer
                .and_then(|v| self.rsvp(e.id, v))
                .map(|r| r.status.clone())
                .unwrap_or_default(),
            series: e.series,
            polling: e.polling(),
        }
    }

    pub fn event_view(
        &self,
        viewer: Option<u32>,
        id: u32,
        now: u64,
    ) -> Result<crate::messages::EventView, ApiError> {
        let (e, access) = self.visible_event(viewer, id)?;
        let g = self.group(e.group);
        let count = |s: &str| self.all_rsvps(id).filter(|r| r.status == s).count() as u32;
        let mine = viewer
            .and_then(|v| self.rsvp(id, v))
            .map(|r| self.rsvp_view(viewer, e, r, access.edit));
        let answer_for = match viewer {
            Some(v) => self
                .answerable_by(v, e)
                .into_iter()
                .filter(|&p| p != v)
                .filter_map(|p| self.person(p))
                .take(100)
                .map(|p| PersonView {
                    id: p.id,
                    display_name: p.display_name.clone(),
                    bio: String::new(),
                    household: p.household,
                    household_name: self.household_name(p.household),
                })
                .collect(),
            None => Vec::new(),
        };
        let slot_ids: Vec<u32> = e.bring.iter().map(|s| s.id).collect();
        let option_ids: Vec<u32> = e.when.iter().map(|o| o.id).collect();
        let shift_ids: Vec<u32> = e.shifts.iter().map(|s| s.id).collect();
        Ok(crate::messages::EventView {
            event: e.clone(),
            group_name: g.map(|g| g.name.clone()).unwrap_or_default(),
            group_slug: g.map(|g| g.slug.clone()).unwrap_or_default(),
            host_names: e.hosts.iter().map(|h| self.name(*h)).collect(),
            heads: self.heads(id),
            going: count("going"),
            maybe: count("maybe"),
            no: count("no"),
            waitlist: count("waitlist"),
            mine,
            can_edit: access.edit,
            can_answer: access.answer && !e.locked(now),
            answer_for,
            claimed: self.counts(id, &slot_ids, |r| &r.bring),
            votes: self.counts(id, &option_ids, |r| &r.when),
            taken: self.counts(id, &shift_ids, |r| &r.shifts),
            locked: e.locked(now),
        })
    }

    /// Events `viewer` may see, soonest first: upcoming ones (and polls)
    /// unless `past`. `after` is the last id of the previous page.
    #[allow(clippy::too_many_arguments)]
    pub fn list_events(
        &self,
        viewer: Option<u32>,
        group: Option<u32>,
        mine: bool,
        past: bool,
        now: u64,
        after: u32,
        limit: usize,
    ) -> (Vec<EventSummary>, u32) {
        let mut found: Vec<&StoredEvent> = self
            .events
            .iter()
            .filter(|e| group.is_none_or(|g| e.group == g))
            .filter(|e| {
                let upcoming = e.active() && (e.polling() || e.effective_end() > now);
                upcoming != past
            })
            .filter(|e| self.event_access(viewer, e).see)
            .filter(|e| {
                !mine
                    || viewer.is_some_and(|v| {
                        e.hosts.contains(&v)
                            || e.invitees.contains(&v)
                            || self.rsvp(e.id, v).is_some_and(|r| r.status != "no")
                    })
            })
            .collect();
        // Polls (no start yet) first, then by time; past events newest first.
        found.sort_by_key(|e| (if past { u64::MAX - e.start } else { e.start }, e.id));
        let start = if after == 0 {
            0
        } else {
            found
                .iter()
                .position(|e| e.id == after)
                .map_or(found.len(), |i| i + 1)
        };
        let page: Vec<EventSummary> = found
            .iter()
            .skip(start)
            .take(limit)
            .map(|e| self.event_summary(viewer, e))
            .collect();
        let next = if start + page.len() < found.len() {
            page.last().map_or(0, |e| e.id)
        } else {
            0
        };
        (page, next)
    }
}
