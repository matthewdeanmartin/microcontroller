# 01 — Overview

## Goals

1. A household of about four plus occasional guests can propose, discuss and
   settle game sessions from their phones without leaving the house's Wi-Fi.
2. It works with the internet unplugged. Internet adds only optional
   extras (Mastodon sign-in on a public instance, Mastodon announcements).
3. A guest with the Wi-Fi password can join in a minute: open
   `http://gamematcher.local`, type an email and a PIN, done. No certificate
   install and no Mastodon account.
4. The board is an appliance: survives power cuts, never needs a shell,
   every data structure has a documented bound
   ([05-storage-limits.md](05-storage-limits.md)), and running out of room is a
   reported state, never a crash.
5. A glance at the board's screen answers "what's on, and do I owe anyone an
   answer?".

## Non-goals

- **Anything internet-facing.** No port forwarding, tunnels, public URLs,
  remote RSVP, or inbound email. (Owner decision 1.)
- **Matching or scheduling algorithms.** No "best time" solver and no
  availability intersection. The board shows counts, and people decide.
- **Per-user time zones.** One household zone, configured at build time.
- **Sending email.** No SMTP in v1. The email address is a login name. See the
  [roadmap](roadmap.md) for optional invitations later.
- **Defending against the LAN.** PINs stop a kid from casually acting as a
  parent. They are not a defense against a determined attacker on the Wi-Fi.
- **Federation, media uploads, multi-board clustering.**
- **Being secure against physical access.** Flash is not encrypted.

## Risk model

| Actor | Can | Must not be able to |
|---|---|---|
| Admin (first user, a parent) | Everything, including resetting other users' PINs | — |
| Household member | Create plans, add games, comment, respond | Edit or delete other people's plans or comments; become admin; read another user's PIN or feed token |
| Guest on the Wi-Fi | Create an account, join plans they can see | Same as member; admin may disable the account afterwards |
| Someone sniffing plain HTTP on the LAN | Read traffic and session cookies of HTTP users | — (accepted: household risk model, owner decision 3) |
| Internet | Nothing inbound | — |

Accepted consequences: a 4-digit PIN is brute-forceable without rate limits,
so rate limits are mandatory ([02](02-accounts-auth.md#pins)). An HTTP session
cookie can be stolen by anyone capturing Wi-Fi traffic. "First user is admin"
can be won by whoever loads the page first after a fresh flash. The deploy
runbook has the owner sign in immediately after flashing.

## Usage profile (design point)

| Quantity | Expected | Designed for |
|---|---|---|
| Accounts | 4 + a few guests | 32 |
| Games in catalog | 30–80 | 256 |
| Open plans at once | 1–5 | 16 |
| Past plans kept | — | 128 (oldest archived out, see [05](05-storage-limits.md)) |
| Comments per plan | 0–20 | 64 |
| Concurrent browsers | 2–5 phones on a game night | 3 served + backlog (C6), more on S3 |
| Request rate | single digits per minute | polling at ≥ 60 s per open tab |

## Architecture

```
 phone / laptop browser                      household calendar app
   │ HTTP :80 (guests, anyone)                  │ GET /cal/<token>.ics
   │ HTTPS :443 + h2 (household CA)             │
   ▼                                            ▼
 ┌────────────── ESP32-C6 (one core, no PSRAM) ─────────────────────┐
 │ miniframework connection loop (HTTP, optional TLS/h2)            │
 │   static Angular bundle (gzip, ETag)   /api/v1/*   /cal/*        │
 │                     │                                            │
 │          gamematcher Service (one Mutex<State>)                  │
 │            accounts · sessions · games · plans · comments        │
 │            notifications · outbound queue                        │
 │                     │                                            │
 │          Store: two checksummed state slots in a data partition  │
 │                                                                  │
 │ main loop (20 ms): LCD strip renderer · BOOT button · LED        │
 │ worker: SNTP, outbound OAuth/Mastodon HTTPS (one at a time)      │
 └──────────────────────────────────────────────────────────────────┘
        │ optional outbound HTTPS
        ▼
   a Mastodon instance (mastomini.local, mastodon.social, …)
```

Like mastomini and minicloud: **state lives in RAM, flash is the durable
copy**. Every mutation is validated under the state lock, then committed
before the response is sent.

## Hardware

### Target: Waveshare ESP32-C6-LCD-1.47 (the minicloud board)

| | |
|---|---|
| Chip | ESP32-C6FH8, one 160 MHz RISC-V core, 512 KiB SRAM, **no PSRAM** |
| Flash | 8 MiB (probed; the listing says 4) |
| Base MAC | `ac:eb:e6:1e:13:40` (currently minicloud) |
| Screen | ST7789 172 × 320, RGB LED on GPIO8, BOOT button, microSD slot (unused) |
| Board notes | [BOARD_SKILL_ESP32_C6_LCD_1_47.md](../../BOARD_SKILL_ESP32_C6_LCD_1_47.md) |

The C6 is **tighter than the S2 in RAM** (no PSRAM at all). The S2 was dropped
because of TLS/OAuth pressure, and the C6 has the same problem, partly worse.
Minicloud measured it on this exact board (`mastomini/minicloud_rs/README.md`):

- about 104 KB free heap with TLS linked in. One TLS session plus handshake
  takes about 45–60 KB and fragments the heap;
- under a browser-like HTTPS load, TLS allocations failed and the board
  rebooted. **Minicloud deployed plain HTTP for that reason**;
- the framework's `BoardConfig::c6()` defaults to `tls_clients: 0`.

What helps here is that HTTP is first-class for this app (guests use it), and HTTP/2
needs only *one* TLS session per browser. The plan is:

| Transport on C6 | Budget |
|---|---|
| HTTP/1.1 | 3 connections (framework default) |
| HTTPS + h2 | **1** TLS session, 0 parallel handshakes beyond it |
| Outbound HTTPS (OAuth, Mastodon) | **1** at a time, serialized by a worker; refused (503 + retry) if the server TLS session is mid-handshake |

**Result (October 17):** passed. See [measurements/c6-gate.md](measurements/c6-gate.md):
45 handshakes and 220 HTTPS requests with no failures, 85/85 outbound fetches to
mastodon.social, minimum free heap 35 KB, smallest largest-block 51 KB. A framework
livelock found on the way was fixed.

**Feasibility gate (Sprint 0).** Measure on the board, with the real
firmware, before writing features: 3 HTTP clients + 1 HTTPS/h2 browser + one
outbound HTTPS token exchange (household CA *and* a public CA via the
certificate bundle). Pass = for 5 minutes of the bench load, no reboot, no
TLS allocation failure in `/api/v1/log`, and the largest free block always ≥ 24 KiB.

- **Pass:** C6 is the v1 board.
- **HTTPS fails, outbound passes:** ship C6 HTTP-only. Household devices use HTTP too,
  Mastodon sign-in still works (the redirect URI is http, see
  [02](02-accounts-auth.md#http-and-https)). The owner decides whether that's
  acceptable.
- **Outbound fails:** Mastodon features need the S3 N16R8. Either move
  Game Matcher to an S3 and use the C6 as a remote display
  ([roadmap](roadmap.md)), or ship v1 on C6 without Mastodon.

Repaving erases the whole board, including minicloud's SPIFFS `storage` partition. The owner
confirmed on October 17 that minicloud's data is ephemeral and need not be kept.
The MAC check from minicloud's runbook still applies before any write.

### Flash layout (C6, proposed)

| Partition | Offset | Size | Use |
|---|---|---|---|
| nvs | `0x9000` | 24 KiB | Wi-Fi, framework kv (boot counters, secrets) |
| phy_init | `0xf000` | 4 KiB | |
| factory | `0x10000` | 2.5 MiB | firmware + gzipped Angular bundle |
| coredump | | 64 KiB | crash report at next boot |
| gmstate | | 512 KiB | NVS partition: two snapshot slots + per-plan comment blobs (see [05](05-storage-limits.md)) |
| (free) | | ~4.8 MiB | reserved for OTA's second slot ([roadmap](roadmap.md)) |
