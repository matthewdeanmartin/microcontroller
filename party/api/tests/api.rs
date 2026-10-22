//! HTTP-level tests of the party API: the Sprint 1 and Sprint 2 exit
//! criteria (`spec/sprints.md`), driven through the same `Service` the
//! board serves.
use miniframework::kv::{Kv, MemKv};
use miniframework::{Reply, Request, Service};
use party::pin::GuardLimits;
use party::{App, Limits};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const ORIGIN: &str = "http://party.local";

fn limits() -> Limits {
    Limits {
        pin_rounds: 2,
        ..Limits::board()
    }
}

struct Res {
    status: u16,
    body: String,
    cookie: Option<String>,
    retry_after: Option<String>,
}

impl Res {
    fn json(&self, field: &str) -> String {
        // Tiny extractor: "field":value or "field":"value".
        let key = format!("\"{field}\":");
        let at = self
            .body
            .find(&key)
            .unwrap_or_else(|| panic!("{field} in {}", self.body))
            + key.len();
        let rest = &self.body[at..];
        if let Some(stripped) = rest.strip_prefix('"') {
            stripped[..stripped.find('"').unwrap()].to_string()
        } else {
            rest[..rest.find([',', '}']).unwrap()].to_string()
        }
    }
}

fn call(app: &App, method: &str, path: &str, cookie: Option<&str>, body: &str) -> Res {
    call_from(app, method, path, cookie, body, Some(ORIGIN))
}

fn call_from(
    app: &App,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: &str,
    origin: Option<&str>,
) -> Res {
    let mut headers = vec![
        ("Host".to_string(), "party.local".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
    ];
    if let Some(o) = origin {
        headers.push(("Origin".into(), o.into()));
    }
    if let Some(c) = cookie {
        headers.push(("Cookie".into(), c.into()));
    }
    let sep = if path.contains('?') { '&' } else { '?' };
    let uri = format!("{path}{sep}fmt=json");
    let req = Request::new(method, &uri, &headers, body.as_bytes(), false);
    let (mut out, mut spare) = (Vec::new(), Vec::new());
    let mut reply = Reply::new(&mut out, &mut spare, 256 * 1024);
    app.handle(&req, &mut reply);
    let find = |name: &str| {
        reply
            .headers()
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    Res {
        status: reply.status(),
        body: String::from_utf8_lossy(reply.body()).into_owned(),
        cookie: find("Set-Cookie").map(|c| c.split(';').next().unwrap().to_string()),
        retry_after: find("Retry-After"),
    }
}

fn setup(app: &App) -> String {
    let r = call(
        app,
        "POST",
        "/api/v1/setup",
        None,
        r#"{"display_name":"Dad","email":"dad@home","pin":"1234"}"#,
    );
    assert_eq!(r.status, 201, "{}", r.body);
    r.cookie.expect("setup signs in")
}

fn signup(app: &App, name: &str) -> String {
    let body = format!(r#"{{"display_name":"{name}","pin":"0000"}}"#);
    let r = call(app, "POST", "/api/v1/auth/signup", None, &body);
    assert_eq!(r.status, 201, "{}", r.body);
    r.cookie.unwrap()
}

fn id_of(app: &App, cookie: &str) -> u32 {
    call(app, "GET", "/api/v1/auth/me", Some(cookie), "")
        .json("id")
        .parse()
        .unwrap()
}

fn new_group(app: &App, cookie: &str, name: &str, visibility: &str, join: &str) -> u32 {
    let body = format!(r#"{{"name":"{name}","visibility":"{visibility}","join":"{join}"}}"#);
    let r = call(app, "POST", "/api/v1/groups", Some(cookie), &body);
    assert_eq!(r.status, 201, "{}", r.body);
    r.json("id").parse().unwrap()
}

// ---- Sprint 0 -----------------------------------------------------------------

#[test]
fn status_before_and_after_setup() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let r = call(&app, "GET", "/api/v1/status", None, "");
    assert_eq!(r.status, 200);
    assert_eq!(r.json("app"), "party");
    assert_eq!(r.json("setup_needed"), "true");
    setup(&app);
    assert_eq!(
        call(&app, "GET", "/api/v1/status", None, "").json("setup_needed"),
        "false"
    );
}

// ---- Sprint 1: accounts ------------------------------------------------------------

#[test]
fn concurrent_setup_has_one_winner() {
    let app = Arc::new(App::open(Arc::new(MemKv::default()), limits()).unwrap());
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let app = app.clone();
            std::thread::spawn(move || {
                let body = format!(r#"{{"display_name":"P{i}","pin":"1234"}}"#);
                call(&app, "POST", "/api/v1/setup", None, &body).status
            })
        })
        .collect();
    let statuses: Vec<u16> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        statuses.iter().filter(|&&s| s == 201).count(),
        1,
        "{statuses:?}"
    );
    assert_eq!(
        statuses.iter().filter(|&&s| s == 409).count(),
        7,
        "{statuses:?}"
    );
}

#[test]
fn cross_origin_writes_are_refused() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let body = r#"{"display_name":"Dad","pin":"1234"}"#;
    let evil = call_from(
        &app,
        "POST",
        "/api/v1/setup",
        None,
        body,
        Some("http://evil.example"),
    );
    assert_eq!(evil.status, 403, "{}", evil.body);
    let none = call_from(&app, "POST", "/api/v1/setup", None, body, None);
    assert_eq!(none.status, 403);
}

#[test]
fn pin_sign_in_by_name_or_email_and_lockout() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    setup(&app);
    signup(&app, "Grandma");
    let ok = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"login":"grandma","pin":"0000"}"#,
    );
    assert_eq!(
        ok.status, 200,
        "an email-less person signs in by name: {}",
        ok.body
    );
    let wrong = r#"{"login":"dad@home","pin":"9999"}"#;
    for _ in 0..5 {
        assert_eq!(
            call(&app, "POST", "/api/v1/auth/pin", None, wrong).status,
            401
        );
    }
    let right = r#"{"login":"DAD","pin":"1234"}"#;
    let locked = call(&app, "POST", "/api/v1/auth/pin", None, right);
    assert_eq!(locked.status, 429, "{}", locked.body);
    assert_eq!(locked.retry_after.as_deref(), Some("60"));
    let unknown = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"login":"who","pin":"1234"}"#,
    );
    assert_eq!(unknown.status, 401);
    assert_eq!(
        unknown.json("error"),
        "wrong_pin",
        "unknown names look like wrong PINs"
    );
}

#[test]
fn board_wide_budget_applies() {
    let guard = GuardLimits {
        board_failures: 3,
        ..GuardLimits::default()
    };
    let app = App::open_with(Arc::new(MemKv::default()), limits(), guard).unwrap();
    setup(&app);
    for who in ["a", "b", "c"] {
        let body = format!(r#"{{"login":"{who}","pin":"1234"}}"#);
        assert_eq!(
            call(&app, "POST", "/api/v1/auth/pin", None, &body).status,
            401
        );
    }
    let r = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"login":"dad","pin":"1234"}"#,
    );
    assert_eq!(r.status, 429);
}

#[test]
fn sessions_are_evicted_per_person() {
    let app = App::open(
        Arc::new(MemKv::default()),
        Limits {
            sessions_per_person: 2,
            ..limits()
        },
    )
    .unwrap();
    let first = setup(&app);
    let sign_in = || {
        call(
            &app,
            "POST",
            "/api/v1/auth/pin",
            None,
            r#"{"login":"dad","pin":"1234"}"#,
        )
        .cookie
        .unwrap()
    };
    let second = sign_in();
    let third = sign_in();
    assert_eq!(
        call(&app, "GET", "/api/v1/auth/me", Some(&first), "").status,
        401,
        "oldest evicted"
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/auth/me", Some(&second), "").status,
        200
    );
    call(&app, "POST", "/api/v1/auth/logout?all=1", Some(&third), "");
    assert_eq!(
        call(&app, "GET", "/api/v1/auth/me", Some(&second), "").status,
        401
    );
}

#[test]
fn members_cannot_reach_admin() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    setup(&app);
    let kid = signup(&app, "Sam");
    for (method, path) in [
        ("GET", "/api/v1/admin/people"),
        ("PATCH", "/api/v1/admin/people/1"),
        ("GET", "/api/v1/admin/settings"),
        ("PATCH", "/api/v1/admin/settings"),
        ("GET", "/api/v1/admin/storage"),
        ("GET", "/api/v1/admin/audit"),
        ("POST", "/api/v1/households"),
        ("DELETE", "/api/v1/households/1"),
        ("PUT", "/api/v1/households/1/members/2"),
    ] {
        let r = call(&app, method, path, Some(&kid), "{}");
        assert_eq!(r.status, 403, "{method} {path}: {}", r.body);
    }
    assert_eq!(
        call(&app, "GET", "/api/v1/admin/people", None, "").status,
        401
    );
}

#[test]
fn admin_manages_people_households_and_settings() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "Sam");
    let sam = id_of(&app, &kid);
    let r = call(
        &app,
        "POST",
        "/api/v1/households",
        Some(&dad),
        r#"{"name":"The Martins"}"#,
    );
    assert_eq!(r.status, 201, "{}", r.body);
    let home: u32 = r.json("id").parse().unwrap();
    let r = call(
        &app,
        "PUT",
        &format!("/api/v1/households/{home}/members/{sam}"),
        Some(&dad),
        "",
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let me = call(&app, "GET", "/api/v1/auth/me", Some(&kid), "");
    assert_eq!(me.json("household_name"), "The Martins");
    let r = call(
        &app,
        "PATCH",
        &format!("/api/v1/admin/people/{sam}"),
        Some(&dad),
        r#"{"pin":"2468"}"#,
    );
    assert_eq!(r.status, 200);
    let r = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"login":"sam","pin":"2468"}"#,
    );
    assert_eq!(r.status, 200, "the admin's temporary PIN works");
    let r = call(
        &app,
        "PATCH",
        "/api/v1/admin/settings",
        Some(&dad),
        r#"{"signup_open":false}"#,
    );
    assert_eq!(r.json("signup_open"), "false");
    let r = call(
        &app,
        "POST",
        "/api/v1/auth/signup",
        None,
        r#"{"display_name":"Guest","pin":"0000"}"#,
    );
    assert_eq!(r.status, 403);
    let r = call(
        &app,
        "PATCH",
        &format!("/api/v1/admin/people/{sam}"),
        Some(&dad),
        r#"{"disabled":true}"#,
    );
    assert_eq!(r.status, 200);
    assert_eq!(
        call(&app, "GET", "/api/v1/auth/me", Some(&kid), "").status,
        401,
        "disabling signs out"
    );
    let audit = call(&app, "GET", "/api/v1/admin/audit", Some(&dad), "");
    for action in [
        "setup",
        "household.create",
        "household.member",
        "settings",
        "person.update",
    ] {
        assert!(audit.body.contains(action), "{action} in {}", audit.body);
    }
}

#[test]
fn state_survives_reopen() {
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    let (dad, group) = {
        let app = App::open(kv.clone(), limits()).unwrap();
        let dad = setup(&app);
        signup(&app, "Sam");
        let group = new_group(&app, &dad, "Sunday dinners", "unlisted", "approval");
        call(
            &app,
            "POST",
            "/api/v1/households",
            Some(&dad),
            r#"{"name":"Grandma's"}"#,
        );
        (dad, group)
    };
    let app = App::open(kv, limits()).unwrap();
    assert_eq!(
        call(&app, "GET", "/api/v1/auth/me", Some(&dad), "").status,
        200,
        "sessions persist"
    );
    let g = call(
        &app,
        "GET",
        &format!("/api/v1/groups/{group}"),
        Some(&dad),
        "",
    );
    assert_eq!(g.json("name"), "Sunday dinners");
    assert_eq!(g.json("my_role"), "organizer");
    let people = call(&app, "GET", "/api/v1/admin/people", Some(&dad), "");
    assert!(people.body.contains("Sam"));
    let h = call(&app, "GET", "/api/v1/households", Some(&dad), "");
    assert!(h.body.contains("Grandma's"));
    let storage = call(&app, "GET", "/api/v1/admin/storage", Some(&dad), "");
    assert!(
        storage.body.contains(r#""quarantined":[]"#),
        "{}",
        storage.body
    );
}

#[test]
fn a_damaged_record_is_quarantined_and_reported() {
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    let dad = {
        let app = App::open(kv.clone(), limits()).unwrap();
        let dad = setup(&app);
        new_group(&app, &dad, "Book club", "public", "open");
        new_group(&app, &dad, "Game club", "public", "open");
        dad
    };
    kv.set("g1", b"\xff not cbor").unwrap();
    let app = App::open(kv, limits()).unwrap();
    let storage = call(&app, "GET", "/api/v1/admin/storage", Some(&dad), "");
    assert!(storage.body.contains("g1:"), "{}", storage.body);
    let groups = call(&app, "GET", "/api/v1/groups", Some(&dad), "");
    assert!(groups.body.contains("Game club") && !groups.body.contains("Book club"));
}

#[test]
fn an_unreadable_meta_record_refuses_to_start() {
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    kv.set("meta", b"\xff garbage").unwrap();
    assert!(App::open(kv, limits()).is_err());
}

/// Counts bytes reaching flash.
#[derive(Default)]
struct CountingKv {
    inner: MemKv,
    written: AtomicUsize,
}

impl Kv for CountingKv {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.get(key)
    }
    fn set(&self, key: &str, value: &[u8]) -> std::io::Result<()> {
        self.written.fetch_add(value.len(), Ordering::SeqCst);
        self.inner.set(key, value)
    }
    fn remove(&self, key: &str) -> std::io::Result<()> {
        self.inner.remove(key)
    }
}

/// A board with `people` accounts, each signed up through the API.
fn seeded(people: usize) -> (Arc<CountingKv>, App, String) {
    let kv = Arc::new(CountingKv::default());
    let app = App::open(kv.clone(), limits()).unwrap();
    setup(&app);
    for i in 1..people {
        let body = format!(r#"{{"display_name":"Person {i}","pin":"0000"}}"#);
        assert_eq!(
            call(&app, "POST", "/api/v1/auth/signup", None, &body).status,
            201
        );
    }
    // Each sign-up opened a session, so Dad's first one was evicted by the
    // board-wide cap long ago.
    let dad = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"login":"dad","pin":"1234"}"#,
    )
    .cookie
    .unwrap();
    (kv, app, dad)
}

#[test]
fn boot_with_2000_people_and_writes_stay_small() {
    let (kv, app, dad) = seeded(2_000);
    let group = new_group(&app, &dad, "Everyone", "public", "open");
    let joiner = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"login":"person 1999","pin":"0000"}"#,
    )
    .cookie
    .unwrap();
    let before = kv.written.load(Ordering::SeqCst);
    let r = call(
        &app,
        "POST",
        &format!("/api/v1/groups/{group}/join"),
        Some(&joiner),
        "",
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let join_cost = kv.written.load(Ordering::SeqCst) - before;
    let before = kv.written.load(Ordering::SeqCst);
    call(
        &app,
        "PATCH",
        "/api/v1/me",
        Some(&joiner),
        r#"{"bio":"Here for the pie"}"#,
    );
    let profile_cost = kv.written.load(Ordering::SeqCst) - before;
    assert!(
        join_cost < 2_048,
        "a join wrote {join_cost} bytes with 2,000 people on the board"
    );
    assert!(
        profile_cost < 1_024,
        "a profile edit wrote {profile_cost} bytes"
    );

    // A "reboot" reads every record back (the desktop half of the Sprint 0
    // boot measurement; the board's number goes in spec/measurements).
    drop(app);
    let started = std::time::Instant::now();
    let app = App::open(kv.clone(), limits()).unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs() < 5, "boot took {elapsed:?}");
    let storage = call(&app, "GET", "/api/v1/admin/storage", Some(&dad), "");
    assert_eq!(storage.json("people"), "2000");
    let full = call(
        &app,
        "POST",
        "/api/v1/auth/signup",
        None,
        r#"{"display_name":"One more","pin":"0000"}"#,
    );
    assert_eq!(full.status, 409);
    assert_eq!(full.json("error"), "full");
}

// ---- Sprint 2: groups -------------------------------------------------------------

#[test]
fn private_groups_never_appear_for_non_members() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "Sam");
    let secret = new_group(&app, &dad, "Surprise party planning", "private", "invite");
    let unlisted = new_group(&app, &dad, "Family", "unlisted", "approval");
    new_group(&app, &dad, "Game club", "public", "open");
    for path in [
        "/api/v1/groups",
        "/api/v1/groups?q=surprise",
        "/api/v1/groups?q=party",
        "/api/v1/groups?mine=1",
    ] {
        let r = call(&app, "GET", path, Some(&kid), "");
        assert!(!r.body.contains("Surprise"), "{path}: {}", r.body);
        assert!(
            !r.body.contains("Family"),
            "unlisted isn't listed either: {path}"
        );
    }
    let anon = call(&app, "GET", "/api/v1/groups", None, "");
    assert!(anon.body.contains("Game club") && !anon.body.contains("Family"));
    for path in [
        format!("/api/v1/groups/{secret}"),
        format!("/api/v1/groups/{secret}/members"),
        "/api/v1/groups/surprise-party-planning".into(),
    ] {
        assert_eq!(
            call(&app, "GET", &path, Some(&kid), "").status,
            404,
            "{path}"
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/groups/{secret}/join"),
            Some(&kid),
            ""
        )
        .status,
        404
    );
    // Unlisted: reachable by link, joinable by request.
    let g = call(
        &app,
        "GET",
        &format!("/api/v1/groups/{unlisted}"),
        Some(&kid),
        "",
    );
    assert_eq!(g.status, 200);
    let r = call(
        &app,
        "POST",
        &format!("/api/v1/groups/{unlisted}/join"),
        Some(&kid),
        "",
    );
    assert_eq!(r.json("my_status"), "requested");
    let mine = call(&app, "GET", "/api/v1/groups?mine=1", Some(&kid), "");
    assert!(mine.body.contains("Family"), "a request shows in my groups");
}

#[test]
fn organizers_run_their_group() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let son = signup(&app, "Son");
    let gran = signup(&app, "Grandma");
    let (son_id, gran_id) = (id_of(&app, &son), id_of(&app, &gran));
    let family = new_group(&app, &dad, "Family", "unlisted", "approval");
    let path = |rest: &str| format!("/api/v1/groups/{family}{rest}");
    // Son asks; Dad approves and makes him co-organizer.
    call(&app, "POST", &path("/join"), Some(&son), "");
    let g = call(&app, "GET", &path(""), Some(&dad), "");
    assert_eq!(g.json("requests"), "1");
    let r = call(
        &app,
        "PUT",
        &path(&format!("/members/{son_id}")),
        Some(&dad),
        r#"{"status":"active","role":"co-organizer"}"#,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    // The co-organizer adds Grandma directly, but can't make her organizer.
    let r = call(
        &app,
        "PUT",
        &path(&format!("/members/{gran_id}")),
        Some(&son),
        "{}",
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let r = call(
        &app,
        "PUT",
        &path(&format!("/members/{gran_id}")),
        Some(&son),
        r#"{"role":"organizer"}"#,
    );
    assert_eq!(r.status, 403);
    // Grandma sees the members, but can't manage.
    let members = call(&app, "GET", &path("/members"), Some(&gran), "");
    assert!(members.body.contains("Son") && members.body.contains("Dad"));
    let r = call(
        &app,
        "PATCH",
        &path(""),
        Some(&gran),
        r#"{"name":"Grandma's group"}"#,
    );
    assert_eq!(r.status, 403);
    // Edits are guarded by rev.
    let rev = call(&app, "GET", &path(""), Some(&son), "").json("rev");
    let body = format!(r#"{{"about":"Sunday dinners and birthdays","rev":{rev}}}"#);
    assert_eq!(
        call(&app, "PATCH", &path(""), Some(&son), &body).status,
        200
    );
    assert_eq!(
        call(&app, "PATCH", &path(""), Some(&son), &body).json("error"),
        "stale"
    );
    // Dad can't leave as the only organizer; he hands over first.
    assert_eq!(
        call(&app, "POST", &path("/leave"), Some(&dad), "").json("error"),
        "last_organizer"
    );
    let r = call(
        &app,
        "POST",
        &path("/transfer"),
        Some(&dad),
        &format!(r#"{{"to":{son_id}}}"#),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json("my_role"), "co-organizer");
    assert_eq!(
        call(&app, "GET", &path(""), Some(&son), "").json("can_own"),
        "true"
    );
    // The slug works as well as the id.
    assert_eq!(
        call(&app, "GET", "/api/v1/groups/family", Some(&gran), "").json("id"),
        family.to_string()
    );
}

#[test]
fn group_bounds_answer_full() {
    let app = App::open(
        Arc::new(MemKv::default()),
        Limits {
            groups: 2,
            members_per_group: 2,
            ..limits()
        },
    )
    .unwrap();
    let dad = setup(&app);
    let a = new_group(&app, &dad, "A", "public", "open");
    new_group(&app, &dad, "B", "public", "open");
    let r = call(
        &app,
        "POST",
        "/api/v1/groups",
        Some(&dad),
        r#"{"name":"C"}"#,
    );
    assert_eq!((r.status, r.json("error")), (409, "full".into()));
    let kid = signup(&app, "Sam");
    let gran = signup(&app, "Grandma");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/groups/{a}/join"),
            Some(&kid),
            ""
        )
        .status,
        200
    );
    let r = call(
        &app,
        "POST",
        &format!("/api/v1/groups/{a}/join"),
        Some(&gran),
        "",
    );
    assert_eq!((r.status, r.json("error")), (409, "full".into()));
}

#[test]
fn people_search_respects_discoverability() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "Sam");
    let stranger = signup(&app, "Stranger");
    let club = new_group(&app, &dad, "Club", "public", "open");
    call(
        &app,
        "POST",
        &format!("/api/v1/groups/{club}/join"),
        Some(&kid),
        "",
    );
    let r = call(&app, "GET", "/api/v1/people?q=sa", Some(&dad), "");
    assert!(r.body.contains("Sam"), "admins see everyone");
    let r = call(&app, "GET", "/api/v1/people", Some(&stranger), "");
    assert!(!r.body.contains("Sam"), "no shared group: {}", r.body);
    call(
        &app,
        "PATCH",
        "/api/v1/me",
        Some(&kid),
        r#"{"discoverable":"public"}"#,
    );
    let r = call(&app, "GET", "/api/v1/people", Some(&stranger), "");
    assert!(r.body.contains("Sam"));
    let sam = id_of(&app, &kid);
    call(
        &app,
        "PATCH",
        "/api/v1/me",
        Some(&kid),
        r#"{"discoverable":"nobody"}"#,
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/v1/people/{sam}"),
            Some(&stranger),
            ""
        )
        .status,
        404
    );
    // Paging: one at a time walks everyone exactly once.
    let mut after = 0;
    let mut seen = 0;
    loop {
        let r = call(
            &app,
            "GET",
            &format!("/api/v1/people?limit=1&after={after}"),
            Some(&dad),
            "",
        );
        if r.body.contains("\"people\":[]") {
            break;
        }
        seen += 1;
        let next: u32 = r.json("next").parse().unwrap();
        if next == 0 {
            break;
        }
        after = next;
    }
    assert_eq!(seen, 3);
}

// ---- Sprints 3-5: events, answers, series, calendars ------------------------------

/// Far enough ahead to always be in the future (Jan 2030).
const LATER: u64 = 1_893_456_000;

fn new_event(app: &App, cookie: &str, group: u32, body: &str) -> u32 {
    let r = call(
        app,
        "POST",
        &format!("/api/v1/groups/{group}/events"),
        Some(cookie),
        body,
    );
    assert_eq!(r.status, 201, "{}", r.body);
    r.json("id").parse().unwrap()
}

/// Sprint 3's exit walk: a potluck, three people answer with guests and
/// dishes, one drops, the waitlist moves up.
#[test]
fn potluck_walk() {
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    let (dad, event) = {
        let app = App::open(kv.clone(), limits()).unwrap();
        let dad = setup(&app);
        let family = new_group(&app, &dad, "Family", "unlisted", "open");
        let people: Vec<String> = ["Son", "Grandma", "Aunt"]
            .iter()
            .map(|n| signup(&app, n))
            .collect();
        for p in &people {
            assert_eq!(
                call(
                    &app,
                    "POST",
                    &format!("/api/v1/groups/{family}/join"),
                    Some(p),
                    ""
                )
                .status,
                200
            );
        }
        let body = format!(
            r#"{{"kind":"potluck","title":"Thanksgiving","start":{LATER},"capacity":4,
                 "bring":[{{"id":0,"name":"Pie","count":1}},{{"id":0,"name":"Salad","count":2}}]}}"#
        );
        let event = new_event(&app, &dad, family, &body);
        let path = format!("/api/v1/events/{event}");
        let r = call(&app, "GET", &path, Some(&people[0]), "");
        assert_eq!(r.json("can_answer"), "true", "{}", r.body);
        // Son +1 bringing pie, Grandma +1 bringing salad: 4 heads, full.
        let son = r#"{"status":"going","plus":1,"bring":[1],"rev_seen":1}"#;
        assert_eq!(
            call(
                &app,
                "PUT",
                &format!("{path}/rsvps/me"),
                Some(&people[0]),
                son
            )
            .status,
            200
        );
        let gran = r#"{"status":"going","plus":1,"bring":[2],"rev_seen":1}"#;
        assert_eq!(
            call(
                &app,
                "PUT",
                &format!("{path}/rsvps/me"),
                Some(&people[1]),
                gran
            )
            .status,
            200
        );
        let pie_too = r#"{"status":"going","bring":[1],"rev_seen":1}"#;
        let r = call(
            &app,
            "PUT",
            &format!("{path}/rsvps/me"),
            Some(&people[2]),
            pie_too,
        );
        assert_eq!((r.status, r.json("error")), (409, "slot_full".into()));
        let aunt = r#"{"status":"going","rev_seen":1}"#;
        let r = call(
            &app,
            "PUT",
            &format!("{path}/rsvps/me"),
            Some(&people[2]),
            aunt,
        );
        assert_eq!(r.json("waitlist"), "1", "{}", r.body);
        // Grandma's guest drops out: the aunt moves up.
        let gran = r#"{"status":"going","plus":0,"rev_seen":1}"#;
        let r = call(
            &app,
            "PUT",
            &format!("{path}/rsvps/me"),
            Some(&people[1]),
            gran,
        );
        assert_eq!(
            (r.json("heads"), r.json("waitlist")),
            ("4".into(), "0".into()),
            "{}",
            r.body
        );
        let notices = call(&app, "GET", "/api/v1/notifications", Some(&people[2]), "");
        assert!(
            notices.body.contains(r#""kind":"promoted""#),
            "{}",
            notices.body
        );
        let list = call(&app, "GET", &format!("{path}/rsvps"), Some(&dad), "");
        assert!(
            list.body.contains("Aunt") && list.body.contains("Son"),
            "{}",
            list.body
        );
        (dad, event)
    };
    // Everything survives a reboot.
    let app = App::open(kv, limits()).unwrap();
    let r = call(
        &app,
        "GET",
        &format!("/api/v1/events/{event}"),
        Some(&dad),
        "",
    );
    assert_eq!(
        (r.json("heads"), r.json("going")),
        ("4".into(), "3".into()),
        "{}",
        r.body
    );
}

#[test]
fn notifications_summary_and_calendars() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "Sam");
    let club = new_group(&app, &dad, "Club", "public", "open");
    call(
        &app,
        "POST",
        &format!("/api/v1/groups/{club}/join"),
        Some(&kid),
        "",
    );
    let event = new_event(
        &app,
        &dad,
        club,
        &format!(r#"{{"title":"Game night","start":{LATER}}}"#),
    );
    let summary = call(&app, "GET", "/api/v1/me/summary", Some(&kid), "");
    assert_eq!(
        summary.json("awaiting"),
        "1",
        "a new event waits for an answer, without a stored notice"
    );
    call(
        &app,
        "PUT",
        &format!("/api/v1/events/{event}/rsvps/me"),
        Some(&kid),
        r#"{"status":"going","rev_seen":1}"#,
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/me/summary", Some(&kid), "").json("awaiting"),
        "0"
    );
    // A change of time tells the people coming.
    let body = format!(r#"{{"start":{}}}"#, LATER + 3600);
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/api/v1/events/{event}"),
            Some(&dad),
            &body
        )
        .status,
        200
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/me/summary", Some(&kid), "").json("unread"),
        "1"
    );
    call(
        &app,
        "POST",
        "/api/v1/notifications/read",
        Some(&kid),
        r#"{"up_to":0}"#,
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/me/summary", Some(&kid), "").json("unread"),
        "0"
    );
    // Per-event download and the personal feed, with token rotation.
    let file = call(
        &app,
        "GET",
        &format!("/api/v1/events/{event}.ics"),
        Some(&kid),
        "",
    );
    assert!(
        file.body.contains("UID:e1@party.local") && file.body.contains("SUMMARY:Game night"),
        "{}",
        file.body
    );
    let path = call(&app, "POST", "/api/v1/me/calendar", Some(&kid), "").json("path");
    let feed = call_from(&app, "GET", &path, None, "", None);
    assert_eq!(feed.status, 200);
    assert!(
        feed.body.contains("BEGIN:VEVENT") && feed.body.contains("STATUS:CONFIRMED"),
        "{}",
        feed.body
    );
    let path2 = call(&app, "POST", "/api/v1/me/calendar", Some(&kid), "").json("path");
    assert_ne!(path, path2);
    assert_eq!(
        call_from(&app, "GET", &path, None, "", None).status,
        404,
        "a rotated token stops working"
    );
    // Cancelling marks it cancelled in the feed.
    call(
        &app,
        "POST",
        &format!("/api/v1/events/{event}/cancel"),
        Some(&dad),
        "",
    );
    let feed = call_from(&app, "GET", &path2, None, "", None);
    assert!(feed.body.contains("STATUS:CANCELLED"), "{}", feed.body);
}

#[test]
fn polls_series_and_permissions_over_http() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "Sam");
    let stranger = signup(&app, "Stranger");
    let family = new_group(&app, &dad, "Family", "private", "invite");
    let kid_id = id_of(&app, &kid);
    call(
        &app,
        "PUT",
        &format!("/api/v1/groups/{family}/members/{kid_id}"),
        Some(&dad),
        "{}",
    );
    let body = format!(
        r#"{{"kind":"poll","title":"When is the reunion?","when":[{{"id":0,"start":{LATER},"end":0}},{{"id":0,"start":{},"end":0}}]}}"#,
        LATER + 86_400
    );
    let poll = new_event(&app, &dad, family, &body);
    let path = format!("/api/v1/events/{poll}");
    assert_eq!(
        call(&app, "GET", &path, Some(&stranger), "").status,
        404,
        "private group: invisible"
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("{path}/rsvps/me"),
            Some(&stranger),
            r#"{"status":"going","rev_seen":1}"#
        )
        .status,
        404
    );
    call(
        &app,
        "PUT",
        &format!("{path}/rsvps/me"),
        Some(&kid),
        r#"{"status":"maybe","when":[2],"rev_seen":1}"#,
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/lock"),
            Some(&kid),
            r#"{"option":2}"#
        )
        .status,
        403
    );
    let r = call(
        &app,
        "POST",
        &format!("{path}/lock"),
        Some(&dad),
        r#"{"option":2}"#,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains(&format!(r#""start":{}"#, LATER + 86_400)),
        "{}",
        r.body
    );
    let mine = call(&app, "GET", &path, Some(&kid), "");
    assert!(
        mine.body.contains(r#""status":"going""#),
        "the winning time's maybes are going: {}",
        mine.body
    );
    // A weekly series makes its window of occurrences.
    let body = format!(
        r#"{{"event":{{"title":"Game club","start":{LATER}}},"rule":"weekly","window":3}}"#
    );
    let r = call(
        &app,
        "POST",
        &format!("/api/v1/groups/{family}/series"),
        Some(&dad),
        &body,
    );
    assert_eq!(r.status, 201, "{}", r.body);
    let series: u32 = r.json("id").parse().unwrap();
    let events = call(
        &app,
        "GET",
        &format!("/api/v1/events?group={family}"),
        Some(&kid),
        "",
    );
    assert_eq!(
        events.body.matches("Game club").count(),
        3,
        "{}",
        events.body
    );
    let r = call(
        &app,
        "PUT",
        &format!("/api/v1/series/{series}/regular"),
        Some(&kid),
        r#"{"on":true}"#,
    );
    assert_eq!(r.json("regular"), "true");
    let mine = call(
        &app,
        "GET",
        &format!("/api/v1/events?group={family}&mine=1"),
        Some(&kid),
        "",
    );
    assert_eq!(
        mine.body.matches(r#""my_status":"going""#).count(),
        4,
        "{}",
        mine.body
    );
}

#[test]
fn an_answer_stays_small_however_many_answered() {
    let (kv, app, dad) = seeded(200);
    let everyone = new_group(&app, &dad, "Everyone", "public", "open");
    let event = new_event(
        &app,
        &dad,
        everyone,
        &format!(r#"{{"title":"Block party","start":{LATER}}}"#),
    );
    let mut last = String::new();
    for i in 1..200 {
        let body = format!(r#"{{"login":"person {i}","pin":"0000"}}"#);
        last = call(&app, "POST", "/api/v1/auth/pin", None, &body)
            .cookie
            .unwrap();
        call(
            &app,
            "POST",
            &format!("/api/v1/groups/{everyone}/join"),
            Some(&last),
            "",
        );
        let r = call(
            &app,
            "PUT",
            &format!("/api/v1/events/{event}/rsvps/me"),
            Some(&last),
            r#"{"status":"going","rev_seen":1}"#,
        );
        assert_eq!(r.status, 200, "{}", r.body);
    }
    let before = kv.written.load(Ordering::SeqCst);
    call(
        &app,
        "PUT",
        &format!("/api/v1/events/{event}/rsvps/me"),
        Some(&last),
        r#"{"status":"maybe","rev_seen":1}"#,
    );
    let cost = kv.written.load(Ordering::SeqCst) - before;
    assert!(cost < 4_096, "an answer with 199 others wrote {cost} bytes");
}

// ---- Sprints 6-8: invitations, channels, publication settings -------------------------

#[test]
fn invitations_over_http() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let family = new_group(&app, &dad, "Family", "private", "invite");
    call(
        &app,
        "PATCH",
        "/api/v1/admin/settings",
        Some(&dad),
        r#"{"signup_open":false}"#,
    );
    let body = format!(r#"{{"target_kind":"group","target":{family},"uses":2}}"#);
    let r = call(&app, "POST", "/api/v1/invitations", Some(&dad), &body);
    assert_eq!(r.status, 201, "{}", r.body);
    let token = r.json("token");
    assert!(r.json("link").ends_with(&format!("/invite/{token}")));
    // Anyone with the link sees what it's for.
    let preview = call(
        &app,
        "GET",
        &format!("/api/v1/invitations/{token}"),
        None,
        "",
    );
    assert_eq!(
        (preview.json("valid"), preview.json("target_name")),
        ("true".into(), "Family".into())
    );
    // Sign-up is closed, but the invitation lets them in, straight into the group.
    let body = format!(r#"{{"display_name":"Aunt","pin":"0000","invite":"{token}"}}"#);
    let r = call(&app, "POST", "/api/v1/auth/signup", None, &body);
    assert_eq!(r.status, 201, "{}", r.body);
    let aunt = r.cookie.unwrap();
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/v1/groups/{family}"),
            Some(&aunt),
            ""
        )
        .json("my_status"),
        "active"
    );
    // A signed-in person redeems it too; then it's used up.
    let cousin = signup_closed_board(&app, &dad, "Cousin");
    let r = call(
        &app,
        "POST",
        &format!("/api/v1/invitations/{token}/redeem"),
        Some(&cousin),
        "",
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let late = signup_closed_board(&app, &dad, "Latecomer");
    let r = call(
        &app,
        "POST",
        &format!("/api/v1/invitations/{token}/redeem"),
        Some(&late),
        "",
    );
    assert_eq!((r.status, r.json("error")), (410, "invite_closed".into()));
    let list = call(
        &app,
        "GET",
        &format!("/api/v1/invitations?kind=group&target={family}"),
        Some(&dad),
        "",
    );
    assert!(
        list.body.contains(r#""status":"used""#) && !list.body.contains(&token),
        "tokens aren't listed: {}",
        list.body
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/v1/invitations?kind=group&target={family}"),
            Some(&aunt),
            ""
        )
        .status,
        403
    );
}

/// A person on a board with sign-up closed: the admin adds them and sets a PIN.
fn signup_closed_board(app: &App, admin: &str, name: &str) -> String {
    let r = call(
        app,
        "POST",
        "/api/v1/people",
        Some(admin),
        &format!(r#"{{"display_name":"{name}","email":"","household":0}}"#),
    );
    assert_eq!(r.status, 201, "{}", r.body);
    let id: u32 = r.json("id").parse().unwrap();
    let r = call(
        app,
        "PATCH",
        &format!("/api/v1/admin/people/{id}"),
        Some(admin),
        r#"{"pin":"2468"}"#,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let r = call(
        app,
        "POST",
        "/api/v1/auth/pin",
        None,
        &format!(r#"{{"login":"{name}","pin":"2468"}}"#),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    r.cookie.unwrap()
}

#[test]
fn people_without_a_pin_and_who_may_add_them() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "Sam");
    let body = r#"{"display_name":"Grandma","email":"gran@example.com","household":0}"#;
    assert_eq!(
        call(&app, "POST", "/api/v1/people", Some(&kid), body).status,
        403,
        "only admins and organizers"
    );
    let r = call(&app, "POST", "/api/v1/people", Some(&dad), body);
    assert_eq!(r.status, 201, "{}", r.body);
    let people = call(&app, "GET", "/api/v1/admin/people", Some(&dad), "");
    assert!(
        people.body.contains(r#""status":"invited""#),
        "{}",
        people.body
    );
    // Without a PIN they can't sign in.
    let r = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"login":"grandma","pin":"0000"}"#,
    );
    assert_eq!(r.status, 401);
}

#[test]
fn channel_settings_keep_their_secrets() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "Sam");
    let r = call(
        &app,
        "PUT",
        "/api/v1/admin/mail",
        Some(&dad),
        r#"{"api_key":"xsmtpsib-nope","sender_email":"","sender_name":""}"#,
    );
    assert_eq!(r.status, 400, "an SMTP key is refused with a hint");
    let r = call(
        &app,
        "PUT",
        "/api/v1/admin/mail",
        Some(&dad),
        r#"{"api_key":"xkeysib-secret-9876","sender_email":"","sender_name":""}"#,
    );
    assert_eq!(r.json("state"), "checking");
    let view = call(&app, "GET", "/api/v1/admin/mail", Some(&dad), "");
    assert_eq!(view.json("key_hint"), "…9876");
    assert!(!view.body.contains("secret"), "{}", view.body);
    let r = call(
        &app,
        "PUT",
        "/api/v1/admin/bot",
        Some(&dad),
        r#"{"instance":"mastodon.social","token":"bot-token-abcd","trust":"public","commands":true,"poll_secs":5}"#,
    );
    assert_eq!(
        (r.json("instance"), r.json("poll_secs")),
        ("https://mastodon.social".into(), "30".into())
    );
    assert!(!r.body.contains("bot-token"));
    let r = call(
        &app,
        "PUT",
        "/api/v1/admin/publish",
        Some(&dad),
        r#"{"repo":"ourparty/party-data","branch":"","token":"github_pat_xyz1","site_url":"https://ourparty.github.io/party-site/","every_minutes":1}"#,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(
        (r.json("branch"), r.json("every_minutes")),
        ("main".into(), "10".into())
    );
    assert!(!r.body.contains("github_pat"));
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/v1/admin/publish",
            Some(&dad),
            r#"{"repo":"nope","branch":"","token":"","site_url":"","every_minutes":10}"#
        )
        .status,
        400
    );
    for path in [
        "/api/v1/admin/mail",
        "/api/v1/admin/bot",
        "/api/v1/admin/publish",
        "/api/v1/admin/delivery",
        "/api/v1/admin/inbox",
        "/api/v1/admin/providers",
    ] {
        assert_eq!(
            call(&app, "GET", path, Some(&kid), "").status,
            403,
            "{path}"
        );
    }
    let sync = call(&app, "GET", "/api/v1/sync/status", None, "");
    assert_eq!(
        sync.json("site_url"),
        "https://ourparty.github.io/party-site/"
    );
    // The personal link appears once publication knows the site.
    let me = call(&app, "POST", "/api/v1/me/link", Some(&kid), "");
    assert!(
        me.json("personal_link")
            .starts_with("https://ourparty.github.io/party-site/#/k/2."),
        "{}",
        me.body
    );
    let again = call(&app, "POST", "/api/v1/me/link?rotate=1", Some(&kid), "");
    assert_ne!(
        again.json("personal_link"),
        me.json("personal_link"),
        "rotating makes a new key"
    );
}

#[test]
fn sign_in_providers_and_pin_reset_need_their_setup() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let r = call(
        &app,
        "POST",
        "/api/v1/admin/providers",
        Some(&dad),
        r#"{"name":"","base":"Mastodon.Social","trust":"public"}"#,
    );
    assert_eq!(r.status, 201, "{}", r.body);
    assert!(
        r.body.contains(r#""base":"https://mastodon.social""#),
        "{}",
        r.body
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/admin/providers",
            Some(&dad),
            r#"{"name":"","base":"mastodon.social","trust":"public"}"#
        )
        .status,
        409
    );
    // Without the OAuth client attached, sign-in says so plainly.
    let r = call(
        &app,
        "POST",
        "/api/v1/auth/oauth/start",
        None,
        r#"{"provider":1,"purpose":"signin","token":""}"#,
    );
    assert_eq!(r.status, 503, "{}", r.body);
    // No email set up: "forgot PIN" points to the admin.
    let r = call(
        &app,
        "POST",
        "/api/v1/auth/pin-reset",
        None,
        r#"{"login":"dad"}"#,
    );
    assert_eq!(r.status, 503);
}
