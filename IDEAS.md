# Ideas for the household boards

Collected 2026-10-19 while deploying the SRE bot's sprint 0. Nothing here is
scheduled; the SRE bot's sprints live in
`microcontroller/miniframework/spec/OPS.md`.

## The boards, and what an S2 is good for

| Board                | Chip                                    | Runs                                                                |
|----------------------|-----------------------------------------|---------------------------------------------------------------------|
| mastomini            | ESP32-S3 N16R8                          | the household Mastodon server                                       |
| mastomini-bots       | ESP32-S3 N16R8                          | the bot farm, including `sre`                                       |
| nanacoin             | ESP32-S3 N16R8                          | the bank                                                            |
| gamematcher          | ESP32-C6 (no PSRAM, app space 92% full) | game nights                                                         |
| meritledger          | ESP32-S3 Touch LCD 1.47, SD card        | the practice ledger                                                 |
| housemetrics         | ESP32-S2 Mini                           | metrics in RAM (original board `…:7b:68`, currently offline)        |
| housemetrics #2      | ESP32-S2 Mini `80:65:99:f0:1c:9c`       | was the nanacoin-s2 test bank; reflashed to housemetrics 2026-10-19 |
| three new LCD boards | —                                       | intended as notification boards                                     |

An S2 (one 240 MHz core, 2 MiB PSRAM, 4 MiB flash, no screen, buttons the
owner would rather not press) suits work that holds little or no state,
needs nobody waiting on it, reads the other boards and produces a small
answer. It will never hold a large app.

## Picks so far

1. **NanaCoin auditor.** Every night, replay the public notebook with an
   independent implementation, recompute every balance and compare with the
   bank's totals. It needs no NanaCoin credentials and could be owned by
   someone other than the banker: it exists to catch accounting shenanigans.
   Its result is one `/.well-known/ops/health` check (`audit.ledger`), so the
   SRE digest reports it with no new bot code.
2. **Web crawler for homelab.** A worker that keeps crawling while homelab's
   `npm start` site only scans on click: continuous mDNS/ARP census
   (first seen, last seen, how long each device stays) plus a small
   cross-site word index, so homelab's search stays fresh and responsive.

## Keeping the bank honest (pairs with the auditor)

- **Ledger notary.** Each night post a hash of the public ledger to
  mastomini. Public, timestamped posts make a quiet rewrite of history
  detectable.
- **Randomness beacon.** Commit-then-reveal hourly values from the ESP32's
  hardware RNG. The NanaCoin lotto draws from a value the bank could not know
  or influence in advance. Game Matcher could use it for who goes first.
- **Economy simulation.** Monte Carlo months of traders, lotto and loans:
  inflation, the lotto's expected value, which trader strategy goes broke.
  Results make good nana_news posts.

## Patient compute for the other apps

- **Lunar-calendar cross-check.** Compute new and full moons for the next
  5–10 years and compare with Merit Ledger's uposatha and feast days, beside
  its golden tests.
- **Game-night solver.** Rank which games fit tonight's people (player
  counts, ownership, preferences, time since last played): a search the C6
  cannot afford.
- **Export verifier.** Stream mastomini's admin export and meritledger's
  `export.json` nightly; check they parse and that counts and checksums match
  the live boards. A restore test without a storage board. Needs read-only
  admin tokens.
- **API conformance runner.** A second, independent client checking
  mastomini's Mastodon API answers nightly.
- **Link checker.** Open Library links, links in vows posts, game links: find
  the ones that quietly died.

## Feeds and screens

- **Hub for the LCD boards.** One small feed the three new screens read: the
  SRE digest's worst status, NanaCoin news, tonight's game, an uposatha day.
  minicloud's kitchen screen used to do this job.
- **RSS/Atom feeds** for every site (vows posts, game nights, NanaCoin news).
- **One household calendar (ICS)**: game nights, uposatha and feast days, bot
  schedules.
- **Mention light.** Blink when you have a mastomini mention or DM.
- **Last-known mirror.** Serve a "this board is down; copy from 03:00" page
  while a board reboots or is reflashed. Availability, not speed: a cache on
  an S2 is slower than the boards' own RAM.

## Hardware add-ons

- **Sensor node.** Temperature/humidity or CO₂ into housemetrics
  (`temp,room=attic`); freezer-too-warm is one rule. Also: every board
  already reports its chip temperature (`temp_c` in `/api/v1/sys`), a free,
  rough room thermometer. (The datacenter story: hot drives showed the AC had
  failed before anyone arrived.)
- **Wi-Fi site-survey wand.** On a USB power bank: log RSSI and access point
  per room label. Settles placements like meritledger's −84 dBm first spot.
- **Wi-Fi serial console** for the S3 boards: their UART plus EN/BOOT pins,
  console over HTTP, remote reset/download mode; maybe esptool's `rfc2217://`
  for remote flashing. Answers the flaky-USB-hub problem.
- **Chore buttons** that record NanaCoin good deeds; **meditation bell**
  (piezo + button) that logs a sit to Merit Ledger; **game-night "I'm in"
  button**.
- **CA install stick.** The S2's native USB as a drive holding
  `household-ca.crt` and a one-page trust guide.
- **Wake-on-LAN** at 02:00 for the PC's heavier nightly jobs.
- **Load/threshold calibrator.** Replay gate/fmtbench traffic at night and
  record heap and latency, to set `Config::ops` limits per board.

## Ruled out

- **An SRE for the SRE.** Overkill.
- **Local time server.** NIST wins.
- **A single status LED.** The LCD boards cover it.
- **A Redis-style cache.** Slower than hitting the board directly.
