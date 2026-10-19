//! Which cards are in the screen's rotation right now (`spec/07-display.md`
//! "The rotation"), built from the state and the recent-events feed.
use crate::messages::{DisplayCard, DisplayPerson, StoredPlan};
use crate::plans::{ON, OPEN};
use crate::state::State;

/// A plan within this long of starting takes over the screen.
pub const SOON: u64 = 2 * 3600;
const UPCOMING_DAYS: u64 = 7;
const NEW_PLAN_HOURS: u64 = 48;
const EVENT_HOURS: u64 = 24;

/// Something that just happened, for the "Sam is in!" / "It's on!" cards.
/// RAM only (a reboot forgets the last day's chatter; nothing is lost).
#[derive(Clone, Debug)]
pub struct FeedEvent {
    /// `rsvp`, `locked` or `cancelled`.
    pub kind: &'static str,
    pub plan: u32,
    pub actor: u32,
    /// For `rsvp`: `in` or `maybe`.
    pub answer: String,
    pub at: u64,
}

pub const FEED: usize = 16;

impl State {
    pub fn record(&mut self, kind: &'static str, plan: u32, actor: u32, answer: &str, at: u64) {
        // A changed mind replaces the earlier event from the same person.
        self.feed
            .retain(|e| !(e.kind == kind && e.plan == plan && e.actor == actor));
        self.feed.push_front(FeedEvent {
            kind,
            plan,
            actor,
            answer: answer.to_string(),
            at,
        });
        self.feed.truncate(FEED);
    }
}

fn chosen_start(p: &StoredPlan) -> Option<(u64, u64)> {
    p.times
        .iter()
        .find(|t| Some(t.id) == p.chosen_time)
        .map(|t| (t.start, t.end))
}

/// The plan's headline game: the first chosen one, else the first candidate.
fn headline(state: &State, p: &StoredPlan) -> (u32, String, String, u32) {
    let id = p
        .chosen_games
        .first()
        .or(p.games.first())
        .copied()
        .unwrap_or(0);
    state
        .game(id)
        .map_or((id, String::new(), "board".into(), 0), |g| {
            (g.id, g.title.clone(), g.kind.clone(), g.min_players)
        })
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
        "Game night".into()
    }
}

fn base(state: &State, p: &StoredPlan, kind: &str) -> DisplayCard {
    let (game_id, _, game_kind, _) = headline(state, p);
    let shown: &[u32] = if p.chosen_games.is_empty() {
        &p.games
    } else {
        &p.chosen_games
    };
    let (start, end) = chosen_start(p)
        .or_else(|| p.times.first().map(|t| (t.start, t.end)))
        .unwrap_or((0, 0));
    let count = |o: &str| p.responses.iter().filter(|r| r.overall == o).count() as u32;
    // People: in first, then maybe (the screen shows who's coming). A host
    // who hasn't answered their own plan is coming.
    let mut people: Vec<DisplayPerson> = Vec::new();
    let host_unanswered = !p.responses.iter().any(|r| r.account == p.host);
    if host_unanswered {
        people.push(DisplayPerson {
            id: p.host,
            name: state.name(p.host),
            answer: "in".into(),
        });
    }
    for answer in ["in", "maybe"] {
        for r in p.responses.iter().filter(|r| r.overall == answer) {
            people.push(DisplayPerson {
                id: r.account,
                name: state.name(r.account),
                answer: answer.into(),
            });
        }
    }
    DisplayCard {
        kind: kind.into(),
        plan: p.id,
        title: title(state, p),
        games: shown
            .iter()
            .filter_map(|&g| state.game(g).map(|g| g.title.clone()))
            .collect(),
        game_kind,
        accent: game_id,
        start,
        end,
        times: p.times.iter().map(|t| t.start).collect(),
        people,
        place: p.place.clone(),
        note: p.notes.lines().next().unwrap_or("").to_string(),
        count_in: count("in") + u32::from(host_unanswered),
        count_maybe: count("maybe"),
        seats: p.seats,
        actor: state.name(p.host),
        actor_id: p.host,
        ..Default::default()
    }
}

/// The cards to rotate through at `now`.
pub fn build(state: &State, now: u64) -> Vec<DisplayCard> {
    // Starting soon takes over everything.
    let mut soon: Vec<(&StoredPlan, u64)> = state
        .plans
        .iter()
        .filter(|p| p.status == ON)
        .filter_map(|p| chosen_start(p).map(|(s, _)| (p, s)))
        .filter(|(_, s)| *s > now && *s - now <= SOON)
        .collect();
    if !soon.is_empty() {
        soon.sort_by_key(|(_, s)| *s);
        return soon
            .into_iter()
            .map(|(p, _)| base(state, p, "soon"))
            .collect();
    }

    let mut cards = Vec::new();
    // Upcoming: locked-in plans in the next week (and ones just started).
    let mut upcoming: Vec<(&StoredPlan, u64)> = state
        .plans
        .iter()
        .filter(|p| p.status == ON)
        .filter_map(|p| chosen_start(p).map(|(s, _)| (p, s)))
        .filter(|(_, s)| *s + 3 * 3600 > now && *s <= now + UPCOMING_DAYS * 86_400)
        .collect();
    upcoming.sort_by_key(|(_, s)| *s);
    cards.extend(
        upcoming
            .into_iter()
            .take(3)
            .map(|(p, _)| base(state, p, "upcoming")),
    );

    // New open plans: "needs players" when short of the headline game's
    // minimum, else "being decided".
    for p in state
        .plans
        .iter()
        .filter(|p| p.status == OPEN && now.saturating_sub(p.created) < NEW_PLAN_HOURS * 3600)
    {
        let (_, game, _, min) = headline(state, p);
        let mut card = base(state, p, "deciding");
        let ins = card.count_in;
        if min > ins {
            card.kind = "needs".into();
            card.needed = min - ins;
        }
        if p.title.is_empty() && !game.is_empty() {
            card.title = if p.games.len() > 1 {
                format!("{game} or {} more", p.games.len() - 1)
            } else {
                game
            };
        }
        cards.push(card);
    }

    // What just happened: answers, lock-ins, cancellations (newest first).
    let mut rsvps = 0;
    for e in state
        .feed
        .iter()
        .filter(|e| now.saturating_sub(e.at) < EVENT_HOURS * 3600)
    {
        let Ok(p) = state.plan(e.plan) else { continue };
        if e.kind == "rsvp" {
            if rsvps == 4 {
                continue;
            }
            rsvps += 1;
        }
        let mut card = base(state, p, e.kind);
        card.actor = state.name(e.actor);
        card.actor_id = e.actor;
        card.answer = e.answer.clone();
        card.at = e.at;
        if e.kind == "rsvp" {
            // Their games and note, not the plan's.
            if let Some(r) = p.responses.iter().find(|r| r.account == e.actor) {
                card.games = r
                    .games
                    .iter()
                    .filter_map(|&g| state.game(g).map(|g| g.title.clone()))
                    .collect();
                card.note = r.note.clone();
            }
            card.accent = e.actor;
        }
        cards.push(card);
    }

    if cards.is_empty() {
        cards.push(DisplayCard {
            kind: "idle".into(),
            ..Default::default()
        });
    }
    cards
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{GameInput, LockIn, PlanInput, ResponseInput, TimeInput};
    use crate::state::Limits;

    const NOW: u64 = 1_792_260_000;

    fn world() -> (State, u32, u32, u32) {
        let mut s = State::new(Limits {
            pin_rounds: 2,
            ..Limits::board()
        });
        let dad = s.setup("dad@h", "Dad", "1234", NOW).unwrap();
        let sam = s.signup("sam@h", "Sam", "1234", NOW).unwrap();
        let catan = s
            .add_game(
                dad,
                &GameInput {
                    title: Some("Catan".into()),
                    min_players: Some(3),
                    ..Default::default()
                },
                NOW,
            )
            .unwrap();
        (s, dad, sam, catan)
    }

    fn plan(s: &mut State, host: u32, game: u32, start: u64) -> u32 {
        let input = PlanInput {
            games: Some(vec![game]),
            times: Some(vec![TimeInput {
                id: 0,
                start,
                end: 0,
            }]),
            ..Default::default()
        };
        s.create_plan(host, &input, NOW).unwrap()
    }

    fn kinds(cards: &[DisplayCard]) -> Vec<&str> {
        cards.iter().map(|c| c.kind.as_str()).collect()
    }

    #[test]
    fn idle_when_nothing() {
        let (s, ..) = world();
        assert_eq!(kinds(&build(&s, NOW)), ["idle"]);
    }

    #[test]
    fn new_plan_needs_players_counting_the_host() {
        let (mut s, _dad, sam, catan) = world();
        let p = plan(&mut s, sam, catan, NOW + 86_400);
        let cards = build(&s, NOW);
        assert_eq!(kinds(&cards), ["needs"]);
        assert_eq!(
            (cards[0].count_in, cards[0].needed),
            (1, 2),
            "Sam hosts, so 1 in, 2 more for Catan's 3"
        );
        assert_eq!(cards[0].title, "Catan");
        assert!(
            build(&s, NOW + 49 * 3600)
                .iter()
                .all(|c| c.plan != p || c.kind != "needs"),
            "only for 48 h"
        );
    }

    #[test]
    fn answers_show_for_a_day_and_soon_takes_over() {
        let (mut s, dad, sam, catan) = world();
        let p = plan(&mut s, dad, catan, NOW + 3 * 3600);
        s.respond(
            sam,
            p,
            &ResponseInput {
                overall: "in".into(),
                games: vec![catan],
                note: "snacks".into(),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        let t = s.plan(p).unwrap().times[0].id;
        s.lock_plan(
            dad,
            p,
            &LockIn {
                time: t,
                games: vec![],
            },
            NOW,
        )
        .unwrap();
        let cards = build(&s, NOW + 60);
        assert_eq!(kinds(&cards), ["upcoming", "locked", "rsvp"]);
        let rsvp = &cards[2];
        assert_eq!((rsvp.actor.as_str(), rsvp.note.as_str()), ("Sam", "snacks"));
        assert_eq!(cards[0].people.len(), 2, "Dad (host) and Sam");
        // Two hours before: only "starting soon".
        assert_eq!(kinds(&build(&s, NOW + 3 * 3600 - 7000)), ["soon"]);
        // A day later the events are gone; the plan is still upcoming.
        let later = build(&s, NOW + 2 * 3600);
        assert!(later.iter().all(|c| c.kind != "rsvp") || later[0].kind == "soon");
    }
}
