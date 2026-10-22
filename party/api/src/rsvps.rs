//! Answers to events (`spec/03-domain.md#rsvps`): going/maybe/no with
//! guests, module answers, capacity counted in heads, a waitlist in
//! arrival order, and answering for someone.
//!
//! Storage: an event's answers live in blocks of [`BLOCK`] (`r<id>`), so one
//! answer rewrites about a kilobyte, however many people answered.
use crate::events::{MAX_GUESTS, SCHEDULED};
use crate::messages::{
    RideBoard, RideEntry, RsvpInput, RsvpView, StoredEvent, StoredRsvp, StoredRsvpBlock,
};
use crate::state::{full, State, ACTIVE, DISABLED};
use miniframework::ApiError;

/// Answers per block record.
pub const BLOCK: usize = 16;
/// Block ids per event: `event * BLOCKS_PER_EVENT + n` (64 × 16 = 1,024 ≥
/// `Limits::rsvps_per_event`).
pub const BLOCKS_PER_EVENT: u32 = 64;

pub const GOING: &str = "going";
pub const MAYBE: &str = "maybe";
pub const NO: &str = "no";
pub const WAITLIST: &str = "waitlist";

fn text(raw: &str, max: usize, what: &str) -> Result<String, ApiError> {
    let t = raw.trim();
    if t.chars().count() > max || t.chars().any(char::is_control) {
        return Err(ApiError::bad_request(format!(
            "{what}: at most {max} characters"
        )));
    }
    Ok(t.to_string())
}

fn heads(r: &StoredRsvp) -> u32 {
    1 + r.plus
}

impl State {
    /// Indexes (in the rsvps table) of `event`'s blocks.
    fn block_range(&self, event: u32) -> std::ops::Range<usize> {
        let lo = event.saturating_mul(BLOCKS_PER_EVENT);
        let hi = lo.saturating_add(BLOCKS_PER_EVENT);
        let start = self.rsvps.partition_point(|b| b.id < lo);
        let end = self.rsvps.partition_point(|b| b.id < hi);
        start..end
    }

    pub fn all_rsvps(&self, event: u32) -> impl Iterator<Item = &StoredRsvp> {
        (*self.rsvps)[self.block_range(event)]
            .iter()
            .flat_map(|b| b.rsvps.iter())
    }

    pub fn rsvp(&self, event: u32, person: u32) -> Option<&StoredRsvp> {
        self.all_rsvps(event).find(|r| r.person == person)
    }

    /// Stores `r` (replacing that person's answer, or in a block with room).
    fn put_rsvp(&mut self, event: u32, r: StoredRsvp) -> Result<(), ApiError> {
        let range = self.block_range(event);
        for i in range.clone() {
            if let Some(j) = self.rsvps[i]
                .rsvps
                .iter()
                .position(|x| x.person == r.person)
            {
                self.rsvps[i].rsvps[j] = r;
                return Ok(());
            }
        }
        if self.all_rsvps(event).count() >= self.limits.rsvps_per_event {
            return Err(full("answers for one event", self.limits.rsvps_per_event));
        }
        if let Some(i) = range.clone().find(|&i| self.rsvps[i].rsvps.len() < BLOCK) {
            self.rsvps[i].rsvps.push(r);
            return Ok(());
        }
        let base = event * BLOCKS_PER_EVENT;
        let used: Vec<u32> = (*self.rsvps)[range].iter().map(|b| b.id).collect();
        let id = (base..base + BLOCKS_PER_EVENT)
            .find(|id| !used.contains(id))
            .ok_or_else(|| full("answers for one event", self.limits.rsvps_per_event))?;
        self.rsvps.push(StoredRsvpBlock {
            id,
            event,
            rsvps: vec![r],
        });
        Ok(())
    }

    pub fn remove_rsvps(&mut self, event: u32) {
        let range = self.block_range(event);
        for _ in range.clone() {
            self.rsvps.remove(range.start);
        }
    }

    /// May this person answer `e` for themselves (ignoring the deadline)?
    pub fn eligible(&self, person: u32, e: &StoredEvent) -> bool {
        self.person(person).is_some_and(|p| p.status != DISABLED)
            && self.event_access(Some(person), e).answer
    }

    /// Who `actor` may answer for: themselves; their household; and, as a
    /// host, everyone eligible.
    pub fn answerable_by(&self, actor: u32, e: &StoredEvent) -> Vec<u32> {
        let access = self.event_access(Some(actor), e);
        let mut out = Vec::new();
        if access.edit {
            if let Some(g) = self.group(e.group) {
                out.extend(
                    g.members
                        .iter()
                        .filter(|m| m.status == ACTIVE)
                        .map(|m| m.person),
                );
            }
            out.extend(e.invitees.iter().copied());
            out.extend(e.hosts.iter().copied());
        } else {
            let home = self.person(actor).map_or(0, |p| p.household);
            if home != 0 {
                out.extend(self.household_members(home).map(|p| p.id));
            }
            out.push(actor);
        }
        out.sort_unstable();
        out.dedup();
        out.retain(|&p| self.eligible(p, e) || (p == actor && access.answer));
        out
    }

    /// Answers event `id` for `person` (who may be someone else, see
    /// [`State::answerable_by`]). Returns false when nothing changed (a
    /// repeated identical answer is no write and tells nobody).
    pub fn answer(
        &mut self,
        actor: u32,
        id: u32,
        person: u32,
        input: &RsvpInput,
        now: u64,
    ) -> Result<bool, ApiError> {
        let (e, access) = self.visible_event(Some(actor), id)?;
        let e = e.clone();
        if e.status != SCHEDULED {
            return Err(ApiError::new(
                409,
                "locked",
                "This event is over or cancelled",
            ));
        }
        if !self.answerable_by(actor, &e).contains(&person) {
            return Err(if person == actor {
                ApiError::new(
                    403,
                    "not_allowed",
                    "Join the group (or get invited) to answer",
                )
            } else {
                ApiError::new(403, "not_allowed", "You can't answer for them")
            });
        }
        if e.locked(now) && !access.edit {
            return Err(ApiError::new(
                409,
                "locked",
                "Answers for this event are closed; ask a host",
            ));
        }
        let status = match input.status.as_str() {
            s @ (GOING | MAYBE | NO) => s,
            _ => return Err(ApiError::bad_request("Answer going, maybe or no")),
        };
        let current = self.rsvp(id, person).cloned();
        let mut r = current.clone().unwrap_or(StoredRsvp {
            person,
            ..Default::default()
        });

        // Lists that refer to the event's items need a current view of them.
        let changed_list = |given: &Option<Vec<u32>>, had: &[u32]| {
            given.as_ref().is_some_and(|g| g.as_slice() != had)
        };
        if input.rev_seen != e.rev
            && (changed_list(&input.bring, &r.bring)
                || changed_list(&input.when, &r.when)
                || changed_list(&input.shifts, &r.shifts))
        {
            return Err(ApiError::new(
                409,
                "stale",
                "The event changed since you opened it; here's what's new",
            ));
        }

        if let Some(plus) = input.plus {
            let max = match e.plus_ones.as_str() {
                "none" => 0,
                "limited" => e.plus_max,
                _ => MAX_GUESTS,
            };
            // Keeping an over-policy count (the rules changed since) is allowed.
            if plus > max && current.as_ref().is_none_or(|c| plus > c.plus) {
                return Err(ApiError::new(
                    400,
                    "too_many_guests",
                    if max == 0 {
                        "This event is just for the people invited".to_string()
                    } else {
                        format!("You can bring at most {max}")
                    },
                ));
            }
            r.plus = plus;
        }
        if let Some(kids) = input.kids {
            r.kids = kids;
        }
        if r.kids > r.plus {
            return Err(ApiError::bad_request("Kids are counted among your guests"));
        }
        if let Some(n) = &input.names {
            r.names = text(n, 200, "Guests' names")?;
        }
        if let Some(n) = &input.note {
            r.note = text(n, 200, "Note")?;
        }
        if let Some(bring) = &input.bring {
            if !e.has("bring") && !bring.is_empty() {
                return Err(ApiError::bad_request("This event has no bring list"));
            }
            let mut bring = bring.clone();
            bring.sort_unstable();
            bring.dedup();
            for slot in &bring {
                let s = e
                    .bring
                    .iter()
                    .find(|s| s.id == *slot)
                    .ok_or_else(|| ApiError::bad_request("No such bring-list item"))?;
                let others = self
                    .all_rsvps(id)
                    .filter(|o| o.person != person && o.status != NO && o.bring.contains(slot))
                    .count() as u32;
                if !r.bring.contains(slot) && others >= s.count {
                    return Err(ApiError::new(
                        409,
                        "slot_full",
                        format!("{} is covered already", s.name),
                    ));
                }
            }
            r.bring = bring;
        }
        if let Some(o) = &input.bring_other {
            r.bring_other = text(o, 100, "Something else")?;
        }
        if let Some(d) = &input.diet {
            r.diet = text(d, 200, "Diet")?;
        }
        if let Some(answers) = &input.answers {
            let mut out = Vec::new();
            for a in answers {
                let q = e
                    .questions
                    .iter()
                    .find(|q| q.id == a.question)
                    .ok_or_else(|| ApiError::bad_request("No such question"))?;
                let value = text(&a.value, 200, "Answer")?;
                let ok = match q.kind.as_str() {
                    "yesno" => value == "yes" || value == "no" || value.is_empty(),
                    "choice" => value.is_empty() || q.choices.contains(&value),
                    _ => true,
                };
                if !ok {
                    return Err(ApiError::bad_request(format!(
                        "\"{}\": pick one of the answers",
                        q.prompt
                    )));
                }
                if !value.is_empty() {
                    out.push(crate::messages::StoredAnswer {
                        question: q.id,
                        value,
                    });
                }
            }
            r.answers = out;
        }
        if let Some(ride) = &input.ride {
            if !matches!(ride.as_str(), "" | "need" | "offer" | "own") {
                return Err(ApiError::bad_request("Ride: need, offer or own"));
            }
            r.ride = ride.clone();
        }
        if let Some(seats) = input.seats {
            if seats > 8 {
                return Err(ApiError::bad_request("Seats: at most 8"));
            }
            r.seats = seats;
        }
        if let Some(when) = &input.when {
            if when.iter().any(|w| !e.when.iter().any(|o| o.id == *w)) {
                return Err(ApiError::bad_request("No such time"));
            }
            let mut when = when.clone();
            when.sort_unstable();
            when.dedup();
            r.when = when;
        }
        if let Some(shifts) = &input.shifts {
            let mut shifts = shifts.clone();
            shifts.sort_unstable();
            shifts.dedup();
            for id_ in &shifts {
                let s = e
                    .shifts
                    .iter()
                    .find(|s| s.id == *id_)
                    .ok_or_else(|| ApiError::bad_request("No such shift"))?;
                let others = self
                    .all_rsvps(id)
                    .filter(|o| o.person != person && o.status != NO && o.shifts.contains(id_))
                    .count() as u32;
                if !r.shifts.contains(id_) && others >= s.count {
                    return Err(ApiError::new(
                        409,
                        "slot_full",
                        format!("{} is full already", s.name),
                    ));
                }
            }
            r.shifts = shifts;
        }
        if status == NO {
            // Not coming: give back what they claimed.
            r.bring.clear();
            r.shifts.clear();
            r.ride.clear();
            r.seats = 0;
        }
        if !e.has("rides") {
            r.ride.clear();
            r.seats = 0;
        }

        // Capacity in heads; a going answer that doesn't fit waits.
        let was_going = current.as_ref().is_some_and(|c| c.status == GOING);
        r.status = status.into();
        if status == GOING && e.capacity > 0 {
            let others: u32 = self
                .all_rsvps(id)
                .filter(|o| o.person != person && o.status == GOING)
                .map(heads)
                .sum();
            let fits = others + heads(&r) <= e.capacity;
            let waiting = current.as_ref().is_some_and(|c| c.status == WAITLIST);
            if !fits || (waiting && !was_going && self.waiting_ahead(id, person)) {
                r.status = WAITLIST.into();
            }
        }
        if r.status == WAITLIST {
            if r.queued == 0 {
                r.queued = self.next_queue;
                self.next_queue += 1;
            }
        } else {
            r.queued = 0;
        }
        r.rev_seen = e.rev;
        r.by = actor;
        r.via = if actor == person {
            "web"
        } else if access.edit {
            "host"
        } else {
            "household"
        }
        .into();

        let same = current.as_ref().is_some_and(|c| {
            let mut c = c.clone();
            c.updated = 0;
            c.by = r.by;
            c.via = r.via.clone();
            c.rev_seen = r.rev_seen;
            let mut n = r.clone();
            n.updated = 0;
            c == n
        });
        if same {
            return Ok(false);
        }
        r.updated = now;
        self.put_rsvp(id, r)?;
        if actor != person {
            self.notify(person, "answered_for", id, actor, now);
        }
        self.promote_waitlist(id, now);
        Ok(true)
    }

    /// Someone earlier on the waitlist is still waiting (they go first).
    fn waiting_ahead(&self, id: u32, person: u32) -> bool {
        let mine = self.rsvp(id, person).map_or(u64::MAX, |r| r.queued);
        self.all_rsvps(id)
            .any(|o| o.person != person && o.status == WAITLIST && o.queued < mine)
    }

    /// Lets waitlisted answers in, oldest first, while they fit.
    pub fn promote_waitlist(&mut self, id: u32, now: u64) {
        let Some(e) = self.event(id) else { return };
        if e.status != SCHEDULED {
            return;
        }
        let capacity = e.capacity;
        let mut waiting: Vec<(u64, u32)> = self
            .all_rsvps(id)
            .filter(|r| r.status == WAITLIST)
            .map(|r| (r.queued, r.person))
            .collect();
        waiting.sort_unstable();
        for (_, person) in waiting {
            let going = self.heads(id);
            let Some(mut r) = self.rsvp(id, person).cloned() else {
                continue;
            };
            if capacity == 0 || going + heads(&r) <= capacity {
                r.status = GOING.into();
                r.queued = 0;
                r.updated = now;
                if self.put_rsvp(id, r).is_ok() {
                    self.notify(person, "promoted", id, 0, now);
                }
            }
        }
    }

    /// Makes `person` going (a poll's winners, a series' regulars), within
    /// capacity; creates the answer if there is none.
    pub fn set_going(&mut self, id: u32, person: u32, now: u64) {
        let Some(e) = self.event(id) else { return };
        let capacity = e.capacity;
        let rev = e.rev;
        let mut r = self.rsvp(id, person).cloned().unwrap_or(StoredRsvp {
            person,
            by: person,
            via: "regular".into(),
            ..Default::default()
        });
        if r.status == GOING {
            return;
        }
        let others: u32 = self
            .all_rsvps(id)
            .filter(|o| o.person != person && o.status == GOING)
            .map(heads)
            .sum();
        if capacity == 0 || others + heads(&r) <= capacity {
            r.status = GOING.into();
            r.queued = 0;
        } else {
            r.status = WAITLIST.into();
            if r.queued == 0 {
                r.queued = self.next_queue;
                self.next_queue += 1;
            }
        }
        r.rev_seen = rev;
        r.updated = now;
        let _ = self.put_rsvp(id, r);
    }

    /// An answer as `viewer` may see it: diet and question answers only for
    /// the person and the hosts.
    pub fn rsvp_view(
        &self,
        viewer: Option<u32>,
        e: &StoredEvent,
        r: &StoredRsvp,
        edit: bool,
    ) -> RsvpView {
        let private = edit || viewer == Some(r.person);
        let max = match e.plus_ones.as_str() {
            "none" => 0,
            "limited" => e.plus_max,
            _ => MAX_GUESTS,
        };
        RsvpView {
            person: r.person,
            display_name: self.name(r.person),
            status: r.status.clone(),
            plus: r.plus,
            kids: r.kids,
            names: r.names.clone(),
            bring: r.bring.clone(),
            bring_other: r.bring_other.clone(),
            diet: if private {
                r.diet.clone()
            } else {
                String::new()
            },
            answers: if private {
                r.answers.clone()
            } else {
                Vec::new()
            },
            ride: r.ride.clone(),
            seats: r.seats,
            when: r.when.clone(),
            shifts: r.shifts.clone(),
            note: r.note.clone(),
            by_name: if r.by != 0 && r.by != r.person {
                self.name(r.by)
            } else {
                String::new()
            },
            via: r.via.clone(),
            updated: r.updated,
            over_policy: r.plus > max,
        }
    }

    /// Everyone's answers, going first, then maybe, the waitlist (in
    /// order) and no.
    pub fn rsvp_list(&self, viewer: u32, id: u32) -> Result<Vec<RsvpView>, ApiError> {
        let (e, access) = self.visible_event(Some(viewer), id)?;
        let order = |s: &str| match s {
            GOING => 0,
            MAYBE => 1,
            WAITLIST => 2,
            _ => 3,
        };
        let mut all: Vec<&StoredRsvp> = self.all_rsvps(id).collect();
        all.sort_by_key(|r| (order(&r.status), r.queued, r.updated));
        Ok(all
            .into_iter()
            .map(|r| self.rsvp_view(Some(viewer), e, r, access.edit))
            .collect())
    }

    /// Who needs a ride and who offers seats (hosts only).
    pub fn ride_board(&self, viewer: u32, id: u32) -> Result<RideBoard, ApiError> {
        let (_, access) = self.visible_event(Some(viewer), id)?;
        if !access.edit {
            return Err(ApiError::new(
                403,
                "not_allowed",
                "Only the hosts see the ride board",
            ));
        }
        let entry = |r: &StoredRsvp| RideEntry {
            person: r.person,
            display_name: self.name(r.person),
            seats: r.seats.max(1),
        };
        let live = |r: &&StoredRsvp| r.status != NO;
        let needs: Vec<RideEntry> = self
            .all_rsvps(id)
            .filter(live)
            .filter(|r| r.ride == "need")
            .map(entry)
            .collect();
        let offers: Vec<RideEntry> = self
            .all_rsvps(id)
            .filter(live)
            .filter(|r| r.ride == "offer")
            .map(entry)
            .collect();
        let spare = offers.iter().map(|o| o.seats as i32).sum::<i32>()
            - needs.iter().map(|n| n.seats as i32).sum::<i32>();
        Ok(RideBoard {
            needs,
            offers,
            spare,
        })
    }
}
