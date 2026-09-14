# microcontroller

Microcontroller experiments, currently a **DiGiYes ESP32-S2 Mini V1.0.0**
(ESP32-S2FN4R2 — 4MB flash, 2MB PSRAM, 2.4GHz WiFi, native USB).

Focus is **self-hosted web things**: small always-on servers on cheap hardware.
No soldering, no GPIO wiring.

## Projects

| Project | What it is |
|---|---|
| [`hello_wifi`](hello_wifi/) | C / ESP-IDF. Status page over WiFi. |
| [`hello_wifi_py`](hello_wifi_py/) | MicroPython. Same page, plus a local preview and a 3-second deploy. |
| [`secret_messages`](secret_messages/) | MicroPython. A pastebin for the house — each message encrypted so only its intended readers can open it. Mastodon DMs the recipient a link, with no key on the board. |

## Docs

Full write-up in [`docs/`](docs/) — build with `cd docs && make serve`.

- **Basic Setup** — ESP-IDF from unboxing to serving a page, including the
  failures along the way
- **MicroPython** — the Python workflow, `esp32.local`, signal and placement,
  the REPL
- **Secret Messages** — the encryption scheme and why it is not public-key
  crypto, the board diagnostics tab, and using Mastodon as a doorbell

## Quick start

```powershell
cd hello_wifi_py
python dev_server.py          # preview at localhost:8000, no board needed
.\deploy.ps1 -Port COM6       # push to the board, ~3 seconds
```

Then <http://esp32.local>.

## Roadmap

Ideas for useful home-server applications, roughly easiest first. All are
web-only — no sensors, no soldering.

### Family message board — ~~done~~, see [`secret_messages`](secret_messages/)

Built, with per-recipient encryption on top. The one part not done is
persistence: it holds 50 messages in RAM and a reboot clears them. Writing to
flash is the obvious next step, and the reason to want it.

### Household dashboard

One page pulling together a few things worth glancing at — weather, transit
times, whatever APIs are handy. The board fetches on a timer and caches.

Teaches outbound HTTP and JSON parsing, which opens up every public API.

### Chore / rota tracker

Checkboxes that reset weekly. A small state machine, and a natural step up from
a static status page.

### Countdown / status sign

"Days until X", "bins go out tomorrow", a clock. Low effort, and nicer as a
dedicated always-on page than as a phone app.

### Uptime monitor

The board pings your other services and shows what is up. A pleasing inversion —
the tiny thing watching the big things — and a natural extension of
`hello_wifi_py/watch.ps1`.

### LAN pastebin — ~~done~~, folded into [`secret_messages`](secret_messages/)

Paste on one device, read it on another. It ended up with accounts after all,
because "only these people can read it" turned out to be the interesting part.

## Constraints worth remembering

The board is a tiny always-on computer, not a server:

- **~1MB RAM** for your code after the interpreter
- **One request at a time** — fine for a household, wrong for real traffic
- **No practical HTTPS** — TLS is painful at this size
- **Slow** — roughly 10–100× slower than C, which rarely matters for I/O

Anything needing real traffic, large data, or secrets over the internet belongs
on a real machine.
