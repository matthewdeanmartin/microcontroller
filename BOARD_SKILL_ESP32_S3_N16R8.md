# Board Skill — ESP32-S3-N16R8

Working notes for this specific board, written from a session that took it from
"just plugged in" to serving a live system dashboard. Everything here was
**measured on the hardware**, not taken from a datasheet. Where a claim came
from a listing or an assumption and turned out wrong, that is recorded too —
the wrong turns are the useful part.

Companion to the ESP32-S2 notes in `docs/basic_setup/`. Read that one for the
S2; the two boards differ more than their names suggest.

---

## Identity

| Property | Value |
|---|---|
| Chip | ESP32-S3 (QFN56) rev v0.2 |
| Core | Dual Xtensa LX7 @ 240MHz + LP core |
| Flash | 16MB (quad lines, per eFuse) |
| PSRAM | **8MB octal**, embedded, AP_3v3 |
| Radio | WiFi 2.4GHz b/g/n + **BT 5 (LE)** |
| Unique ID / MAC | `aca7042c2c04` |
| RGB LED | one WS2812 on **GPIO48** |
| Firmware | MicroPython 1.29.0, `ESP32_GENERIC_S3-SPIRAM_OCT` |
| Build | `MicroPython-1.29.0-xtensa-IDFv5.5.2-with-newlib4.3.0` |
| `.mpy` format | 11014 |
| Modules present | 38 of 45 probed |

Ask the chip rather than trusting a listing:

```powershell
python -m esptool --port COM8 --chip esp32s3 flash-id
```

```text
Features: Wi-Fi, BT 5 (LE), Dual Core + LP Core, 240MHz, Embedded PSRAM 8MB (AP_3v3)
Detected flash size: 16MB
```

---

## The two USB ports

**The single most useful thing to know about this board.** It has two, they are
not interchangeable, and picking right removes nearly every annoyance the S2
had.

| Port | VID | Device | Behaviour |
|---|---|---|---|
| **COM8** | `1A86` | USB-Enhanced-SERIAL CH343 | **stable**, always present |
| COM10 | `303A` | native USB-CDC on the S3 | moves on every reboot |

The CH343 is a **separate chip**. It enumerates on power regardless of what the
S3 is doing, and it drives BOOT/RESET over RTS/DTR.

**Use the bridge port (COM8) for everything**: flashing, deploying, REPL.

Two consequences:

- **No buttons.** `esptool` enters the bootloader by itself. The
  hold-BOOT-tap-RESET dance that is mandatory on the S2 is unnecessary here.
- **The port does not move.** The S2's defining annoyance — COM port vanishing
  on reset, or absent entirely on empty flash — does not apply to the bridge.

The native port behaves exactly like the S2's. It is simply avoidable — *except*
as a recovery route, see Gotcha 4.

Identify which is which:

```powershell
Get-CimInstance Win32_PnPEntity | Where-Object { $_.Name -match 'COM\d+' } |
  Select-Object Name, DeviceID
```

No driver install was needed on Windows 11; both were inbox.

---

## Flashing

```powershell
python -m esptool --chip esp32s3 --port COM8 erase-flash
python -m esptool --chip esp32s3 --port COM8 --baud 460800 write-flash -z 0x0 firmware.bin
```

Note `esptool` v5 renamed the subcommands: `erase-flash` / `write-flash`, with
hyphens. The old underscore forms still work but warn.

### Two ways to silently brick it

Both differ from the S2 and **neither produces an error at flash time**.

**1. Offset `0x0`, not `0x1000`.** The S3 bootloader lives at zero. Flash at
`0x1000` and esptool reports success, verifies the hash, and leaves a board
that never boots.

**2. The `SPIRAM_OCT` build.** N16R8 is *octal* PSRAM. MicroPython ships two S3
builds; the plain `ESP32_GENERIC_S3` assumes quad and leaves the 8MB switched
off. The wrong build boots fine and behaves normally until memory pressure.

The tell is the heap:

```text
plain build   gc.mem_free() ->   167,328     (~167KB)
OCT build     gc.mem_free() -> 8,318,880     (~8.3MB)
```

**This board arrived misconfigured**, running the plain build on MicroPython
1.19.1 (June 2022). It was not faulty — 8MB of PSRAM was sitting there switched
off. Reflashing changed everything:

| | Before | After |
|---|---|---|
| Free heap | 167 KB | **8.32 MB** |
| Filesystem | 6 MB | **14.7 MB** |
| CPU | 160 MHz | **240 MHz** |
| MicroPython | 1.19.1 | **1.29.0** |

**Lesson: reflash a new board before concluding anything about it.** Nothing in
a version string reveals a PSRAM misconfiguration; only the heap figure does.

---

## Five kinds of storage

The distinction decides where data should live, and they are easy to conflate.

| Storage | Size | Survives | For |
|---|---|---|---|
| **RAM** (`gc`) | 7.9 MB | nothing | working data |
| **RTC memory** | 8 KB | soft reset, deep sleep — **not** power loss | crash breadcrumbs |
| **NVS** | 24 KB | everything | small durable key/value |
| **Filesystem** (`vfs`) | 14.7 MB | everything | files, logs, rows |
| **`factory`** | 2 MB | everything | firmware; read-only to you |

Three are partitions in the same 16MB chip:

```text
0x009000  nvs            24,576 B
0x00f000  phy_init        4,096 B
0x010000  factory     2,031,616 B   <- the firmware
0x200000  vfs        14,680,064 B   <- your files
```

That table is the answer to "why does a 16MB chip show a 14MB filesystem": the
firmware is simply large.

**NVS** — wear-levelled key/value, verified working. Wear-levelling matters
because raw flash dies after ~100k erase cycles; a counter written every boot
would otherwise punch through one sector.

```python
nvs = esp32.NVS('stats')
nvs.set_i32('boots', 42)
nvs.commit()            # nothing is durable until this
buf = bytearray(32)
n = nvs.get_blob('key', buf)
```

### The filesystem is real

`vfs.mount()` enumerates mounted volumes — the board's equivalent of "drives".
Normally exactly one:

```python
>>> vfs.mount()
[(<VfsLfs2>, '/')]
```

**LittleFS2**, not FAT. Log-structured and power-fail safe, which is why
pulling the plug mid-write does not corrupt it the way FAT would. An SD card
via `machine.SDCard` would appear here as a second entry.

Full POSIX-ish API: `listdir`, `ilistdir`, `mkdir`, `rmdir`, `remove`, `rename`,
`stat`, `statvfs`, `getcwd`, `chdir`, `sync`, `mount`, `umount`.

**Use `ilistdir()`, not `listdir()` + `stat()`.** One call returns name, mode,
inode and size together — the difference between one pass and N+1 syscalls,
which matters when serving it over HTTP on a single-threaded board:

```python
>>> list(os.ilistdir('/'))[:1]
[('app.py', 32768, 0, 40662)]
#   name     mode   inode  size
```

Mode is `0x4000` for a directory, `0x8000` for a regular file.

Two numbers worth comparing: the volume reports **122,880 B used** for
**95,924 B of files**. The ~27KB gap is LittleFS block overhead and metadata —
expected, but a surprise if you assume file bytes equal disk bytes.

Bound any listing you expose. `MAX_ENTRIES` and `MAX_DEPTH` in `sysinfo.py`
cap the walk so a pathological tree cannot stall the server or blow the heap.

**RTC memory** — 8KB of SRAM in the RTC power domain, verified working:

```python
machine.RTC().memory(b'breadcrumb')   # survives reset, not power loss
```

Right for "what was I doing when I died?". Wrong for anything that must outlive
a pulled cable.

---

## What the board can tell you about itself

The richest finds, all verified:

**`esp32.idf_heap_info(esp32.HEAP_DATA)`** — the allocator *underneath*
MicroPython's `gc`. Nine regions, each `(total, free, largest, min_free)`.
`min_free` is a **high-water mark: the worst moment since boot**. That is the
figure that predicts an out-of-memory crash, and nothing else on the board
exposes it. `largest / free` gives fragmentation — a big allocation can fail
with megabytes nominally free.

**`esp32.Partition.find()`** — the flash map above.

**`machine.unique_id()`** — same bytes as the WiFi MAC.

**`esp32.mcu_temperature()`** — die temperature, not ambient. Idles ~38-40C.

**`micropython.stack_use()`** — ~720 bytes at rest.

**`platform.platform()`** — the closest thing to a build fingerprint:
toolchain (GCC 14.2.0), libc (newlib 4.3.0), IDF version.

**Capability probing** — importing each of 45 optional modules and recording
which succeed is more useful than any version string: it says what you can
import *before* you try. 38 present here, including `bluetooth`, `espnow`,
`ssl`, `cryptolib`, `btree`, `framebuf`, `asyncio`.

**Live pin map** — `Pin(n, Pin.IN).value()` across 0-48. Pins that raise on
construction are claimed by flash/PSRAM/USB, and *the refusals are the data*:

```text
readable: 33 of 49
claimed : 22-37          <- octal PSRAM and flash
high    : 0, 20, 43, 44  <- pin 0 is BOOT, unpressed
```

---

## Measured performance

### Boot, reset to first page served

```text
interpreter+imports     492 ms
wifi join              3573 ms     <- 84%
ntp sync                189 ms
serving                   2 ms
TOTAL                  4256 ms
```

**WiFi join dominates.** Nothing else is worth optimising until it is, and it is
largely outside the code's control — DHCP and association on a marginal link.
Everything the code governs totals under 700ms.

`ticks_ms()` counts from reset, so the first call in `main.py` already measures
interpreter startup plus imports. That number is free and easy to discard.

### CPU, against desktop CPython

| Test | S3 @ 240MHz | Desktop CPython | Ratio |
|---|---|---|---|
| integer | 272k ops/s | 9.8M | 36x |
| float | 151k ops/s | 15.4M | 102x |
| **string concat** | **20k ops/s** | 15.2M | **765x** |
| list | 165k ops/s | 12.0M | 73x |

**The string figure is the design input.** Naive concatenation allocates on
every step, and that is where a small interpreter on a small heap suffers most —
three orders of magnitude, against 36x for integer arithmetic.

Concrete consequence: **keep string building off the board.** Serve small flat
JSON and let the browser assemble anything textual.

### HTTP

- `/api/stats` (~850B JSON): **5-50ms** to route
- `/api/static` (~1.4KB): **570ms** — the module-probing loop dominates
- Page (~32KB HTML): served fine, but it is the single largest payload

---

## Design rules this board teaches

**The board serves data; the browser does the work.** The real constraint is not
memory — 8MB is plenty — it is that the board answers **one request at a time**.
Every cycle spent describing itself is a cycle not spent serving. So `/api/stats`
is a few hundred bytes of flat JSON, and the browser keeps history, scales axes,
draws charts and formats every number.

**Split endpoints by how the data behaves, not by topic.**

| | Cost | Cadence |
|---|---|---|
| static (build, flash map, capabilities) | 570ms | once per page load |
| dynamic (heap, temp, signal) | ~5ms | 2s |
| deep (per-region heap) | ~30ms | 15s |
| on demand (GC, pins, benchmark) | varies | explicit click |

Polling 1.4KB of immutable build information 30x a minute is pure waste.

**Fixed-size structures only.** A dict keyed on request path is a memory leak
with a URL as its trigger — cap it. Ring buffers for recent events.

**Report `None`, never a plausible fake.** Off-board, half these figures are
meaningless. `signal_quality(None)` returns `"unknown"`, not a guess. A fake
number on a diagnostics page is worse than a blank, because you might believe it.

---

## Gotchas, in the order they bit

### 1. The REPL and the app compete

`mpremote exec` interrupts `main.py`. A board left at a REPL prompt **pings but
refuses port 80** — that combination means "alive, app not running", not a
network fault. Reset to restore service.

This caused several false "the board is down" alarms during development. If the
web server vanishes right after you ran a REPL command, that is why.

### 2. `wifi_scan()` wedges the radio

`network.WLAN.scan()` drops the link while it runs. On this board, scanning
while associated **wedged it for over two minutes and took the web server down**
— the page that triggered the scan could not be reloaded to see the result.

Keep it out of any UI. It belongs in a deliberate REPL session. The data is
genuinely useful (29 visible APs; channel congestion explains a marginal RSSI
better than RSSI does) but not at that price.

### 3. A stuck process holds the COM port

A hung `mpremote` keeps the port after the process is killed; Windows does not
release the handle promptly. `taskkill` reported "process not found" while the
port stayed locked.

### 4. The second port is the recovery route

When COM8 was locked, **COM10 still worked** and was used to reset the board.
This is the one situation where the native USB port earns its keep. Having both
cables plugged in is worth it for this alone.

### 5. WS2812 cannot be read

Write-only protocol, no return path. `np[0]` *looks* like a read but returns
MicroPython's own bytearray — a record of what was last **sent**:

```python
np.buf[0] = 99
bytes(np.buf)      # -> b'c\x07\x00'   the LED never showed this
```

`Pin(48).value()` reads `0`, the idle level of a pulse line. Meaningless.

**Track what you set instead.** The module that decides the state is
authoritative anyway, and it keeps working when the LED is missing or disabled.

### 6. No battery-backed RTC

Boots at `2000-01-01 00:00:00` every time. Every timestamp is fiction until NTP
runs. Sync immediately after the link comes up, and **report `synced` alongside
the time** rather than quietly emitting a confident wrong answer.

MicroPython has no tzdata, so timezone is a fixed offset in config — an hour out
across a DST boundary, which is the right trade on a device that cannot know the
rules.

### 7. `esptool` v5 renamed subcommands

`erase_flash` → `erase-flash`, `write_flash` → `write-flash`. Underscores still
work but warn.

### 8. CPython has no `ticks_ms`

Shim it for the dev server, and use `perf_counter_ns` rather than `monotonic` —
these intervals are sub-millisecond on a desktop and `monotonic()` rounds them
to zero.

---

## Safety when exposing an endpoint that does work

An unauthenticated endpoint on a LAN is a free CPU-burn primitive for anything
that can reach port 80. The benchmark here carries three guards:

**Thermal.** Refuses above 70C (chip rated 85C), reports die temperature either
side of the run. The first draft ran **272ms and raised the die 2C** — too much
for a button anyone can hold down — so counts were cut 5x **by measurement**. It
now runs **54ms with a 1C rise**, and the throughput figures are unchanged,
confirming the shorter run measures the same thing.

**Rate limiting.** One real run per 30s; requests inside the window get the
cached result — cheaper *and* more useful than an error. Verified by hammering:

```text
10 rapid requests -> 0 real runs, 10 served from cache
die temperature   -> flat at 39-40C
```

**Blocking.** One request at a time means a benchmark stalls everything. At 54ms
a polling dashboard sees one slow frame, not a gap.

Generalise: **size any on-demand work by measuring it, not by guessing**, and
cache rather than erroring when refusing.

---

## Signal

This board reads **-77 to -81 dBm (marginal)** where it currently sits, on
channel 11 at 20dBm TX, with 29 visible APs competing.

RSSI is the **downlink only** — how well the board hears the router. It says
nothing about whether the board's replies get back, and that is the weaker
direction: the router has a real antenna and mains power, the board has a trace
on a PCB.

Practical effect: **mDNS is the first thing to fail.** A dozen rapid requests to
`esp32s3.local` lost two; the same dozen to `192.168.1.158` lost none. The
server was fine both times — name resolution was not.

Fixes, in order: move the board; failing that, a DHCP reservation on
`ac:a7:04:2c:2c:04` and address it by IP.

---

## Quick reference

```powershell
# identify
python -m esptool --port COM8 --chip esp32s3 flash-id

# flash (note offset 0x0 and the OCT build)
python -m esptool --chip esp32s3 --port COM8 erase-flash
python -m esptool --chip esp32s3 --port COM8 --baud 460800 `
  write-flash -z 0x0 firmware\ESP32_GENERIC_S3-SPIRAM_OCT-*.bin

# deploy app files (~3 seconds)
.\deploy.ps1 -Port COM8

# REPL  (Ctrl+] to exit)
python -m mpremote connect COM8 repl

# health check without a cable
curl http://192.168.1.158/api/stats
curl http://192.168.1.158/api/files     # volumes + file tree
curl http://192.168.1.158/api/static    # data sheet, queried from the device
```

Sanity check after any reflash — if `mem_free` is not ~8.3MB, the PSRAM build
is wrong:

```python
import gc, machine, os
gc.collect()
print(gc.mem_free(), machine.freq(), os.uname().machine)
# 8318880 240000000 Generic ESP32S3 module with Octal-SPIRAM with ESP32-S3
```
