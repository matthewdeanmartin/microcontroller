//! Feature regression tests using only in-memory storage and mocked transports.
use super::*;
use crate::outbound::{StoredBot, StoredJob, StoredOutbox};
use crate::replies::{Inbox, Mention, PostLink, RemoteAccount, RemoteStatus};
use miniframework::fetch::{Fetch, Fetched, Outgoing};
use miniframework::kv::MemKv;
use std::collections::HashMap;

#[derive(Default)]
struct Network {
    mentions: Mutex<HashMap<String, Vec<Mention>>>,
    calls: Mutex<Vec<(String, String, String)>>,
    fail: std::sync::atomic::AtomicBool,
}
impl Fetch for Network {
    fn get(&self, _: &str, _: &str, _: usize) -> std::io::Result<Fetched> {
        unreachable!()
    }
    fn send(&self, req: &Outgoing<'_>) -> std::io::Result<Fetched> {
        let key = req
            .headers
            .iter()
            .find(|(k, _)| *k == "Idempotency-Key")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();
        self.calls.lock().unwrap().push((
            req.url.into(),
            String::from_utf8_lossy(req.body).into_owned(),
            key,
        ));
        if req.url.contains("/notifications?") {
            let host = if req.url.contains("home.local") {
                "home"
            } else {
                "public"
            };
            let cursor = req.url.split("min_id=").nth(1).unwrap_or("0");
            let mut list = self
                .mentions
                .lock()
                .unwrap()
                .get(host)
                .cloned()
                .unwrap_or_default();
            list.retain(|m| crate::replies::newer(&m.id, cursor));
            list.sort_by(|a, b| (a.id.len(), &a.id).cmp(&(b.id.len(), &b.id)));
            list.truncate(8);
            let wrapped =
                wire::to_vec(Format::Json, &crate::replies::Mentions { items: list }).unwrap();
            let text = String::from_utf8(wrapped).unwrap();
            let start = text.find('[').unwrap();
            return Ok(Fetched {
                status: 200,
                content_type: "application/json".into(),
                body: text.as_bytes()[start..text.len() - 1].to_vec(),
            });
        }
        Ok(Fetched {
            status: if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
                503
            } else {
                200
            },
            content_type: "application/json".into(),
            body: br#"{"id":"posted1","url":"https://social.example/@games/posted1"}"#.to_vec(),
        })
    }
}

fn put<M: miniframework::Message>(kv: &dyn Kv, key: &str, message: &M) {
    kv.set(key, &wire::to_vec(Format::CborInt, message).unwrap())
        .unwrap();
}
fn seed_bot(kv: &dyn Kv, prefix: &str, host: &str) {
    put(
        kv,
        &format!("{prefix}bot"),
        &StoredBot {
            instance: format!("https://{host}"),
            token: "test".into(),
            handle: format!("@games@{host}"),
            anchor: if host.ends_with(".local") {
                "household-ca"
            } else {
                "public"
            }
            .into(),
            visibility: "unlisted".into(),
            replies: true,
            ..Default::default()
        },
    );
    put(
        kv,
        &format!("{prefix}inbox"),
        &Inbox {
            initialized: true,
            cursor: "0".into(),
            ..Default::default()
        },
    );
    put(
        kv,
        &format!("{prefix}outbox"),
        &StoredOutbox {
            links: vec![PostLink {
                status: "parent".into(),
                plan: 1,
            }],
            ..Default::default()
        },
    );
}
fn fixture() -> (App, Arc<MemKv>, Arc<Network>, Arc<Outbox>, Arc<Outbox>) {
    let kv = Arc::new(MemKv::default());
    seed_bot(kv.as_ref(), "", "home.local");
    seed_bot(kv.as_ref(), "pub_", "social.example");
    let network = Arc::new(Network::default());
    let house = Arc::new(Outbox::open(kv.clone(), network.clone(), network.clone()));
    let public = Arc::new(Outbox::open_public(
        kv.clone(),
        network.clone(),
        network.clone(),
    ));
    let mut app = App::open(
        kv.clone(),
        Limits {
            pin_rounds: 2,
            ..Limits::board()
        },
    )
    .unwrap();
    app.attach_outbox(house.clone());
    app.attach_public_outbox(public.clone());
    app.mutate(|inner| {
        let s = &mut inner.state;
        let host = s.setup("host@example.com", "Host", "1234", 1)?;
        for (email, name, domain) in [
            ("kid@example.com", "Kid", "home.local"),
            ("friend@example.com", "Friend", "social.example"),
        ] {
            let id = s.signup(email, name, "1234", 1)?;
            s.account_mut(id)?.mastodon = Some(StoredIdentity {
                host: domain.into(),
                subject: "42".into(),
                handle: format!("@person@{domain}"),
            });
        }
        let game = s.add_game(
            host,
            &GameInput {
                title: Some("Catan".into()),
                ..Default::default()
            },
            1,
        )?;
        s.create_plan(
            host,
            &PlanInput {
                title: Some("Public game".into()),
                games: Some(vec![game]),
                times: Some(vec![TimeInput {
                    id: 0,
                    start: wall().unwrap() + 3600,
                    end: 0,
                }]),
                announce_public: Some(true),
                ..Default::default()
            },
            1,
        )?;
        Ok(())
    })
    .unwrap();
    (app, kv, network, house, public)
}
fn mention(id: &str, acct: &str, text: &str) -> Mention {
    Mention {
        id: id.into(),
        account: RemoteAccount {
            id: "42".into(),
            acct: acct.into(),
        },
        status: Some(RemoteStatus {
            content: text.into(),
            in_reply_to_id: Some("parent".into()),
        }),
    }
}

#[test]
fn rsvps_use_native_identity_not_handle_and_survive_replay() {
    let (app, kv, network, house, public) = fixture();
    network.mentions.lock().unwrap().insert(
        "home".into(),
        vec![mention(
            "1",
            "renamed-kid",
            "<p><a>@<span>games</span></a> in</p>",
        )],
    );
    network.mentions.lock().unwrap().insert(
        "public".into(),
        vec![
            mention("1", "friend", "<p>@games maybe</p>"),
            mention("2", "imposter@elsewhere.example", "<p>@games out</p>"),
            mention("3", "friend", "<p>@games in 999</p>"),
        ],
    );
    let now = wall().unwrap();
    assert!(house.run_once(now));
    assert!(public.run_once(now));
    assert_eq!(house.pending_replies().len(), 1);
    app.receive_replies(now);
    {
        let inner = app.lock();
        let p = inner.state.plan(1).unwrap();
        assert_eq!(p.responses.len(), 2);
        assert_eq!(
            p.responses.iter().find(|r| r.account == 2).unwrap().overall,
            "in"
        );
        assert_eq!(
            p.responses.iter().find(|r| r.account == 3).unwrap().overall,
            "maybe"
        );
    }
    assert!(house.pending_replies().is_empty());
    assert!(public.pending_replies().is_empty());
    let notifications = app.lock().state.account(1).unwrap().notifications.clone();
    // Simulate the crash window after state commit, before inbox acknowledgement.
    put(
        kv.as_ref(),
        "inbox",
        &Inbox {
            initialized: true,
            cursor: "1".into(),
            pending: vec![mention("1", "kid", "<p>out</p>")],
        },
    );
    let replay = Arc::new(Outbox::open(kv.clone(), network.clone(), network));
    let mut reopened = App::open(
        kv,
        Limits {
            pin_rounds: 2,
            ..Limits::board()
        },
    )
    .unwrap();
    reopened.attach_outbox(replay);
    reopened.receive_replies(now + 1);
    let inner = reopened.lock();
    assert_eq!(inner.state.plan(1).unwrap().responses[0].overall, "in");
    assert_eq!(inner.state.account(1).unwrap().notifications, notifications);
}

#[test]
fn public_announcements_and_digest_exclude_household_only_plans() {
    let (app, _, _, house, public) = fixture();
    let private = app
        .mutate(|inner| {
            inner.state.create_plan(
                1,
                &PlanInput {
                    title: Some("Kids-only secret".into()),
                    games: Some(vec![1]),
                    announce: Some(true),
                    ..Default::default()
                },
                1,
            )
        })
        .unwrap();
    app.announce_plan(private, News::New, 2);
    app.announce_plan(1, News::New, 2);
    assert_eq!(house.view().jobs.len(), 1);
    assert_eq!(public.view().jobs.len(), 1);
    assert!(house.view().jobs[0].text.contains("Kids-only secret"));
    assert!(!public.view().jobs[0].text.contains("Kids-only"));
    let inner = app.lock();
    let digest = announce::digest_for(&inner.state, "http://home.local", 2, 0, true).unwrap();
    assert!(digest.contains("Public game"));
    assert!(!digest.contains("Kids-only"));
}

#[test]
fn manual_retry_preserves_the_mastodon_idempotency_key() {
    let kv = Arc::new(MemKv::default());
    seed_bot(kv.as_ref(), "", "home.local");
    put(
        kv.as_ref(),
        "outbox",
        &StoredOutbox {
            next_id: 2,
            jobs: vec![StoredJob {
                id: 1,
                kind: "post".into(),
                text: "Hello".into(),
                state: "queued".into(),
                created: 1,
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    let network = Arc::new(Network::default());
    network
        .fail
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let o = Outbox::open(kv, network.clone(), network.clone());
    assert!(o.run_once(10));
    assert_eq!(o.view().jobs[0].attempts, 1);
    o.retry(1, 11).unwrap();
    assert!(o.retry(1, 11).is_err());
    network
        .fail
        .store(false, std::sync::atomic::Ordering::Relaxed);

    assert!(o.run_once(11));
    assert_eq!(o.view().jobs[0].state, "sent");
    assert!(o.retry(1, 12).is_err());
    let calls = network.calls.lock().unwrap();
    assert_eq!(calls[0].2, calls[1].2);
}

#[test]
fn incoming_backlog_pages_forward_without_losing_commands() {
    let (app, _, network, house, _) = fixture();
    network.mentions.lock().unwrap().insert(
        "home".into(),
        (1..=20)
            .map(|n| {
                mention(
                    &n.to_string(),
                    "person",
                    if n % 2 == 0 {
                        "<p>out</p>"
                    } else {
                        "<p>in</p>"
                    },
                )
            })
            .collect(),
    );
    let now = wall().unwrap();
    for step in 0..3 {
        let at = now + step * 60;
        // Drain confirmations from the previous page, then fetch the next page.
        while house.run_once(at) {}
        let expected = if step < 2 { 8 } else { 4 };
        assert_eq!(house.pending_replies().len(), expected);
        app.receive_replies(at);
    }
    let inner = app.lock();
    assert_eq!(inner.state.plan(1).unwrap().responses[0].overall, "out");
    assert!(inner
        .state
        .account(2)
        .unwrap()
        .last_house_reply
        .ends_with("|20"));
}

#[test]
fn invalid_and_replayed_commands_do_not_commit_empty_snapshots() {
    let (app, _, network, house, _) = fixture();
    network.mentions.lock().unwrap().insert(
        "home".into(),
        vec![
            mention("1", "not-local@somewhere.example", "<p>in</p>"),
            mention("2", "person", "<p>in 999</p>"),
            mention("3", "person", "<p>maybe later</p>"),
        ],
    );
    let commits = app.lock().state.commits;
    assert!(house.run_once(wall().unwrap()));
    app.receive_replies(wall().unwrap());
    assert_eq!(app.lock().state.commits, commits);
    assert!(house.pending_replies().is_empty());
}
