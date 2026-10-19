# Sprint 0 gate: the Waveshare C6, measured

October 17, 2026. Board `ac:eb:e6:1e:13:40` (ESP32-C6FH8, no PSRAM), firmware
with `http2,gate`, 1 TLS session + 3 HTTP connections, 192.168.1.163 on the
household Wi-Fi. Load from `tools/gate.py`; raw samples in
[c6-gate-http2.json](c6-gate-http2.json).

## Verdict

**Pass on hardware: the C6 is the v1 board, with HTTP, HTTPS and HTTP/2.**
Outbound HTTPS works under load with public CAs. Outbound HTTPS to *household*
hosts was blocked by a certificate problem, not by memory, and is fixed by
reissuing the CA (below).

## Load

5 minutes, all at once:

- 3 keep-alive HTTP/1.1 clients polling `/api/v1/status` and the UI (gzip)
  every 0.5 s;
- 1 HTTPS client (household CA, SNI `gamematcher.local`) reconnecting every
  5 requests, so a full handshake every ~6 s;
- the board's own gate worker fetching `https://mastodon.social/.well-known/oauth-authorization-server`
  (public CA bundle) and a household host in turn, one connection at a time,
  every ~2 s;
- `/api/v1/sys` sampled every 10 s.

## Results (final run)

| | Result |
|---|---|
| Reboots / hangs | none (uptime continuous, reset reason unchanged) |
| HTTP requests | 1,603 ok, 29 dropped keep-alives (`RemoteDisconnected` when the 4th client queued behind 3 slots; browsers retry these) |
| HTTPS requests | 220 ok, 0 failed |
| TLS handshakes (server) | 45, 0 failures; ~1.4 s each from curl |
| Outbound, public CA | 85 ok, 0 failed; 1.3 s typical, 2.6 s max |
| Outbound, household CA | 0 ok, 85 failed: CA parse error (below) |
| Internal heap free | 73–110 KB under load (134 KB idle) |
| Minimum free ever | 35,268 bytes |
| Smallest "largest free block" | 51,200 bytes (bar: 24 KiB) |
| Latency max | HTTP 1.1 s, HTTPS 2.8 s (handshakes are 1+ s of CPU on the one core) |
| PIN hash | 2,000 rounds = 197 ms → set to **1,000 rounds = 99 ms** |
| App image | 1.76 MB of 2.5 MiB (67 %) |

## Found and fixed: serving task livelock (framework bug)

The first two runs hung the board after 70–300 s: no HTTP, no ping, and the
task watchdog on serial reported `IDLE (CPU 0)` starved by `mf-serve`. With a
line-table build, `addr2line` placed the spin in `std::sync::mpsc` `try_recv`
(`spin_light` in the bounded channel), called from the serving loop's TLS
hand-over.

Cause: std's bounded channel is lock-free; a receiver that finds a slot
claimed but not yet published spins until the sender publishes it. The C6
profile runs the serving task at priority 5 on the only core, the TLS
handshake task at priority 4. When the handshake task was preempted between
claim and publish, it never ran again. (The S2 serves from `main` at priority 1,
below the handshake task, and the S3 has two cores, which is why neither showed it.)

Fix (miniframework `esp/mod.rs`): `Handoff`, a `Mutex<VecDeque<Socket>>`. The
receiver only `try_lock`s, and FreeRTOS mutexes inherit priority. After the fix:
two 5-minute runs, no hang.

## Found, then fixed by reissuing the CA: the household CA vs mbedTLS

Every outbound request to a household host failed before connecting:
`mbedtls_x509_crt_parse of CA cert returned -0x2562`. The household CA has a
**critical Name Constraints** extension (`.local`, `.lan`, `home.arpa`,
`internal`, private IP ranges). mbedTLS doesn't implement name constraints,
so it refuses the whole certificate. Browsers and phones are unaffected,
because they enforce the constraints. No board had used the household CA as an
*outbound* trust anchor before (housemetrics scrapes plain HTTP).

ESP-IDF 5.5.3 has no Kconfig to allow unsupported critical extensions
(tried; no effect). mastomini and mastomini_bots had worked around it with an
mbedTLS extension callback.

**Resolution (owner decision, October 17):** the household CA certificate was
reissued with the **same key and subject** and the name constraints kept but
**not critical**. No board or device needed redeploying. Browsers still
enforce the constraints, and mbedTLS now loads the CA. The motivation is in
mastomini `docs/security/https.md`, "Why the name constraints are not critical".

## After the CA reissue (October 17)

A 60-second rerun of the gate firmware with the reissued household CA
([c6-gate-household-ca.json](c6-gate-household-ca.json)): **household-CA
outbound 12 ok / 0 failed** (`https://housemetrics.local`), public CA 12 / 0,
HTTPS 40 / 0, minimum free heap 48 KB, smallest largest-block 39 KB, no
reboot. `gate.py`: **GATE PASSED**.

## Not measured here

- A real browser on HTTP/2 (ALPN `h2` was confirmed with a Python client;
  the load used HTTP/1.1 over TLS, which is the heavier case per connection).
- A full game night of 6–8 phones (Sprint 9).
- Power-cut behaviour of the NVS snapshot slots (Sprint 9). Reset persistence
  was checked in Sprint 1.
