# 05 — Storage and limits

## Persistence model

Follows minicloud's proven-on-this-board scheme (`mastomini/minicloud_rs/ARCHITECTURE.md`):

- The whole domain state is one compact binary snapshot (framework wire
  format, CBOR with integer keys, so it has a schema with versioned fields that only get appended).
- Two **slots**, stored as two blobs (`snap0`, `snap1`) in the framework `Kv`:
  NVS in a dedicated `gmstate` partition on the board, files on desktop, memory
  in tests. Each slot has a header: magic, schema version, generation, length,
  CRC-32. A write goes to the older slot and becomes current only once it is complete.
  NVS already replaces a blob atomically. The second slot and the CRC also protect
  against a corrupt but complete value, and give one generation of rollback.
- Boot reads both and takes the newest valid one. A torn newest slot falls back
  to the previous generation, losing at most the last change. If both are invalid,
  the board **refuses to start serving** and shows the error on the LCD and on
  port 8080 (framework startup-failure page). It never wipes silently.
- No raw partition code and no `unsafe`. NVS does wear levelling and
  power-loss-safe replacement. The partition must hold about 3x the largest
  snapshot (old + new + garbage collection), plus the comment blobs.

### Write budget (flash wear)

A commit rewrites the snapshot (≤ 128 KiB slot; realistic 30–40 KiB, about 10 sectors). Wear-levelling
across two slots means each sector sees every second commit. Budget:

- Mutations from people (responses, comments, plans): commit immediately.
- `last_seen`, notification read flags, interest toggles: **dirty, flushed
  within 60 s** (one commit for a burst).
- Flash rated 100k erase cycles per sector → 200k commits total for the board.
  At 200 commits/day that's about 2.7 years before trouble. That's acceptable, but if
  Sprint 9 measures more, the fix is a third and fourth slot (spread wear) before
  anything cleverer. The `/api/v1/admin/storage` endpoint reports the commit count.

Secrets that aren't domain state (session-signing salt, OAuth client
registrations, bot token) live in NVS via the framework `kv`.

## Bounds

Every collection is a fixed-capacity slab. When one is full the API returns
**409 `full`** with which bound, never evicts silently, except for the
explicitly ring-evicted ones below.

| Collection | Bound | Full → |
|---|---|---|
| Accounts | 32 | 409 |
| Sessions | 64 total, 4 per account | evict that account's oldest |
| Games | 256 (archived count) | 409 |
| Open + on plans | 16 | 409 |
| Done/cancelled plans | 128 | ring: oldest is dropped with its comments and responses |
| Responses | per plan: one per account (≤ 32) | — |
| Time options / games per plan | 6 / 8 | 400 |
| Comments | 64 per plan, 1,024 total | 409 per plan. Total ring-evicts the oldest *done* plan's comments |
| Notifications | 32 per account | ring |
| Outbound queue | 16 | 409 for new admin-triggered jobs. Notifications drop with an admin warning |
| Provider invite lists | 32 handles total | 409 |

### Size estimate (snapshot)

| | Each | Count | Bytes |
|---|---|---|---|
| Account | ~120 | 32 | 3.8 K |
| Session | 48 | 64 | 3.1 K |
| Game | ~110 | 256 | 28 K (realistic 80 → 9 K) |
| Open/on plan | ~250 + responses 40 × ≤ 32 | 16 | 24 K worst, ~5 K realistic |
| Done plan (compacted) | ~64 | 128 | 8 K |
| Comments | ~150 avg | 1,024 | 150 K worst |

Without comments the worst case is about 67 KiB, realistic 25–35 KiB, inside a 128 KiB slot.
**Comments do not go in the snapshot.** Each plan's thread is its own blob
(`c{plan id}`, ≤ 64 comments, about 10 KiB worst) in the same NVS partition. Posting
rewrites only that plan's blob, and the snapshot keeps per-plan comment counts.
This keeps the hot snapshot small and comments cheap to write.

Done-plan detail is compacted to title, chosen time, chosen games and "who was in"
when a plan becomes `done` or `cancelled`.

## RAM budget (C6, no PSRAM)

| | Budget |
|---|---|
| Domain state in RAM | ≤ 48 KiB realistic, ≤ 72 KiB at full bounds (comments not resident: read from the log per request, paged) |
| Snapshot encode | one in-RAM buffer per commit (NVS takes a whole blob), ≤ 96 KiB worst, about 20 KiB realistic. Commits are serialized |
| Request bodies | 4 KiB max (comment, plan) |
| Response budget | framework default 48 KiB unsent, streamed encoders for lists |
| Largest free block | ≥ 24 KiB at all times (the Sprint 0 gate) |

These are design numbers. Sprint 0 and Sprint 9 replace them with measured ones.

## Schema evolution

Fields are appended with new numbers and never renumbered (framework rule).
Snapshot header `schema_version` bumps on any semantic change. Boot migrates
forward in RAM and writes the new version on the next commit. Old firmware refuses a
newer schema rather than misreading it.
