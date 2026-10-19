//! What the API shows: domain state turned into the caller's view of it.
use crate::messages::*;
use crate::plans::{self, active};
use crate::state::{self, State};

pub fn game(state: &State, g: &StoredGame, caller: u32) -> GameView {
    GameView {
        id: g.id,
        title: g.title.clone(),
        kind: g.kind.clone(),
        min_players: g.min_players,
        max_players: g.max_players,
        minutes: g.minutes,
        notes: g.notes.clone(),
        added_by: g.added_by,
        added_by_name: state.name(g.added_by),
        archived: g.archived,
        interested: state.interested_in(g.id).count() as u32,
        mine: state
            .account(caller)
            .is_some_and(|a| a.interests.contains(&g.id)),
        editable: state.can_edit_game(caller, g),
    }
}

pub fn games(state: &State, caller: u32, archived: bool) -> GameList {
    let mut games: Vec<GameView> = state
        .games
        .iter()
        .filter(|g| archived || !g.archived)
        .map(|g| game(state, g, caller))
        .collect();
    games.sort_by_key(|g| g.title.to_lowercase());
    GameList { games }
}

pub fn game_detail(state: &State, g: &StoredGame, caller: u32) -> GameDetail {
    let mut plans: Vec<&StoredPlan> = state
        .plans
        .iter()
        .filter(|p| p.games.contains(&g.id))
        .collect();
    plans.sort_by_key(|p| std::cmp::Reverse(p.updated));
    GameDetail {
        game: game(state, g, caller),
        interested: state
            .interested_in(g.id)
            .map(|a| UserSummary {
                id: a.id,
                display_name: a.display_name.clone(),
                role: state::role(a).into(),
            })
            .collect(),
        plans: plans
            .into_iter()
            .take(20)
            .map(|p| summary(state, p, caller))
            .collect(),
    }
}

fn answered(p: &StoredPlan, time: u32, answer: &str) -> Vec<u32> {
    p.responses
        .iter()
        .filter(|r| r.times.iter().any(|a| a.time == time && a.answer == answer))
        .map(|r| r.account)
        .collect()
}

pub fn plan(state: &State, p: &StoredPlan, caller: u32) -> PlanView {
    PlanView {
        id: p.id,
        host: p.host,
        host_name: state.name(p.host),
        title: p.title.clone(),
        notes: p.notes.clone(),
        place: p.place.clone(),
        seats: p.seats,
        status: p.status.clone(),
        games: p
            .games
            .iter()
            .map(|&id| {
                let g = state.game(id);
                PlanGame {
                    id,
                    title: g.map_or_else(|| "(removed game)".into(), |g| g.title.clone()),
                    kind: g.map(|g| g.kind.clone()).unwrap_or_default(),
                    min_players: g.map_or(0, |g| g.min_players),
                    max_players: g.map_or(0, |g| g.max_players),
                    minutes: g.map_or(0, |g| g.minutes),
                    archived: g.is_none_or(|g| g.archived),
                    ticks: p
                        .responses
                        .iter()
                        .filter(|r| r.games.contains(&id))
                        .map(|r| r.account)
                        .collect(),
                }
            })
            .collect(),
        times: p
            .times
            .iter()
            .map(|t| PlanTime {
                id: t.id,
                start: t.start,
                end: t.end,
                yes: answered(p, t.id, "yes"),
                if_needed: answered(p, t.id, "if_needed"),
                no: answered(p, t.id, "no"),
            })
            .collect(),
        responses: p
            .responses
            .iter()
            .map(|r| ResponseView {
                account: r.account,
                name: state.name(r.account),
                overall: r.overall.clone(),
                times: r.times.clone(),
                games: r.games.clone(),
                note: r.note.clone(),
                updated: r.updated,
            })
            .collect(),
        chosen_time: p.chosen_time,
        chosen_games: p.chosen_games.clone(),
        created: p.created,
        updated: p.updated,
        manageable: active(p) && state.can_manage(caller, p),
        announced: p.announce,
        announced_public: p.announce_public,
        series: if p.series == 0 {
            vec![]
        } else {
            state
                .plans
                .iter()
                .filter(|other| other.series == p.series)
                .map(|p| p.id)
                .collect()
        },
        report: p.report.clone(),
        reportable: state.can_manage(caller, p) && matches!(p.status.as_str(), "on" | "done"),
    }
}

/// The time a list shows: the chosen one, else the earliest option.
fn headline_time(p: &StoredPlan) -> (u64, u64) {
    p.times
        .iter()
        .find(|t| Some(t.id) == p.chosen_time)
        .or_else(|| p.times.first())
        .map_or((0, 0), |t| (t.start, t.end))
}

pub fn summary(state: &State, p: &StoredPlan, caller: u32) -> PlanSummary {
    let (start, end) = headline_time(p);
    let count = |o: &str| p.responses.iter().filter(|r| r.overall == o).count() as u32;
    PlanSummary {
        id: p.id,
        title: p.title.clone(),
        status: p.status.clone(),
        host_name: state.name(p.host),
        games: p
            .games
            .iter()
            .filter_map(|&g| state.game(g).map(|g| g.title.clone()))
            .collect(),
        start,
        end,
        times: p.times.len() as u32,
        in_count: count("in"),
        maybe_count: count("maybe"),
        answered: p.responses.iter().any(|r| r.account == caller),
        seats: p.seats,
        updated: p.updated,
    }
}

/// `status` is a comma list (`open,on` by default). Active plans come
/// soonest first (shouts, with no time, last); past plans newest first.
pub fn plans(state: &State, caller: u32, status: &str) -> PlanList {
    let wanted: Vec<&str> = status.split(',').map(str::trim).collect();
    let mut chosen: Vec<&StoredPlan> = state
        .plans
        .iter()
        .filter(|p| wanted.contains(&p.status.as_str()))
        .collect();
    chosen.sort_by_key(|p| {
        let (start, _) = headline_time(p);
        if active(p) {
            (0, if start == 0 { u64::MAX } else { start })
        } else {
            (1, u64::MAX - p.updated)
        }
    });
    PlanList {
        plans: chosen
            .into_iter()
            .map(|p| summary(state, p, caller))
            .collect(),
    }
}

pub fn valid_statuses(status: &str) -> bool {
    status.split(',').all(|s| {
        matches!(
            s.trim(),
            plans::OPEN | plans::ON | plans::DONE | plans::CANCELLED
        )
    })
}

/// Comments per page (64 × 500 characters would pass the board's 32 KiB).
pub const COMMENT_PAGE: usize = 25;

/// Comments of `plan` after id `after` (0 = from the start), one page.
pub fn comments(
    state: &State,
    plan: u32,
    thread: &StoredThread,
    caller: u32,
    after: u32,
    now: u64,
) -> CommentList {
    let admin = state.account(caller).is_some_and(|a| a.admin);
    let rest: Vec<&StoredComment> = thread.comments.iter().filter(|c| c.id > after).collect();
    CommentList {
        more: rest.len() > COMMENT_PAGE,
        sent_dms: 0,
        comments: rest
            .into_iter()
            .take(COMMENT_PAGE)
            .map(|c| CommentView {
                id: c.id,
                author: c.author,
                author_name: state.name(c.author),
                text: c.text.clone(),
                created: c.created,
                edited: c.edited,
                mine: c.author == caller,
                deletable: c.author == caller || admin,
            })
            .collect(),
        open: state.comments_open(plan, now),
    }
}

pub fn notifications(state: &State, caller: u32) -> NotificationList {
    let list = state
        .account(caller)
        .map(|a| a.notifications.as_slice())
        .unwrap_or(&[]);
    NotificationList {
        notifications: list
            .iter()
            .map(|n| NotificationView {
                id: n.id,
                kind: n.kind.clone(),
                plan: n.plan,
                plan_title: state.plan(n.plan).map_or_else(
                    |_| "a past plan".into(),
                    |p| summary(state, p, caller).title_or_games(),
                ),
                count: n.count,
                actor_name: if n.actor == 0 {
                    "Game Matcher".into()
                } else {
                    state.name(n.actor)
                },
                created: n.created,
                read: n.read,
            })
            .collect(),
    }
}

impl PlanSummary {
    /// The plan's title, or its games when it has none.
    pub fn title_or_games(&self) -> String {
        if !self.title.is_empty() {
            self.title.clone()
        } else if self.games.len() == 1 {
            self.games[0].clone()
        } else {
            format!("Game night: {}", self.games.join(", "))
        }
    }
}

pub fn providers(state: &State) -> ProviderList {
    ProviderList {
        providers: state
            .providers
            .iter()
            .map(|p| ProviderView {
                id: p.id,
                name: p.name.clone(),
                base: p.base.clone(),
                trust: p.trust.clone(),
                anchor: p.anchor.clone(),
                invites: p.invites.clone(),
            })
            .collect(),
    }
}
