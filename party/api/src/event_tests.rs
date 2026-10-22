//! Rule tests for events, answers, series and notifications (Sprints 3-5).
use crate::groups::{CO_ORGANIZER, ORGANIZER};
use crate::messages::{
    EventInput, GroupInput, RsvpInput, SeriesInput, StoredOption, StoredShift, StoredSlot,
};
use crate::rsvps::{GOING, MAYBE, NO, WAITLIST};
use crate::state::{Limits, State};
use crate::tz::days_from_civil;

/// Wed Oct 21 2026, noon UTC: "now" in these tests.
const NOW: u64 = 1_792_584_000;
const DAY: u64 = 86_400;

struct W {
    s: State,
    org: u32,
    co: u32,
    a: u32,
    b: u32,
    c: u32,
    d: u32,
    e: u32,
    outsider: u32,
    group: u32,
}

fn world() -> W {
    let mut s = State::new(Limits {
        pin_rounds: 2,
        ..Limits::board()
    });
    let org = s.setup("Org", "", "1234", 0).unwrap();
    let [co, a, b, c, d, e, outsider] =
        ["Co", "A", "B", "C", "D", "E", "Outsider"].map(|n| s.signup(n, "", "1234", 0).unwrap());
    let group = s
        .create_group(
            org,
            &GroupInput {
                name: Some("Family".into()),
                ..Default::default()
            },
            0,
        )
        .unwrap();
    s.set_member(org, group, co, Some(CO_ORGANIZER), None, 0)
        .unwrap();
    for p in [a, b, c, d, e] {
        s.set_member(org, group, p, None, None, 0).unwrap();
    }
    W {
        s,
        org,
        co,
        a,
        b,
        c,
        d,
        e,
        outsider,
        group,
    }
}

fn event(w: &mut W, input: EventInput) -> u32 {
    let input = EventInput {
        title: input.title.or(Some("Sunday dinner".into())),
        start: input.start.or(Some(NOW + 3 * DAY)),
        ..input
    };
    w.s.create_event(w.org, w.group, &input, NOW).unwrap()
}

fn rsvp(status: &str) -> RsvpInput {
    RsvpInput {
        status: status.into(),
        rev_seen: 1,
        ..Default::default()
    }
}

fn status(w: &W, id: u32, p: u32) -> String {
    w.s.rsvp(id, p)
        .map(|r| r.status.clone())
        .unwrap_or_default()
}

#[test]
fn kinds_preset_modules_and_defaults() {
    let mut w = world();
    let id = event(
        &mut w,
        EventInput {
            kind: Some("potluck".into()),
            ..Default::default()
        },
    );
    let e = w.s.event(id).unwrap();
    assert_eq!(e.modules, ["headcount", "bring", "diet"]);
    assert_eq!((e.plus_ones.as_str(), e.plus_max), ("limited", 2));
    let dinner = event(
        &mut w,
        EventInput {
            kind: Some("dinner".into()),
            ..Default::default()
        },
    );
    let e = w.s.event(dinner).unwrap();
    assert_eq!(
        e.rsvp_by,
        e.start - 2 * DAY,
        "dinners close answers two days ahead"
    );
    // A poll needs times; a time-less event needs a poll.
    let poll = EventInput {
        title: Some("Thanksgiving".into()),
        kind: Some("poll".into()),
        ..Default::default()
    };
    assert_eq!(
        w.s.create_event(w.org, w.group, &poll, NOW)
            .unwrap_err()
            .status,
        400
    );
    let poll = EventInput {
        when: Some(vec![
            StoredOption {
                id: 0,
                start: NOW + 30 * DAY,
                end: 0,
            },
            StoredOption {
                id: 0,
                start: NOW + 31 * DAY,
                end: 0,
            },
        ]),
        ..poll
    };
    let id = w.s.create_event(w.org, w.group, &poll, NOW).unwrap();
    assert!(w.s.event(id).unwrap().polling());
    let ids: Vec<u32> = w.s.event(id).unwrap().when.iter().map(|o| o.id).collect();
    assert_eq!(ids, [1, 2], "new items get ids");
    // Public events need a public group; members need permission to create.
    let public = EventInput {
        title: Some("Open house".into()),
        start: Some(NOW + DAY),
        visibility: Some("public".into()),
        ..Default::default()
    };
    assert_eq!(
        w.s.create_event(w.org, w.group, &public, NOW)
            .unwrap_err()
            .status,
        400
    );
    let plain = EventInput {
        visibility: None,
        ..public
    };
    assert_eq!(
        w.s.create_event(w.a, w.group, &plain, NOW)
            .unwrap_err()
            .code,
        "not_allowed"
    );
    w.s.update_group(
        w.org,
        w.group,
        &GroupInput {
            members_create_events: Some(true),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    w.s.create_event(w.a, w.group, &plain, NOW).unwrap();
}

#[test]
fn invite_only_events_and_drafts() {
    let mut w = world();
    let invitees = vec![w.a, w.outsider];
    let secret = event(
        &mut w,
        EventInput {
            visibility: Some("invite-only".into()),
            invitees: Some(invitees),
            ..Default::default()
        },
    );
    assert!(w.s.event_access(Some(w.b), w.s.event(secret).unwrap()) == Default::default());
    let outsider =
        w.s.event_access(Some(w.outsider), w.s.event(secret).unwrap());
    assert!(
        outsider.see && outsider.answer,
        "invitees answer without joining"
    );
    assert!(w.s.notices(w.outsider).iter().any(|n| n.kind == "invited"));
    let draft = event(
        &mut w,
        EventInput {
            status: Some("draft".into()),
            ..Default::default()
        },
    );
    assert!(!w.s.event_access(Some(w.a), w.s.event(draft).unwrap()).see);
    assert!(w.s.event_access(Some(w.co), w.s.event(draft).unwrap()).edit);
    let (mine, _) = w.s.list_events(Some(w.b), None, false, false, NOW, 0, 50);
    assert!(mine.iter().all(|e| e.id != secret && e.id != draft));
    // Publishing a draft tells its invitees then.
    w.s.update_event(
        w.org,
        draft,
        &EventInput {
            status: Some("scheduled".into()),
            invitees: Some(vec![w.e]),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    assert!(w
        .s
        .notices(w.e)
        .iter()
        .any(|n| n.kind == "invited" && n.event == draft));
}

#[test]
fn the_waitlist_promotes_in_order_and_only_what_fits() {
    let mut w = world();
    let id = event(
        &mut w,
        EventInput {
            capacity: Some(4),
            plus_ones: Some("unlimited".into()),
            ..Default::default()
        },
    );
    let with = |plus| RsvpInput {
        plus: Some(plus),
        ..rsvp(GOING)
    };
    w.s.answer(w.a, id, w.a, &with(1), NOW).unwrap();
    w.s.answer(w.b, id, w.b, &with(1), NOW).unwrap();
    for (p, plus) in [(w.c, 0), (w.d, 1), (w.e, 0)] {
        w.s.answer(p, id, p, &with(plus), NOW).unwrap();
        assert_eq!(status(&w, id, p), WAITLIST);
    }
    assert_eq!(w.s.heads(id), 4);
    // A drops out: C (1 head) fits, D (2) doesn't, E (1) fits after C.
    w.s.answer(w.a, id, w.a, &rsvp(NO), NOW).unwrap();
    assert_eq!(
        [
            status(&w, id, w.c),
            status(&w, id, w.d),
            status(&w, id, w.e)
        ],
        [GOING, WAITLIST, GOING]
    );
    assert_eq!(w.s.heads(id), 4);
    assert!(w.s.notices(w.c).iter().any(|n| n.kind == "promoted"));
    assert!(!w.s.notices(w.d).iter().any(|n| n.kind == "promoted"));
    // More room lets D in.
    w.s.update_event(
        w.org,
        id,
        &EventInput {
            capacity: Some(6),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    assert_eq!(status(&w, id, w.d), GOING);
    // Someone joining later waits behind nobody now, and fits.
    w.s.update_event(
        w.org,
        id,
        &EventInput {
            capacity: Some(7),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    w.s.answer(w.co, id, w.co, &rsvp(GOING), NOW).unwrap();
    assert_eq!(status(&w, id, w.co), GOING);
}

#[test]
fn plus_one_rules_and_policy_changes() {
    let mut w = world();
    let id = event(
        &mut w,
        EventInput {
            plus_ones: Some("limited".into()),
            plus_max: Some(2),
            ..Default::default()
        },
    );
    let with = |plus| RsvpInput {
        plus: Some(plus),
        ..rsvp(GOING)
    };
    w.s.answer(w.a, id, w.a, &with(2), NOW).unwrap();
    assert_eq!(
        w.s.answer(w.b, id, w.b, &with(3), NOW).unwrap_err().code,
        "too_many_guests"
    );
    w.s.update_event(
        w.org,
        id,
        &EventInput {
            plus_ones: Some("none".into()),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    assert_eq!(
        w.s.rsvp(id, w.a).unwrap().plus,
        2,
        "a policy change doesn't rewrite answers"
    );
    let e = w.s.event(id).unwrap().clone();
    assert!(
        w.s.rsvp_view(Some(w.org), &e, w.s.rsvp(id, w.a).unwrap(), true)
            .over_policy
    );
    let mut keep = with(2);
    keep.note = Some("running late".into());
    w.s.answer(w.a, id, w.a, &keep, NOW).unwrap();
    assert_eq!(
        w.s.answer(w.a, id, w.a, &with(3), NOW).unwrap_err().code,
        "too_many_guests"
    );
    assert_eq!(
        w.s.answer(w.b, id, w.b, &with(1), NOW).unwrap_err().code,
        "too_many_guests"
    );
    let kids = RsvpInput {
        plus: Some(0),
        kids: Some(1),
        ..rsvp(GOING)
    };
    assert_eq!(
        w.s.answer(w.b, id, w.b, &kids, NOW).unwrap_err().status,
        400
    );
}

#[test]
fn the_same_answer_twice_is_no_change() {
    let mut w = world();
    let id = event(&mut w, EventInput::default());
    assert!(w.s.answer(w.a, id, w.a, &rsvp(MAYBE), NOW).unwrap());
    let first = w.s.rsvp(id, w.a).unwrap().updated;
    w.s.saved();
    assert!(!w.s.answer(w.a, id, w.a, &rsvp(MAYBE), NOW + 60).unwrap());
    assert_eq!(w.s.rsvp(id, w.a).unwrap().updated, first);
    assert!(w.s.rsvps.dirty().is_empty(), "nothing to write");
    assert!(w.s.inboxes.dirty().is_empty(), "nobody told");
}

#[test]
fn bring_slots_and_shifts_fill_up_and_free() {
    let mut w = world();
    let id = event(
        &mut w,
        EventInput {
            kind: Some("workday".into()),
            bring: Some(vec![StoredSlot {
                id: 0,
                name: "Pie".into(),
                count: 1,
            }]),
            shifts: Some(vec![StoredShift {
                id: 0,
                name: "Setup".into(),
                start: NOW + 3 * DAY,
                end: NOW + 3 * DAY + 3600,
                count: 1,
            }]),
            ..Default::default()
        },
    );
    let e = w.s.event(id).unwrap().clone();
    let (pie, setup) = (e.bring[0].id, e.shifts[0].id);
    let claim = RsvpInput {
        bring: Some(vec![pie]),
        shifts: Some(vec![setup]),
        ..rsvp(GOING)
    };
    w.s.answer(w.a, id, w.a, &claim, NOW).unwrap();
    assert_eq!(
        w.s.answer(w.b, id, w.b, &claim, NOW).unwrap_err().code,
        "slot_full"
    );
    w.s.answer(w.a, id, w.a, &rsvp(NO), NOW).unwrap();
    assert!(
        w.s.rsvp(id, w.a).unwrap().bring.is_empty(),
        "not coming gives claims back"
    );
    w.s.answer(w.b, id, w.b, &claim, NOW).unwrap();
    let view = w.s.event_view(Some(w.org), id, NOW).unwrap();
    assert_eq!(view.claimed[0].names, ["B"]);
    assert_eq!(view.taken[0].count, 1);
}

#[test]
fn stale_forms_are_refused_only_where_it_matters() {
    let mut w = world();
    let id = event(
        &mut w,
        EventInput {
            kind: Some("potluck".into()),
            bring: Some(vec![StoredSlot {
                id: 0,
                name: "Salad".into(),
                count: 2,
            }]),
            ..Default::default()
        },
    );
    let salad = w.s.event(id).unwrap().bring[0].id;
    w.s.update_event(
        w.org,
        id,
        &EventInput {
            bring: Some(vec![
                StoredSlot {
                    id: salad,
                    name: "Salad".into(),
                    count: 2,
                },
                StoredSlot {
                    id: 0,
                    name: "Dessert".into(),
                    count: 1,
                },
            ]),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    assert_eq!(w.s.event(id).unwrap().rev, 2);
    assert!(w.s.notices(w.a).is_empty(), "nobody had answered yet");
    let old = RsvpInput {
        bring: Some(vec![salad]),
        ..rsvp(GOING)
    };
    assert_eq!(
        w.s.answer(w.a, id, w.a, &old, NOW).unwrap_err().code,
        "stale"
    );
    w.s.answer(w.a, id, w.a, &rsvp(GOING), NOW).unwrap();
    let fresh = RsvpInput { rev_seen: 2, ..old };
    w.s.answer(w.a, id, w.a, &fresh, NOW).unwrap();
    // A later time change tells the people coming.
    w.s.update_event(
        w.org,
        id,
        &EventInput {
            start: Some(NOW + 4 * DAY),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    assert!(w.s.notices(w.a).iter().any(|n| n.kind == "changed"));
}

#[test]
fn answering_for_household_and_as_host() {
    let mut w = world();
    let home = w.s.create_household(w.org, "Grandma's", 0).unwrap();
    for p in [w.a, w.b] {
        w.s.set_household(w.org, p, home, 0).unwrap();
    }
    let id = event(
        &mut w,
        EventInput {
            rsvp_by: Some(NOW + DAY),
            ..Default::default()
        },
    );
    w.s.answer(w.a, id, w.b, &rsvp(GOING), NOW).unwrap();
    let r = w.s.rsvp(id, w.b).unwrap();
    assert_eq!((r.by, r.via.as_str()), (w.a, "household"));
    assert!(w.s.notices(w.b).iter().any(|n| n.kind == "answered_for"));
    assert_eq!(
        w.s.answer(w.c, id, w.b, &rsvp(NO), NOW).unwrap_err().code,
        "not_allowed"
    );
    assert_eq!(
        w.s.answer(w.outsider, id, w.outsider, &rsvp(GOING), NOW)
            .unwrap_err()
            .code,
        "not_allowed"
    );
    // Past the deadline only hosts can still change answers.
    let late = NOW + 2 * DAY;
    assert_eq!(
        w.s.answer(w.c, id, w.c, &rsvp(GOING), late)
            .unwrap_err()
            .code,
        "locked"
    );
    w.s.answer(w.co, id, w.c, &rsvp(GOING), late).unwrap();
    assert_eq!(w.s.rsvp(id, w.c).unwrap().via, "host");
}

#[test]
fn a_poll_locks_in_and_carries_answers_over() {
    let mut w = world();
    let id = event(
        &mut w,
        EventInput {
            kind: Some("poll".into()),
            start: Some(0),
            when: Some(vec![
                StoredOption {
                    id: 0,
                    start: NOW + 30 * DAY,
                    end: NOW + 30 * DAY + 7200,
                },
                StoredOption {
                    id: 0,
                    start: NOW + 31 * DAY,
                    end: 0,
                },
            ]),
            ..Default::default()
        },
    );
    let (one, two) = (1, 2);
    let vote = |s: &str, when: Vec<u32>| RsvpInput {
        when: Some(when),
        ..rsvp(s)
    };
    w.s.answer(w.a, id, w.a, &vote(MAYBE, vec![one]), NOW)
        .unwrap();
    w.s.answer(w.b, id, w.b, &vote(GOING, vec![two]), NOW)
        .unwrap();
    w.s.answer(w.c, id, w.c, &vote(GOING, vec![one, two]), NOW)
        .unwrap();
    w.s.answer(w.d, id, w.d, &vote(NO, vec![one]), NOW).unwrap();
    let view = w.s.event_view(Some(w.org), id, NOW).unwrap();
    assert_eq!(
        view.votes.iter().map(|v| v.count).collect::<Vec<_>>(),
        [2, 2],
        "no doesn't count"
    );
    w.s.lock_in(w.org, id, one, NOW).unwrap();
    let e = w.s.event(id).unwrap();
    assert!(!e.polling());
    assert_eq!((e.start, e.end), (NOW + 30 * DAY, NOW + 30 * DAY + 7200));
    assert_eq!(
        status(&w, id, w.a),
        GOING,
        "maybe + picked the winner = going"
    );
    assert_eq!(status(&w, id, w.c), GOING);
    assert_eq!(
        status(&w, id, w.b),
        GOING,
        "kept their answer, and was told"
    );
    assert_eq!(status(&w, id, w.d), NO);
    assert!(w.s.notices(w.b).iter().any(|n| n.kind == "changed"));
    assert_eq!(
        w.s.lock_in(w.org, id, two, NOW).unwrap_err().code,
        "not_polling"
    );
}

#[test]
fn rides_reminders_and_done() {
    let mut w = world();
    let id = event(
        &mut w,
        EventInput {
            kind: Some("outing".into()),
            reminder_secs: Some(3600),
            ..Default::default()
        },
    );
    let ride = |r: &str, seats| RsvpInput {
        ride: Some(r.into()),
        seats: Some(seats),
        ..rsvp(GOING)
    };
    w.s.answer(w.a, id, w.a, &ride("offer", 3), NOW).unwrap();
    w.s.answer(w.b, id, w.b, &ride("need", 1), NOW).unwrap();
    w.s.answer(w.c, id, w.c, &ride("need", 2), NOW).unwrap();
    let board = w.s.ride_board(w.org, id).unwrap();
    assert_eq!(
        (board.needs.len(), board.offers.len(), board.spare),
        (2, 1, 0)
    );
    assert_eq!(w.s.ride_board(w.a, id).unwrap_err().code, "not_allowed");
    let start = w.s.event(id).unwrap().start;
    assert!(!w.s.send_reminders(start - 2 * 3600));
    assert!(w.s.send_reminders(start - 1800));
    assert!(!w.s.send_reminders(start - 1700), "once");
    assert_eq!(
        w.s.notices(w.b)
            .iter()
            .filter(|n| n.kind == "reminder")
            .count(),
        1
    );
    assert!(w.s.advance_events(start + 4 * 3600));
    assert_eq!(w.s.event(id).unwrap().status, "done");
    assert_eq!(w.s.summary(w.e, start + 4 * 3600).awaiting, 0);
}

#[test]
fn a_series_rolls_forward_with_regulars_and_following_edits() {
    let mut w = world();
    // Fridays 19:00 local, from Oct 23 2026 (EDT) into November (EST).
    let first = w.s.zone.to_utc(days_from_civil(2026, 10, 23), 19 * 3600);
    let input = SeriesInput {
        event: Some(EventInput {
            title: Some("Game club".into()),
            kind: Some("meeting".into()),
            start: Some(first),
            end: Some(first + 3 * 3600),
            ..Default::default()
        }),
        rule: Some("weekly".into()),
        window: Some(3),
        ..Default::default()
    };
    let sid = w.s.create_series(w.org, w.group, &input, NOW).unwrap();
    let occurrences = |s: &State| {
        let mut v: Vec<(u32, u64)> = s
            .events
            .iter()
            .filter(|e| e.series == sid)
            .map(|e| (e.occurrence, e.start))
            .collect();
        v.sort();
        v
    };
    assert_eq!(occurrences(&w.s).len(), 3);
    for (_, start) in occurrences(&w.s) {
        let l = w.s.zone.local(start);
        assert_eq!(
            (l.weekday, l.hour),
            (5, 19),
            "Friday 19:00 local, before and after DST"
        );
    }
    w.s.set_regular(w.a, sid, true, NOW).unwrap();
    assert!(occurrences(&w.s).iter().all(|(n, _)| {
        let e =
            w.s.events
                .iter()
                .find(|e| e.series == sid && e.occurrence == *n)
                .unwrap();
        w.s.rsvp(e.id, w.a).is_some_and(|r| r.status == GOING)
    }));
    // Once the first is over the window moves on, and the regular is already going.
    let later = first + 4 * 3600;
    assert!(w.s.materialize_all(later));
    let all = occurrences(&w.s);
    assert_eq!(all.len(), 4);
    let fourth =
        w.s.events
            .iter()
            .find(|e| e.series == sid && e.occurrence == 3)
            .unwrap()
            .id;
    assert_eq!(status(&w, fourth, w.a), GOING);
    assert!(
        !w.s.materialize_all(later),
        "nothing more until the window has room"
    );
    // "This and following" from the second: a new title, later ones too.
    let second =
        w.s.events
            .iter()
            .find(|e| e.series == sid && e.occurrence == 1)
            .unwrap()
            .id;
    let rename = EventInput {
        title: Some("Board game club".into()),
        ..Default::default()
    };
    w.s.update_following(w.org, second, &rename, NOW).unwrap();
    let title = |s: &State, n: u32| {
        s.events
            .iter()
            .find(|e| e.series == sid && e.occurrence == n)
            .unwrap()
            .title
            .clone()
    };
    assert_eq!(title(&w.s, 0), "Game club");
    assert_eq!(title(&w.s, 1), "Board game club");
    assert_eq!(title(&w.s, 3), "Board game club");
    w.s.materialize_all(first + 14 * DAY + 4 * 3600);
    assert_eq!(
        title(&w.s, 4),
        "Board game club",
        "new occurrences copy the template"
    );
    let view = w.s.series_view(w.b, sid, NOW).unwrap();
    assert_eq!(view.summary, "Every Friday at 19:00");
    w.s.end_series(w.org, sid, true, NOW).unwrap();
    assert!(w
        .s
        .events
        .iter()
        .filter(|e| e.series == sid && e.start > NOW)
        .all(|e| e.status == "cancelled"));
    let _ = (w.co, ORGANIZER);
}

#[test]
fn rsvps_live_in_blocks_and_cost_one_block_per_answer() {
    let mut w = world();
    let id = event(&mut w, EventInput::default());
    let mut people = Vec::new();
    for i in 0..40 {
        let p = w.s.signup(&format!("Guest {i}"), "", "1234", 0).unwrap();
        w.s.set_member(w.org, w.group, p, None, None, 0).unwrap();
        people.push(p);
    }
    for &p in &people {
        w.s.answer(p, id, p, &rsvp(GOING), NOW).unwrap();
    }
    assert_eq!(w.s.rsvps.len(), 3, "40 answers in blocks of 16");
    w.s.saved();
    w.s.answer(people[20], id, people[20], &rsvp(MAYBE), NOW)
        .unwrap();
    assert_eq!(w.s.rsvps.dirty().len(), 1, "one block rewritten");
    w.s.delete_event(w.org, id, NOW).unwrap();
    assert!(w.s.rsvps.is_empty());
}

/// The board holds records written by the Sprint 1-2 firmware, before
/// fields were appended to meta, people and groups. They must still load,
/// with the new fields at their defaults.
#[test]
fn records_from_the_sprint_2_firmware_still_load() {
    use crate::messages::{StoredGroup, StoredMeta, StoredPerson};
    use miniframework::message;
    use miniframework::wire::{self, Format};
    message! {
        struct OldMeta {
            1 schema: u32,
            2 next_person: u32,
            3 next_household: u32,
            4 next_group: u32,
            5 signup_closed: bool,
            6 groups_admin_only: bool,
            7 next_audit: u64,
        }
    }
    message! {
        struct OldPerson {
            1 id: u32,
            2 display_name: String,
            11 last_seen: u64,
        }
    }
    message! {
        struct OldGroup {
            1 id: u32,
            3 name: String,
            13 created_by: u32,
        }
    }
    let meta = wire::to_vec(
        Format::CborInt,
        &OldMeta {
            schema: 1,
            next_person: 5,
            next_group: 2,
            next_audit: 9,
            ..Default::default()
        },
    )
    .unwrap();
    let meta: StoredMeta = wire::decode(Format::CborInt, &meta).unwrap();
    assert_eq!(
        (meta.next_person, meta.next_audit, meta.next_event),
        (5, 9, 0)
    );
    assert_eq!(meta.zone, "");
    let p = wire::to_vec(
        Format::CborInt,
        &OldPerson {
            id: 3,
            display_name: "Dad".into(),
            last_seen: 7,
        },
    )
    .unwrap();
    let p: StoredPerson = wire::decode(Format::CborInt, &p).unwrap();
    assert_eq!(
        (p.id, p.display_name.as_str(), p.feed_hash.as_str()),
        (3, "Dad", "")
    );
    let g = wire::to_vec(
        Format::CborInt,
        &OldGroup {
            id: 1,
            name: "Family".into(),
            created_by: 3,
        },
    )
    .unwrap();
    let g: StoredGroup = wire::decode(Format::CborInt, &g).unwrap();
    assert!(!g.members_create_events);
    // And a state built from them works: ids continue, the zone defaults.
    let s = State::from_parts(meta, Default::default(), Limits::board());
    assert_eq!(s.next_event, 1);
    assert_eq!(s.zone.offset(0), -5 * 3600, "US Eastern in January 1970");
}
