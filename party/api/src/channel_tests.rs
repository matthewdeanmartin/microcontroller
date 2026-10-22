//! The outbound channels end to end against a fake HTTP client that plays
//! Brevo, a Mastodon instance and GitHub (Sprints 6-8). Nothing here talks
//! to the network.
use crate::api::App;
use crate::messages::*;
use crate::outbound::{BOT, MAIL, PUBLISH};
use crate::state::Limits;
use crate::work::Fetchers;
use miniframework::fetch::{Fetch, Fetched, Outgoing};
use miniframework::kv::MemKv;
use miniframework::{wall_ms, ApiError};
use std::sync::{Arc, Mutex};

/// A recorded request.
#[derive(Clone, Debug)]
struct Call {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: String,
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<Call>>,
    /// What notifications return next (a JSON array).
    mentions: Mutex<String>,
    /// Brevo answers this status to sends.
    brevo_status: Mutex<u16>,
    /// The data repo's branch exists (after the first push).
    branch: Mutex<bool>,
}

impl Fake {
    fn new() -> Arc<Self> {
        let f = Self::default();
        *f.mentions.lock().unwrap() = "[]".into();
        *f.brevo_status.lock().unwrap() = 201;
        Arc::new(f)
    }

    fn calls(&self, needle: &str) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.url.contains(needle))
            .cloned()
            .collect()
    }
}

impl Fetch for Fake {
    fn get(&self, url: &str, accept: &str, limit: usize) -> std::io::Result<Fetched> {
        self.send(&Outgoing {
            method: "GET",
            url,
            headers: &[("Accept", accept)],
            body: &[],
            limit,
        })
    }

    fn send(&self, r: &Outgoing<'_>) -> std::io::Result<Fetched> {
        self.calls.lock().unwrap().push(Call {
            method: r.method.into(),
            url: r.url.into(),
            headers: r
                .headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: String::from_utf8_lossy(r.body).into(),
        });
        let ok = |status: u16, body: &str| {
            Ok(Fetched {
                status,
                content_type: "application/json".into(),
                body: body.as_bytes().to_vec(),
            })
        };
        let u = r.url;
        if u.ends_with("/v3/senders") {
            return ok(
                200,
                r#"{"senders":[{"email":"bot@example.com","active":true,"name":"x"}]}"#,
            );
        }
        if u.ends_with("/v3/smtp/email") {
            let s = *self.brevo_status.lock().unwrap();
            return ok(s, r#"{"messageId":"m1"}"#);
        }
        if u.ends_with("/accounts/verify_credentials") {
            return ok(200, r#"{"id":"100","acct":"party","username":"party"}"#);
        }
        if u.contains("/api/v1/notifications") {
            let body = std::mem::replace(&mut *self.mentions.lock().unwrap(), "[]".into());
            return ok(200, &body);
        }
        if u.ends_with("/api/v1/statuses") {
            return ok(200, r#"{"id":"999"}"#);
        }
        if u.contains("api.github.com") {
            let mut branch = self.branch.lock().unwrap();
            return match (r.method, u) {
                ("GET", u) if u.contains("/git/ref/heads/") => {
                    if *branch {
                        ok(200, r#"{"object":{"sha":"c1"}}"#)
                    } else {
                        ok(404, r#"{"message":"Not Found"}"#)
                    }
                }
                ("GET", u) if u.contains("/git/commits/") => {
                    ok(200, r#"{"sha":"c1","tree":{"sha":"t1"}}"#)
                }
                ("POST", u) if u.ends_with("/git/blobs") => ok(201, r#"{"sha":"b1"}"#),
                ("POST", u) if u.ends_with("/git/trees") => ok(201, r#"{"sha":"t2"}"#),
                ("POST", u) if u.ends_with("/git/commits") => ok(201, r#"{"sha":"c2"}"#),
                ("POST", u) if u.ends_with("/git/refs") => {
                    *branch = true;
                    ok(201, "{}")
                }
                ("PATCH", _) => ok(200, "{}"),
                _ => ok(404, "{}"),
            };
        }
        ok(404, "{}")
    }
}

fn now() -> u64 {
    wall_ms().map_or(0, |ms| ms / 1000)
}

/// Runs the worker until it has nothing to do.
fn drain(app: &App, fake: &Fake) {
    let f = Fetchers {
        household: fake,
        public: fake,
    };
    for _ in 0..50 {
        if !app.work(&f) {
            return;
        }
    }
    panic!("the worker never ran dry");
}

fn edit(app: &App, f: impl FnOnce(&mut crate::state::State)) {
    app.mutate(|inner| {
        f(&mut inner.state);
        Ok::<(), ApiError>(())
    })
    .unwrap();
}

struct World {
    app: App,
    fake: Arc<Fake>,
    dad: u32,
    son: u32,
    group: u32,
}

fn world() -> World {
    let app = App::open(
        Arc::new(MemKv::default()),
        Limits {
            pin_rounds: 2,
            ..Limits::board()
        },
    )
    .unwrap();
    let mut ids = (0, 0, 0);
    edit(&app, |s| {
        let dad = s.setup("Dad", "dad@example.com", "1234", 0).unwrap();
        let son = s.signup("Son", "", "1234", 0).unwrap();
        let g = s
            .create_group(
                dad,
                &GroupInput {
                    name: Some("Family".into()),
                    visibility: Some("public".into()),
                    join: Some("open".into()),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        s.join_group(son, g, 0).unwrap();
        ids = (dad, son, g);
    });
    World {
        app,
        fake: Fake::new(),
        dad: ids.0,
        son: ids.1,
        group: ids.2,
    }
}

fn event(w: &World, title: &str) -> u32 {
    let mut id = 0;
    edit(&w.app, |s| {
        id = s
            .create_event(
                w.dad,
                w.group,
                &EventInput {
                    title: Some(title.into()),
                    start: Some(now() + 10 * 86_400),
                    place: Some("Riverside Park".into()),
                    address: Some("12 Elm St".into()),
                    where_kind: Some("place".into()),
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
    });
    id
}

#[test]
fn an_email_only_relative_gets_an_invitation_with_a_calendar_invite() {
    let w = world();
    edit(&w.app, |s| {
        s.channel_mut(MAIL).mail = Some(StoredMail {
            api_key: "xkeysib-test1234".into(),
            state: "checking".into(),
            sender_name: "party".into(),
            ..Default::default()
        });
    });
    drain(&w.app, &w.fake);
    assert_eq!(
        w.app.lock().state.mail().unwrap().state,
        "ready",
        "the key was checked"
    );
    assert_eq!(
        w.app.lock().state.mail().unwrap().sender_email,
        "bot@example.com"
    );
    let e = event(&w, "Thanksgiving");
    edit(&w.app, |s| {
        let gran = s
            .add_person(
                w.dad,
                &NewPerson {
                    display_name: "Grandma".into(),
                    email: "gran@example.com".into(),
                    household: 0,
                },
                now(),
            )
            .unwrap();
        s.create_invite(
            w.dad,
            &InviteInput {
                target_kind: "event".into(),
                target: e,
                person: gran,
                ..Default::default()
            },
            now(),
        )
        .unwrap();
    });
    drain(&w.app, &w.fake);
    let sends = w.fake.calls("/smtp/email");
    assert_eq!(
        sends.len(),
        1,
        "one email: the invitation with its calendar entry"
    );
    let body = &sends[0].body;
    assert!(
        body.contains("gran@example.com") && body.contains("Invitation: Thanksgiving"),
        "{body}"
    );
    assert!(
        body.contains(r#""replyTo":{"email":"dad@example.com"}"#),
        "replies reach the host"
    );
    let ics = body
        .split("\"content\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let ics = String::from_utf8(decode64(ics)).unwrap();
    assert!(
        ics.contains("METHOD:REQUEST")
            && ics.contains("ATTENDEE;CN=Grandma")
            && ics.contains("UID:e"),
        "{ics}"
    );
    assert!(sends[0]
        .headers
        .iter()
        .any(|(k, v)| k == "api-key" && v == "xkeysib-test1234"));
    // A cancellation sends a CANCEL.
    edit(&w.app, |s| s.cancel_event(w.dad, e, now()).unwrap());
    drain(&w.app, &w.fake);
    let last = w.fake.calls("/smtp/email").pop().unwrap();
    let ics = last
        .body
        .split("\"content\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(String::from_utf8(decode64(ics))
        .unwrap()
        .contains("METHOD:CANCEL"));
}

fn decode64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        _ => 63,
    };
    let bytes: Vec<u8> = s.bytes().filter(|c| *c != b'=').map(val).collect();
    let mut out = Vec::new();
    for c in bytes.chunks(4) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, v)| n | (u32::from(*v) << (18 - 6 * i)));
        out.push((n >> 16) as u8);
        if c.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if c.len() > 3 {
            out.push(n as u8);
        }
    }
    out
}

#[test]
fn failed_sends_back_off_and_retry() {
    let w = world();
    edit(&w.app, |s| {
        s.channel_mut(MAIL).mail = Some(StoredMail {
            api_key: "k".into(),
            sender_email: "bot@example.com".into(),
            state: "ready".into(),
            ..Default::default()
        });
        s.enqueue(
            StoredJob {
                kind: "mail".into(),
                to: "a@example.com".into(),
                subject: "x".into(),
                priority: 1,
                ..Default::default()
            },
            now(),
        );
    });
    *w.fake.brevo_status.lock().unwrap() = 500;
    drain(&w.app, &w.fake);
    let job = w.app.lock().state.jobs[0].clone();
    assert_eq!((job.state.as_str(), job.attempts), ("queued", 1));
    assert!(job.next_at > now() + 60, "waits before trying again");
    assert!(job.error.contains("HTTP 500"));
    // Due again: it goes through.
    *w.fake.brevo_status.lock().unwrap() = 201;
    edit(&w.app, |s| s.jobs.get_mut(job.id).unwrap().next_at = 0);
    drain(&w.app, &w.fake);
    assert_eq!(w.app.lock().state.jobs[0].state, "sent");
}

fn mention(id: &str, acct: &str, account_id: &str, text: &str, visibility: &str) -> String {
    format!(
        r#"{{"id":"{id}","type":"mention","account":{{"id":"{account_id}","acct":"{acct}","username":"x"}},"status":{{"id":"s{id}","content":"<p><span class=\"h-card\"><a>@party</a></span> {text}</p>","visibility":"{visibility}"}}}}"#
    )
}

#[test]
fn the_bot_runs_dm_commands_once_and_answers_privately() {
    let w = world();
    let e = event(&w, "Game night");
    edit(&w.app, |s| {
        s.channel_mut(BOT).bot = Some(StoredBot {
            instance: "https://mastodon.social".into(),
            token: "tok".into(),
            trust: "public".into(),
            state: "checking".into(),
            commands: true,
            poll_secs: 30,
            ..Default::default()
        });
        s.person_mut(w.son)
            .unwrap()
            .identities
            .push(StoredIdentity {
                host: "home.example".into(),
                subject: "5".into(),
                handle: "@son@home.example".into(),
            });
    });
    // The check, then the first poll: it only finds where "now" is.
    *w.fake.mentions.lock().unwrap() = format!(
        "[{}]",
        mention(
            "10",
            "son@home.example",
            "777",
            &format!("going E{e}"),
            "direct"
        )
    );
    drain(&w.app, &w.fake);
    {
        let inner = w.app.lock();
        let b = inner.state.bot().unwrap();
        assert_eq!(
            (b.handle.as_str(), b.cursor.as_str()),
            ("@party@mastodon.social", "10")
        );
        assert!(
            inner.state.rsvp(e, w.son).is_none(),
            "nothing from before runs"
        );
    }
    // A real batch: the son answers by DM, a stranger tries, someone posts publicly.
    *w.fake.mentions.lock().unwrap() = format!(
        "[{},{},{}]",
        mention(
            "12",
            "son@home.example",
            "777",
            &format!("going E{e} +1</p><p>me"),
            "direct"
        ),
        mention(
            "13",
            "stranger@else.example",
            "778",
            &format!("going E{e}"),
            "direct"
        ),
        mention("14", "loud@else.example", "779", "help", "public"),
    );
    edit(&w.app, |s| {
        s.channel_mut(BOT).bot.as_mut().unwrap().last_poll = 0
    });
    drain(&w.app, &w.fake);
    let inner = w.app.lock();
    let r = inner.state.rsvp(e, w.son).expect("the son's DM answered");
    assert_eq!((r.status.as_str(), r.plus), ("going", 1));
    assert_eq!(inner.state.bot().unwrap().cursor, "14");
    drop(inner);
    let posts = w.fake.calls("/api/v1/statuses");
    let to = |who: &str| {
        posts
            .iter()
            .find(|c| c.body.contains(who))
            .unwrap_or_else(|| panic!("no reply to {who}: {posts:?}"))
            .clone()
    };
    let son = to("@son@home.example");
    assert!(
        son.body.contains("\"visibility\":\"direct\""),
        "{}",
        son.body
    );
    assert!(
        son.body.contains("going to E") && son.body.contains("+1"),
        "{}",
        son.body
    );
    assert!(son.body.contains("party ack 12"), "{}", son.body);
    assert!(son.headers.iter().any(|(k, _)| k == "Idempotency-Key"));
    assert!(to("@stranger@else.example").body.contains("don't know you"));
    assert!(to("@loud@else.example").body.contains("privately"));
    // The same batch again (a crash before the cursor moved) changes nothing.
    let before = w.fake.calls("/api/v1/statuses").len();
    *w.fake.mentions.lock().unwrap() = format!(
        "[{}]",
        mention(
            "12",
            "son@home.example",
            "777",
            &format!("no E{e}"),
            "direct"
        )
    );
    edit(&w.app, |s| {
        let b = s.channel_mut(BOT).bot.as_mut().unwrap();
        b.last_poll = 0;
        b.cursor = "11".into();
    });
    drain(&w.app, &w.fake);
    assert_eq!(
        w.app.lock().state.rsvp(e, w.son).unwrap().status,
        "going",
        "a processed id never runs twice"
    );
    assert_eq!(w.fake.calls("/api/v1/statuses").len(), before);
}

#[test]
fn dm_notifications_follow_the_persons_settings() {
    let w = world();
    let e = event(&w, "Picnic");
    edit(&w.app, |s| {
        s.channel_mut(BOT).bot = Some(StoredBot {
            instance: "https://mastodon.social".into(),
            token: "tok".into(),
            state: "ready".into(),
            handle: "@party@mastodon.social".into(),
            ..Default::default()
        });
        s.person_mut(w.son)
            .unwrap()
            .identities
            .push(StoredIdentity {
                host: "mastodon.social".into(),
                subject: "5".into(),
                handle: "@son@mastodon.social".into(),
            });
        s.answer(
            w.son,
            e,
            w.son,
            &RsvpInput {
                status: "going".into(),
                rev_seen: 1,
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        s.update_event(
            w.dad,
            e,
            &EventInput {
                start: Some(now() + 11 * 86_400),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
    });
    drain(&w.app, &w.fake);
    let posts = w.fake.calls("/api/v1/statuses");
    assert_eq!(posts.len(), 1, "{posts:?}");
    assert!(
        posts[0]
            .body
            .contains("@son@mastodon.social Changed: Picnic"),
        "{}",
        posts[0].body
    );
    edit(&w.app, |s| {
        s.person_mut(w.son).unwrap().dm_off = true;
        s.update_event(
            w.dad,
            e,
            &EventInput {
                start: Some(now() + 12 * 86_400),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
    });
    drain(&w.app, &w.fake);
    assert_eq!(w.fake.calls("/api/v1/statuses").len(), 1, "stop means stop");
}

#[test]
fn publication_pushes_public_data_only_and_only_when_it_changed() {
    let w = world();
    let e = event(&w, "Open house");
    edit(&w.app, |s| {
        s.channel_mut(PUBLISH).publish = Some(StoredPublish {
            repo: "ourparty/party-data".into(),
            branch: "main".into(),
            token: "ghp".into(),
            site_url: "https://ourparty.github.io/party-site/".into(),
            every_minutes: 10,
            asked: true,
            ..Default::default()
        });
    });
    drain(&w.app, &w.fake);
    let first = w.fake.calls("api.github.com");
    assert!(
        first
            .iter()
            .any(|c| c.method == "POST" && c.url.ends_with("/git/refs")),
        "the first push makes the branch"
    );
    let blobs: Vec<String> = first
        .iter()
        .filter(|c| c.url.ends_with("/git/blobs"))
        .map(|c| c.body.clone())
        .collect();
    let all = blobs
        .iter()
        .map(|b| {
            String::from_utf8(decode64(
                b.split("\"content\":\"")
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap(),
            ))
            .unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all.contains("Open house") && all.contains("Riverside Park"),
        "{all}"
    );
    assert!(!all.contains("12 Elm St"), "no addresses: {all}");
    assert!(!all.contains("dad@example.com"), "no emails");
    let commit = first
        .iter()
        .find(|c| c.url.ends_with("/git/commits") && c.method == "POST")
        .unwrap();
    assert!(
        commit.body.contains("\"parents\":[]"),
        "always one commit deep"
    );
    assert_eq!(w.app.lock().state.publish().unwrap().generation, 1);
    // Nothing changed: nothing is pushed, however often it's checked.
    let calls = w.fake.calls("api.github.com").len();
    edit(&w.app, |s| {
        s.channel_mut(PUBLISH)
            .publish
            .as_mut()
            .unwrap()
            .last_attempt = 0
    });
    drain(&w.app, &w.fake);
    assert_eq!(w.fake.calls("api.github.com").len(), calls);
    // A change: only the changed files go up, onto the existing tree.
    edit(&w.app, |s| {
        s.update_event(
            w.dad,
            e,
            &EventInput {
                title: Some("Open house!".into()),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        s.channel_mut(PUBLISH)
            .publish
            .as_mut()
            .unwrap()
            .last_attempt = 0;
    });
    drain(&w.app, &w.fake);
    let second: Vec<Call> = w
        .fake
        .calls("api.github.com")
        .into_iter()
        .skip(calls)
        .collect();
    let uploaded = second
        .iter()
        .filter(|c| c.url.ends_with("/git/blobs"))
        .count();
    assert!(
        uploaded >= 2 && uploaded < blobs.len(),
        "events, ics and manifest, not everything: {uploaded}"
    );
    assert!(second
        .iter()
        .any(|c| c.url.ends_with("/git/trees") && c.body.contains("\"base_tree\":\"t1\"")));
    assert!(second
        .iter()
        .any(|c| c.method == "PATCH" && c.body.contains("\"force\":true")));
    assert_eq!(w.app.lock().state.publish().unwrap().generation, 2);
    let _ = w.son;
}
