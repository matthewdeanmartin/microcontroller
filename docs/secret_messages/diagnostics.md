# The Board Tab

The third tab in the app: what the hardware is actually doing. The same
figures the `hello_wifi` status page showed, on a device now doing something
useful enough that you cannot just open a status page instead.

## What it shows

| | |
|---|---|
| **Running on** | MicroPython and version — or CPython, if this is the dev server |
| **Uptime** | since boot, formatted: `3d 4h`, not `273600s` |
| **Free memory** | `gc.mem_free()` after a collect, so it reflects reclaimable memory |
| **Network** | the SSID actually joined |
| **Address** | the current IP |
| **MAC** | for a DHCP reservation |
| **Signal** | RSSI in dBm, with a quality label and a bar |

Everything comes from `compat.diagnostics()` — one function returning plain
data, so the page and the JSON API cannot drift apart.

## Why the signal bar matters

RSSI is the number that predicts whether this project is pleasant or
infuriating, and `-67` means nothing without a scale. So the tab draws a bar,
using the same thresholds as the label:

| RSSI | Label | Bar |
|---|---|---|
| -30 to -60 | excellent | green |
| -60 to -70 | good | green |
| -70 to -80 | marginal | amber |
| below -80 | unreliable | red |

The range -90 to -30 is mapped onto the bar's width, since those are the ends
of the useful scale in practice.

The thresholds live in `compat.py` as named constants and are checked at their
boundaries in `test_app.py` — an off-by-one here would mislabel a working
signal as a failing one, which is worse than not showing it at all.

## The caveat, restated

This is the **downlink** only: how well the board hears the router. It says
nothing about whether the board's replies get back, and that is the weaker
direction — the router has a real antenna and mains power, the S2 has a trace
on a PCB.

So a board showing `-55 dBm (excellent)` can still serve pages slowly or drop
connections. If the tab looks healthy and the app does not feel healthy, the
uplink is the thing to suspect, and moving the board still helps. See
[Signal and Placement](../micropython/signal_and_placement.md), which covers
this in more detail for the earlier project.

## Off the board

Under `python dev_server.py` most of these have no meaning. The page says so
rather than inventing numbers:

```text
Free memory   n/a
Network       n/a
Address       n/a
Signal        n/a (dev server)

Most of these only mean something on the board. This is the dev server.
```

`compat.diagnostics()` returns `None` for each, and `signal_quality(None)` is
`"unknown"` rather than a guess. A fake number on a diagnostics page is worse
than a blank, because you might believe it.

## Fetched, not polled

The tab loads its figures when you open it, and not otherwise.

The board answers one request at a time. A diagnostics panel refreshing every
few seconds would mean the board spending a meaningful share of its life
answering questions about itself instead of serving messages — and the
figures are not changing fast enough to be worth it.

For actual monitoring over time, `watch.ps1` from `hello_wifi_py` still works
unchanged against `/health`:

```powershell
.\watch.ps1 -Target secrets.local
```

That endpoint is plain text and needs no sign-in, precisely so the existing
tooling keeps working:

```text
ok messages=3
```

The diagnostics API, by contrast, **does** require a session. The figures are
harmless, but the SSID and MAC are house details and there is no reason to
hand them to an unauthenticated caller.
