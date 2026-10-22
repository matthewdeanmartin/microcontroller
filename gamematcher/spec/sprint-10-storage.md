# Sprint 10: per-record storage

Status: **built October 21, 2026**, desktop-tested. Firmware not yet built or
flashed. Run it **before** Sprint 9 (hardening): Sprint 9's power-cut and
flash-wear tests should measure the store v1 ships with, not the old one.

## What was built, and where it differs from the plan below

- `api/src/table.rs`: `Table<T>`, a list that tracks changed rows and keeps
  undo entries, with no `DerefMut`, so the compiler found every write. Undo copies only the
  rows a change touches. Bulk loops (game delete/merge, series linking) now
  touch only rows that mention the game or plan.
- `api/src/records.rs`: the record store. Keys `meta`, `idx`, `a<id>`,
  `g<id>`, `p<id>`, `v<id>`, `s<id>` (one account's sessions), `tx`, `cnt`.
  One record is written directly. More than one goes through the `tx` journal
  and is replayed at boot.
- **Changed from the plan:** no framework `kv-tables` enumeration. An `idx`
  record lists which rows exist, written only when a row is added or removed,
  so the framework `Kv` stays as it was. party can still add enumeration.
- **Changed from the plan:** notifications stay inside the account record and
  are written in the same commit as the change that caused them, rather than
  flushed lazily. That's stronger durability, and the record is small (≤ ~2 KB).
  Mark-read now writes one account record instead of the whole state.
- **Changed from the plan:** the commit count lives in `cnt`, saved by
  housekeeping at most once a minute, so it isn't a second write on every commit.
- **Not done:** the outbox still rewrites its whole queue (`outbound.rs`). It's
  bounded at 32 jobs and the bot isn't set up yet. Left for later.
- **Not done:** the safety-copy endpoint. The owner said losing the data is
  acceptable (the game list can be re-entered, nothing is pending). Migration
  still runs automatically and leaves `snap0`/`snap1` untouched.
- Measured on desktop (`write_cost_does_not_grow_with_the_board`): response
  652 B → 659 B, star 165 B → 165 B, sign-in 692 B → 692 B, from a 2-account board to
  a full one (32 accounts, 256 games, 16 open plans).
  `power_cut_at_every_write_is_all_or_nothing` cuts power after each write
  of a journaled commit. Every reboot shows the change completely or not at all.
- Admin → Board now shows records, commits, last/largest commit bytes,
  migration and quarantined records. `gm_snapshot_bytes` in `/metrics` became
  `gm_last_commit_bytes` and `gm_max_commit_bytes`.

Still to do on the board: `make firmware`, `make update` (app-only, after
verifying the MAC), then check Admin → Board shows `migrated` once and the
commit sizes stay small.

## The bug

Every write costs O(whole state), not O(what changed). One `PUT …/response`
(a few dozen bytes of real change) does all of this, holding the state lock
throughout:

1. `mutate()` clones the entire state as an undo copy: `inner.state.to_stored()`
   (`api/src/api.rs`, `fn mutate`).
2. `commit()` encodes the entire state to CBOR: `wire::to_vec(…, &inner.state.to_stored())`,
   which is a **second** full clone plus the encode buffer.
3. `SlotStore::commit` writes the whole blob to NVS (`api/src/store.rs`). For
   the realistic 25–35 KiB that's about 10 sectors, worst case 128 KiB.
4. On any error, `State::from_stored(backup, …)` rebuilds the entire state.

So write latency, transient RAM (about 3× state on a board with no PSRAM) and
flash wear all grow with the number of accounts, games, plans and sessions,
even though each action touches one or two of them. Every request, including
reads and the screen's `display_deck`, waits on the same lock while the flash
write runs. Even marking notifications read (`POST /notifications/read`) pays
the full cost via `mutate(|_| Ok(()))`. The outbox does the same: `Outbox::save`
rewrites the whole `outbox` blob (and `bot`, `mail`) on every queue change
(`api/src/outbound.rs`).

[05 § Persistence model](05-storage-limits.md#persistence-model) made this
choice on purpose ("follows minicloud's scheme") and the bounds kept it
tolerable at 4 people. It is still the wrong shape: cost has to scale with the
change, not with the board's contents.

## The fix

Move to **keyed records in the `gmstate` NVS partition**, mastomini's style
(`mastomini/mastomini_rs/src/store/`), which party will use too
([party/spec/07](../../party/spec/07-storage-limits.md)). Domain state stays
fully in RAM, as it is now. Only persistence and the undo path change.

### Records

| Key | Holds | Written when |
|---|---|---|
| `meta` | schema version, id counters (`next_*`), `signup_closed`, `house_feed_hash`, commit count | counters advance, settings change |
| `a<id>` | one account, including interests and notify settings | profile, PIN, interests, link/unlink |
| `n<id>` | one account's notification ring | dirty, flushed ≤ 60 s (below) |
| `s<id>` | one account's sessions (≤ 4) | sign-in/out; `last_seen` touch-only flush as now |
| `g<id>` | one game | add, edit, archive |
| `p<id>` | one plan with its responses and time options (open, or compacted done/cancelled) | create, edit, respond, lock-in, cancel, compaction |
| `v<id>` | one OAuth provider + invite list | admin edits |
| `c<id>` | one plan's comment thread | unchanged (already per-plan) |
| `ob<id>` / `pb<id>` | one outbound job (household / public bot) | enqueue, attempt, finish |
| `bot`, `pubbot`, `mail` | channel settings and cursors (without the queue) | setup, cursor advance |
| `tx` | redo journal for the rare multi-record change (below) | only during such a change |

All values are CBOR with integer keys (`message!`, append-only fields), as now.
Keys fit NVS's 15-character limit in base-36.

At current bounds that's at most about 32 + 32 + 32 + 256 + 144 + providers +
comments + 64 jobs ≈ 650 keys, well within a 512 KiB NVS partition.

### Writes

- `mutate()` gets a **change set** instead of a full undo copy. The closure
  works through `Tx`, which clones an entity the first time it's touched,
  records its key, and keeps the original for rollback. Commit encodes and
  writes only the touched records, then swaps them into the live state. An
  error discards the change set. Nothing is rebuilt.
- **One commit point per action.** Most actions touch one record (a response
  is `p<id>`, a star is `a<id>`). Where an action touches several, order the
  writes so the last one makes it real and boot can repair the rest:
  - New plan: write `p<id>`, then `meta` with the bumped counter. Boot treats a
    `p<id>` at or above the counter as committed and advances the counter
    (the `max(…)` logic already in `State::from_stored`).
  - Plan leaves the done ring: write the new state, then remove `p<old>` and `c<old>`.
    Boot drops orphans past the ring bound.
- **Multi-record changes** (admin game merge across every account and plan, admin
  delete-account, provider delete that unlinks identities): write a `tx` record
  listing every key and its new value (or a delete) first, then apply them, then
  remove `tx`. Boot replays a `tx` it finds. These are rare admin actions, so a
  `tx` up to ~16 KiB is fine.
- **Notifications** are derived bell items. They join the touch-only class
  (like `last_seen`): changed in RAM, written per account within 60 s. A power cut
  can lose the last minute of bell items, never a plan, response or comment.
  Email and DMs are unaffected, because they're queued as `ob<id>` records in the
  same commit as the change that caused them.
- **Mark-read** stops committing at all. It becomes a touch-only change.
- **The outbox** writes one `ob<id>` per job change, not the whole queue.
- **The lock.** Writes are now 0.1–2 KB, so holding the state lock across them
  is acceptable. Don't introduce a second lock in this sprint. Measure first.

### Boot

1. Read `meta`. If it's missing and `snap0`/`snap1` exist, **migrate** (below).
2. Load all records (`a*`, `g*`, `p*`, …). That needs key enumeration, which the
   framework `Kv` doesn't have (`get/set/remove` only). Add it in the framework
   as an optional feature (`kv-tables`: `list(prefix)` on NVS, file and memory
   backends) that party will reuse. Gamematcher is its first consumer.
3. Replay `tx` if present, repair counters and orphans, rebuild indexes and
   derived counts.
4. A record that fails to decode is **quarantined**: logged, reported on
   `/api/v1/admin/storage` and in ops health, and left in flash. Never silently
   dropped. If `meta` itself is unreadable, refuse to serve, as now.

### Migration (the household's data is on this board)

The board holds the daughter's data, so this ships as an app-only `make update`
and **must never require an erase**.

1. Before flashing, download a safety copy with a new admin endpoint,
   `GET /api/v1/admin/storage/snapshot` (the raw current slot, as stored).
   Keep it with `tools/` and document it in `DEPLOY.md`.
2. First boot of the new firmware: decode the newest valid slot with the
   existing `SlotStore`, write every record, **write `meta` last** (the commit
   point), and log the counts.
3. Leave `snap0`/`snap1` in place for this release. If the update has to be
   rolled back, the old firmware still finds its snapshot (stale since the
   migration, but intact). The release after this one deletes them.
4. A crash mid-migration: no `meta`, so the next boot migrates again from the
   untouched slots, overwriting partial records.

The schema goes to version 2. Old firmware never reads the new records, so
rolling back is safe for as long as the slots remain.

## Tasks

- [ ] Framework: `kv-tables` feature with `list(prefix)` for NVS, `FileKv` and
      the memory `Kv`, plus tests. (party Sprint 1 reuses this rather than adding
      its own.)
- [ ] `api/src/store.rs`: record store (keys, encode/decode, load-all,
      quarantine, `tx` journal, repair). Keep `SlotStore` for migration only.
- [ ] `api/src/state.rs`: dirty tracking per entity, touch-only class for
      notifications and read flags, `Tx` change set replacing the full
      `to_stored()` undo copy in `mutate`/`mutate_optional`.
- [ ] `api/src/api.rs`: `commit` writes the change set. Mark-read no longer
      commits. `GET /api/v1/admin/storage` reports records per table, bytes,
      quarantine, commits, and **bytes written per commit** (last and p95).
- [ ] `api/src/outbound.rs`: per-job records for both bots, settings split from the queue.
- [ ] Migration from slots, the safety-copy endpoint, `DEPLOY.md` steps.
- [ ] Spec: rewrite [05](05-storage-limits.md) (persistence model, write budget,
      RAM budget) to match, and mark the old scheme as superseded.

## Tests

- **The regression test for this bug:** a counting `Kv` wrapper records bytes
  written. Seed state at 1 account / 1 game / 1 plan, then at full bounds
  (32 accounts, 256 games, 16 open + 128 done plans, 64 sessions). The bytes
  written by one response, one star, one comment and one sign-in must be the
  **same at both sizes** (within the size of the touched record). This test
  fails on today's code and must pass after.
- Same shape for allocations: peak heap during a response at full bounds stays
  under 8 KiB above idle (desktop allocation counter).
- Rollback: an error halfway through a multi-entity mutation leaves RAM and
  flash both unchanged.
- Crash points: kill after each write of a new plan, a ring eviction, a merge
  `tx`, and mid-migration. Every reopen is consistent (fault-injecting `Kv`
  that fails the Nth `set`).
- Migration: decode a snapshot produced by the **current** release (check one in
  as a fixture, made from seeded data, not the household's), migrate, and compare
  every entity with the old in-RAM state.
- Quarantine: a corrupted `g<id>` loads everything else and is reported.
- Outbox: 64 queued jobs, finishing one writes one record.

## Exit

- The bytes-written test passes, and `/api/v1/admin/storage` on the board shows
  bytes per response commit under 2 KiB at the household's real state.
- On the board, seeded to full bounds on a **spare** C6 or the desktop build
  (never the household board's data): `PUT …/response` latency, before and
  after, recorded in `spec/measurements/storage-v2.md`. It no longer grows with n.
- The household board takes `make update` with the safety copy saved first.
  Afterwards every account, game, plan and comment is present (counts compared
  with the safety copy), and people can still sign in with their PINs. Agents ask
  the owner before flashing, verify the MAC (`ac:eb:e6:1e:13:40`), and never erase.
- `make test` green, clippy/fmt clean, [05](05-storage-limits.md) updated.
