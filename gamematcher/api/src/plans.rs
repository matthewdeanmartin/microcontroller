//! Plans and responses: the negotiation (`spec/03-domain.md#plans`).
//!
//! A plan has 1–8 candidate games and 0–6 time options. People tick the
//! games they'd play and say yes / if needed / no to each time. The board
//! shows counts and names and never picks: the host locks it in.
//!
//! open ──lock──▶ on ──(chosen time + 6 h)──▶ done
//!   │ ◀─reopen──┘
//!   └──────cancel──────▶ cancelled
use crate::messages::{
    LockIn, PlanInput, ResponseInput, StoredPlan, StoredResponse, StoredTime, TimeAnswer,
};
use crate::notify;
use crate::state::State;
use miniframework::ApiError;

pub const OPEN: &str = "open";
pub const ON: &str = "on";
pub const DONE: &str = "done";
pub const CANCELLED: &str = "cancelled";

pub const MAX_GAMES: usize = 8;
pub const MAX_TIMES: usize = 6;
/// A locked-in plan becomes done this long after its chosen time.
pub const DONE_AFTER: u64 = 6 * 3600;

pub fn active(plan: &StoredPlan) -> bool {
    plan.status == OPEN || plan.status == ON
}

fn text(raw: &str, max: usize, what: &str) -> Result<String, ApiError> {
    let t = raw.trim();
    if t.chars().count() > max || t.chars().any(|c| c.is_control() && c != '\n') {
        return Err(ApiError::bad_request(format!(
            "{what}: up to {max} characters"
        )));
    }
    Ok(t.to_string())
}

fn changed() -> ApiError {
    ApiError::new(
        409,
        "plan_changed",
        "The plan changed while you were answering; here it is again",
    )
}

fn closed() -> ApiError {
    ApiError::new(409, "plan_closed", "This plan is finished or cancelled")
}

impl State {
    pub fn plan(&self, id: u32) -> Result<&StoredPlan, ApiError> {
        self.plans
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ApiError::not_found("No such plan"))
    }

    fn plan_index(&self, id: u32) -> Result<usize, ApiError> {
        self.plans
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| ApiError::not_found("No such plan"))
    }

    pub fn can_manage(&self, actor: u32, plan: &StoredPlan) -> bool {
        plan.host == actor || self.account(actor).is_some_and(|a| a.admin)
    }

    fn managed(&self, actor: u32, id: u32) -> Result<usize, ApiError> {
        let i = self.plan_index(id)?;
        if !self.can_manage(actor, &self.plans[i]) {
            return Err(ApiError::new(
                403,
                "not_yours",
                "Only the host, or an admin, can change this plan",
            ));
        }
        Ok(i)
    }

    fn check_games(&self, games: &[u32]) -> Result<Vec<u32>, ApiError> {
        let mut out: Vec<u32> = Vec::new();
        for &g in games {
            if self.game(g).is_none() {
                return Err(ApiError::bad_request(format!("No game {g}")));
            }
            if !out.contains(&g) {
                out.push(g);
            }
        }
        if out.is_empty() || out.len() > MAX_GAMES {
            return Err(ApiError::bad_request(format!(
                "Pick 1 to {MAX_GAMES} games"
            )));
        }
        Ok(out)
    }

    /// Applies `input` to `plan`; games and times only while open.
    fn apply_plan(&self, plan: &mut StoredPlan, input: &PlanInput) -> Result<(), ApiError> {
        if let Some(title) = &input.title {
            plan.title = text(title, 48, "Title")?;
        }
        if let Some(notes) = &input.notes {
            plan.notes = text(notes, 500, "Notes")?;
        }
        if let Some(place) = &input.place {
            plan.place = text(place, 64, "Where")?;
        }
        if let Some(announce) = input.announce_public {
            plan.announce_public = announce;
        }
        if let Some(announce) = input.announce {
            plan.announce = announce;
        }
        if let Some(seats) = input.seats {
            if seats > 99 {
                return Err(ApiError::bad_request("Seats: up to 99"));
            }
            plan.seats = seats;
        }
        if (input.games.is_some() || input.times.is_some()) && plan.status != OPEN {
            return Err(ApiError::new(
                409,
                "plan_locked",
                "Reopen the plan to change its games or times",
            ));
        }
        if let Some(games) = &input.games {
            plan.games = self.check_games(games)?;
            for r in &mut plan.responses {
                r.games.retain(|g| plan.games.contains(g));
            }
        }
        if let Some(times) = &input.times {
            if times.len() > MAX_TIMES {
                return Err(ApiError::bad_request(format!("Up to {MAX_TIMES} times")));
            }
            let mut next = Vec::with_capacity(times.len());
            for t in times {
                if t.start == 0
                    || (t.end != 0 && t.end <= t.start)
                    || t.end.saturating_sub(t.start) > 48 * 3600
                {
                    return Err(ApiError::bad_request(
                        "Each time needs a start, and an end after it",
                    ));
                }
                let kept = plan
                    .times
                    .iter()
                    .find(|old| old.id == t.id && old.start == t.start && old.end == t.end);
                next.push(match kept {
                    Some(old) => old.clone(),
                    None => {
                        plan.next_time += 1;
                        StoredTime {
                            id: plan.next_time,
                            start: t.start,
                            end: t.end,
                        }
                    }
                });
            }
            next.sort_by_key(|t| t.start);
            plan.times = next;
            for r in &mut plan.responses {
                r.times
                    .retain(|a| plan.times.iter().any(|t| t.id == a.time));
            }
        }
        Ok(())
    }

    pub fn create_plan(&mut self, host: u32, input: &PlanInput, now: u64) -> Result<u32, ApiError> {
        if input.games.is_none() {
            return Err(ApiError::bad_request(format!(
                "Pick 1 to {MAX_GAMES} games"
            )));
        }
        if self.plans.iter().filter(|p| active(p)).count() >= self.limits.active_plans {
            return Err(ApiError::new(
                409,
                "full",
                format!(
                    "At most {} plans can be open at once; finish or cancel one",
                    self.limits.active_plans
                ),
            ));
        }
        let mut plan = StoredPlan {
            id: self.next_plan,
            host,
            place: "Home".into(),
            status: OPEN.into(),
            created: now,
            updated: now,
            ..Default::default()
        };
        self.apply_plan(&mut plan, input)?;
        self.next_plan += 1;
        let id = plan.id;
        self.plans.push(plan);
        self.on_plan_created(id, host, now);
        Ok(id)
    }

    pub fn edit_plan(
        &mut self,
        actor: u32,
        id: u32,
        input: &PlanInput,
        now: u64,
    ) -> Result<(), ApiError> {
        if input.repeat.is_some() {
            return Err(ApiError::bad_request(
                "Repeat is only available when creating a new series",
            ));
        }
        let i = self.managed(actor, id)?;
        if !active(&self.plans[i]) {
            return Err(closed());
        }
        let mut plan = self.plans[i].clone();
        self.apply_plan(&mut plan, input)?;
        if let Some(time) = &input.reschedule {
            if plan.status != ON {
                return Err(ApiError::bad_request(
                    "Only a locked-in plan can be rescheduled",
                ));
            }
            if time.start <= now
                || (time.end != 0 && time.end <= time.start)
                || time.end.saturating_sub(time.start) > 48 * 3600
            {
                return Err(ApiError::bad_request(
                    "Choose a future start and an end after it",
                ));
            }
            let chosen = plan
                .times
                .iter_mut()
                .find(|t| Some(t.id) == plan.chosen_time)
                .ok_or_else(|| ApiError::bad_request("No locked-in time"))?;
            if chosen.start != time.start || chosen.end != time.end {
                plan.next_time += 1;
                chosen.id = plan.next_time;
                chosen.start = time.start;
                chosen.end = time.end;
                plan.chosen_time = Some(chosen.id);
                plan.reminded = false;
                for response in &mut plan.responses {
                    response
                        .times
                        .retain(|a| plan.times.iter().any(|t| t.id == a.time));
                }
                plan.times.sort_by_key(|t| t.start);
            }
        }
        plan.updated = now;
        plan.edits += 1;
        let previous = &self.plans[i];
        let times_changed = plan.times != previous.times
            || plan.games != previous.games
            || plan.title != previous.title
            || plan.notes != previous.notes
            || plan.place != previous.place
            || plan.seats != previous.seats;
        self.plans[i] = plan;
        if times_changed {
            self.on_plan_event(id, notify::CHANGED, actor, now);
        }
        Ok(())
    }

    /// The host records what was played and who won, then closes the plan.
    pub fn report_plan(
        &mut self,
        actor: u32,
        id: u32,
        report: &crate::messages::PostGameReport,
        now: u64,
    ) -> Result<(), ApiError> {
        let i = self.managed(actor, id)?;
        let p = &self.plans[i];
        if p.status != DONE
            && !(p.status == ON
                && p.times
                    .iter()
                    .any(|t| Some(t.id) == p.chosen_time && t.start <= now))
        {
            return Err(ApiError::bad_request(
                "A report can be saved after a confirmed game has started",
            ));
        }
        let summary = text(&report.summary, 1000, "Report")?;
        if report.games.len() > MAX_GAMES {
            return Err(ApiError::bad_request("Up to 8 game results"));
        }
        let mut seen = Vec::new();
        for game in &report.games {
            if !p.games.contains(&game.game) || seen.contains(&game.game) {
                return Err(ApiError::bad_request(
                    "Choose each played game once from this plan",
                ));
            }
            seen.push(game.game);
            let mut winners = game.winners.clone();
            winners.sort_unstable();
            winners.dedup();
            if winners.len() != game.winners.len()
                || winners.iter().any(|&id| {
                    id != p.host
                        && !p
                            .responses
                            .iter()
                            .any(|r| r.account == id && r.overall == "in")
                })
            {
                return Err(ApiError::bad_request(
                    "Winners must be the host or attendees",
                ));
            }
        }
        self.plans[i].report = Some(crate::messages::PostGameReport {
            summary,
            games: report.games.clone(),
        });
        self.plans[i].updated = now;
        self.plans[i].edits += 1;
        if self.plans[i].status == ON {
            self.plans[i].status = DONE.into();
            self.finish(i, now);
        }
        Ok(())
    }

    /// Replaces `account`'s whole response. A time or game the plan no
    /// longer has means the caller saw an old version: 409 `plan_changed`.
    pub fn respond(
        &mut self,
        account: u32,
        id: u32,
        input: &ResponseInput,
        now: u64,
    ) -> Result<(), ApiError> {
        let i = self.plan_index(id)?;
        let plan = &self.plans[i];
        if !active(plan) {
            return Err(closed());
        }
        if !matches!(input.overall.as_str(), "in" | "maybe" | "out") {
            return Err(ApiError::bad_request("overall: in, maybe or out"));
        }
        let mut times: Vec<TimeAnswer> = Vec::new();
        for a in &input.times {
            if !plan.times.iter().any(|t| t.id == a.time) {
                return Err(changed());
            }
            if !matches!(a.answer.as_str(), "yes" | "if_needed" | "no") {
                return Err(ApiError::bad_request("answer: yes, if_needed or no"));
            }
            times.retain(|t| t.time != a.time);
            times.push(a.clone());
        }
        let mut games: Vec<u32> = Vec::new();
        for &g in &input.games {
            if !plan.games.contains(&g) {
                return Err(changed());
            }
            if !games.contains(&g) {
                games.push(g);
            }
        }
        let note = text(&input.note, 140, "Note")?;
        let plan = &mut self.plans[i];
        plan.responses.retain(|r| r.account != account);
        plan.responses.push(StoredResponse {
            account,
            overall: input.overall.clone(),
            times,
            games,
            note,
            updated: now,
        });
        plan.updated = now;
        if input.overall != "out" {
            self.record("rsvp", id, account, &input.overall, now);
        }
        self.on_response(id, account, now);
        Ok(())
    }

    pub fn lock_plan(
        &mut self,
        actor: u32,
        id: u32,
        lock: &LockIn,
        now: u64,
    ) -> Result<(), ApiError> {
        let i = self.managed(actor, id)?;
        let plan = &mut self.plans[i];
        if !active(plan) {
            return Err(closed());
        }
        if !plan.times.iter().any(|t| t.id == lock.time) {
            return Err(ApiError::bad_request("Pick one of the plan's times"));
        }
        if lock.games.iter().any(|g| !plan.games.contains(g)) {
            return Err(ApiError::bad_request("Pick from the plan's games"));
        }
        let mut games = lock.games.clone();
        games.dedup();
        let moved = plan.chosen_time != Some(lock.time);
        plan.status = ON.into();
        plan.chosen_time = Some(lock.time);
        plan.chosen_games = games;
        plan.updated = now;
        plan.edits += 1;
        if moved {
            // A new time needs a new reminder.
            plan.reminded = false;
        }
        self.on_plan_event(id, notify::LOCKED, actor, now);
        self.record("locked", id, actor, "", now);
        Ok(())
    }

    pub fn reopen_plan(&mut self, actor: u32, id: u32, now: u64) -> Result<(), ApiError> {
        let i = self.managed(actor, id)?;
        let plan = &mut self.plans[i];
        if plan.status != ON {
            return Err(ApiError::new(
                409,
                "not_locked",
                "Only a locked-in plan can be reopened",
            ));
        }
        plan.status = OPEN.into();
        plan.chosen_time = None;
        plan.chosen_games.clear();
        plan.updated = now;
        plan.edits += 1;
        plan.reminded = false;
        self.on_plan_event(id, notify::REOPENED, actor, now);
        Ok(())
    }

    pub fn cancel_plan(&mut self, actor: u32, id: u32, now: u64) -> Result<(), ApiError> {
        let i = self.managed(actor, id)?;
        if !active(&self.plans[i]) {
            return Err(closed());
        }
        self.on_plan_event(id, notify::CANCELLED, actor, now);
        self.record("cancelled", id, actor, "", now);
        let i = self.plan_index(id)?;
        self.plans[i].status = CANCELLED.into();
        self.finish(i, now);
        Ok(())
    }

    pub fn hand_over(&mut self, actor: u32, id: u32, to: u32, now: u64) -> Result<(), ApiError> {
        let i = self.managed(actor, id)?;
        match self.account(to) {
            Some(a) if !a.disabled => {}
            _ => {
                return Err(ApiError::bad_request(
                    "Hand it to someone with an active account",
                ))
            }
        }
        if !active(&self.plans[i]) {
            return Err(closed());
        }
        self.plans[i].host = to;
        self.plans[i].updated = now;
        self.notify(to, notify::HOST, id, actor, now);
        Ok(())
    }

    /// A plan just became done or cancelled: keep who was in and the
    /// choice, drop the rest, and keep only the newest past plans.
    fn finish(&mut self, i: usize, now: u64) {
        let plan = &mut self.plans[i];
        plan.updated = now;
        plan.finished = now;
        plan.edits += 1;
        let chosen = plan.chosen_time;
        plan.times.retain(|t| Some(t.id) == chosen);
        plan.responses.retain(|r| r.overall == "in");
        for r in &mut plan.responses {
            r.times.clear();
            r.games.clear();
            r.note.clear();
        }
        let past: Vec<usize> = (0..self.plans.len())
            .filter(|&j| !active(&self.plans[j]))
            .collect();
        if past.len() > self.limits.past_plans {
            let oldest = *past.iter().min_by_key(|&&j| self.plans[j].updated).unwrap();
            let gone = self.plans.remove(oldest);
            if gone.comments > 0 {
                self.dropped_threads.push(gone.id);
            }
        }
    }

    /// Locked-in plans whose time has passed become done. Returns whether
    /// anything changed (the caller commits).
    pub fn advance_plans(&mut self, now: u64) -> bool {
        let mut changed = false;
        let mut i = 0;
        while i < self.plans.len() {
            let plan = &self.plans[i];
            let due = plan.status == ON
                && plan
                    .times
                    .iter()
                    .find(|t| Some(t.id) == plan.chosen_time)
                    .is_some_and(|t| now >= t.end.max(t.start) + DONE_AFTER);
            if due {
                let id = plan.id;
                self.plans[i].status = DONE.into();
                self.finish(i, now);
                changed = true;
                // `finish` may have removed an older plan; find ours again.
                i = self
                    .plans
                    .iter()
                    .position(|p| p.id == id)
                    .map_or(0, |j| j + 1);
                continue;
            }
            i += 1;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{GameInput, TimeInput};
    use crate::state::Limits;

    const SAT: u64 = 1_793_000_000;

    struct World {
        s: State,
        dad: u32,
        kid: u32,
        mom: u32,
        catan: u32,
        wingspan: u32,
    }

    fn world() -> World {
        let mut s = State::new(Limits {
            pin_rounds: 2,
            active_plans: 3,
            past_plans: 2,
            ..Limits::board()
        });
        let dad = s.setup("dad@h", "Dad", "1234", 0).unwrap();
        let kid = s.signup("kid@h", "Sam", "1234", 0).unwrap();
        let mom = s.signup("mom@h", "Mom", "1234", 0).unwrap();
        let game = |t: &str| GameInput {
            title: Some(t.into()),
            ..Default::default()
        };
        let catan = s.add_game(dad, &game("Catan"), 0).unwrap();
        let wingspan = s.add_game(dad, &game("Wingspan"), 0).unwrap();
        World {
            s,
            dad,
            kid,
            mom,
            catan,
            wingspan,
        }
    }

    fn input(games: Vec<u32>, times: &[(u64, u64)]) -> PlanInput {
        PlanInput {
            games: Some(games),
            times: Some(
                times
                    .iter()
                    .map(|&(start, end)| TimeInput { id: 0, start, end })
                    .collect(),
            ),
            ..Default::default()
        }
    }

    fn answer(overall: &str, times: &[(u32, &str)], games: Vec<u32>) -> ResponseInput {
        ResponseInput {
            overall: overall.into(),
            times: times
                .iter()
                .map(|&(time, a)| TimeAnswer {
                    time,
                    answer: a.into(),
                })
                .collect(),
            games,
            note: String::new(),
        }
    }

    #[test]
    fn negotiate_then_lock() {
        let mut w = world();
        let id =
            w.s.create_plan(
                w.kid,
                &input(
                    vec![w.catan, w.wingspan],
                    &[(SAT + 3600, 0), (SAT, SAT + 4 * 3600)],
                ),
                1,
            )
            .unwrap();
        let plan = w.s.plan(id).unwrap();
        assert_eq!(plan.status, OPEN);
        assert_eq!(plan.place, "Home");
        assert_eq!(plan.times[0].start, SAT, "times sorted");
        let (t1, t2) = (plan.times[0].id, plan.times[1].id);
        w.s.respond(
            w.dad,
            id,
            &answer("in", &[(t1, "yes"), (t2, "no")], vec![w.catan]),
            2,
        )
        .unwrap();
        w.s.respond(
            w.mom,
            id,
            &answer("maybe", &[(t1, "if_needed")], vec![w.catan, w.wingspan]),
            2,
        )
        .unwrap();
        // Answering again replaces, never duplicates.
        w.s.respond(
            w.dad,
            id,
            &answer("in", &[(t1, "yes")], vec![w.wingspan]),
            3,
        )
        .unwrap();
        assert_eq!(w.s.plan(id).unwrap().responses.len(), 2);

        assert_eq!(
            w.s.lock_plan(
                w.mom,
                id,
                &LockIn {
                    time: t1,
                    games: vec![]
                },
                4
            )
            .unwrap_err()
            .code,
            "not_yours"
        );
        w.s.lock_plan(
            w.kid,
            id,
            &LockIn {
                time: t1,
                games: vec![w.catan],
            },
            4,
        )
        .unwrap();
        let plan = w.s.plan(id).unwrap();
        assert_eq!((plan.status.as_str(), plan.chosen_time), (ON, Some(t1)));
        // Locked: responses still allowed (people back out), times not editable.
        w.s.respond(w.mom, id, &answer("out", &[], vec![]), 5)
            .unwrap();
        assert_eq!(
            w.s.edit_plan(w.kid, id, &input(vec![w.catan], &[]), 5)
                .unwrap_err()
                .code,
            "plan_locked"
        );
        w.s.reopen_plan(w.dad, id, 6).unwrap();
        assert_eq!(w.s.plan(id).unwrap().chosen_time, None);
    }

    #[test]
    fn reschedule_is_atomic_and_reminds_again_without_reading_old_notifications() {
        let mut w = world();
        let id =
            w.s.create_plan(w.kid, &input(vec![w.catan], &[(SAT, 0)]), 1)
                .unwrap();
        let t = w.s.plan(id).unwrap().times[0].id;
        w.s.respond(w.dad, id, &answer("in", &[(t, "yes")], vec![w.catan]), 2)
            .unwrap();
        w.s.lock_plan(
            w.kid,
            id,
            &LockIn {
                time: t,
                games: vec![w.catan],
            },
            3,
        )
        .unwrap();
        w.s.mail.clear();
        assert!(w.s.send_reminders(SAT - 900));
        assert_eq!(w.s.mail.len(), 2);
        let edit = PlanInput {
            reschedule: Some(TimeInput {
                id: t,
                start: SAT + 3600,
                end: 0,
            }),
            ..Default::default()
        };
        assert!(w.s.edit_plan(w.mom, id, &edit, SAT - 800).is_err());
        assert_eq!(w.s.plan(id).unwrap().times[0].start, SAT);
        w.s.edit_plan(w.kid, id, &edit, SAT - 800).unwrap();
        let p = w.s.plan(id).unwrap();
        assert_eq!(p.status, ON);
        assert_eq!(p.chosen_games, vec![w.catan]);
        assert_ne!(p.chosen_time, Some(t));
        assert!(p.responses[0].times.is_empty());
        assert!(!p.reminded);
        assert_eq!(w.s.mail.last().unwrap().kind, notify::CHANGED);
        w.s.mail.clear();
        assert!(!w.s.send_reminders(SAT + 2699));
        assert!(w.s.send_reminders(SAT + 2700));
        assert_eq!(
            w.s.mail.len(),
            2,
            "unread old reminders must not suppress delivery"
        );
        assert!(!w.s.send_reminders(SAT + 2800));
        let invalid = PlanInput {
            reschedule: Some(TimeInput {
                id: 0,
                start: 1,
                end: 0,
            }),
            ..Default::default()
        };
        assert!(w.s.edit_plan(w.kid, id, &invalid, SAT).is_err());
        assert_eq!(w.s.plan(id).unwrap().times[0].start, SAT + 3600);
    }

    #[test]
    fn edited_time_makes_old_answers_stale() {
        let mut w = world();
        let id =
            w.s.create_plan(
                w.kid,
                &input(vec![w.catan], &[(SAT, 0), (SAT + 86_400, 0)]),
                1,
            )
            .unwrap();
        let old = w.s.plan(id).unwrap().times.clone();
        w.s.respond(
            w.dad,
            id,
            &answer("in", &[(old[0].id, "yes"), (old[1].id, "yes")], vec![]),
            2,
        )
        .unwrap();
        // Keep the first time as is, move the second.
        let edit = PlanInput {
            times: Some(vec![
                TimeInput {
                    id: old[0].id,
                    start: SAT,
                    end: 0,
                },
                TimeInput {
                    id: old[1].id,
                    start: SAT + 90_000,
                    end: 0,
                },
            ]),
            ..Default::default()
        };
        w.s.edit_plan(w.kid, id, &edit, 3).unwrap();
        let plan = w.s.plan(id).unwrap();
        assert_eq!(plan.times[0].id, old[0].id, "unchanged time keeps its id");
        assert_ne!(plan.times[1].id, old[1].id, "moved time is a new option");
        assert_eq!(
            plan.responses[0].times.len(),
            1,
            "answer to the moved time dropped"
        );
        let stale = answer("in", &[(old[1].id, "yes")], vec![]);
        assert_eq!(
            w.s.respond(w.mom, id, &stale, 4).unwrap_err().code,
            "plan_changed"
        );
        let stale_game = answer("in", &[], vec![w.wingspan]);
        assert_eq!(
            w.s.respond(w.mom, id, &stale_game, 4).unwrap_err().code,
            "plan_changed"
        );
    }

    #[test]
    fn validation() {
        let mut w = world();
        assert_eq!(
            w.s.create_plan(w.kid, &input(vec![], &[]), 1)
                .unwrap_err()
                .status,
            400
        );
        assert_eq!(
            w.s.create_plan(w.kid, &input(vec![999], &[]), 1)
                .unwrap_err()
                .status,
            400
        );
        assert_eq!(
            w.s.create_plan(w.kid, &input(vec![w.catan], &[(SAT, SAT - 1)]), 1)
                .unwrap_err()
                .status,
            400
        );
        let seven: Vec<(u64, u64)> = (0..7).map(|i| (SAT + i * 3600, 0)).collect();
        assert_eq!(
            w.s.create_plan(w.kid, &input(vec![w.catan], &seven), 1)
                .unwrap_err()
                .status,
            400
        );
        // A shout: no times yet is fine.
        let id =
            w.s.create_plan(w.kid, &input(vec![w.catan], &[]), 1)
                .unwrap();
        assert_eq!(
            w.s.respond(w.dad, id, &answer("yes", &[], vec![]), 2)
                .unwrap_err()
                .status,
            400
        );
        assert_eq!(
            w.s.lock_plan(
                w.kid,
                id,
                &LockIn {
                    time: 1,
                    games: vec![]
                },
                2
            )
            .unwrap_err()
            .status,
            400,
            "nothing to lock in without times"
        );
    }

    #[test]
    fn active_bound_and_past_ring() {
        let mut w = world();
        let ids: Vec<u32> = (0..3)
            .map(|i| {
                w.s.create_plan(w.kid, &input(vec![w.catan], &[(SAT + i, 0)]), i)
                    .unwrap()
            })
            .collect();
        assert_eq!(
            w.s.create_plan(w.kid, &input(vec![w.catan], &[]), 9)
                .unwrap_err()
                .code,
            "full"
        );
        for (n, id) in ids.iter().enumerate() {
            w.s.cancel_plan(w.dad, *id, 10 + n as u64).unwrap();
        }
        let past: Vec<u32> = w.s.plans.iter().map(|p| p.id).collect();
        assert_eq!(past, ids[1..], "oldest past plan dropped (ring of 2)");
        assert_eq!(
            w.s.respond(w.dad, ids[2], &answer("in", &[], vec![]), 20)
                .unwrap_err()
                .code,
            "plan_closed"
        );
    }

    #[test]
    fn locked_plans_become_done_and_compact() {
        let mut w = world();
        let id =
            w.s.create_plan(
                w.kid,
                &input(vec![w.catan], &[(SAT, SAT + 7200), (SAT + 86_400, 0)]),
                1,
            )
            .unwrap();
        let t = w.s.plan(id).unwrap().times[0].id;
        w.s.respond(w.dad, id, &answer("in", &[(t, "yes")], vec![w.catan]), 2)
            .unwrap();
        w.s.respond(w.mom, id, &answer("out", &[], vec![]), 2)
            .unwrap();
        w.s.lock_plan(
            w.kid,
            id,
            &LockIn {
                time: t,
                games: vec![w.catan],
            },
            3,
        )
        .unwrap();
        assert!(!w.s.advance_plans(SAT + 7200 + DONE_AFTER - 1));
        assert!(w.s.advance_plans(SAT + 7200 + DONE_AFTER));
        let plan = w.s.plan(id).unwrap();
        assert_eq!(plan.status, DONE);
        assert_eq!(plan.times.len(), 1, "only the chosen time kept");
        assert_eq!(plan.responses.len(), 1, "only who was in");
        assert!(plan.responses[0].games.is_empty());
    }

    #[test]
    fn hand_over_and_merge_follow_plans() {
        let mut w = world();
        let id =
            w.s.create_plan(w.kid, &input(vec![w.catan, w.wingspan], &[(SAT, 0)]), 1)
                .unwrap();
        w.s.respond(w.dad, id, &answer("in", &[], vec![w.wingspan]), 2)
            .unwrap();
        w.s.hand_over(w.kid, id, w.mom, 3).unwrap();
        assert_eq!(
            w.s.edit_plan(w.kid, id, &PlanInput::default(), 4)
                .unwrap_err()
                .code,
            "not_yours"
        );
        w.s.merge_games(w.wingspan, w.catan).unwrap();
        let plan = w.s.plan(id).unwrap();
        assert_eq!(plan.games, vec![w.catan]);
        assert_eq!(plan.responses[0].games, vec![w.catan]);
        assert!(
            !w.s.delete_game(w.catan).unwrap(),
            "used by a plan: archived"
        );
        assert!(w.s.game(w.catan).unwrap().archived);
    }
}
