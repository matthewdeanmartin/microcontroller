# Deploy housemetrics to the ESP32-S2 Mini

## October 3 observability update (deployed)

The live collector at `192.168.1.159` already held both its own health and
mastomini-bots series; the old dashboard grouped all of them under `board`.
The deployed UI adds a Board selector and groups by source/host. An embedded,
validated `config/scrapes.json` defines the four household HTTP `/metrics`
targets every 30 seconds and reconciles them on boot without duplicate URLs,
unrelated target deletion or device-token changes. The S2 profile now allows
64 series with 691,200 bytes reserved at capacity, slightly below the old
48-series / 706,560-byte reservation.

Framework desktop scraping also needed a response-framing fix: complete
Content-Length/chunked bodies now finish without waiting for socket closure.
The ESP-IDF client already uses framed reads. The isolated desktop preview
successfully collected all five real bots fields after this correction.

Validation: 49 framework Rust tests plus its doc test, 17 app Rust tests,
11 TypeScript tests, formatting/clippy and Angular build passed. Browser
smoke passed board filtering, charts in all five formats, System, format lab
and Devices, with no console errors. It ran only against the isolated
loopback preview, not the live collector.

S2 app: 1,555,072 / 4,063,232 bytes. Build/check logs and preview UI images
are under `miniframework/.local/observability-*`. See the deployment record
below for the flash and live checks.

### October 3, 2026: configuration and board dashboard deployed

- Owner authorized deployment and entered ROM download mode (hold BOOT,
  tap RST, release BOOT). COM4, `VID_303A&PID_0002`; `read_mac` confirmed
  Housemetrics `80:65:99:f0:7b:68` before writing.
- Reused the already built images with `make flash PORT=COM4`; no further
  compilation. App SHA-256:
  `c8fefe63509fd7ebd83aae3541c55ed0d8e043b6e94363b34092156eb39c6960`.
  Bootloader, partition table and app hashes all verified. The subsequent
  Windows port-disappearance error occurred after verification; the owner
  tapped RST and the app started normally.
- Writes covered `0x1000–0x6fff`, `0x8000–0x8fff`, and
  `0x10000–0x18bfff`. NVS at `0x9000–0xefff` was untouched. Metrics in RAM
  reset as expected. A complete pre-flash token inventory was unavailable
  because the board had entered download mode; the post-flash API has zero
  device records, so token preservation was not independently compared.
- HTTPS verified against the household CA and the exact built leaf
  certificate. Board reports `healthy`, reset `power on`, correct MAC,
  and the new 64-series profile. Startup completed in 3.874 seconds.
  Initial internal heap free 43,543 bytes, PSRAM free 1,346,680 bytes.
- API confirmed all four file-defined targets at 30-second intervals.
  Existing bots target was adopted. Mastomini collected nine fields;
  the live browser Board selector filtered those nine successfully with
  no page errors (`.local/housemetrics-live-board.png`).
- Remaining peer failures at this deployment: bots
  `ESP_ERR_HTTP_CONNECT` (hostname lookup fails; last known IP
  `192.168.1.162` also times out), NanaCoin HTTP 404, Minicloud HTTP 503.
  The NanaCoin and Minicloud route fixes are built but not deployed.
  Do not treat installation of a target as a successful scrape.
- Flash log: `.local/housemetrics-flash-2026-10-03.log`; safe post-flash
  evidence: `.local/hm-deploy-after.json`. The private probe loads the
  existing admin credential locally and never prints it.

Runbook for building, flashing and checking housemetrics on its board, and
the record of what happened on each deployment. Modeled on NanaCoin's
`DEPLOY.md`. Read **Rules** before touching a board.

| | housemetrics board |
|---|---|
| Address | `https://housemetrics.local`, `http://housemetrics.local` |
| Board | DiGiYes ESP32-S2 Mini (ESP32-S2FNR2 rev 1.0): one core, 4 MiB flash, 2 MiB PSRAM |
| MAC | `80:65:99:f0:7b:68` |
| Known IP | 192.168.1.159 (DHCP, October 2, 2026) |
| Location | upstairs, on the development PC's USB |
| USB | native USB, used only by the chip's ROM loader (download mode: `VID_303A&PID_0002`, seen on COM4 and COM18) |
| Flash layout | NVS 24 KiB at `0x9000`, phy 4 KiB, factory app 3.88 MiB at `0x10000`, core dump 64 KiB at `0x3F0000` (`partitions.csv`) |
| Console | UART0 (GPIO 43/44). The app does not use USB: Windows sees nothing while it runs. The log is served at `/api/v1/log.txt` |
| Build profile | `sdkconfig.defaults`, `Profile::s2()`, `Limits::small_board()` |
| Cargo target dir | `C:/mfw-s2` |
| Certificate | `certs/housemetrics.crt` for `housemetrics.local`, `localhost`, `127.0.0.1`; signed by mastomini's household CA (SHA-256 `C2:8F:EE:1E:…:54:FD`); valid to 2028-12-30 |
| Data | metrics in RAM only (gone on every reset); device tokens and scrape targets in NVS namespace `hm` |

**The other S2 Mini** (`80:65:99:f0:1c:9c`) is NanaCoin's second bank
(`nanacoin-s2.local`, downstairs). Never flash it from here. `tools/flash.sh`
prints the MAC before writing and refuses a board whose partition table has
NanaCoin's `ledger` partition unless given `--replace-nanacoin`.

## Rules for humans and automation

1. **Identify before writing.** Read the MAC; it must be `80:65:99:f0:7b:68`.
   Do not infer which board is attached from files in a repo.
2. **No erase.** Flashing writes bootloader, partition table and app only.
   NVS (device tokens, scrape targets) survives. Never `erase_flash`
   without the owner's say-so.
3. **No new CA.** Certificates come from mastomini's CA via `make certs`.
   Reissue the leaf (same CA) for a new IP; never rotate the CA to fix a
   deployment.
4. **A flash resets the metrics.** They live in RAM. Say so before flashing
   a board someone is using.
5. **Agents ask before flashing or resetting.** The owner has to press the
   buttons anyway.

## Prerequisites

- Git Bash, the Windows ESP-IDF install (`C:\Espressif`, IDF v5.5.3), the
  `esp` Rust toolchain, Node 24, uv. `tools/build-firmware.sh` sets every
  ESP-IDF variable itself; no PowerShell or `idf.py` needed.
- `apps/housemetrics/.env` (gitignored) with `HOUSEMETRICS_ADMIN_PASSWORD`,
  and Wi-Fi either there (`HOUSEMETRICS_WIFI_SSID` / `_PASSWORD`) or in the
  file named by `MINIFRAMEWORK_ENV`. This household uses mastomini's:
  `MINIFRAMEWORK_ENV=C:/github/mastomini/mastomini_rs/.env`.

## 1. Check the tree

```sh
cd miniframework
make check        # Rust tests, clippy, fmt, TypeScript decoder tests, Angular build
```

## 2. Build

Scrape destinations come from this app's `config/scrapes.json`. The build
validates and embeds it; boot reconciles its managed targets into NVS while
preserving unrelated manual targets and device tokens. To select a different
file, set `HOUSEMETRICS_CONFIG` to its absolute path. Keep credentials in the
existing private settings, never in this non-secret file. The four default
targets use HTTP `/metrics` and need no admin credentials or TLS handshakes.
Changes to the file require an app rebuild.

```sh
MINIFRAMEWORK_ENV=C:/github/mastomini/mastomini_rs/.env make firmware
```

Builds the Angular app, bundles it (gzip only), and the firmware. Output:
`C:/mfw-s2/xtensa-esp32s2-espidf/release/housemetrics-esp32.bin` plus
`bootloader.bin` and `partition-table.bin`. The script refuses an image
larger than the app partition. A first build (or an `sdkconfig.defaults`
change) rebuilds ESP-IDF: several minutes. No board is touched.

## 3. Download mode

Build once and use those images for Step 4; `make flash` does not rebuild.
If source, config or certificate inputs change, rebuild before flashing.
Mastomini's two-socket S3 advice does not replace this S2's manual BOOT/RST
procedure.

The S2 Mini has no auto-reset circuit and esptool cannot reset it into the
loader. By hand: **hold BOOT (`0`), tap RST, release BOOT.** Then find the
port:

```sh
powershell -NoProfile -Command "Get-CimInstance Win32_PnPEntity | Where-Object { \$_.Name -match 'COM\d' } | Select Name, DeviceID"
```

Look for `VID_303A&PID_0002`. Because the running app shows the same ID,
the only proof the board is in download mode is that esptool connects.
"No serial data received" on a `PID_0002` port means it is the running app:
press the buttons again.

## 4. Flash

```sh
make flash PORT=COMn
```

The script: reads the MAC (prints it: check it is `…:7b:68`), waits for the
port to come back (the loader drops off USB after every esptool step), reads
the partition table (refuses a NanaCoin bank), then writes bootloader
(`0x1000`), partition table (`0x8000`) and app (`0x10000`) and verifies each
hash. Three `Hash of data verified.` lines mean the image is on the board.
A pySerial "device attached to the system is not functioning" error **after**
those lines is esptool losing the port as the chip leaves USB; it is
harmless.

Then **tap RST** to start the app (it stays in the loader otherwise).

## 5. Watch it come up

The blue LED (GPIO 15) is the boot log you can see from across the room:

| What you see | Meaning |
|---|---|
| 1.5 s on, then 1–6 quick flashes | POST: firmware started. Flashes = why it reset: 1 power on, 2 reset button/software/USB, 3 crash, 4 watchdog, 5 brownout, 6 other |
| N blinks, 1.6 s dark, repeating | Startup failed at step N: 1 system, 2 NVS, 3 Wi-Fi driver, 4 Wi-Fi join, 5 time/mDNS, 6 app setup, 7 listeners |
| Slow blink, 1 s on / 1 s off | Not on Wi-Fi yet (joining retries forever) |
| Solid on | On Wi-Fi, server not listening yet |
| `housemetrics` in Morse, three slow blinks, 3 s dark | Healthy and serving (one cycle ≈ 30 s) |
| Mostly on, short dark beat | Degraded: no mDNS, or a TLS/connection failure in the last 10 s |
| Fast blink, 5 per second | Stalled: the serving loop or TLS task stopped turning |

A normal boot: POST, slow blink for a few seconds, Morse within ~10 s.
`/api/v1/sys` reports the same state as `status`, and `reset_reason`.

**The board's log is served by the board** (no USB needed). After a C-level
crash it starts with `crash report:` and `crash backtrace:` lines from the
core dump; map the addresses with
`bash tools/addr2line.sh housemetrics-esp32 0x40... 0x40...` (same build only).


```sh
curl http://housemetrics.local/api/v1/log.txt          # this boot, plus the previous boot's last lines after a crash
curl "http://housemetrics.local/api/v1/log?after=120"   # only lines after #120 (any wire format)
```

and the **Board log** card on the System page. It holds every Rust log line
and ESP-IDF's own C log lines (Wi-Fi, esp-tls, mbedTLS) in a 16 KiB ring,
plus a health line every 60 s (memory and low-water marks, free stack per
task, TLS ok/failed, open connections). The newest ~2 KiB is mirrored in RTC
memory, so after a crash, watchdog or software reset the next boot shows
`previous boot ended: <reason>` and that boot's last lines, including a Rust
panic's message. Not captured: the ESP-IDF panic handler's register dump
(it prints through the ROM).

Serial console (fallback): a `PID_0002` port while the app runs; read with
pySerial at 115200 with DTR/RTS low. It did not reliably enumerate on
October 2.

## 6. Prove it is serving

```sh
curl -s http://housemetrics.local/api/v1/sys | python -m json.tool | head -20
curl -s --cacert apps/housemetrics/certs/household-ca.crt https://housemetrics.local/api/v1/sys >/dev/null && echo https ok
```

Check: `reset_reason` (`power on` / `reset pin` after a flash; **`crash`
means it fell over**), `status` `healthy`, `heap.psram_min` above ~200 KiB,
`build` matches `git rev-parse --short HEAD` (`+` = uncommitted changes).

Windows resolves `.local` slowly (seconds); use the IP for measurements.
HTTPS by IP needs the IP in the certificate:

```sh
mv apps/housemetrics/certs/housemetrics.{crt,key} ../.local/   # keep the old pair
MINIFRAMEWORK_CERT_IPS=192.168.1.159 make certs firmware        # same CA, nothing to re-trust
```

## 7. Measure

```sh
make bench URL=https://housemetrics.local     # fresh / resumed / warm TLS x payloads x formats
cd bench && uv run writer --url http://192.168.1.159 --token hm_... --backfill 6h --live
```

and the **Format lab** page. Crash check after any load test: `uptime_ms`
did not reset and `reset_reason` is unchanged.

## Stop conditions

| Symptom | Meaning / action |
|---|---|
| Flash script prints a MAC other than `…:7b:68` | Wrong board. Stop |
| "This board holds a NanaCoin bank" | It is NanaCoin's S2. Stop unless the owner decided to end that bank |
| "No ESP32-S2 answered" | Not in download mode (or it is the running app on a `PID_0002` port). BOOT+RST again |
| Fewer than three "Hash of data verified" | Write incomplete. Re-enter download mode and flash again |
| LED blinks a number | Startup failed at that step (table above); `startup failed at step N: …` repeats in the log every 5 s |
| `reset_reason: crash` after use | Something crashed it. Note what was running; check the history below for known causes |
| Slow blink forever | Wi-Fi credentials or signal. RSSI was −76 dBm upstairs on October 2 |

## Memory budget (`Profile::s2()`)

| What | Size |
|---|---|
| Raw metric blocks | 1536 × 256 B = 384 KiB (+ 36 KiB metadata) |
| Rollups | 48 series × 288 × 20 B = 270 KiB (3 days at 15 min) |
| Connections | 3 TLS + 3 HTTP; 12 KiB request buffer each; TLS ~25 KiB per session |
| Response scratch | 96 KiB, plus up to 192 KiB of unsent responses |
| gzip (when asked) | ~230 KiB heap + a 96 KiB helper stack, while compressing |

Measured idle after boot (October 2): PSRAM 1.26 MB free of 2.09 MB,
internal RAM 51 KB free (31 KB largest block).

## History

### October 2, 2026: first installation

- The attached S2 Mini was assumed (from NanaCoin's `boards.py`) to be
  NanaCoin's second bank. It was not: MAC `80:65:99:f0:7b:68`, an earlier
  plain ESP-IDF app (factory app at `0x10000`, no ledger), silent on its
  serial port. The owner cleared it for housemetrics.
- `tools/flash.sh` had two bugs that only show on hardware: it passed a Git
  Bash path (`/tmp/...`) to Windows Python (and hid the error), and did not
  wait for the loader to re-enumerate between esptool steps. Both fixed;
  the guard stopped before writing each time.
- First flash (build `6403c95+`, 1,486,176 bytes): hashes verified. The board
  ran (the owner used the Format lab), but a watcher looking for the app's
  serial port saw nothing: it filtered out `PID_0002`, which the app also
  uses. Lesson recorded in step 3.
- Owner's Format lab results on the board: for the smallest payloads every
  format is about a tie; from 10/100 rows up JSON is a noticeable loser
  (server time); protobuf wins. **With large payloads the board fell over.**

### October 2, 2026: status light, crash and throughput findings

- Added the status light (POST, startup step codes, health rhythms, Morse;
  `crates/miniframework/src/status.rs`). Flashed (1,495,824 bytes, hashes
  verified). Board up at 192.168.1.159, `status: healthy`, but
  `reset_reason: crash` with 87 s uptime: it had just fallen over.
- **Crash 1, found and reproduced: dynamic gzip.** One request with `?gz=1`
  reboots the board (request dropped, then `uptime 20 s, reset crash`).
  Cause: `miniz_oxide::compress_to_vec` builds its compressor on the stack,
  including a 64 KiB inline buffer; the serving task has a 32 KiB stack.
  The desktop never shows it (megabyte stacks). Fix: compression runs on a
  short-lived helper thread with a 96 KiB PSRAM stack, and is skipped
  (response sent uncompressed) when PSRAM is short; the response copy is now
  fallible (503 instead of an out-of-memory abort).
- Without gzip, the board survived every payload over HTTP: 100, 1000 and
  2000 rows, rows and columns, all five formats (oversized ones get 507:
  JSON past 1000 rows, MessagePack/CBOR at 2000). No reboot.
- **Throughput cap, found:** 1000 rows took 0.6–1.2 s (≈ 57 KB/s).
  `CONFIG_FREERTOS_HZ=100` makes every 1 ms sleep a 10 ms tick, and the
  serving loop slept every turn while sending at most 1 KiB per client per
  turn: < 100 KB/s by construction. That is why bigger formats (JSON) lost so
  clearly. Fix: 1 kHz tick, up to 16 KiB per client per turn, sleep only when
  idle (or every 20 ms under load, for the idle task's watchdog). NanaCoin's
  board loop has the same 10 ms tick and 1 KiB-per-turn design (not changed).
- The app's USB serial console did not enumerate after these boots (no
  `VID_303A` device at all while the board served over Wi-Fi). Not yet
  explained; the sdkconfig has `CONFIG_ESP_CONSOLE_USB_CDC=y`.

### October 2, 2026: gzip and throughput fixes deployed

- Flashed via COM4 (MAC `…:7b:68`, 1,499,088 bytes, hashes verified),
  `CONFIG_FREERTOS_HZ=1000`. After RST it reported `reset_reason: crash` with
  123 s uptime and no other clients: **it crashed once at or just after
  boot**, then ran. Cause unknown (no console, see below).
- **gzip fixed:** `?gz=1` now returns 200 (100 rows: 1.7 KB) without a
  reboot; `heap.psram_min` fell by ~300 KB during compression, as expected.
- **Throughput roughly doubled:** 1000 rows as columns, protobuf 34.5 KB in
  0.50 s (was 0.81 s), JSON 52.8 KB in 0.71 s (was 1.12 s): ~70 KB/s at
  −80 dBm. Not yet split into server time vs. link time.
- **Fresh HTTPS handshakes:** 0.9–3.3 s (curl and Python), TLS 1.2
  ECDHE-RSA-AES256-GCM; the board's own average was 1,021 ms.
- **New bug, reproducible: a new TLS handshake fails after a client drops a
  used connection without a TLS close.** Python: request on connection A,
  close the socket, open connection B → `UNEXPECTED_EOF` during B's
  handshake in 7 of 8 trials (with or without a 0.5 s gap); the board's
  `tls_failures` counts each one; no reboot. curl (which closes cleanly) and
  handshake-only connections never fail. Browsers drop connections like
  this, so this can look like "the board stopped responding". `make bench`
  hits it (its resumed mode). Not yet diagnosed.
- **No USB console at all** while the app runs (no `VID_303A` device), so
  neither the boot crash nor the TLS failures have logs yet.

### October 2, 2026: the board serves its own log

- Added the served log (`/api/v1/log`, `/api/v1/log.txt`, System page) with
  ESP-IDF's C log lines, a health line every minute, TLS failure details,
  and the previous boot's last lines kept in RTC memory.
- Flashed via COM4 (1,521,072 bytes). **First result:** after an RST the
  board crashes once, early. The previous boot's kept log shows it got all
  the way to `housemetrics ready at https://housemetrics.local/` at 3.8 s,
  then crashed with nothing logged and no Rust panic message: a C-level
  fault in the first seconds of serving, only on the first boot after a
  hardware reset. The reboot that follows runs normally.
- The USB console was never missing by accident: Windows reported "USB device
  not recognized" while the app ran (LED healthy). The S2's ROM-based USB CDC
  console does not enumerate on this board. Moved the console to UART0.
- Added core dumps to flash (64 KiB partition carved from the app partition;
  `CONFIG_ESP_COREDUMP_ENABLE_TO_FLASH`). The next boot after a crash logs the
  task, cause, PC and backtrace. Build 1,544,576 bytes; partition table
  changed (NVS untouched), so this flash writes the new table.

### October 2, 2026: core dumps, UART console; both problems gone

- Flashed via COM4 (1,544,576 bytes; new partition table with `coredump`),
  then RST. Booted cleanly (`reset power on`) and **did not crash**: the
  crash after every hardware reset is gone. No core dump was needed.
- **The dropped-connection TLS failures are gone too:** the same Python test
  that failed 7 of 8 times passed 8 of 8, and `make bench` (fresh, resumed
  and warm TLS) ran 96 handshakes with 0 failures and no reboot.
- The only change relevant to both: the console moved from USB (which never
  enumerated) to UART. Likely mechanism, not directly observed: log output
  to the stuck USB console blocked the task writing it (esp-tls logs when a
  peer drops a connection mid-stream; that stalled the TLS task until the
  next client gave up), and the USB console faulted while Windows tried to
  enumerate it after a hardware reset. Watch the served log for any
  `crash report:` line over the next days before calling it settled.
- First board measurements of the format question: see `docs/FORMATS.md`.
  Fresh TLS ~930 ms, resumed ~75 ms; ~135 KB/s over TLS at −78 dBm.
