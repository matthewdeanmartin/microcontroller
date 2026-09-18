# hello_wifi_s3_py

MicroPython on an **ESP32-S3-N16R8** — a live system dashboard served over WiFi,
with the on-board RGB LED as a status indicator.

The board serves JSON; the browser draws the charts. See
[the docs](../docs/esp32s3/) for the reasoning.

| | |
|---|---|
| Chip | ESP32-S3 (QFN56) rev v0.2, dual-core LX7 @ 240MHz |
| Flash | 16MB |
| PSRAM | 8MB octal — **8.29MB free heap** |
| Filesystem | 14.7MB |
| Radio | WiFi 2.4GHz + Bluetooth 5 (LE) |
| RGB LED | WS2812 on **GPIO48** |
| Firmware | MicroPython 1.29.0 (`ESP32_GENERIC_S3-SPIRAM_OCT`) |
| MAC | `ac:a7:04:2c:2c:04` |

## Quick start

```powershell
python dev_server.py            # preview at localhost:8123, no board needed
.\deploy.ps1 -Port COM8         # push to the board, ~3 seconds
```

Then <http://esp32s3.local>.

## The two USB ports

This board exposes both, and they are not interchangeable:

| Port | What it is | Use it for |
|---|---|---|
| **COM8** | CH343 UART bridge (VID `1A86`) | **everything** — flashing, deploying, REPL |
| COM10 | native USB-CDC on the S3 itself (VID `303A`) | nothing, unless the bridge fails |

The bridge port is stable: it is a separate chip, so it stays enumerated no
matter what the S3 is doing, and it drives BOOT/RESET automatically over
RTS/DTR. **No button presses are needed to flash this board** — unlike the S2,
where the port vanishes on reset and the BOOT/RESET dance is mandatory.

The native port re-enumerates and changes number whenever the board reboots.
That is the behaviour documented at length in `docs/basic_setup/` for the S2;
on this board you can simply avoid it.

## Files

| File | Role |
|---|---|
| `app.py` | **edit this** — routes and the dashboard page |
| `compat.py` | CPython/MicroPython differences; `diagnostics()` |
| `led.py` | WS2812 status indicator, fails soft |
| `main.py` | device entry point — WiFi, socket loop |
| `dev_server.py` | local preview under CPython |
| `deploy.ps1` | copy files to the board |
| `watch.ps1` | poll `/health` over time |
| `config.py` | WiFi credentials + hostname + LED pin (gitignored) |
| `reference/` | the factory NeoPixel demo, preserved before erase |

## Endpoints

| Path | Returns |
|---|---|
| `/` | the dashboard (HTML + inline JS, ~9.6KB, no CDN) |
| `/api/stats` | JSON: uptime, memory, RSSI, temp, flash, network |
| `/health` | `ok rssi=-78` — plain text, for `watch.ps1` |

## LED status

| Colour | Meaning |
|---|---|
| white | booting |
| amber, pulsing | joining WiFi |
| green, breathing | serving |
| blue flash | a request was handled |
| red | WiFi failed — the page will never come up |

Set `NEOPIXEL_PIN = None` in `config.py` to disable. Everything in `led.py`
fails soft: a wrong pin costs you the LED, never the web server.

## Why the smarts are in JavaScript

The board answers **one request at a time**. That, not memory, is the real
constraint — 8MB of PSRAM is plenty, but a busy dashboard would still have the
board spending its life describing itself instead of doing its job.

So `/api/stats` is a few hundred bytes of flat JSON and nothing more. The
browser keeps the 60-sample ring buffer, scales the axes, draws the sparklines
and formats every figure. History costs the board nothing and lives as long as
the tab is open. Polling pauses when the tab is hidden.

Charts are hand-drawn SVG with no external library — the board cannot reach a
CDN, and neither can you if this LAN has no internet.

## Signal

This board currently reads **-78 dBm (marginal)**. It works, but mDNS lookups
drop occasionally under load; addressing it by IP is reliable where
`esp32s3.local` is not. Moving the board is the fix — see
[signal and placement](../docs/micropython/signal_and_placement.md).

## Reflashing MicroPython

```powershell
.\flash_micropython.ps1 -Port COM8
```

Note the S3 flashes at offset **`0x0`**, not the S2's `0x1000`. Writing S3
firmware at `0x1000` produces a board that never boots, with no error at flash
time. The variant matters too: N16R8 is **octal** PSRAM, so the build must be
`ESP32_GENERIC_S3-SPIRAM_OCT`. The plain build assumes quad and leaves the
8MB switched off — which is exactly how this board arrived, reporting 167KB
of heap instead of 8.29MB.
