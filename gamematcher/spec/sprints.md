# Sprints

Each sprint ends with a demo on the desktop build, plus the board from Sprint 0 on.
"Done" means: `make test` green (Rust + TypeScript), clippy/fmt clean,
Angular budgets met, and the exit criteria below checked. Board work follows
the framework's deploy rules: agents ask before flashing, and verify the MAC first.
Flashes are app-only after Sprint 0, so state survives.

Sprints are sized as one focused chunk of work each, not calendar weeks.

## Sprint 0: skeleton and the C6 gate

**Done October 17, 2026.** Gate passed ([measurements/c6-gate.md](measurements/c6-gate.md)).

- Repave `ac:eb:e6:1e:13:40`. The owner authorized it on October 17, since minicloud's data is
  ephemeral. Verify the MAC and flash size before erasing.
- `gamematcher/api`: crate from the housemetrics recipe (`BoardConfig::c6()`,
  desktop + esp32 bins, `build.rs`, `sdkconfig.defaults`, new partition table
  from [01](01-overview.md#flash-layout-c6-proposed)). Features `https`, `http2`,
  `display`, `oauth`.
- `bash tools/certs.sh` → `gamematcher.local` leaf from the household CA.
- `gamematcher/ui`: Angular 22 workspace consuming `miniframework-ng`. An empty
  shell that shows `/api/v1/status`.
- `Makefile` (Git Bash): `test`, `run`, `run-bundle`, `web`, `web-dev`, `firmware`, `flash`.
- **Gate measurement** on the board ([01 § Hardware](01-overview.md#hardware)):
  bench load of 3 HTTP + 1 HTTPS/h2 browser + an outbound HTTPS POST to
  a household-CA host and a public-CA host (`esp_crt_bundle`), 5 minutes. Read
  results from `/api/v1/sys` and `/api/v1/log`, not long serial captures.
- **Exit:** gate result written to `spec/measurements/c6-gate.md` and the
  hardware decision recorded in [open-questions.md](open-questions.md).

## Sprint 1: storage, accounts, PIN sign-in

**Done October 17, 2026.** 28 Rust tests + framework session tests; `tools/board_accept.py`
passed on the board, including persistence across a hardware reset.

- Snapshot store: two slots, CRC, generations, refuse-on-double-corruption.
  `MemStore` for tests, file store on desktop, partition store on board.
- Framework `session` module (see [OAUTH.md](../../miniframework/spec/OAUTH.md#sessions)),
  usable without `oauth`.
- First-run setup, sign-up (open/closed), PIN sign-in, PIN change, sign out (everywhere).
- PIN hashing with round count measured on the C6 (target ~100 ms), rate limits.
- Origin check on unsafe methods.
- Admin: users list, disable, reset PIN, promote/demote (≥ 1 admin invariant).
- UI: setup, sign-in, sign-up, profile, admin users.
- **Exit:** tests for lockout timing, the first-admin race (two concurrent setups → one
  409), session expiry/eviction, and storage torn-write recovery (truncate the newest
  slot in a test → previous generation loads). Data survives a board reboot.

## Sprint 2: game catalog and interests

**Done October 17, 2026.** Catalog UI, star toggle, game page with edit/archive and admin merge/delete.

- Games CRUD, normalized-title dedupe, archive, admin merge.
- Interest toggle, "who's into it".
- UI: catalog, game page, inline "add a game".
- **Exit:** duplicate title returns the existing id; merge moves interests; bounds return
  409 `full`.

## Sprint 3: plans and responses

**Done October 17, 2026.** The exit walk ran in a real browser against the board
(`tools/ui_walk.py`, 9.7 s, free heap never below 100 KB) and survived a reset. Time
options use stable ids rather than index + rev ([04](04-api.md#example-respond-to-a-plan)).
Found on the board: the API schema grew past the C6's 32 KiB response limit; the
framework now sends it compact (defaults omitted: 32.6 KB → 19.8 KB).

- Plans with 1–8 games, 0–6 time options (with per-option rev), notes, where, seats.
- `PUT …/response`, stale-rev 409, counts/names view, player-range hints.
- Lock in, reopen, cancel, hand over host. `done` transition from the clock.
- Done-plan compaction and the done/cancelled ring.
- UI: home, new plan, plan screen.
- **Exit:** the end-to-end walk on the board with three phones: create, respond,
  edit a time (stale response rejected), lock in, cancel. No reboot,
  `/api/v1/sys` heap within budget.

## Sprint 4: comments and notifications

**Done October 17, 2026.** Threads are per-plan Kv blobs (deleted when a plan leaves
the past ring). Notifications coalesce while unread. Extra kinds beyond the table: `reopened`,
`changed` (times edited), `host` (handed to you). The poll is `/api/v1/me/summary`, at most once a
minute while visible, plus on focus; two failures show "the board isn't answering".

- Comment log region (append, tombstones, CRC, wrap), paging, edit/delete, admin delete.
- Notification ring per user, dedupe while unread, coalescing per plan.
- Triggers from [03](03-domain.md#notifications), reminders when the clock is synced.
- `/api/v1/me/summary` polling (60 s, visible-only) and the unread badge.
- **Exit:** a comment storm test (64 comments on one plan → 409, log wrap test
  with tombstones). One tab idle for 10 minutes makes ≤ 10 requests.

## Sprint 5: calendar

**Done October 17, 2026.** RFC 5545 written by hand (`ics.rs`), checked by unit tests and by
parsing a live feed with the `icalendar` package in `tools/ui_walk.py`. Feeds keep done plans 60 days
and cancelled ones 14 days (as STATUS:CANCELLED), newest 40 events. Not yet tried in iOS Calendar
or Thunderbird by a person.

- Per-plan `.ics`, per-user feed token + rotate, optional household feed.
- Stable UIDs, SEQUENCE, UTC times. Folded lines, CRLF, escaping per RFC 5545.
- UI: download buttons, feed URL with copy and the "cloud calendars can't see it"
  note.
- **Exit:** feeds validate (an RFC 5545 validator in tests) and import into iOS Calendar and
  Thunderbird on the LAN. Rotating a token makes the old URL 404.

## Sprint 6: framework OAuth + Sign in with Mastodon

**Done October 17, 2026 (desktop; firmware builds, not yet on the board).** The
framework's `oauth` feature does code + PKCE with every network step on a worker:
the browser gets a ticket and polls it (start → redirect → callback → done), so a
1–3 s TLS handshake to an instance never stalls serving. Tokens are used once and
revoked. Tested against an in-process mock in the framework, and in a real browser
against `tools/mock_mastodon.py` (`tools/oauth_walk.py`). The desktop server speaks
HTTPS to real instances (rustls, live-checked against mastodon.social's public
discovery document). Not yet done by a person against mastodon.social or mastomini.
Also this sprint: the UI reloads itself after a board update (stale lazy chunks),
ported from mawkingbird, landing on the page you were going to.

- Framework: `oauth` feature per [OAUTH.md](../../miniframework/spec/OAUTH.md):
  PKCE client, Mastodon preset (dynamic registration, discovery),
  `Fetch` gains POST + trust anchors (household CA / public bundle), desktop
  HTTPS client under `desktop-tls`, mock provider for tests.
- **Household CA trust:** reissued October 17 with non-critical name constraints,
  so mbedTLS loads it. Verify outbound HTTPS to mastomini.local on the board. See
  [c6-gate.md](measurements/c6-gate.md).
- Game Matcher: provider admin (instance, trust level, invite list), sign-in,
  link/unlink, identity rules.
- Confirm mastodon.social accepts `http://gamematcher.local/auth/callback`
  (expected: its redirect validation is permissive).
- **Exit:** sign in against mastomini.local (household trust) and
  mastodon.social (invited trust) from both HTTP and HTTPS origins, on the
  board. A non-invited mastodon.social account is refused. The access token is
  not persisted anywhere (checked by reading the snapshot and kv).

## Sprint 7: onboard display and LED

**Display done October 17, 2026, on the board.** Atkinson Hyperlegible (OFL) rasterized
at build time into 4-bit atlases (`fonts.rs`). Cards are drawn per 24-row strip in
80–90 ms with integer-only blending (the C6 has no FPU). Text fits by shrinking,
then wrapping, then listing "+N", and cuts with "…" only as the last resort.
`/api/v1/display` (the deck) and `/api/v1/display/frame.bmp?card=N`
(the board's own render, streamed strip by strip) let anyone see the screen remotely.
Pixels go to the panel in native byte order (big-endian came out pink).
LED patterns: not yet. App image now 2.21 MB of 2.5 MB.

- Copy minicloud's C display component (ST7789 strip renderer) behind `display`.
- Views 1–3, BOOT cycling, auto-return, dimming, QR code for the IP URL.
- LED patterns.
- `/api/v1/display` summary endpoint.
- **Exit:** screen updates within 2 s of a lock-in. The renderer never holds the
  state lock (asserted in a desktop test with a fake panel). QR scans on
  Android and iOS.

## Sprint 8: outbound Mastodon (bot + DMs)

**Built October 17, 2026 (desktop-tested; firmware built, not yet flashed).** `api/src/outbound.rs`
(queue, setup, posting) and `api/src/announce.rs` (what it says). The admin page
sets the bot up either by creating it on a mastomini through its admin API
(the admin password is used once, never stored) or from a pasted access token.
Plans have a "post to Mastodon" tick (new plan, lock-in, cancellation). A comment can go
as one DM to everyone coming whose linked account the bot can reach: its own
instance, or any non-`.local` one when the bot is on a public instance. The
optional morning summary is skipped when nothing is planned. Queued posts survive reboots and
retry with the same Idempotency-Key, backing off for up to 24 h. Sign-in runs first on the shared worker.

- Bot settings, per-plan "announce", visibility rules.
- Persistent outbound queue, single worker sharing the outbound TLS slot,
  backoff, Idempotency-Key, coalescing.
- Per-user DM notifications (same-instance only).
- Admin outbound page.
- **Exit:** unplug the internet, lock in a plan, plug it back in, and it posts exactly
  once. A reboot mid-queue still posts once. Sign-in takes priority over a queued post.

## Sprint 10: per-record storage (run before Sprint 9)

**Built October 21, 2026 (desktop-tested; firmware not yet built or flashed).** Every write used to
re-encode and rewrite the whole state, so cost grew with n. Now a commit writes only the records it
touched. Details and deviations in [sprint-10-storage.md](sprint-10-storage.md).

## Sprint 9: hardening and v1 release

- Bench (`miniframework/bench`) load profile: a game night of 6 phones on
  HTTP + 2 on HTTPS/h2, polling + bursts of responses, 5 minutes.
- Power-cut test: cut power during commits (×20). Must boot to the latest or
  previous generation every time.
- Flash-wear projection from the measured commit rate. Add slots if under 3 years.
- Accessibility pass (keyboard, screen reader on the plan grid).
- `gamematcher/DEPLOY.md` runbook (identity, flash, first-run admin claim
  immediately after flashing, verification probe).
- Acceptance run (below). Tag v1.

## v1 acceptance

1. A fresh board shows setup. The first visitor becomes admin with a PIN.
2. A guest on HTTP with no CA creates an account in under a minute.
3. Anyone adds a game. Duplicates are caught.
4. Someone posts a plan with two games and two time windows. Interested people
   get a notification.
5. Three people respond. Counts and names are right. A response against an edited
   time is rejected and retried.
6. The host locks in. Responders are notified and the LCD shows it.
7. Comments work, and the author can edit and delete.
8. The calendar feed shows the plan on an iPhone on the Wi-Fi.
9. With the internet unplugged, items 1–8 still work. The status shows Mastodon unreachable.
10. A household member signs in with mastomini, and an invited guest with mastodon.social.
11. Power cut during use → state preserved (at most the in-flight change lost).
12. A member cannot edit someone else's plan or comment, or reach admin
    endpoints (API tests, not UI).
