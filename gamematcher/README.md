# Game Matcher

A household game-night planner served from an ESP32-C6 board at
`gamematcher.local`. Someone posts a plan, everyone ticks the games they'd
play and the times they can do, and the house sorts out the rest in person.
The website stays on the local network. An optional public Mastodon bot lets
linked users RSVP from outside the house without exposing the board.

Built on [miniframework](../miniframework/): Rust API in `api/`, Angular 22 UI
in `ui/` (using `miniframework-ng`), served over HTTP, HTTPS and HTTP/2.
Design and sprints: [spec/](spec/README.md).

## Status

| Sprint | What | State |
|---|---|---|
| 0 | Skeleton, C6 build and deploy, hardware gate | see [spec/measurements/c6-gate.md](spec/measurements/c6-gate.md) |
| 1 | Snapshot storage, accounts, PIN sign-in, sessions, admin | done (desktop and board) |
| 2 | Game catalog, stars, admin merge/delete | done (desktop and board) |
| 3 | Plans: times, answers, lock-in, hand-over, done/cancelled | done (desktop and board) |
| 4 | Comments, notifications (bell, coalescing, reminders) | done (desktop; board update verified) |
| 5 | Calendar: plan download, personal and household feeds | done (desktop; board update verified) |
| 6 | Sign in with Mastodon (framework OAuth + PKCE), self-reload after updates | done (desktop + mock instance); firmware built, not flashed |
| 7 | The board's screen: card rotation, BOOT button, live frame endpoint | done (on the board) |
| 8+ | LED, outbound Mastodon, hardening | [spec/sprints.md](spec/sprints.md) |

## Planning, reports, and delivery status

- **Best times:** ranks definite attendance, then flexibility; shows who cannot
  attend and leaves missing answers unknown. Host conflicts sort last.
- **Recurring plans:** weekly, fortnightly, or monthly; 2–12 occurrences created
  atomically, within the 16-active-plan limit. Dates follow household wall time,
  preserve duration, and clamp monthly dates without drifting. DST gaps are
  rejected. Each occurrence has its own RSVPs and can be edited/cancelled.
- **Reports:** after the confirmed start, the host/admin can record a summary,
  games played, and winners (ties/teams or no winner). Saving marks that occurrence
  played. Reports remain editable in history.
- **Status:** admins open /status for both bots' and email delivery queues,
  errors, attempt counts, next retry times, and manual retries. Sent means
  provider acceptance, not that the recipient read it.
- **Two bots:** Admin has separate household and public configurations. Existing
  household settings stay intact. Each plan has separate announcement checkboxes;
  public summaries include only plans explicitly shared to the public bot.
  Reminders and comment DMs route through the appropriate bot.
- **Reply to RSVP:** tokens need write:statuses, read:accounts, read:notifications.
  Enable incoming RSVPs in the bot settings, and link each person's account in
  Profile. Reply **in**, **maybe**, or **out** to a plan post, or mention the bot
  with **in 42** for plan #42. Accepted RSVPs get a private confirmation.
  Polling runs every minute with a durable inbox and replay protection.
  Only linked identities native to that bot's instance are accepted: mastomini
  users via mastomini, mastodon.social users via mastodon.social. The first poll
  skips historical mentions. Website/calendar links still require home access.

Use tools/features_walk.py against a fresh, isolated desktop server to exercise
the recurring/ranking/report/status/two-bot UI flow.

## Quick start (desktop)

```sh
cd gamematcher
make test          # Rust tests + UI type check
make run-bundle    # build the UI, serve UI + API on http://127.0.0.1:8088
```

Open http://127.0.0.1:8088. The first account you make becomes the admin.
For UI work: `make run` and, in another shell, `make web-dev`
(http://localhost:4200, hot reload, `/api` proxied to :8088).

State lives in `api/.local/state` (two snapshot slots); delete it to start over.

## Accounts in one paragraph

Everyone signs in with an email address (just a login name; nothing is ever
sent to it) and a 4–8 digit PIN, over plain HTTP so guests without the
household CA can join. The first person to sign in on a fresh board becomes
admin. Anyone on the Wi-Fi can make an account unless the admin closes
sign-up. Five wrong PINs lock an account for a minute (doubling up to an
hour); 30 wrong PINs board-wide in ten minutes pause all PIN sign-ins.
Sessions are HttpOnly cookies that survive reboots. Mastodon sign-in comes in
Sprint 6 ([spec/02-accounts-auth.md](spec/02-accounts-auth.md)).

## On the board

Read [DEPLOY.md](DEPLOY.md) first. In short:

```sh
make certs                      # once: gamematcher.local from the household CA
make firmware                   # HTTP + HTTPS + HTTP/2 (TRANSPORT=http|https|http2)
make flash PORT=COM17           # FIRST INSTALL ONLY: erases the whole board
make update PORT=COM17          # afterwards: app only, keeps accounts
```

Wi-Fi settings come from `gamematcher/.env` (gitignored):
`GAMEMATCHER_WIFI_SSID`, `GAMEMATCHER_WIFI_PASSWORD`, and optionally
`GAMEMATCHER_SETUP_CODE` to require a code for first-run setup. Email: a Brevo
API key (`BREVO_API_KEY=xkeysib-…`) in that file or the household `C:\github\.env`
turns on invites, "forgot PIN?" codes and plan news ([spec/08](spec/08-mastodon.md#email)).

## Layout

```
api/            Rust crate: the API, storage and the C6 firmware binary
  src/api.rs      routes (spec/04-api.md)
  src/state.rs    accounts and their rules, no HTTP
  src/catalog.rs  games and stars
  src/plans.rs    plans, answers, lock-in (the negotiation)
  src/comments.rs comment threads (one Kv blob per plan)
  src/notify.rs   who hears about what, coalescing, reminders
  src/ics.rs      iCalendar files and feeds
  src/identity.rs Mastodon instances, who may sign in, linking
  src/views.rs    domain state → what the API shows
  src/store.rs    two CRC-checked snapshot slots over the framework Kv
  src/pin.rs      PIN hashing and lockouts
  src/gate.rs     Sprint 0 outbound-HTTPS gate (feature `gate`, never released)
  tests/api.rs    the API end to end
ui/             Angular workspace (miniframework-ng by path)
tools/          build-c6.sh, deploy-c6.py, manifest.py, gate.py,
                board_accept.py (Sprint 1 API on a fresh board), ui_walk.py (browser walk),
                mock_mastodon.py + oauth_walk.py (sign-in without a real instance),
                display_mockups.py (LCD card mockups)
spec/           design, sprints, roadmap
```
