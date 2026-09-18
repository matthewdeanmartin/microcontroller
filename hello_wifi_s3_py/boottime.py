"""Where the seconds between reset and 'serving' actually go.

The board already knows how long it has been up. What it did not record was
how it *got* there - and boot is where the slow, failure-prone work happens:
joining WiFi, reaching an NTP server, binding a socket.

`time.ticks_ms()` counts from reset, so the very first call in main.py is
already a measurement of everything before it - the interpreter starting,
imports resolving, main.py being compiled. That number is free and was
previously thrown away.

Phases are recorded as they complete. A board that dies during WiFi join still
has its earlier phases readable from the REPL, which is exactly when you want
them.
"""

import time

# See the note in bench.py: CPython has no ticks_* counters.
if not hasattr(time, "ticks_ms"):
    time.ticks_ms = lambda: int(time.monotonic() * 1000)
    time.ticks_diff = lambda a, b: a - b

_phases = []       # [(name, ms_at_completion, duration_ms), ...]
_last = 0
_ready = None      # ms from reset to serving


def mark(name):
    """Record the completion of a boot phase."""
    global _last
    now = time.ticks_ms()
    _phases.append((name, now, time.ticks_diff(now, _last)))
    _last = now
    return now


def ready():
    """Called once the socket is bound and the board is serving."""
    global _ready
    _ready = mark("serving")
    return _ready


def status():
    """The boot timeline, for the JSON API.

    `total_ms` is time from chip reset to first request served. Everything
    before the first mark() is attributed to 'interpreter+imports', which is
    honest: it is one bucket covering several things this module cannot see
    inside.
    """
    return {
        "total_ms": _ready,
        "phases": [
            {"name": n, "at_ms": at, "duration_ms": d} for n, at, d in _phases
        ],
    }
