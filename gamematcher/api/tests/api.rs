//! The Sprint 1 API end to end, through `Service::handle` with real
//! requests: setup, sign-up, PIN sign-in, sessions, admin, persistence.
use gamematcher::pin::GuardLimits;
use gamematcher::{App, Limits};
use miniframework::kv::{Kv, MemKv};
use miniframework::{Reply, Request, Service};
use std::sync::Arc;

const ORIGIN: &str = "http://gamematcher.local";

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
        ("Host".to_string(), "gamematcher.local".to_string()),
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
    let mut reply = Reply::new(&mut out, &mut spare, 64 * 1024);
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
        r#"{"email":"dad@home","display_name":"Dad","pin":"1234"}"#,
    );
    assert_eq!(r.status, 201, "{}", r.body);
    r.cookie.expect("setup signs in")
}

fn signup(app: &App, email: &str, name: &str) -> String {
    let body = format!(r#"{{"email":"{email}","display_name":"{name}","pin":"0000"}}"#);
    let r = call(app, "POST", "/api/v1/auth/signup", None, &body);
    assert_eq!(r.status, 201, "{}", r.body);
    r.cookie.unwrap()
}

#[test]
fn first_run_setup_then_status() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let r = call(&app, "GET", "/api/v1/status", None, "");
    assert_eq!(r.json("setup_needed"), "true");
    assert_eq!(r.json("signup_open"), "true", "open sign-up is the default");
    let cookie = setup(&app);
    let r = call(&app, "GET", "/api/v1/me", Some(&cookie), "");
    assert_eq!(r.status, 200);
    assert_eq!(r.json("role"), "admin");
    assert_eq!(r.json("email"), "dad@home");
    assert_eq!(
        call(&app, "GET", "/api/v1/status", None, "").json("setup_needed"),
        "false"
    );
    let again = call(
        &app,
        "POST",
        "/api/v1/setup",
        None,
        r#"{"email":"kid@home","display_name":"Kid","pin":"1234"}"#,
    );
    assert_eq!(again.status, 409);
}

#[test]
fn concurrent_setup_has_one_winner() {
    let app = Arc::new(App::open(Arc::new(MemKv::default()), limits()).unwrap());
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let app = app.clone();
            std::thread::spawn(move || {
                let body = format!(r#"{{"email":"p{i}@home","display_name":"P{i}","pin":"1234"}}"#);
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
fn cross_origin_and_originless_writes_are_refused() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let body = r#"{"email":"dad@home","display_name":"Dad","pin":"1234"}"#;
    let evil = call_from(
        &app,
        "POST",
        "/api/v1/setup",
        None,
        body,
        Some("http://evil.example"),
    );
    assert_eq!(evil.status, 403);
    let none = call_from(&app, "POST", "/api/v1/setup", None, body, None);
    assert_eq!(none.status, 403);
    // GETs need no Origin.
    assert_eq!(
        call_from(&app, "GET", "/api/v1/status", None, "", None).status,
        200
    );
}

#[test]
fn pin_sign_in_and_lockout() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    setup(&app);
    let wrong = r#"{"email":"dad@home","pin":"9999"}"#;
    for _ in 0..4 {
        assert_eq!(
            call(&app, "POST", "/api/v1/auth/pin", None, wrong).status,
            401
        );
    }
    let fifth = call(&app, "POST", "/api/v1/auth/pin", None, wrong);
    assert_eq!(fifth.status, 401);
    // Now locked, even with the right PIN.
    let right = r#"{"email":"DAD@home ","pin":"1234"}"#;
    let locked = call(&app, "POST", "/api/v1/auth/pin", None, right);
    assert_eq!(locked.status, 429, "{}", locked.body);
    assert_eq!(locked.retry_after.as_deref(), Some("60"));
    // Unknown emails say the same thing as wrong PINs.
    let unknown = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"email":"who@x","pin":"1234"}"#,
    );
    assert_eq!(unknown.status, 401);
    assert_eq!(unknown.json("error"), "wrong_pin");
}

#[test]
fn board_wide_budget_applies() {
    let guard = GuardLimits {
        board_failures: 3,
        ..GuardLimits::default()
    };
    let app = App::open_with(Arc::new(MemKv::default()), limits(), guard).unwrap();
    setup(&app);
    for who in ["a@x", "b@x", "c@x"] {
        let body = format!(r#"{{"email":"{who}","pin":"1234"}}"#);
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
        r#"{"email":"dad@home","pin":"1234"}"#,
    );
    assert_eq!(r.status, 429);
}

#[test]
fn sessions_logout_and_sign_out_everywhere() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let first = setup(&app);
    let second = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"email":"dad@home","pin":"1234"}"#,
    )
    .cookie
    .unwrap();
    assert_eq!(
        call(&app, "GET", "/api/v1/me", Some(&second), "").json("sessions"),
        "2"
    );
    let out = call(&app, "POST", "/api/v1/auth/logout", Some(&second), "");
    assert_eq!(out.status, 200);
    assert_eq!(out.cookie.as_deref(), Some("gm_session="));
    assert_eq!(
        call(&app, "GET", "/api/v1/me", Some(&second), "").status,
        401
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/me", Some(&first), "").status,
        200
    );
    call(&app, "POST", "/api/v1/auth/logout?all=1", Some(&first), "");
    assert_eq!(
        call(&app, "GET", "/api/v1/me", Some(&first), "").status,
        401
    );
}

#[test]
fn members_cannot_reach_admin() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    setup(&app);
    let kid = signup(&app, "kid@home", "Sam");
    assert_eq!(
        call(&app, "GET", "/api/v1/admin/users", Some(&kid), "").status,
        403
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            "/api/v1/admin/users/1",
            Some(&kid),
            r#"{"role":"member"}"#
        )
        .status,
        403
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            "/api/v1/admin/settings",
            Some(&kid),
            r#"{"signup_open":false}"#
        )
        .status,
        403
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/admin/users", None, "").status,
        401
    );
    // Members can see who else is here, without emails.
    let users = call(&app, "GET", "/api/v1/users", Some(&kid), "");
    assert_eq!(users.status, 200);
    assert!(!users.body.contains("dad@home"));
}

#[test]
fn admin_manages_accounts() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "kid@home", "Sam");
    let users = call(&app, "GET", "/api/v1/admin/users", Some(&dad), "");
    assert!(users.body.contains("kid@home"));

    // Reset the kid's PIN; the new one works.
    let r = call(
        &app,
        "PATCH",
        "/api/v1/admin/users/2",
        Some(&dad),
        r#"{"pin":"2468"}"#,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let r = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"email":"kid@home","pin":"2468"}"#,
    );
    assert_eq!(r.status, 200);

    // Disabling signs the kid out and blocks sign-in.
    call(
        &app,
        "PATCH",
        "/api/v1/admin/users/2",
        Some(&dad),
        r#"{"disabled":true}"#,
    );
    assert_eq!(call(&app, "GET", "/api/v1/me", Some(&kid), "").status, 401);
    let r = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"email":"kid@home","pin":"2468"}"#,
    );
    assert_eq!(r.status, 403);

    // The only admin can't demote themself.
    let r = call(
        &app,
        "PATCH",
        "/api/v1/admin/users/1",
        Some(&dad),
        r#"{"role":"member"}"#,
    );
    assert_eq!(r.status, 409);
    assert_eq!(
        call(&app, "GET", "/api/v1/me", Some(&dad), "").json("role"),
        "admin"
    );

    // Close sign-up.
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
        r#"{"email":"g@x","display_name":"G","pin":"1111"}"#,
    );
    assert_eq!(r.status, 403);
}

#[test]
fn pin_change_needs_current() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let r = call(
        &app,
        "PUT",
        "/api/v1/me/pin",
        Some(&dad),
        r#"{"current":"0000","new_pin":"5678"}"#,
    );
    assert_eq!(r.status, 403);
    let r = call(
        &app,
        "PUT",
        "/api/v1/me/pin",
        Some(&dad),
        r#"{"current":"1234","new_pin":"5678"}"#,
    );
    assert_eq!(r.status, 200);
    let r = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"email":"dad@home","pin":"5678"}"#,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn state_survives_reopen() {
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    let cookie = {
        let app = App::open(kv.clone(), limits()).unwrap();
        let dad = setup(&app);
        signup(&app, "kid@home", "Sam");
        call(
            &app,
            "PATCH",
            "/api/v1/admin/settings",
            Some(&dad),
            r#"{"signup_open":false}"#,
        );
        dad
    };
    // A "reboot": same storage, new process.
    let app = App::open(kv.clone(), limits()).unwrap();
    let me = call(&app, "GET", "/api/v1/me", Some(&cookie), "");
    assert_eq!(me.status, 200, "sessions persist");
    assert_eq!(
        call(&app, "GET", "/api/v1/status", None, "").json("signup_open"),
        "false"
    );
    let users = call(&app, "GET", "/api/v1/admin/users", Some(&cookie), "");
    assert!(users.body.contains("kid@home"));
    let storage = call(&app, "GET", "/api/v1/admin/storage", Some(&cookie), "");
    assert_eq!(storage.json("fallback_used"), "false");
}

#[test]
fn torn_write_recovers_previous_generation() {
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    let dad = {
        let app = App::open(kv.clone(), limits()).unwrap();
        let dad = setup(&app);
        signup(&app, "kid@home", "Sam");
        dad
    };
    // Tear the newest slot (whichever has the higher generation).
    let generation = |k: &str| {
        kv.get(k)
            .map(|b| u64::from_le_bytes(b[8..16].try_into().unwrap()))
    };
    let newest = if generation("snap0") > generation("snap1") {
        "snap0"
    } else {
        "snap1"
    };
    let mut bytes = kv.get(newest).unwrap();
    bytes.truncate(bytes.len() / 2);
    kv.set(newest, &bytes).unwrap();
    let app = App::open(kv, limits()).unwrap();
    let storage = call(&app, "GET", "/api/v1/admin/storage", Some(&dad), "");
    assert_eq!(storage.status, 200, "{}", storage.body);
    assert_eq!(storage.json("fallback_used"), "true");
    // The kid's sign-up was the lost change.
    let users = call(&app, "GET", "/api/v1/admin/users", Some(&dad), "");
    assert!(!users.body.contains("kid@home"));
}

#[test]
fn unreadable_state_refuses_to_start() {
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    kv.set("snap0", b"garbage").unwrap();
    kv.set("snap1", b"more garbage").unwrap();
    assert!(App::open(kv, limits()).is_err());
}

// ---- Sprint 2: games ---------------------------------------------------------

fn add_game(app: &App, cookie: &str, title: &str) -> u32 {
    let body = format!(r#"{{"title":"{title}","kind":"board","min_players":2,"max_players":4}}"#);
    let r = call(app, "POST", "/api/v1/games", Some(cookie), &body);
    assert_eq!(r.status, 201, "{}", r.body);
    r.json("id").parse().unwrap()
}

#[test]
fn catalog_add_dedupe_interest() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "kid@home", "Sam");
    let catan = add_game(&app, &kid, "Catan");
    let dup = call(
        &app,
        "POST",
        "/api/v1/games",
        Some(&dad),
        r#"{"title":" CATAN! "}"#,
    );
    assert_eq!(dup.status, 409);
    assert_eq!(dup.json("error"), "duplicate_title");
    let r = call(
        &app,
        "PUT",
        &format!("/api/v1/games/{catan}/interest"),
        Some(&dad),
        "",
    );
    assert_eq!(r.json("interested"), "1");
    assert_eq!(r.json("mine"), "true");
    assert_eq!(r.json("editable"), "true", "admins may edit anything");
    let detail = call(
        &app,
        "GET",
        &format!("/api/v1/games/{catan}"),
        Some(&kid),
        "",
    );
    assert!(
        detail.body.contains(r#""display_name":"Dad""#),
        "{}",
        detail.body
    );
    // Signed out sees nothing.
    assert_eq!(call(&app, "GET", "/api/v1/games", None, "").status, 401);
    // A member can't edit someone else's game.
    let mom = signup(&app, "mom@home", "Mom");
    let r = call(
        &app,
        "PATCH",
        &format!("/api/v1/games/{catan}"),
        Some(&mom),
        r#"{"notes":"x"}"#,
    );
    assert_eq!(r.status, 403);
    let r = call(
        &app,
        "PATCH",
        &format!("/api/v1/games/{catan}"),
        Some(&kid),
        r#"{"notes":"needs the expansion"}"#,
    );
    assert_eq!(r.json("notes"), "needs the expansion");
}

#[test]
fn admin_merge_and_delete() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "kid@home", "Sam");
    let a = add_game(&app, &kid, "Settlers");
    let b = add_game(&app, &kid, "Catan");
    call(
        &app,
        "PUT",
        &format!("/api/v1/games/{a}/interest"),
        Some(&kid),
        "",
    );
    let body = format!(r#"{{"into":{b}}}"#);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/admin/games/{a}/merge"),
            Some(&kid),
            &body
        )
        .status,
        403
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/admin/games/{a}/merge"),
            Some(&dad),
            &body
        )
        .status,
        200
    );
    assert_eq!(
        call(&app, "GET", &format!("/api/v1/games/{b}"), Some(&kid), "").json("interested"),
        "1"
    );
    assert_eq!(
        call(&app, "GET", &format!("/api/v1/games/{a}"), Some(&kid), "").status,
        404
    );
    let r = call(
        &app,
        "DELETE",
        &format!("/api/v1/admin/games/{b}"),
        Some(&dad),
        "",
    );
    assert_eq!(r.json("ok"), "true", "unused game really deleted");
}

// ---- Sprint 3: plans ---------------------------------------------------------

const SAT: u64 = 1_793_000_000;

#[test]
fn plan_end_to_end() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "kid@home", "Sam");
    let mom = signup(&app, "mom@home", "Mom");
    let catan = add_game(&app, &dad, "Catan");
    let wingspan = add_game(&app, &dad, "Wingspan");

    let body = format!(
        r#"{{"title":"Game night","games":[{catan},{wingspan}],"times":[{{"id":0,"start":{s},"end":{e}}},{{"id":0,"start":{s2},"end":0}}]}}"#,
        s = SAT,
        e = SAT + 4 * 3600,
        s2 = SAT + 86_400
    );
    let r = call(&app, "POST", "/api/v1/plans", Some(&kid), &body);
    assert_eq!(r.status, 201, "{}", r.body);
    let plan: u32 = r.json("id").parse().unwrap();
    assert_eq!(r.json("status"), "open");
    assert_eq!(r.json("manageable"), "true");
    let plan_path = format!("/api/v1/plans/{plan}");
    // Time ids from the view: "times":[{"id":N,...
    let t1: u32 = r
        .body
        .split(r#""times":[{"id":"#)
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .parse()
        .unwrap();

    let answer = format!(
        r#"{{"overall":"in","times":[{{"time":{t1},"answer":"yes"}}],"games":[{catan}],"note":"bringing snacks"}}"#
    );
    let r = call(
        &app,
        "PUT",
        &format!("{plan_path}/response"),
        Some(&dad),
        &answer,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains(r#""yes":[1]"#), "{}", r.body);
    assert!(r.body.contains(r#""ticks":[1]"#));
    assert_eq!(r.json("manageable"), "true", "dad is admin");

    // A stale answer (unknown time) is refused.
    let stale = r#"{"overall":"in","times":[{"time":999,"answer":"yes"}],"games":[],"note":""}"#;
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("{plan_path}/response"),
            Some(&mom),
            stale
        )
        .json("error"),
        "plan_changed"
    );

    // Lists: mom hasn't answered; dad has.
    let mine = call(&app, "GET", "/api/v1/plans", Some(&mom), "");
    assert_eq!(mine.json("answered"), "false");
    assert_eq!(mine.json("in_count"), "1");
    assert_eq!(
        call(&app, "GET", "/api/v1/plans", Some(&dad), "").json("answered"),
        "true"
    );

    // Only the host (or an admin) locks in.
    let lock = format!(r#"{{"time":{t1},"games":[{catan}]}}"#);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{plan_path}/lock"),
            Some(&mom),
            &lock
        )
        .status,
        403
    );
    let r = call(
        &app,
        "POST",
        &format!("{plan_path}/lock"),
        Some(&kid),
        &lock,
    );
    assert_eq!(r.json("status"), "on");
    assert_eq!(r.json("chosen_time"), t1.to_string());

    // Hand over, then cancel; past plans are listed separately.
    let r = call(
        &app,
        "POST",
        &format!("{plan_path}/host"),
        Some(&kid),
        r#"{"user":3}"#,
    );
    assert_eq!(r.json("host_name"), "Mom");
    assert_eq!(
        call(&app, "POST", &format!("{plan_path}/cancel"), Some(&kid), "").status,
        403
    );
    assert_eq!(
        call(&app, "POST", &format!("{plan_path}/cancel"), Some(&mom), "").json("status"),
        "cancelled"
    );
    assert!(!call(&app, "GET", "/api/v1/plans", Some(&mom), "")
        .body
        .contains("Game night"));
    let past = call(
        &app,
        "GET",
        "/api/v1/plans?status=done,cancelled",
        Some(&mom),
        "",
    );
    assert!(past.body.contains("Game night"));
    assert_eq!(
        call(&app, "GET", "/api/v1/plans?status=bogus", Some(&mom), "").status,
        400
    );
}

#[test]
fn plans_survive_reopen() {
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    let (dad, plan) = {
        let app = App::open(kv.clone(), limits()).unwrap();
        let dad = setup(&app);
        let g = add_game(&app, &dad, "Catan");
        let r = call(
            &app,
            "POST",
            "/api/v1/plans",
            Some(&dad),
            &format!(r#"{{"games":[{g}],"times":[]}}"#),
        );
        (dad, r.json("id"))
    };
    let app = App::open(kv, limits()).unwrap();
    let r = call(
        &app,
        "GET",
        &format!("/api/v1/plans/{plan}"),
        Some(&dad),
        "",
    );
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Catan"));
}

// ---- Sprint 4: comments and notifications ----------------------------------

fn new_plan(app: &App, cookie: &str, game: u32) -> u32 {
    let body = format!(r#"{{"games":[{game}],"times":[{{"id":0,"start":{SAT},"end":0}}]}}"#);
    let r = call(app, "POST", "/api/v1/plans", Some(cookie), &body);
    assert_eq!(r.status, 201, "{}", r.body);
    r.json("id").parse().unwrap()
}

#[test]
fn comments_thread_and_permissions() {
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    let app = App::open(kv.clone(), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "kid@home", "Sam");
    let mom = signup(&app, "mom@home", "Mom");
    let g = add_game(&app, &dad, "Catan");
    let p = new_plan(&app, &kid, g);
    let path = format!("/api/v1/plans/{p}/comments");
    let r = call(
        &app,
        "POST",
        &path,
        Some(&mom),
        r#"{"text":"bringing snacks"}"#,
    );
    assert_eq!(r.status, 201, "{}", r.body);
    assert_eq!(r.json("author_name"), "Mom");
    assert_eq!(r.json("open"), "true");
    assert!(
        kv.get(&format!("c{p}")).is_some(),
        "the thread is its own blob"
    );
    let id = r.json("id");
    let r = call(&app, "GET", &path, Some(&kid), "");
    assert_eq!(r.json("mine"), "false");
    assert_eq!(r.json("deletable"), "false");
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("{path}/{id}"),
            Some(&kid),
            r#"{"text":"x"}"#
        )
        .status,
        403
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("{path}/{id}"),
            Some(&mom),
            r#"{"text":"bringing chips"}"#
        )
        .status,
        200
    );
    assert!(call(&app, "GET", &path, Some(&kid), "")
        .body
        .contains("bringing chips"));
    assert_eq!(
        call(&app, "DELETE", &format!("{path}/{id}"), Some(&dad), "").status,
        200,
        "admins moderate"
    );
    assert!(
        kv.get(&format!("c{p}")).is_none(),
        "an empty thread leaves no blob"
    );
    // Paging: 30 comments come back 25 + 5.
    for n in 0..30 {
        call(
            &app,
            "POST",
            &path,
            Some(&mom),
            &format!(r#"{{"text":"comment {n}"}}"#),
        );
    }
    let first = call(&app, "GET", &path, Some(&kid), "");
    assert_eq!(first.json("more"), "true");
    assert_eq!(first.body.matches("\"id\":").count(), 25);
}

#[test]
fn notifications_summary_and_read() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "kid@home", "Sam");
    let g = add_game(&app, &dad, "Catan");
    call(
        &app,
        "PUT",
        &format!("/api/v1/games/{g}/interest"),
        Some(&dad),
        "",
    );
    let p = new_plan(&app, &kid, g);
    let summary = call(&app, "GET", "/api/v1/me/summary", Some(&dad), "");
    assert_eq!(summary.json("unread"), "1");
    assert_eq!(summary.json("awaiting"), "1");
    let list = call(&app, "GET", "/api/v1/notifications", Some(&dad), "");
    assert_eq!(list.json("kind"), "new_plan");
    assert_eq!(list.json("actor_name"), "Sam");
    assert_eq!(list.json("plan"), p.to_string());
    call(
        &app,
        "POST",
        &format!("/api/v1/plans/{p}/comments"),
        Some(&kid),
        r#"{"text":"one"}"#,
    );
    call(
        &app,
        "POST",
        &format!("/api/v1/plans/{p}/comments"),
        Some(&kid),
        r#"{"text":"two"}"#,
    );
    // Dad hasn't answered, so comments don't reach him; the host's own don't either.
    assert_eq!(
        call(&app, "GET", "/api/v1/me/summary", Some(&dad), "").json("unread"),
        "1"
    );
    let answer = r#"{"overall":"in","times":[],"games":[],"note":""}"#;
    call(
        &app,
        "PUT",
        &format!("/api/v1/plans/{p}/response"),
        Some(&dad),
        answer,
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/me/summary", Some(&kid), "").json("unread"),
        "1"
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/notifications/read",
            Some(&dad),
            r#"{"up_to":0}"#
        )
        .status,
        200
    );
    let summary = call(&app, "GET", "/api/v1/me/summary", Some(&dad), "");
    assert_eq!(summary.json("unread"), "0");
    assert_eq!(summary.json("awaiting"), "0");
}

// ---- Sprint 5: calendar ------------------------------------------------------

#[test]
fn calendar_downloads_and_feeds() {
    let app = App::open(Arc::new(MemKv::default()), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "kid@home", "Sam");
    let g = add_game(&app, &dad, "Catan");
    let p = new_plan(&app, &kid, g);
    let file = call(
        &app,
        "GET",
        &format!("/api/v1/plans/{p}/calendar.ics"),
        Some(&dad),
        "",
    );
    assert_eq!(file.status, 200);
    assert!(file.body.contains("STATUS:TENTATIVE"), "{}", file.body);
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/v1/plans/{p}/calendar.ics"),
            None,
            ""
        )
        .status,
        401
    );

    // A personal feed: the URL is shown once; the token is the credential.
    let r = call(&app, "POST", "/api/v1/me/feed", Some(&kid), "");
    let url = r.json("url");
    assert!(
        url.starts_with("http://gamematcher.local/cal/") && url.ends_with(".ics"),
        "{url}"
    );
    assert!(r.json("webcal").starts_with("webcal://"));
    let feed_path = &url["http://gamematcher.local".len()..];
    let feed = call_from(&app, "GET", feed_path, None, "", None);
    assert_eq!(feed.status, 200);
    assert!(
        !feed.body.contains("BEGIN:VEVENT"),
        "open plans aren't in feeds"
    );
    let t: u32 = call(&app, "GET", &format!("/api/v1/plans/{p}"), Some(&kid), "")
        .body
        .split(r#""times":[{"id":"#)
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    call(
        &app,
        "POST",
        &format!("/api/v1/plans/{p}/lock"),
        Some(&kid),
        &format!(r#"{{"time":{t},"games":[]}}"#),
    );
    let feed = call_from(&app, "GET", feed_path, None, "", None);
    assert!(
        feed.body
            .contains(&format!("UID:plan-{p}@gamematcher.local")),
        "{}",
        feed.body
    );
    // Rotating kills the old URL.
    call(&app, "POST", "/api/v1/me/feed", Some(&kid), "");
    assert_eq!(
        call_from(&app, "GET", feed_path, None, "", None).status,
        404
    );
    assert_eq!(
        call_from(&app, "GET", "/cal/nope.ics", None, "", None).status,
        404
    );

    // The household feed is admin-only and off by default.
    assert_eq!(
        call(&app, "GET", "/api/v1/admin/house-feed", Some(&dad), "").json("enabled"),
        "false"
    );
    assert_eq!(
        call(&app, "POST", "/api/v1/admin/house-feed", Some(&kid), "").status,
        403
    );
    let house = call(&app, "POST", "/api/v1/admin/house-feed", Some(&dad), "").json("url");
    let house_path = &house["http://gamematcher.local".len()..];
    assert!(call_from(&app, "GET", house_path, None, "", None)
        .body
        .contains("BEGIN:VEVENT"));
    call(&app, "DELETE", "/api/v1/admin/house-feed", Some(&dad), "");
    assert_eq!(
        call_from(&app, "GET", house_path, None, "", None).status,
        404
    );
}

// ---- Board limits --------------------------------------------------------------

/// The browser loads `/api/v1/schema` before anything else, and the C6's
/// response limit is 32 KiB. Once it went past that and the UI could not
/// start on the board; keep a 4 KiB margin.
#[test]
fn schema_fits_the_boards_response_limit() {
    use miniframework::desktop::DesktopPlatform;
    use miniframework::{http, site::Scratch, Config, Site};
    let mut config = Config::new("gamematcher", "gamematcher.local");
    config.response_limit = 28 * 1024;
    let site = Site::new(
        config,
        App::open(Arc::new(MemKv::default()), limits()).unwrap(),
        DesktopPlatform,
    );
    let raw = b"GET /api/v1/schema?fmt=json HTTP/1.1\r\nHost: gamematcher.local\r\n\r\n";
    let request = http::parse(raw, 1024).unwrap().unwrap();
    let mut scratch = Scratch::new(28 * 1024);
    let (status, _, length, _) = site.respond(&request, false, &mut scratch).into_parts();
    assert_eq!(status, 200, "schema is over 28 KiB (length {length:?})");
}

/// A Mastodon instance that accepts everything and remembers the statuses.
#[derive(Default)]
struct Instance {
    statuses: std::sync::Mutex<Vec<String>>,
}

impl miniframework::fetch::Fetch for Instance {
    fn get(
        &self,
        url: &str,
        _: &str,
        limit: usize,
    ) -> std::io::Result<miniframework::fetch::Fetched> {
        self.send(&miniframework::fetch::Outgoing {
            method: "GET",
            url,
            headers: &[],
            body: &[],
            limit,
        })
    }
    fn send(
        &self,
        r: &miniframework::fetch::Outgoing<'_>,
    ) -> std::io::Result<miniframework::fetch::Fetched> {
        let body = if r.url.ends_with("/verify_credentials") {
            r#"{"id":"1","username":"gamematcher","acct":"gamematcher"}"#.to_string()
        } else {
            self.statuses
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(r.body).into_owned());
            r#"{"id":"2","url":"https://m.example/@gamematcher/2"}"#.to_string()
        };
        Ok(miniframework::fetch::Fetched {
            status: 200,
            content_type: "application/json".into(),
            body: body.into_bytes(),
        })
    }
}

#[test]
fn the_household_bot_posts_announced_plans() {
    use gamematcher::outbound::Outbox;
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    let instance = Arc::new(Instance::default());
    let outbox = Arc::new(Outbox::open(kv.clone(), instance.clone(), instance.clone()));
    let mut app = App::open(kv, limits()).unwrap();
    app.attach_outbox(outbox.clone());
    let admin = setup(&app);
    let kid = signup(&app, "kid@home.lan", "Sam");
    assert_eq!(
        call(&app, "GET", "/api/v1/admin/bot", Some(&kid), "").status,
        403
    );
    let r = call(
        &app,
        "POST",
        "/api/v1/admin/bot",
        Some(&admin),
        r#"{"instance":"https://m.example","token":"secret"}"#,
    );
    assert_eq!(r.json("setup"), "working", "{}", r.body);
    assert!(outbox.run_once(1));
    assert_eq!(
        call(&app, "GET", "/api/v1/status", None, "").json("bot"),
        "@gamematcher@m.example"
    );

    let game = add_game(&app, &kid, "Catan");
    // Not announced: nothing queued.
    new_plan(&app, &kid, game);
    let body = format!(
        r#"{{"title":"Catan night","announce":true,"games":[{game}],"times":[{{"id":0,"start":{SAT},"end":0}}]}}"#
    );
    let r = call(&app, "POST", "/api/v1/plans", Some(&kid), &body);
    assert_eq!(r.json("announced"), "true");
    let plan: u32 = r.json("id").parse().unwrap();
    let at = r.body.find(r#""times":[{"id":"#).unwrap() + 15;
    let time: u32 = r.body[at..r.body[at..].find(',').unwrap() + at]
        .parse()
        .unwrap();
    let r = call(
        &app,
        "POST",
        &format!("/api/v1/plans/{plan}/lock"),
        Some(&kid),
        &format!(r#"{{"time":{time},"games":[]}}"#),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    while outbox.run_once(2) {}
    let statuses = instance.statuses.lock().unwrap().clone();
    assert_eq!(statuses.len(), 2, "{statuses:?}");
    assert!(
        statuses[0].contains("New game plan: Catan night")
            && statuses[0].contains("http://gamematcher.local/plans/")
    );
    assert!(statuses[1].contains("It's on: Catan night"));
    assert!(statuses
        .iter()
        .all(|s| s.contains(r#""visibility":"unlisted""#)));

    // Nobody coming has a Mastodon account: a DM request reaches no one.
    let r = call(
        &app,
        "POST",
        &format!("/api/v1/plans/{plan}/comments"),
        Some(&kid),
        r#"{"text":"Bring snacks","dm":true}"#,
    );
    assert_eq!(
        (r.status, r.json("sent_dms").as_str()),
        (201, "0"),
        "{}",
        r.body
    );

    let r = call(&app, "POST", "/api/v1/admin/bot/digest", Some(&admin), "");
    assert_eq!(r.status, 200, "{}", r.body);
    let r = call(
        &app,
        "PATCH",
        "/api/v1/admin/bot",
        Some(&admin),
        r#"{"digest":true,"digest_time":"7am"}"#,
    );
    assert_eq!(r.status, 400);
    let r = call(&app, "DELETE", "/api/v1/admin/bot", Some(&admin), "");
    assert_eq!(r.json("configured"), "false");
}

/// Brevo's transactional API: one verified sender; remembers the mail.
#[derive(Default)]
struct Brevo {
    mail: std::sync::Mutex<Vec<String>>,
}

impl miniframework::fetch::Fetch for Brevo {
    fn get(
        &self,
        url: &str,
        _: &str,
        limit: usize,
    ) -> std::io::Result<miniframework::fetch::Fetched> {
        self.send(&miniframework::fetch::Outgoing {
            method: "GET",
            url,
            headers: &[],
            body: &[],
            limit,
        })
    }
    fn send(
        &self,
        r: &miniframework::fetch::Outgoing<'_>,
    ) -> std::io::Result<miniframework::fetch::Fetched> {
        let keyed = r
            .headers
            .iter()
            .any(|(k, v)| *k == "api-key" && *v == "xkeysib-test-key-0123456789");
        let (status, body) = match (keyed, r.url) {
            (false, _) => (401, String::new()),
            (_, "https://api.brevo.com/v3/senders") => {
                (200, r#"{"senders":[{"id":1,"name":"Dad","email":"dad@example.com","active":true,"ips":[]}]}"#.into())
            }
            (_, "https://api.brevo.com/v3/smtp/email") => {
                self.mail.lock().unwrap().push(String::from_utf8_lossy(r.body).into_owned());
                (201, r#"{"messageId":"<1@smtp-relay.mailin.fr>"}"#.into())
            }
            _ => (404, String::new()),
        };
        Ok(miniframework::fetch::Fetched {
            status,
            content_type: "application/json".into(),
            body: body.into_bytes(),
        })
    }
}

#[test]
fn email_notifications_invites_and_forgotten_pins() {
    use gamematcher::outbound::{MailSetup, Outbox};
    let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
    let brevo = Arc::new(Brevo::default());
    let outbox = Arc::new(Outbox::open(kv.clone(), brevo.clone(), brevo.clone()));
    let mut app = App::open(kv, limits()).unwrap();
    app.attach_outbox(outbox.clone());
    let dad = setup(&app);
    let kid = signup(&app, "kid@home.lan", "Sam");
    assert_eq!(
        call(&app, "GET", "/api/v1/status", None, "").json("mail"),
        "false"
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/admin/mail",
            Some(&dad),
            r#"{"api_key":"xsmtpsib-not-the-right-kind-of-key"}"#
        )
        .status,
        400,
        "an SMTP key is refused with a hint"
    );
    outbox
        .begin_mail_setup(MailSetup {
            api_key: "xkeysib-test-key-0123456789".into(),
            built_in: true,
            ..Default::default()
        })
        .unwrap();
    assert!(outbox.run_once(1));
    let r = call(&app, "GET", "/api/v1/admin/mail", Some(&dad), "");
    assert_eq!(
        r.json("sender"),
        "Game Matcher <dad@example.com>",
        "{}",
        r.body
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/status", None, "").json("mail"),
        "true"
    );

    // Sam answers Dad's plan; Dad locks it in: Sam gets one email.
    let game = add_game(&app, &dad, "Catan");
    let plan = new_plan(&app, &dad, game);
    let r = call(
        &app,
        "GET",
        &format!("/api/v1/plans/{plan}"),
        Some(&dad),
        "",
    );
    let at = r.body.find(r#""times":[{"id":"#).unwrap() + 15;
    let time: u32 = r.body[at..r.body[at..].find(',').unwrap() + at]
        .parse()
        .unwrap();
    let answer = r#"{"overall":"in","times":[],"games":[],"note":""}"#;
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/api/v1/plans/{plan}/response"),
            Some(&kid),
            answer
        )
        .status,
        200
    );
    let lock = format!(r#"{{"time":{time},"games":[]}}"#);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/plans/{plan}/lock"),
            Some(&dad),
            &lock
        )
        .status,
        200
    );
    while outbox.run_once(2) {}
    {
        let mail = brevo.mail.lock().unwrap();
        assert_eq!(mail.len(), 1, "{mail:?}");
        assert!(
            mail[0].contains(r#""to":[{"email":"kid@home.lan","name":"Sam"}]"#)
                && mail[0].contains("It's on"),
            "{}",
            mail[0]
        );
    }

    // Sam turns plan emails off.
    let r = call(
        &app,
        "PATCH",
        "/api/v1/me",
        Some(&kid),
        r#"{"email_notify":false}"#,
    );
    assert_eq!(r.json("email_notify"), "false");

    // Invites: checked addresses, at most 8.
    let path = format!("/api/v1/plans/{plan}/invite");
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            Some(&kid),
            r#"{"emails":["not an address"]}"#
        )
        .status,
        400
    );
    let r = call(
        &app,
        "POST",
        &path,
        Some(&kid),
        r#"{"emails":["gran@example.com, uncle@example.org"]}"#,
    );
    assert_eq!(r.json("sent"), "2", "{}", r.body);

    // Forgot PIN: a code by email, then a new PIN with it.
    let start = r#"{"email":"kid@home.lan"}"#;
    assert_eq!(
        call(&app, "POST", "/api/v1/auth/pin-reset", None, start).status,
        200
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/auth/pin-reset",
            None,
            r#"{"email":"nobody@x.com"}"#
        )
        .status,
        200,
        "says nothing"
    );
    while outbox.run_once(3) {}
    let code = {
        let mail = brevo.mail.lock().unwrap();
        assert_eq!(
            mail.len(),
            4,
            "lock-in, 2 invites, 1 code (not one for nobody@)"
        );
        let last = mail.last().unwrap();
        let at = last.find("Your Game Matcher code: ").unwrap() + 24;
        last[at..at + 6].to_string()
    };
    let wrong = r#"{"email":"kid@home.lan","code":"000000x","pin":"4321"}"#;
    assert_eq!(
        call(&app, "POST", "/api/v1/auth/pin-reset/confirm", None, wrong).status,
        401
    );
    let body = format!(r#"{{"email":"kid@home.lan","code":"{code}","pin":"4321"}}"#);
    let r = call(&app, "POST", "/api/v1/auth/pin-reset/confirm", None, &body);
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.cookie.is_some(), "signed in");
    assert_eq!(
        call(&app, "POST", "/api/v1/auth/pin-reset/confirm", None, &body).status,
        401,
        "a code works once"
    );
    let r = call(
        &app,
        "POST",
        "/api/v1/auth/pin",
        None,
        r#"{"email":"kid@home.lan","pin":"4321"}"#,
    );
    assert_eq!(r.status, 200, "the new PIN works");
}

#[test]
fn recurring_series_reports_and_status_permissions() {
    use gamematcher::messages::{DeliveryStatus, GameResult, PlanView, PostGameReport};
    use miniframework::wire::{self, Format};
    let kv = Arc::new(MemKv::default());
    let app = App::open(kv.clone(), limits()).unwrap();
    let dad = setup(&app);
    let kid = signup(&app, "kid@home", "Kid");
    let game = call(
        &app,
        "POST",
        "/api/v1/games",
        Some(&dad),
        r#"{"title":"Catan"}"#,
    )
    .json("id");
    let now = miniframework::wall_ms().unwrap() / 1000;
    let body = format!(
        r#"{{"title":"Friday games","games":[{game}],"times":[{{"start":{},"end":0}}],"repeat":{{"frequency":"weekly","count":3}}}}"#,
        now + 86400
    );
    let created = call(&app, "POST", "/api/v1/plans", Some(&dad), &body);
    assert_eq!(created.status, 201, "{}", created.body);
    let plan: PlanView = wire::decode(Format::Json, created.body.as_bytes()).unwrap();
    assert_eq!(plan.series.len(), 3);
    for (i, id) in plan.series.iter().enumerate() {
        let got = call(&app, "GET", &format!("/api/v1/plans/{id}"), Some(&dad), "");
        let p: PlanView = wire::decode(Format::Json, got.body.as_bytes()).unwrap();
        assert_eq!(p.times[0].start, now + 86400 + i as u64 * 7 * 86400);
        assert!(p.responses.is_empty());
    }
    // A single cancelled occurrence leaves its siblings intact.
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/plans/{}/cancel", plan.series[1]),
            Some(&dad),
            ""
        )
        .status,
        200
    );
    let first = call(
        &app,
        "GET",
        &format!("/api/v1/plans/{}", plan.id),
        Some(&dad),
        "",
    );
    assert_eq!(first.json("status"), "open");
    // Invalid series must leave no partial plans behind.
    let before = call(
        &app,
        "GET",
        "/api/v1/plans?status=open,on,done,cancelled",
        Some(&dad),
        "",
    )
    .body;
    let invalid = body.replace("\"weekly\"", "\"nonsense\"");
    assert_eq!(
        call(&app, "POST", "/api/v1/plans", Some(&dad), &invalid).status,
        400
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/plans?status=open,on,done,cancelled",
            Some(&dad),
            ""
        )
        .body,
        before
    );

    // Make a game that has already started, RSVP, and save its report.
    let body = format!(
        r#"{{"title":"Played","games":[{game}],"times":[{{"start":{},"end":0}}]}}"#,
        now - 60
    );
    let res = call(&app, "POST", "/api/v1/plans", Some(&dad), &body);
    let p: PlanView = wire::decode(Format::Json, res.body.as_bytes()).unwrap();
    let path = format!("/api/v1/plans/{}", p.id);
    call(
        &app,
        "PUT",
        &format!("{path}/response"),
        Some(&kid),
        r#"{"overall":"in"}"#,
    );
    let lock = format!(r#"{{"time":{},"games":[{game}]}}"#, p.times[0].id);
    assert_eq!(
        call(&app, "POST", &format!("{path}/lock"), Some(&dad), &lock).status,
        200
    );
    let report = r#"{"summary":"A close finish!","games":[{"game":1,"winners":[1,2]}]}"#;
    assert_eq!(
        call(&app, "PUT", &format!("{path}/report"), Some(&kid), report).status,
        403
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("{path}/report"),
            Some(&dad),
            &report.replace("[1,2]", "[999]")
        )
        .status,
        400
    );
    let res = call(&app, "PUT", &format!("{path}/report"), Some(&dad), report);
    assert_eq!(res.status, 200, "{}", res.body);
    let done: PlanView = wire::decode(Format::Json, res.body.as_bytes()).unwrap();
    assert_eq!(done.status, "done");
    assert_eq!(
        done.report,
        Some(PostGameReport {
            summary: "A close finish!".into(),
            games: vec![GameResult {
                game: 1,
                winners: vec![1, 2]
            }]
        })
    );
    assert!(done.reportable);
    assert!(!done.manageable);
    assert_eq!(
        call(&app, "GET", "/api/v1/admin/delivery", Some(&kid), "").status,
        403
    );
    let status = call(&app, "GET", "/api/v1/admin/delivery", Some(&dad), "");
    assert_eq!(status.status, 200);
    let _: DeliveryStatus = wire::decode(Format::Json, status.body.as_bytes()).unwrap();
    let reopened = App::open(kv, limits()).unwrap();
    let restored = call(&reopened, "GET", &path, Some(&dad), "");
    let restored: PlanView = wire::decode(Format::Json, restored.body.as_bytes()).unwrap();
    assert_eq!(restored.report, done.report);
}
