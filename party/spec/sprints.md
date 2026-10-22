# Sprints

Each sprint is one focused chunk of work that can be handed to one agent or
person, not a calendar week. Each ends with a demo on the desktop build, plus
the board from Sprint 0 on.

**Done** means: `make test` green (Rust + TypeScript), clippy/fmt clean,
Angular budgets met, the exit criteria checked, and the spec updated where the
build differed from it.

House rules for every sprint:

- Shell is Git Bash. `Makefile` recipes, no `.bat`/PowerShell helpers. Python
  tools run through `uv run`.
- Board work follows the framework deploy rules: **ask before flashing**, verify
  the MAC first, app-only updates after Sprint 0 (state survives), read the
  board's `/api/v1/log` and `/api/v1/sys` rather than long serial captures.
- Never commit. Leave work in the working tree for the owner.
- Never publish anything externally (no Mastodon posts from a real account, no
  pushes to real GitHub repos) except to the test accounts and repos the owner
  set up for the sprint, and only in the steps listed.
- Tests talk to mocks: `tools/mock_mastodon.py` (from Game Matcher, extended),
  a mock GitHub Git Data API, a mock Brevo, a mock OpenRouter.

Dependencies: 0 → 1 → 2 → 3, then 4 and 5 in parallel, 6 after 3, 7 after 6,
8 after 3, 9 after 8, 10 after 7 and 9, 11 after 7, 12 after 10, 13 last.

## Sprint 0: skeleton and the S3 gate

**Built October 21, 2026, except the board measurements.** Crate, desktop and
`party-s3` binaries, partition table, `sdkconfig.defaults`, Angular workspace
(`local`, `companion`, `demo` build, `party-core`), Makefile, `build-s3.sh`,
`manifest.py`, guarded `deploy-s3.py` (it needs `MAC=`, with no default).
The gate firmware (`GATE=1`) has storage, crypto and outbound-HTTPS runs on a
worker. *(Correction, October 21: the 532 KB image reported here was built
without Wi-Fi settings, so the compiler removed the app as dead code. `build.rs`
now refuses such a build. The real image is about 2 MB, 48% of `app0`.)*
**Open:** no board assigned yet, so
[measurements/s3-gate.md](measurements/s3-gate.md) is empty, and `make certs`
hasn't been run.

- `party/api`: crate from the Game Matcher recipe, `BoardConfig::s3()`, desktop +
  `party-s3` bins, `build.rs`, `sdkconfig.defaults` (PSRAM octal on), partition
  table from [01](01-overview.md#flash-layout-proposed-16-mib). Features `https`,
  `http2`, `oauth`, `session`, `ops`, `public-ca`.
- mDNS `party.local`, household-CA leaf via `tools/certs.sh`.
- `party/ui`: workspace with `party-core`, `local`, `companion` projects. Empty
  shells showing `/api/v1/status` (local) and "hello" (companion).
- `Makefile`: `test`, `run`, `run-bundle`, `web`, `web-dev`, `site`, `demo`,
  `firmware`, `flash`, `update`.
- **Gate measurements**, written to `spec/measurements/s3-gate.md`:
  - NVS: write 20,000 ~200 B records across 10 namespaces in `partydata`. Boot
    enumeration time, free entries, a random read's time.
  - Concurrent outbound TLS: while serving 4 HTTP + 1 HTTPS clients, run
    back-to-back HTTPS calls to mastodon.social, api.github.com and
    api.brevo.com (public CA bundle) for 5 minutes. Record handshake times,
    largest free internal block, failures.
  - AES-256-GCM + gzip of a 256 KB buffer: time and peak memory.
- **Exit:** the gate file exists. If boot enumeration at 20k records > 10 s or
  internal free block < 32 KiB, revise [07](07-storage-limits.md) before Sprint 1.
  The board identity (MAC) is recorded in `party/DEPLOY.md`.

## Sprint 1: record store, people, PIN sign-in

**Built October 21, 2026 (desktop).** The record store (see
[07](07-storage-limits.md#record-store); index records instead of a framework
`kv-tables` feature). People sign in with an email **or their display name**
(names are unique), assuming [open question 2](open-questions.md). There is
an audit ring of 1,024 entries in 32-entry blocks. All exit tests pass (`api/tests/api.rs`),
and with 2,000 people a write stays under 200 B. Not yet run on a board.

- Framework: namespaced, enumerable `Kv` (`kv-tables` feature), NVS + file +
  memory backends, quarantine on decode failure. Tests in the framework.
- party store: tables, `message!` records, boot index build, single-commit-point
  helpers, `/admin/storage`.
- People, sessions, first-run admin, open sign-up, PIN sign-in/change/lockout,
  admin people (disable, reset PIN, promote, ≥ 1 admin), audit log ring.
  Copy Game Matcher's `pin.rs` and its tests, re-measure rounds on the S3.
- Households (CRUD, membership).
- UI: setup, sign-in, sign-up, me, admin people, households.
- **Exit:** lockout timing, first-admin race (one 409), session eviction, a
  corrupted record is quarantined and reported, data survives a board reset,
  boot with 2,000 seeded people meets the Sprint 0 budget.

## Sprint 2: groups and membership

**Built October 21, 2026 (desktop).** `api/src/groups.rs` holds the permission
table and is tested for every role × visibility × join policy. Group pages, member management, transfer
and people search are in the local app. Removed members can't rejoin by themselves.
Leaving deletes the membership, so `left` is never stored.

- Groups CRUD, visibility, join policies, membership states, roles, transfer,
  last-organizer invariant, archive.
- Permission module: one function per capability, table-driven tests covering
  every role × visibility × join-policy combination.
- Discoverability for people and group search (`?q=`, normalized).
- UI: groups list/search, group page, organizer member management.
- **Exit:** permission table tests pass, a private group never appears in any
  list or search for a non-member (API tests), bounds return 409 `full`.

## Sprint 3: events, kinds, RSVPs

**Built October 21, 2026 (desktop; firmware built, not yet on the board).**
`events.rs`, `rsvps.rs`. Answers are stored in blocks of 16 per event (`r<id>`), so
one answer rewrites about 1 KB however many answered (tested with 200). The
potluck walk runs as an HTTP test (`potluck_walk`), not in a browser on the board.

- Event core, `rev`, status from the clock, kinds as presets, modules
  `headcount`, `bring`, `diet`, `questions`, `cost` (others in Sprint 5).
- RSVPs: statuses, plus-one policy, capacity in heads, waitlist with promotion,
  stale-rev rules, idempotent no-op, answering for household members and as a host
  (`by`, `via`).
- UI: new event (kind → modules), event page, RSVP form generated from modules,
  who's coming, bring list.
- **Exit:** waitlist promotion order test, plus-one policy changes don't rewrite
  answers, a repeated identical answer produces no notification, the walk
  "create potluck, 3 people answer with +1s and dishes, one drops, waitlist
  promotes" runs in a real browser against the board.

## Sprint 4: recurrence, notifications, calendars

**Built October 21, 2026 (desktop).** `series.rs`, `notify.rs`, `ics.rs`, and
`tz.rs`: a POSIX TZ rule parser, so the desktop and the board agree. The admin sets the
zone, default `EST5EDT,M3.2.0,M11.1.0`. "A new event in your group" is computed
(the summary's `awaiting`), not stored, so one event never writes hundreds of
inboxes. Exceptions are implicit: each occurrence is made once and edited on
its own. Feeds parse in tests. Not yet tried in a phone's calendar app.

- Series: rules, rolling window, exceptions ("this one / following"), regulars.
  Start from Game Matcher's `recurring.rs`.
- Notification ring, coalescing, triggers from [03](03-domain.md#notifications),
  `/me/summary`, reminders when the clock is synced.
- `.ics`: per event, personal LAN feed with rotatable token. Reuse Game Matcher's `ics.rs`.
- **Exit:** DST transition test for "first Friday 7pm" in the instance zone,
  editing "this and following" keeps past answers, feeds parse with the
  `icalendar` package and import into a phone calendar on the LAN, a rotated token 404s.

## Sprint 5: the remaining modules

**Built October 21, 2026 (desktop),** together with Sprint 3: rides (host
board), time polls (lock-in makes winners going), shifts with capacities.

- `rides` (need/offer, host match list), `when` (time poll, lock-in turns it into a
  fixed-time event), `shifts`.
- Event kinds `outing`, `poll`, `workday` complete.
- **Exit:** a poll locks in and answers carry over to "going" for people who picked
  the winning time. Ride matching is shown to hosts only (API test).

## Sprint 6: invitations, Mastodon sign-in, email

**Built October 21, 2026 (desktop, against fakes).** `invites.rs`, `identity.rs`,
`mail.rs`, `outbound.rs`, `api_more.rs`. Deliveries are rows in the state (`o<id>`),
so a change and its email commit together. Email-only people (`invited`, no PIN)
get email but no inbox. An invitation that names one is applied at once, and
redeeming it activates *that* person. An admin setting a PIN does too. Differences from the plan: personal keys are made on
request (Me → "Make my link", or the bot's `link`), not at sign-up, and there's no QR
code yet (the link is shown as text). **Owner steps:** a live sign-in against
mastodon.social, and opening a real calendar invitation in a calendar app.

- Invitations: create, hashed tokens, expiry, uses, revoke, local redemption
  (PIN sign-up or Mastodon).
- Board-side Mastodon OAuth via the framework (`signin`, `link`, `redeem`), identity
  uniqueness, up to 3 identities.
- Email through Brevo: invitations, updates and reminders with `REQUEST`/`CANCEL`
  `.ics` attachments, Reply-To organizer, forgot-PIN, daily cap. Port Game
  Matcher's `mail.rs`/`outbound.rs` and widen the queue to 64 entries with priorities.
- Personal keys exist from here (generated per person, shown as a QR link in Me),
  even though the site can't use them until Sprint 9.
- **Exit:** an organizer-created email-only person gets an invite email whose
  `.ics` opens in a calendar app and updates on a time change (manual check by the
  owner, a mock test for the bytes). Sign in with mastodon.social on the board.
  Revoked/expired tokens are refused.

## Sprint 7: the bot

**Built October 21, 2026 (desktop, against a fake instance).** `dsl.rs`,
`bot.rs`, `work.rs`, and `spec/dsl-vectors.json` (52 vectors, round-tripped).
The cursor and the commands' effects commit in one write, so the processed-id ring is
128, not 4,096. Senders on the bot's own instance are matched by account id, others by
`user@host` (an instance only knows remote accounts by handle). Differences from the plan:
`stop`/`start` are global, not per group, and `undo` is RAM-only (a reboot forgets it).
**Owner step:** one live run with a test bot account.

- Bot settings (paste token, verify, show handle). Inbox worker: mention polling,
  direct-only, first-poll rule, batches, processed-id ring, cursor commit order.
- DSL parser in Rust + `spec/dsl-vectors.json`. Name resolution limited to what
  the sender can see, clarifications, `help`.
- Commands: `me`, `events`, `event`, `groups`, `find`, `join`, `leave`, `going/maybe/no/rsvp`,
  `bring`, `unbring`, `ride`, `when`, `shift`, `answer`, `regular`, `redeem`, `link`,
  `newlink`, `stop/start`, `undo`.
- Replies with ack lines, coalesced per sender per batch. DM notifications by preference.
- `/admin/inbox` log.
- **Exit:** with the internet unplugged, a DM sent meanwhile is processed exactly
  once after reconnect. A reboot mid-batch replays nothing. A display-name
  spoof (same name, different account) is refused. Every DSL vector passes. One live
  run against a test bot account on mastodon.social, by the owner.

## Sprint 8: publication (public data) and the companion skeleton

**Built October 21, 2026 (desktop, against a fake GitHub).** `publish.rs`,
`INSTALL.md`, `deploy/party-site/`, and the companion site and demo in `ui/`.
Published events are exactly those a signed-out viewer may see. Places appear by name
only, never addresses. `published_types_hold_only_public_fields` walks every published
type, with a deliberate failing case. Each publication is a parentless commit. Unchanged
files aren't uploaded, and with no changes nothing is pushed. Differences from
the plan: no token-expiry warning, because the framework's HTTP client doesn't return response
headers; the demo has no persona picker yet, since personas only matter for sealed data
(Sprint 9); and the site is built here and copied into `party-site` (see INSTALL.md)
instead of being built by its own Action. **Owner steps:** create the organization and
repos, then a live publication.

- Data classes on every field (an annotation the publisher reads) and the test that
  walks all published types.
- Publisher: manifest, public files, public group `.ics`. Git Data API single
  orphan commit + forced ref, changed files only, cadence and "publish now", token
  expiry warning, `/sync/status`.
- Companion site: `SnapshotDataSource`, landing, public group and event pages,
  freshness banner, CSP. GitHub Action in `party-site` that builds and deploys.
- Demo build: `DemoDataSource` with the fictional instance, persona picker, reset.
- Install guide part 1: create the organization, the two repos, the token.
- **Exit:** the classing test fails if a `direct` field is added to a public type
  (proved with a deliberate failing case). Against the mock API, two publishes with
  no changes push nothing. One live publish to the owner's test repo shows on
  Pages. The demo runs with no network calls except its own files.

## Sprint 9: sealed data and personal links

- AES-256-GCM envelopes on the board, WebCrypto on the site, shared test vectors.
- Scope keys per sealed group and invite-only event, `kid` rotation on
  leave/remove/on demand.
- Keyrings per person, HMAC file names, personal link format, `newlink`.
- Companion: `#/k/` intake, key storage, "forget this device", decrypt
  manifest-listed scopes, the "me" page from the keyring.
- **Exit:** a removed member's old link opens nothing published after their
  removal (test). A swapped envelope (scope A's file renamed to B) fails to
  decrypt. The published tree contains no plaintext title of a sealed group (grep
  test over the mock repo). Keyring rewrite only for people whose content changed.

## Sprint 10: writing from the companion site

- Browser Mastodon PKCE (`profile write:statuses`), per-instance app registration
  cached, token in `localStorage`, sign-out.
- RSVP form → DSL preview → `direct` status to the bot → pending, confirmed when a
  publication shows it. "Copy the line" fallback without OAuth. Email-only messaging.
- Invite page and remote redemption via `redeem <token>` DM. The bot DMs back the
  personal link.
- Public Mastodon strip (bot announcements, group hashtag, public API only).
- **Exit:** end to end with the owner's test accounts: a family member at another
  house redeems an invite, opens their link, answers "going +1 bringing salad", gets
  the bot's DM, and the site shows confirmed after the next publish. Measure and
  record the time from send to confirmed.

## Sprint 11: LLM and organizer commands

- OpenRouter client on the outbound worker, model id in settings, prompt with
  grammar + sender's visible events, one-line answer re-parsed, budgets, timeout,
  `llm_ok`, admin disclosure text.
- Organizer DSL: `new event` (guided, multi-message, with `confirm`), `cancel`,
  `invite`, `remind`, all needing `confirm <code>`.
- **Exit:** with a mock model returning hostile output (a command for an event
  the sender can't see, an admin command, garbage), nothing changes without a
  valid parse and permission. A small live sample against the real model is
  recorded with its accuracy, by the owner.

## Sprint 12: the optional relay

- `party-relay` template repo: Worker + Durable Object, `/cmd`, `/inbox`, `/ack`,
  `/cal`, `/health`, limits, Deploy button.
- Board: relay channel (polled), MAC verification, nonce ring, ack push, opt-in
  calendar push.
- Companion: relay form when the manifest lists a relay, works with only a personal link.
- **Exit:** an email-only person answers from their link through the relay. A
  forged MAC and a replayed command are rejected by the board. Removing the relay on the
  board falls back cleanly to DMs. Done with `wrangler dev` locally. A live deploy
  is the owner's step.

## Sprint 13: hardening and v0.1 release

- Bench load: a party night of 8 phones on HTTP + 2 on HTTPS/h2, polling and bursts
  of answers, while the bot polls and a publish runs. 10 minutes.
- Power-cut test: cut power during commits ×20. Every boot is consistent, and
  quarantine is empty or explained.
- Commit-rate measurement and partition life projection.
- Accessibility pass on the RSVP form and event page.
- `party/DEPLOY.md` runbook and the install guide part 2 (bot, Brevo, optional relay).
- The acceptance run below.

## v0.1 acceptance

1. A fresh board shows setup. The first visitor becomes admin with a PIN.
2. A household member signs up on the LAN, creates a circle "Family", and makes
   the son a co-organizer.
3. The organizer creates households for the son's house and Grandma's, and an
   email-only person for Grandma.
4. A potluck with a bring list, plus-ones up to 2, capacity 14. Invitations go out:
   DM to the son, email to Grandma with a calendar invite and her personal link.
5. The son redeems remotely by Mastodon, answers by DM ("going +1, bringing
   pie"), and gets a reply. The house sees it at once on `party.local`.
6. Grandma opens her link on her tablet and sees the event and who's coming.
   She phones, and the host answers for her ("by Sam, via email-proxy").
7. A weekly "Game club" series in a public group shows on the companion site
   with no private data. Its public `.ics` imports.
8. The published tree contains nothing classed above `public` in plaintext.
9. With the internet unplugged, the house keeps working. On reconnect, queued DMs,
   emails and the publication catch up, each exactly once.
10. A power cut during use loses at most the change in flight.
11. The demo site shows all of the above with fictional data.
12. A member can't edit others' answers (outside household/host rules), can't see
    private groups, and can't reach admin endpoints (API tests).
