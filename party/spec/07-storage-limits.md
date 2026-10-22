# 07 — Storage and limits

## Which store

The repo has three styles. party uses the **record store** (mastomini's), with
packed structs only for hot in-RAM indexes.

| Style | Used by | Fit for party |
|---|---|---|
| Whole-state snapshot, two slots | Game Matcher | No. Every RSVP would rewrite all state. At family scale that's ~40 KB, fine, but party has to scale to thousands of people, where it becomes hundreds of KB per answer: flash wear and a long stall |
| Packed fixed-size records in RAM | NanaCoin | For indexes only. It pays off when records are tiny, numerous and fixed-size. party's records are mostly variable-length text |
| Keyed records in NVS namespaces ("the NoSQL one") | mastomini | **Yes.** One small write per change, atomic per key, NVS does wear levelling and power-loss safety, and it's proven on the S3 |

### Record store

**Built in Sprint 1** (`api/src/records.rs`, `api/src/table.rs`). It is
Game Matcher's Sprint 10 store with one index record per table. Where this
section's original plan and the build differ, the build wins:

| Key | Holds |
|---|---|
| `meta` | counters and settings |
| `p<id>` `h<id>` `g<id>` `l<id>` | a person, a household, a group **with its members**, a block of 32 audit entries |
| `s<id>` | one person's sessions |
| `ip` `ih` `ig` `il` `is` | which rows of that table exist, in order; written only when a row comes or goes |
| `tx` | journal for a commit that writes more than one record; replayed at boot |
| `cnt` | commit count, saved by housekeeping |

- The framework `Kv` needed no enumeration: boot reads each table's index,
  then its rows. In RAM a table is a `Table<T>`, kept sorted by id (binary
  search) and tracking which rows changed and how to undo them. A commit
  writes only those rows.
- **A group's members live in its record** (≤ 500 × ~16 B), so a join
  rewrites one group record, never a separate membership table. That's
  128 B for a small group (measured on desktop).
- Adding a person rewrites `ip` (~3 B per person, ~6 KB at 2,000). That is
  the one write that grows with the board, and only on sign-up.
- Later sprints add tables the same way (events, RSVPs, invitations,
  inbox, outbox). Each gets its own prefix and index.

What the original plan said, for reference:

- The `partydata` NVS partition (7.5 MiB). One namespace per table: `cfg`, `person`,
  `house`, `ident`, `group`, `member`, `series`, `event`, `rsvp`, `invite`,
  `inbox`, `outbox`, `pub`, `audit`, `notif`.
- Records are `message!` structs encoded as **CBOR with integer keys**
  (framework wire format, append-only fields). Hand-written codecs, no
  reflection (TinyGo taught that lesson. Rust doesn't need it, but the rule stays).
- Keys within NVS's 15 chars: `p<id>`, `e<id>`, `r<event>.<person>` in base-36, etc.
- **No multi-key transactions.** Like mastomini, every command has one
  commit point, the record whose write makes the change real. Other writes are
  ordered so boot can repair them (e.g. write the RSVP, then the event's cached
  counts. Boot recomputes counts from RSVPs).
- **Inbox atomicity:** an RSVP record carries the inbox status id that caused it,
  so "state change + processed" is one write. The reply job is derived on boot if
  missing.
- **Boot** enumerates every namespace and builds the indexes in PSRAM. If a
  record fails its decode, it's quarantined (reported in `/admin/storage` and
  ops health), never silently dropped. If `cfg` is unreadable, the board refuses
  to serve, like Game Matcher.

(Superseded: the framework `Kv` keeps `get/set/remove`. Per-table index
records replaced enumeration; see above.)

Desktop runs the same store on files (one file per key, as `FileKv`), tests on
memory. A future SD-card backend implements the same trait.

### In-RAM indexes (PSRAM)

| Index | Entry | Purpose |
|---|---|---|
| people by id, by identity hash, by email hash | packed, ~16 B | sign-in, inbox sender lookup |
| membership | `(group u32, person u32, role u8, status u8)` 10 B | permission checks without reads |
| events by group and start | `(start u32, group u32, event u32, status u8)` | lists, calendars |
| RSVP summary per event | counts per status, heads, waitlist head | lists without reading RSVPs |
| processed inbox ids | ring of 4,096 u64 | dedupe |

Full records are read from NVS on demand. Hot ones are cached in a bounded PSRAM LRU.

## Write budget

People's actions commit immediately (one or two small records). Bookkeeping
(`last_seen`, notification read flags, publish hashes) is dirty and flushed
within 60 s. NVS spreads small writes across the partition's pages, so wear is
by total bytes written, not per-record. Sprint 13 measures the commit rate and
projects partition life. The target is > 5 years at family scale.

## Bounds (S3 profile)

Fixed capacities, 409 `full` when reached, no silent eviction except the rings.
Bounds are a per-board table chosen at build time, so a smaller profile can
exist later.

| Collection | S3 bound | Full → |
|---|---|---|
| People | 2,000 | 409 |
| Households | 256 | 409 |
| Groups | 128 | 409 |
| Members per group | 500 | 409 |
| Upcoming + draft events | 512 | 409 |
| Past events kept | 2,048 | ring, oldest compacted then dropped |
| RSVPs per event | 500 (heads: 2,000) | waitlist / 409 |
| Series | 128, occurrence window ≤ 8 | 409 |
| Module slots per event | bring 24, shifts 16, questions 4, when 6 | 400 |
| Open invitations | 1,024 | 409 |
| Sessions | 1,024 total, 6 per person | evict that person's oldest |
| Notifications | 32 per person | ring |
| Outbound queue | 64 | 409 for admin jobs. Notifications drop with an admin warning |
| Inbox processed ids | 4,096 | ring |
| Audit log | 1,024 | ring |

These cover the family and leave room for the internet stretch. Past ~2,000
people the design question is no longer storage but the bot's throughput
([04](04-channels.md#throughput-ceiling)) and publish size.

### Size estimate

| Record | Each | Realistic family | At full bounds |
|---|---|---|---|
| Person | ~250 B | 30 → 8 KB | 500 KB |
| Event | ~600 B (with modules) | 60 → 36 KB | 1.5 MB |
| RSVP | ~80 B | 600 → 48 KB | ~1.2 MB (active events) |
| Others | | ~30 KB | ~400 KB |

NVS overhead (32-byte entries, page headers, room for garbage collection) roughly
doubles that. Full bounds are near the partition's limit, which is why Sprint
0 measures it rather than trusting this table.

## RAM budget (S3 N16R8)

| | Budget |
|---|---|
| Indexes + LRU (PSRAM) | ≤ 1.5 MiB at full bounds |
| Outbound queue, inbox batch (PSRAM) | ≤ 256 KiB |
| Publication build (PSRAM) | one file at a time, ≤ 512 KiB buffer, streamed into gzip + AES-GCM |
| Internal RAM | TLS sessions (serving + 1 outbound), connection loop. Largest free internal block ≥ 32 KiB at all times (Sprint 0 gate) |

Design numbers. Sprints 0 and 13 replace them with measured ones.

## Schema evolution

Fields are appended with new numbers and never renumbered. Each namespace has a
schema version in `cfg`. Boot migrates records lazily (on read, rewritten on the
next write) or eagerly for small tables. Old firmware refuses a newer schema.
