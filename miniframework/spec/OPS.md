# Ops: boards an SRE bot can read, and later act on

**Status:** Sprint 0 is deployed. Sprint 1 and the first Sprint 2 action path,
approval UI and bounded trend reader are deployed on bots, Mastomini and the
collector (2026-10-19). Configuration, operational acceptance and observation
remain open; deployment is not sprint completion. Backup/restore is deferred
to the end of the roadmap, without a sprint number. No Sprint 4 is defined.
Start here after a break: **Next work**, then **Where things stand**.

## Next work (reconciled after all three deployments)

**Hostname correction completed:** the attached S2 `80:65:99:f0:1c:9c`
now serves `housemetrics.local` (registry `s2`), with strict IP/name probes
and all seven UI assets verified. Homelab seeds restored. Receipt baseline
`housemetrics.local=6f335bf+`; image SHA distinguishes this configuration
rebuild from the prior v2 image. The old S2 remains offline for repurposing.
The identification/flash steps in item 1 below are now complete; saved bot
settings still need inspection. Older v2 deployment notes are historical.

### Finish Sprint 1: configured, trustworthy observation

1. Owner correction: retain **`housemetrics.local`** for the one permanent
   collector; the second identical-looking S2 will be repurposed. The recent
   `housemetrics-v2.local` rename was an incorrect role assumption. Keep the
   SRE source default `housemetrics.local`. Identify the intended physical S2
   by MAC, reconcile its build profile/certificate, then perform an authorized
   app-only hostname correction and strict probe. Only after verification,
   reconcile saved target/collector settings and homelab seeds with the final
   name. Do not give both boards the collector identity. As of this correction,
   the attached USB device is Mastomini COM11 (`ac:a7:04:2c:29:9c`), not an S2;
   no hostname correction has been flashed. The replacement S2 last flashed
   was `80:65:99:f0:1c:9c`; the original S2 is `80:65:99:f0:7b:68`.
2. Configure the posting key, optional separate `admin:read` counts key,
   and verified-build baselines from deployment receipts. Check connections
   before enabling scheduled digests. No activation or live configuration
   was performed during flashing; inspect actual settings before claiming
   their current values. Keep approved-action execution off at this stage.
3. Observe normal and representative loaded use: internal free/minimum heap,
   largest block, allocation failures, TLS and scrape failures. Check that
   the collector retains paired heap/uptime series for the intended hosts
   within its 64-series budget. Do not add targets blindly.
4. Verify daily digest/history and weekly counts/drift behavior, including
   unavailable data and retries. Activation/posting is a separate operator
   step, not part of read-only deployment verification. Accumulate seven
   days of history and the planned roughly two-week observation period.
5. NanaCoin's build identity fix still awaits its own build/deployment.
   Keep its unknown build explicit meanwhile. SD byte-format adoption and
   Meritledger thresholds remain with that repository's owner.

Exit: correct targets, connection checks pass, no persistent false alarms,
history/digests verified, and measured memory behavior recorded. Small free
heap is an accepted operating constraint; zero spare memory is not assumed.

### Finish Sprint 2: accept the collector action, then expand deliberately

1. Provision the existing collector safe/approval secrets into SRE settings
   privately; choose the collector as the single action board. Verify catalog
   and dry run with execution disabled. Check approval expiry/cancellation,
   wrong-account/code rejection and uncertain outcomes in desktop tests.
2. With explicit approval for a real maintenance window, exercise one planned
   collector restart. Verify scraper quiescence, connection drain, retained
   planned reason, recovery, and handling of a lost response without duplicate
   execution. Collector history is volatile and will be lost on restart.
   Measure bots/collector heap during the real approval and execution flow.
3. Validate trend freshness and shared-outage suppression using controlled
   fixtures; do not disrupt household networking to create failures.
4. Remaining Sprint 2 implementation: domain checkpoint integrations after
   their owners verify write quiescence; bounded long-job/status support only
   for an installed action that needs it; paging policy and a chosen delivery
   channel. Current digest code explicitly never pages. The four-entry audit
   refuses new work when full and is boot-local, not the proposed rolling log.
   Preserve replay protection if extending it.

Exit: the one installed action works safely on hardware with evidence;
remaining catalog/job/paging scope is explicitly implemented or deferred,
not implied by the presence of the approval UI. Mastomini's catalog stays
empty until there is an independently justified app integration.

### Last, deferred: optional backups with demonstrated restore

Owner moved all backup/restore implementation after the other SRE work.
This is not the next sprint. There is no existing Sprint 4 in this spec;
future non-backup sprint scope must be chosen explicitly. The bots board
has no SD card per the owner. Meritledger is the only documented SD-equipped
board; this does not establish it as an available shared backup destination.
Do not assume suitable dedicated backup hardware is available.

Build the optional framework backup contract and desktop reference tests:
bounded streaming export, verification and age; a completed verified backup
advances freshness, while partial/failed exports do not. Define retention,
storage exhaustion behavior and access control before installing `backup.now`.
Extend the typed action catalog only alongside a working bounded adapter.
Add `backup.age` and a desktop restore check of exported records/invariants,
not merely a successful write or matching counts.

Meritledger/SD is the planned first deployment, but **do not work in
`merit_ledger`** while its other developer owns it. Prepare the framework
contract and integration handoff; actual SD backup/restore acceptance waits
for that owner. Other boards keep backup support off until a storage
destination and verified adapter exist. No new storage board is assumed.

### UI and board scope

SRE configuration, history and approvals belong in the existing bots Angular
admin app. Housemetrics remains the metrics dashboard. Keep Mastomini changes
narrow: its deployed optional admin counts endpoint and small moderation
button already exist; no additional SRE console belongs there. Generic board
operations use `/.well-known/ops/*`; the existing counts API is app-specific.

The dated implementation notes below retain earlier validation history;
their pre-deployment statements are superseded by this section and the
individual deployment records.

## Proposed follow-on work: failure, response, evidence

Owner correction: design from failure scenarios and useful interventions,
then collect the evidence needed to choose them. High CPU or a growing count
alone does not establish a problem or a helpful response. These are proposals,
not implemented features. Backup/restore stays last.

| Failure / consequence | Useful response | Evidence required |
|---|---|---|
| A bot misses a scheduled household task | Repair auth/config, or retry once when safe | Expected deadline, last success, classified error, idempotency/retry state |
| A board approaches an unrecoverable memory condition | Approved drained restart, only if restart addresses the cause | Sustained heap/largest-block trend, allocation failures, uptime, quiescence capability; distinguish a stable small budget from a leak |
| NanaCoin cannot durably record a transaction | Alert the operator; preserve state; checkpoint only when safe | Write error, journal headroom, existing integrity checks, checkpoint outcome; never modify balances automatically |
| Collector stops seeing a board | Distinguish board failure from failed collection before acting | Last successful scrape, independent health reachability, sample gaps and collector capacity |
| Several boards disappear together | Investigate shared power/network; do not restart each board | Same-window reachability and independent observer evidence |
| Mastomini scheduled work stops completing | Repair the specific scheduling/storage fault | Overdue work age relative to due time, operation errors, clock validity |
| TLS certificate is about to expire | Renew from the existing household CA and deploy in time | Expiry, affected identity, deployment lead time; never rotate the CA merely to clear an alert |
| Suspicious authentication or ops requests occur | Review bounded evidence; decide whether to revoke a key | Failure burst, successful privileged changes, invalid/replayed grants; failures alone do not prove compromise |
| Installed SDK has a relevant known defect/security fix | Prepare a tested household build and approved upgrade | Exact deployed SDK/chip/config, affected version range, enabled feature, advisory and fixed release |

### DevOps bot: monthly upstream review, then approved upgrades

Owner proposes monthly polling of manufacturer updates. Official sources:
[ESP-IDF releases](https://github.com/espressif/esp-idf/releases),
[ESP-IDF security advisories](https://github.com/espressif/esp-idf/security/advisories),
[Espressif advisories](https://www.espressif.com/en/support/documents/advisories),
and [support policy](https://github.com/espressif/esp-idf/blob/master/SUPPORT_POLICY.md).
These are SDK, chip and support notices, not interchangeable ready-to-flash
household application images. Check relevant Rust binding/framework releases
as well when planning an SDK change.

A useful finding says: affected boards, what can fail, whether the vulnerable
feature is enabled, proposed fixed version, compatibility/test work, and an
upgrade path. Unknown applicability stays unknown. A new major version alone
is not a reason to upgrade. Deduplicate already reviewed notices and record
last successful check; failed polling must not report "no updates". Monthly
is the requested routine cadence; an urgent advisory need not wait for that
review if received separately. No scheduled job has been installed yet.

Build and test on the PC, produce a private per-household artifact and receipt,
then offer an approved deployment. OTA is a desired follow-on, with an existing
unimplemented design in `spec/OTA.md`. Current collector, bots and Mastomini
partition tables are factory-only: no OTA path is available now. Reconcile
that older design's board inventory and flash sizes before selecting a pilot.
OTA needs an eligible spare slot, authenticated/signed compatible images,
trial-boot health validation and rollback, plus storage-schema compatibility.
A repartitioning/bootloader migration is a separate reviewed USB operation,
not part of this hostname correction or an ordinary app-only update.

Suggested sequence after Sprint 1/2 acceptance: actionable domain runbooks,
then upstream release/security review, then a separately scoped OTA pilot.
Do not add separate bots merely for labels: capacity, security, data quality
and incident timelines can begin as sections of the same evidence/report flow.
SRE owns runtime response; DevOps owns changing the deployed software.

A dedicated SRE board could isolate it from bot-host failure and contention.
It needs no SD card for bounded state, but shares Wi-Fi/power unless deliberately
separated. An independent observer must detect its own missed heartbeat.
No new board or extra polling workers are required for this planning work.

## Where things stand (2026-10-19)

**Mastomini deployment:** owner-authorized Procedure A on COM11, MAC
`ac:a7:04:2c:29:9c`, passed app hash, documented boot-log, strict IP/name
TLS probes, all 21 UI asset comparisons and working-tree fingerprint check.
Boot reports 18 accounts, 120 posts, zero repairs. Health is ok under the
calibrated thresholds: 8 KiB free, minimum 6 KiB, zero allocation failures;
loaded-memory acceptance remains pending. Admin digest requires authentication,
and the action catalog is empty. Baseline `mastomini.local=81fa4917af49`.
See `mastomini/mastomini_rs/DEPLOY.md`; no bot was activated.

**Collector deployment:** replacement S2 `80:65:99:f0:1c:9c` now serves
`https://housemetrics-v2.local` at `192.168.1.157`. Guarded app-only update,
flash hash, strict TLS and all seven UI assets passed. Health is ok at one
minute: 34 KiB internal free, minimum 30 KiB, zero allocation failures.
Approval-required restart is advertised but was not executed. Receipt baseline
`housemetrics-v2.local=6f335bf+`; see `apps/housemetrics/DEPLOY_S2.md`.
Update configured SRE target/collector URLs to the new name before use;
bot settings and enable flags were not changed during deployment.

**Latest deployment:** mastomini-bots Sprint 1/2 image is now on COM15's
board `ac:a7:04:2c:38:b8`, at `192.168.1.162`. App-only flash/hash verification,
boot log, strict IP/name TLS probes and all 10 Angular asset comparisons passed.
15 bots and the existing admin password were retained. Startup health is ok:
54 KiB internal free, largest block 31 KiB, minimum 8 KiB, no allocation failures.
No settings or enable flags were changed, no ops action was run, and no other
board was flashed. Receipt baseline: `mastomini-bots.local=1792455558566`.
Full record: `mastomini/mastomini_bots/DEPLOY.md`. The remaining deployment
and loaded-memory acceptance work below still applies.

**Deployed with the `ops` feature.** Each board's DEPLOY.md has the record.

| Board | Address | Health on first reading |
|---|---|---|
| mastomini | 192.168.1.161 | **fail**: `heap.internal` 8 KiB free (lowest 6 KiB), `heap.largest` 6 KiB |
| mastomini-bots | 192.168.1.162 | ok (48 KiB free) |
| nanacoin (S3) | 192.168.1.158 | ok; Wi-Fi −73 dBm, 2 dB from warn |
| gamematcher | 192.168.1.163 | ok |
| meritledger | 192.168.1.151 | ok; SD card present |
| housemetrics #2 (S2 `…:1c:9c`) | 192.168.1.157 | warn: `heap.internal` 31 KiB, normal for an S2 |

- **The `sre` bot is on mastomini-bots but switched off.** To start it: give it
  a Mastodon key in the bots admin site, press **Check connections** (it lists
  each board's status without posting), then turn it on.
- The original housemetrics board (`80:65:99:f0:7b:68`) is offline. The S2 that
  was the nanacoin-s2 test bank now runs housemetrics in its place; the
  nanacoin S2 bank is retired.

**Findings from the Sprint 0 deployment** (local fixes and remaining rollout
work are recorded under **Sprint 1** below):

1. **`tools/flash.sh` guard did not fire.** It read the old partition table
   successfully but found no `ledger`, and flashed housemetrics over the
   nanacoin-s2 bank (harmless this time: test data, owner retiring it). It
   also never compares the MAC with the app's expected board. Fix before the
   next S2 flash: require the expected MAC (`--confirm-mac`) and refuse an
   unknown or empty table.
2. **mastomini's heap thresholds.** Either 8 KiB free is its normal level
   (then set mastomini's own `Config::ops`) or it is a real problem. Decide
   from housemetrics history before turning the bot on, or the digest says
   "mastomini fail" every morning.
3. **Dead scrape target.** `apps/housemetrics/config/scrapes.json` still lists
   `minicloud.local`, which was repaved into gamematcher. It fails every 30 s.
4. **Hostname clash.** If the original housemetrics board returns, both claim
   `housemetrics.local`. Give the S2 its own hostname when it gets its own
   role (see `C:\github\IDEAS.md`: auditor, crawler).
5. **Cosmetic.** `Check::usage` prints raw numbers ("507904 of 31251120128"
   for the SD card): round bytes to KiB/MiB/GiB. NanaCoin's health reports
   `build: dev`: its `Config` sets no build fingerprint.
6. **Records.** Update `DEPLOY_S2.md` (the S2 that runs housemetrics now),
   nanacoin's runbook (S2 bank retired) and homelab's `seed-manifest.json`.

### Earlier October 19 follow-up: initial findings and guard fix

- **Flash guard fixed locally:** `flash.sh` requires `--confirm-mac` (Make:
  `CONFIRM_MAC`), fails on a failed chip probe, and validates partition-table
  structure/checksum and exact agreement with the built layout. Empty and
  unknown tables are refused. `--replace-nanacoin` cannot bypass identity or
  layout checks; a different layout needs a separate reviewed migration.
- **Mastomini thresholds remain unchanged.** Housemetrics returned 64 raw
  samples over approximately 31.5 minutes, starting at Unix ms 1792447625801.
  Free internal heap ranged from 7,587 to 8,975 bytes; the low-water mark was
  6,167 bytes, largest block 6,912 bytes. The live health check reported zero
  failed allocations. Retained incidents show low heap by 10 seconds after
  boot and the 6,167-byte minimum by 295 seconds. This is a persistent low
  budget, not evidence that lowering fail thresholds is safe under load.
  Keep the bot disabled pending memory-budget review and a representative
  workload observation; do not calibrate away this finding from idle data.
- **New collector capacity blocker:** `/api/v1/series` reports 64/64 series.
  Review the S2 memory/rollup budget before adding gamematcher or other
  targets. Missing series cannot be treated as evidence of good health.
- Removed the dead `minicloud.local` managed scrape from the source config;
  it is removed on the next deployment's startup reconciliation. No live
  collector configuration was changed.
- Updated `DEPLOY_S2.md`, NanaCoin's `DEPLOY.md`, and homelab seeds. Removed
  retired `minicloud.local`, `nanacoin-s2.local`, and the currently unused
  `housemetrics-v2.local` seeds. The board registry still describes the old
  primary/test-bed arrangement; reconcile it with the permanent role before
  an app-only update. Hostname resolution remains an open deployment item.
- No flash, erase, reset, bot activation, or post was performed. The cosmetic
  fixes, planned-restart reason, and weekly digest
  remain outstanding. No work was performed in `merit_ledger`.
- Validation: `make check` passed with isolated `CARGO_TARGET_DIR=C:/tmp/mfw-sre-check`
  (log `.local/sre-check.log`), including 28 boardsafe tests. The updated
  housemetrics config test and both homelab seed tests passed. Bash syntax
  and missing/invalid MAC refusal were checked without touching serial ports.

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

### Local implementation (October 19; not deployed)

- Owner confirms the apps intentionally operate with little internal RAM.
  Mastomini now uses warn/fail free-heap floors of 6/4 KiB and a 4 KiB
  largest-block warning, based on the 7,587–8,975 B observed band and
  6,167 B minimum. Housemetrics warns below 24 KiB (16 KiB fail), versus
  about 31 KiB observed free. Bots uses 8/4 KiB free and 4 KiB largest-block
  floors: a later live reading had 14,183 B free, ~5.5 KiB low-water, 6,912 B
  largest block, zero failed allocations. NanaCoin (67,903 B free / 30,720 B
  largest) and gamematcher (93,972 B / 67,584 B) retain the default thresholds.
  Meritledger calibration stays with its owner. These are provisional operating thresholds,
  not a claim that OOM is impossible. Allocation failures still warn.
- `esp::Board::restart_planned(Reason)` is an asynchronous, typed request
  (`Heap`, `Maintenance`, `Configuration`). It stops new admission, drains
  existing clients and writes one RTC word immediately before reboot. If
  clients remain busy for 30 seconds it cancels rather than force-closing
  them. Apps must quiesce their own background writes before requesting it.
  No remote action endpoint or automatic restart is enabled in Sprint 1.
  Only software resets consume a valid marker as a planned reason; a crash,
  brownout or power-on clears it. Cost: 12 bytes of state plus 4 bytes RTC
  and a stack-local drain timer; no added task, buffer, flash write or text.
- Weekly digest is optional in the SRE bot's settings (off by default),
  running on the first daily run of each Monday-based UTC week. It adds two
  bounded reply posts: admin counts and firmware drift. Pending content and
  parent ID survive retries; the completed week advances only after both
  posts succeed. Counts contain no names, report text or invite secrets.
  The SRE card shows the last completed digest on demand.
- Mastomini's private `GET /api/mastomini/v1/admin/digest` requires an admin
  role and `admin:read` scope. It computes unresolved reports and invites
  expiring within seven days directly from existing state. Pending approvals
  are explicitly zero: the app is invite-only, with no approval queue.
  Moderation's **Show admin counts** reads it on demand. The bot uses a
  separate read-only admin key on its configured Mastodon server; the posting
  key is not elevated. Missing keys/unavailable counts are reported explicitly.
- Disabled/failing bot counts are captured locally when queuing the SRE job;
  it never calls its own HTTP server. Firmware comparison uses **Verified
  firmware builds**, one `host=build` per line. Missing/`dev` values count as
  unknown, not matching. NanaCoin now reports a timestamped build-instance
  identity, rather than `dev` (not a content hash).
- Boardsafe `probe --receipt FILE` saves a verified runtime baseline;
  `update --receipt FILE --verify-address IP` probes after the update and
  also records the deployed image SHA-256. Copy the receipt's `baseline`
  into the bot settings. This is a local record of an observed runtime build
  and checked identity/assets, not remote attestation of firmware bytes.
- The next `BOARD=s2v2` build is the replacement collector, with the unique
  `housemetrics-v2.local` hostname and normal scrape list. Change the bot
  target and homelab seed only after that board passes its deployment probe.
- `Check::usage_bytes` formats B/KiB/MiB/GiB without changing percentages or
  stable check IDs. Adopting it for `sd.space` is left to the `merit_ledger`
  owner; this task does not edit that repo.

### Rollout and observation still required

Validated local artifact copies (not deployed), under
`C:/github/mastomini/mastomini_bots/.local/sprint1-20261019/`:

| Image | Bytes / app partition | SHA-256 |
|---|---|---|
| `mastomini-esp32.bin` | 2,608,704 / 4,194,304 | `04d0a6a11ab5bfe18ba8ad0d064602254c9f6be3811cbf1bd4a75edbe1a20f2d` |
| `mastomini-bots-esp32.bin` | 2,142,976 / 4,194,304 | `3ad5d7e99399a03105ea4986339ba3ab1d6f3944d1ac07fbef8f23f88a0038ac` |
| `housemetrics-v2.bin` | 1,663,952 / 4,063,232 | `903e050660f473cb67fb4f9097e90e14c7443e9376d0c1df221938c5ac8ca231` |

The bots/main images passed their firmware image generators; housemetrics
also passed `boardsafe-image --board s2v2`. The collector build reused the
household Wi-Fi settings in the build process, without changing its `.env`
or printing secrets. Snapshots contain firmware credentials: keep them local.

Validation: framework `make check` passed with serialized Rust tests after
one earlier localhost timeout under parallel compilation; 150 framework
ops/HTTP2 tests and strict ops Clippy passed. Bots: 152 Rust tests and Clippy;
Mastomini: 249 unit + 10 property tests, Clippy, smoke, 27 client tests,
139 conformance passes (58 skips, 13 expected failures). Both UI suites and
production builds passed (bots 16 tests; Mastomini 56). NanaCoin's tests and
smoke passed; Minicloud's tests and HTTP/MQTT smoke passed. Boardsafe: 33
tests. Firmware builds compile the final RTC/handshake-drain code for both
S3 and S2. Actual planned resets and live heap impact remain untested.

After operator approval, use **app-only** updates, verifying physical chip,
MAC and partition layout before each write. Deploy mastomini's counts API
and thresholds, then the collector with its unique name, then bots. Run
strict probes and save receipts; update the SRE target to the collector's
new name and adjust the homelab seed. Keep SRE disabled until its connection
check succeeds with its configured keys. NanaCoin's fingerprint change is
source-only here, pending its own next build/deployment; it is not one of
these three images. The SD summary call-site belongs to the other developer.

No boards have been flashed or reset by this work. Configure the SRE key,
optional read-only admin key, and verified-build baselines after deployment;
use **Check connections** before enabling. A two-week observation window,
live heap measurements and further per-board calibration cannot be replaced
by desktop tests. The collector remains limited to 64 series; do not add
scrape targets until its storage budget is reviewed.

- Close the open items above, mastomini's thresholds first.
- **Per-board thresholds:** each app sets `Config::ops` from measurements, not
  the framework defaults. The calibration source is housemetrics history (or
  the load/threshold calibrator in `C:\github\IDEAS.md`).
- Run sprint 0 for about two weeks, then tune thresholds until a quiet morning
  is really quiet.
- **Planned restart with a recorded reason** (framework, built locally):
  - `Board::restart_planned(reason)` drains connections and writes the reason to
    RTC memory before restarting.
  - The `reset` check then says `planned: heap` instead of `software restart`.
    Without this, the bot would report its own fixes as crashes.
- **A health history on the bot (built locally, not deployed):** latest
  observation per UTC day for the current targets, retained for seven UTC
  days. Up to twelve boards; one status byte per board/day, no check summaries.
  Stored in the bot's existing durable state, capped at 4 KiB encoded (normal
  household names use much less). Existing job/state copies still contribute
  to transient memory; this is not a 4 KiB total peak-RAM claim. No separate
  resident cache or timer. Failed posts retain observations without advancing
  the digest's last-successful statuses. Connection checks remain read-only.
  The SRE card loads the session-protected `/api/v1/bots/sre/history` only
  when requested; dates/times are UTC and missing observations are explicit.
  Reordering targets preserves history by hostname; removed targets leave
  the history, and added targets have unknown prior observations.
  Validation: 149 bot Rust tests, Clippy with warnings denied, 16 UI tests, and production Angular build
  passed. No firmware build/deployment or live-board RAM validation yet.
- **Admin digests** (weekly, in the app plus the bot; built locally):
  - unresolved mastomini reports, pending approvals, invites about to expire
  - bots that are switched off or failing
  - firmware drift: `build` compared with what `boardsafe` last deployed

## Sprint 2: actions behind approval

### Local implementation, October 19

The UI lives in **mastomini-bots' existing Angular admin app**, `/app/`,
under the SRE bot: on-demand health history and pending-action review.
Housemetrics keeps its existing Angular metrics dashboard. The bots board's
public bot-profile/status HTML is separate from its Angular admin app.

- With `ops`, every newly built board serves an actions catalog. It is empty
  unless its app explicitly supplies `Config::ops_actions`. No action is
  inferred from a health finding, model response or runbook name.
- `ops_actions::Gate` supports explicitly installed checkpoint or planned
  restart callbacks, an HTTPS safe bearer, and a **different** approval
  bearer. The safe key can dry-run but cannot approve disruptive work.
  `POST /.well-known/ops/approvals/restart-planned` with the approval key
  issues a random, five-minute, boot-local grant bound to the exact action
  and `Idempotency-Key`. Execution uses the safe bearer plus `X-Ops-Approval`.
  Only empty parameters are supported; unknown actions and queries fail closed.
- Four fixed audit/idempotency entries retain accepted or uncertain outcomes.
  Unlike the proposed rolling audit below, this first version refuses new
  work when full rather than evicting replay protection. Audit and cooldowns
  are volatile; a reboot invalidates every grant. A callback failure is
  uncertain and never invokes the callback again for that key.
- The bot keeps **one** pending action in its own durable `sre.action` record,
  capped at 2 KiB. Catalog inspection and dry run precede review. Approval is
  tied to its random id/code and displayed UTC window. It saves `executing`
  before network I/O; interrupted or lost-response executions require operator
  inspection, with no automatic retry. Store-write failure prevents dispatch.
- The authenticated Angular page can prepare, approve or cancel. Optional
  mention approval requires the configured exact account **ID** on the same
  instance, a reply to the recorded digest, and `approve <one-use-code>`.
  The private admin page displays the code and digest id. Mentions are polled
  at most once a minute while approval is pending; nothing is posted just to
  solicit approval. Other accounts, threads, expired or consumed codes do not
  authorize anything. The posting key needs permission to read notifications
  if this optional path is used.
- Execution requires both the SRE bot and **Execute approved actions** on.
  Both remain off. Defaults contain no board keys or action board. Exactly
  one configured HTTPS action board must also occur in the health targets.
- The UTC quiet window is 30 minutes, default 02:00. A fresh declining-heap
  estimate can prepare a proposal, **never approve it**, if the window starts
  at least 15 minutes before the predicted warning threshold. Its deadline
  is shortened to five minutes before that estimate. Existing result records
  are retained rather than repeatedly proposing from the same observation.
- Optional collector reads cover up to four hosts, sequentially: paired heap
  and uptime, 90 minutes, at most 192 raw samples per series (the managed
  targets scrape every 30 seconds). Reject paged/bucketed, ambiguous, stale,
  nonfinite, misaligned, gapped, reboot-spanning or noisy data. Require at
  least six samples over an hour, a decline of 256 B/h, R² ≥ 0.8 and a crossing
  within 24 hours. The endpoint currently publishes the **warning threshold**,
  not a hard OOM floor; the digest labels the estimate accordingly. Faster
  series exceeding the page bound remain unknown. Missing series do not mean
  healthy, and the collector's 64-series budget is unchanged.
- Multiple unreachable boards in one health batch produce one shared-outage
  finding, suppress individual down-board replies and prepare no trend action.
  Before an approved execution, the worker checks reachability again; two
  failures suppress execution. Correlation alone does not establish AP failure.

**Initial board integration:** housemetrics alone has the planned-restart
callback. Provision `HOUSEMETRICS_OPS_SAFE_KEY` and
`HOUSEMETRICS_OPS_APPROVAL_KEY` together in its local build environment,
using distinct random secrets of at least 32 bytes. Without them its catalog
is empty and no gate is allocated. This sprint generated the collector's two
64-hex-character keys in ignored `.local/ops-housemetrics.env` and built its
image with them. They were not printed or sent to a board. Keep that file
private and explicitly load it into the environment for future collector
builds; normal builds without it intentionally have an empty catalog. After
deployment, copy those values into SRE's separate safe/approval key fields
and choose `https://housemetrics-v2.local` as the action board. Leave execution
off until live verification. Its scrape worker uses one atomic byte to pause before restart;
a scrape already in progress refuses the action. Self-sampling pauses too,
and work resumes if connection draining times out. Restart loses its volatile
metrics; the approval page warns about this. Recorded reason is maintenance.

Other apps advertise empty catalogs until their owners implement and verify
their own quiescence/checkpoint hooks. No long-running action is installed,
so no job queue/task or jobs endpoint is enabled yet. Domain checkpoint and
long-job adapters remain follow-up work; do not mark those parts deployed.
No work was done in `merit_ledger`.

**Memory:** the gate is 392 bytes on the desktop test host and is allocated
only when provisioned; the inactive configuration adds one optional pointer.
The bot adds one optional pending-record pointer and a mention polling time,
with no new worker, timer thread, polling UI or cached time series. Transport
read limits are 8 KiB for health/actions, 16 KiB for approval mentions and
32 KiB for collector responses; these are checked while receiving, before
JSON parsing. These are bounded transient allocations, not claims of zero
RAM cost. Actual internal/PSRAM heap and largest-block impact still need
measurement on the boards before enabling actions.

Local validation: framework `make check`, 152 ops/HTTP2 tests plus its doc
test and strict Clippy; bots 161 Rust tests and strict Clippy; bots Angular
17 tests and production bundle. Mastomini's unit/property tests, smoke,
27 client tests and conformance (139 passed, 58 skipped, 13 expected failures)
passed. NanaCoin tests/smoke and Minicloud tests/HTTP/MQTT smoke passed.
No live action, reset, heap acceptance test, flash or bot activation was run.

### Firmware ready for the next board trip

Frozen copies of the final builds are in
`C:/github/mastomini/mastomini_bots/.local/sprint2-20261019/`, alongside
`SHA256SUMS`. These supersede the Sprint 1 snapshots for the three boards:

| Image | Bytes / app partition | SHA-256 |
|---|---|---|
| `mastomini-bots-esp32.bin` | 2,214,768 / 4,194,304 | `668b5cde8f71f7754d9004299e177c6b39b704f335bec7e3c2dea6de64997888` |
| `mastomini-esp32.bin` | 2,624,640 / 4,194,304 | `e8afd6b6f6e6efec77c68f27391fc72a537c9ed13d12623bc2ec243725db582a` |
| `housemetrics-v2.bin` | 1,682,592 / 4,063,232 | `395e7dce31ad93b3f4335a3d9fb7cf589234aa9443cd831a1c230b95eacfb6b7` |

All three firmware builds and image generation passed. The provisioned
collector image also passed `boardsafe-image --board s2v2`; its embedded ops
credentials were checked against the local provisioning file without printing
them. These are builds of the shared, uncommitted working trees, not releases.
Keep images and provisioning keys local. No physical board was accessed.

Next: obtain operator approval for app-only flashing, verify each chip/MAC
and layout, then probe the three boards and save verified receipts. Confirm
the collector resolves only under `housemetrics-v2.local`; update SRE targets,
collector URL and homelab seed accordingly. Check action catalog and dry run,
measure loaded heap/largest block, and only then consider enabling the bot
and approved-action execution. A real planned restart needs its own operator
approval and observation of connection drain, scraper pause/resume, boot
reason and recovery. Do not treat desktop tests as hardware acceptance.

The contract below includes later catalog entries and long-job support;
the limits above describe what is actually available now.

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
- **Correlation:** if several boards become unreachable in one observation,
  suspect shared network or power trouble. Report once and restart nothing;
  confirm the cause before attributing it to the access point.

## Deferred final work: backups, for boards that have storage

Originally Sprint 3; moved to the end by the owner. No backup/restore work
starts ahead of the remaining digest, action and paging work. No Sprint 4
was previously specified.

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
