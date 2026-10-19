# Ops: boards an SRE bot can read, and later act on

**Status:** sprint 0 built (read-only health and a morning digest).
Sprints 1–3 are designs.

## Why

The household has six miniframework boards: mastomini, mastomini-bots, nanacoin,
gamematcher, meritledger and housemetrics. Each has its own admin API, and they
all differ. A bot that knows each one would be six bots. Instead each board
answers the same few **well-known** paths. The SRE bot in mastomini-bots reads
those paths and needs no code per app.

The aim is an SRE that **fixes things or waits until morning**, and pages a
person only when it needs a decision from them. It doesn't just wake someone
at 3am.

## Rules

1. **Detection is rule-based.** Checks and thresholds on the board, plus trends
   from housemetrics, decide what is wrong. A model never decides that.
2. **The model only explains and correlates.** It reads findings and writes the
   headline. From sprint 2 it may pick a runbook, but only from the board's
   published catalog. It can never make up an action.
3. **Findings are untrusted data.** Check summaries, logs and Mastodon mentions
   go into prompts as data. Only the admin's own account can approve an action.
4. **Each piece goes where it belongs:**
   - the framework, when it is generic (heap, Wi-Fi, clock, restarts, backups)
   - the app, when it is domain logic (the nanacoin journal, gamematcher limits)
   - mastomini-bots, for anything that talks to Mastodon or a model
5. **Every feature is optional.** The C6 boards have no spare RAM for any of it.

## Roles

| Board | Role |
|---|---|
| every app | Serves `/.well-known/ops/health`: framework checks plus its own. |
| housemetrics | **Collects data.** It scrapes every board's `/metrics` into its RAM time-series store. The bot reads its query API for trends (sprint 2). It serves only the built-in health checks. |
| mastomini-bots | The `sre` bot: reads health, compares with its last run, posts to mastomini. Later it also runs approved actions. |
| a big storage board (future) | The backup destination for boards without SD cards. |

## Sprint 0 (built): read-only health

### `GET /.well-known/ops/health` (cargo feature `ops`)

This path is public and read-only, like `/metrics` and `/.well-known/incidents`.
A summary must be safe to show anyone on the LAN, so it never carries names,
messages or anyone's data. The response can be in any wire format; JSON is the
default.

```json
{
  "schema": "miniframework.health/1",
  "app": "nanacoin", "host": "nanacoin.local", "version": "0.1.0", "build": "…",
  "uptime_ms": 86400000,
  "status": "warn",
  "checks": [
    {"id": "heap.internal", "status": "warn", "summary": "20 KiB internal RAM free, lowest 14 KiB",
     "value": 20480, "limit": 32768, "unit": "B", "runbook": "restart-planned"},
    {"id": "ledger.journal", "status": "ok", "summary": "Journal records: 31% used (1270 of 4096)",
     "value": 31, "limit": 60, "unit": "%", "runbook": "checkpoint"}
  ]
}
```

- `status` is the worst check's status: `ok` < `warn` < `fail`.
- A check id is stable, because the bot remembers checks by id. Don't rename one
  that is in use.
- A check describes how things are **now**. The reader keeps the history:
  housemetrics stores it, and the bot remembers the last status.
- `/.well-known/miniframework` lists the path as link rel `health`.

**Framework checks:** `ops::builtin`, with thresholds in `Config::ops`.

| id | warn | fail | runbook |
|---|---|---|---|
| `heap.internal` | < 32 KiB free | < 16 KiB (`HEAP_LOW`) | `restart-planned` |
| `heap.largest` | largest block < 8 KiB | — | `restart-planned` |
| `heap.allocations` | any failed allocation since boot | — | `restart-planned` |
| `wifi.signal` (only with a station) | < −75 dBm | < −85 dBm | `wifi` |
| `clock` | SNTP has not answered | — | `clock` |
| `reset` | last reset was a crash or watchdog; brownout | — | `crash`, `power` |
| `server` | `starting`, `degraded` | `stalled` | — |

**App checks** come from `Service::checks()`:

| App | Checks |
|---|---|
| mastomini | `store.entries` (80% / 95%) |
| nanacoin | `ledger.storage` (a journal write failed → fail), `ledger.journal` (rotation at 50%; warn at 60%, fail at 90%) |
| gamematcher | `store.slot` (booted from the fallback snapshot), `store.accounts`, `store.games` |
| meritledger | `store.people`, `store.slot`, `sd.space` (only with a card), `sd.journal` (damaged files or torn bytes) |
| housemetrics, mastomini-bots | framework checks only |

### The `sre` bot (mastomini-bots)

- **Schedule:** daily at 07:30 by default. It reads each listed board's health.
  An unreachable board is `down`, and a 404 is `none` (the board doesn't have
  the feature on).
- **Posts:**
  - one headline, e.g. `SRE digest: 3 ok, 1 warn, 1 down. New: nanacoin. Recovered: gamematcher.`
  - one reply per board that isn't ok, worst first, naming its checks that
    aren't ok
  - followers-only by default
  - every post fits the server's limit (mastomini: 140)
- **Model headline:** optional. The model only sees the findings, and the
  rule-written headline is used whenever the model fails or there's no key.
- **Retries:** posts carry per-slot keys. Remembered statuses update only after
  every post is out, so a retried run says the same "New:".
- **Doesn't do:** it never pages, takes no actions, and doesn't read DMs. On
  mastomini a bot's key can't read DMs anyway.
- **mastomini-bots itself isn't a target:** the run would wait on its own server.

## Sprint 1: trust the digest

- Run sprint 0 for about two weeks, then tune thresholds until a quiet morning
  is really quiet.
- **Planned restart with a recorded reason** (framework):
  - `Board::restart_planned(reason)` drains connections and writes the reason to
    RTC memory before restarting.
  - The `reset` check then says `planned: heap` instead of `software restart`.
    Without this, the bot would report its own fixes as crashes.
- **A health history on the bot:** the last 7 days of statuses per board, shown
  on the bots admin site.
- **Admin digests** (weekly, in the app plus the bot):
  - unresolved mastomini reports, pending approvals, invites about to expire
  - bots that are switched off or failing
  - firmware drift: `build` compared with what `boardsafe` last deployed

## Sprint 2: actions behind approval

### `GET /.well-known/ops/actions`, `POST /.well-known/ops/actions/{id}`

```json
{"id": "checkpoint", "summary": "Rotate the journal now", "risk": "safe",
 "params": "CheckpointParams", "cooldown_s": 3600, "needs_approval": false}
```

- **Risk levels:**
  - `observe`: never listed.
  - `safe`: idempotent, e.g. checkpoint, `backup.now`, test mail.
  - `disruptive`: planned restart, maintenance mode.
- **Never listed:** anything destructive, such as nanacoin reset, reform, issue
  and retire; mastomini member reset and role changes; meritledger import; and
  OTA (`boardsafe` stays on the PC).
- **Requests:**
  - `?dry_run=1` says what would happen
  - an `Idempotency-Key` header is required
  - long jobs go through the `queue` feature: `GET /.well-known/ops/jobs/{id}`
- **Audit:** `GET /.well-known/ops/audit` keeps a bounded ring of who ran what
  and the outcome.
- **Auth:** an ops bearer token with a risk ceiling, generalized from nanacoin's
  `KeyScope`. The bot's token is capped at `safe`. Each `disruptive` action
  needs an approval.
- **Approvals:**
  - a pending-action page on the bots admin site, or
  - a mention reply from the admin's account with a one-time code
  - mentions from anyone else are only conversation
- **Trends:** the bot asks housemetrics for slopes, e.g. heap falling 2 KiB/h
  reaches the floor at 03:40. It then schedules a planned restart in the quiet
  window before that point.
- **Correlation:** if several boards drop Wi-Fi at the same moment, the cause is
  the access point. The bot reports it once and restarts nothing.

## Sprint 3: backups, for boards that have storage

Backups are **optional**. Only boards with an SD card (meritledger today) or a
future big storage board turn on the `backup` feature:

- a `Backup` trait in the app: `export`, `verify`, `age`
- the framework adds a `backup.age` check and a `backup.now` action
- boards without storage leave the feature off. Later, the storage board may
  pull their existing exports (mastomini `admin/export`, meritledger
  `export.json`).
- a restore test runs on the desktop build: import the newest backup and compare
  the counts

## Runbooks

| id | Trigger | Who | What |
|---|---|---|---|
| `restart-planned` | low or fragmented heap, failed allocations | bot (sprint 2: after approval) | Restart in the quiet window, before the predicted floor. Note it in the digest. |
| `wifi` | weak signal, disconnects | bot reports | Several boards at once → the access point. One board → move it or add an antenna. |
| `clock` | SNTP unanswered | bot reports | Schedules and dates are wrong until it's fixed; check the router's NTP/DNS. |
| `crash` | crash or watchdog reset | bot diagnoses, person decides | Attach `/.well-known/incidents` `previous` and the log tail. Repeats → page. |
| `power` | brownout | person | Supply or cable. |
| `storage` | a store filling up, failed writes, fallback snapshot, SD damage | bot reports, person decides | Never repair automatically. A failed nanacoin write pages. |
| `checkpoint` | nanacoin journal past rotation | bot (sprint 2, `safe`) | Run the checkpoint, then report the records freed. |

**Paging rule (sprint 2):** the bot pages only when users can see the outage,
no `safe` runbook applies, and the risk is data loss or a broken money
invariant. Everything else goes into the morning digest.
