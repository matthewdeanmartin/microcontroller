"""The on-board RGB LED, used as a status indicator.

One WS2812 ("NeoPixel") on GPIO48. Unlike a plain LED it carries colour, so
the board can report *which* state it is in rather than just "alive":

    booting      white     powered, running main.py
    connecting   amber     pulsing, trying to join WiFi
    serving      green     breathing slowly, ready
    error        red       WiFi failed; the page will never come up

The point is a board that explains itself without a serial cable. The S2
project's one real annoyance was that a board which failed to join the network
looked exactly like a board that was working - you had to plug in to find out.

Everything here fails soft. A wrong pin, a board without the LED, a driver
that raises - none of it should stop the web server, which is the actual job.
So every public function swallows exceptions and the module sets
`available` to False rather than raising at import.
"""

try:
    from machine import Pin
    from neopixel import NeoPixel
except ImportError:  # CPython, under dev_server.py - no hardware at all.
    Pin = None
    NeoPixel = None

try:
    from config import NEOPIXEL_PIN
except ImportError:
    NEOPIXEL_PIN = None

# Brightness ceiling. The WS2812 at full scale is genuinely painful to look at
# on a desk, and nothing here needs to be visible across a room.
MAX = 40

_np = None
available = False

if Pin is not None and NEOPIXEL_PIN is not None:
    try:
        _np = NeoPixel(Pin(NEOPIXEL_PIN, Pin.OUT), 1)
        available = True
    except Exception:  # noqa: BLE001 - a missing LED must not stop the server
        _np = None
        available = False


# ---------------------------------------------------------------------------
# State tracking
#
# The WS2812 cannot be read back. It is a write-only protocol - one data line,
# no return path - so there is no way to ask the hardware what colour it is
# showing. `_np[0]` looks like a read but only returns MicroPython's own
# bytearray, which is a record of what was last *sent*, not what the LED is
# *doing*. Write to `np.buf` directly and it will happily report a colour the
# LED never displayed.
#
# So instead of pretending to read the light, this module remembers what it
# set. That is strictly better for the dashboard's purpose: the browser wants
# to know which state the board is in, and this module is the thing that
# decides, so it is the authoritative source either way.
#
# `_history` keeps transitions, not samples. serving() is called on every loop
# iteration and would otherwise flood the buffer with identical entries; only
# an actual change of state is recorded.
# ---------------------------------------------------------------------------

HISTORY = 12

_state = "init"          # current state name
_colour = (0, 0, 0)      # last colour written, for display
_since = 0               # uptime seconds when the current state began
_history = []            # [(state, uptime_seconds), ...] oldest first
_changes = 0             # total transitions, including ones aged out
_requests = 0            # blue flashes, i.e. requests served since boot


def _uptime():
    """Seconds since boot. Imported lazily to avoid a circular import."""
    try:
        import compat

        return compat.uptime_seconds()
    except Exception:  # noqa: BLE001
        return 0


def _enter(state):
    """Record a state transition, if this is actually a change."""
    global _state, _since, _changes
    if state == _state:
        return
    _state = state
    _since = _uptime()
    _changes += 1
    _history.append((state, _since))
    if len(_history) > HISTORY:
        _history.pop(0)


def _show(r, g, b, state=None):
    """Set the LED, or do nothing if there isn't one.

    The state is recorded even when there is no LED, so the dashboard still
    reports what the board is doing on a board whose light is missing or
    disabled.
    """
    global _colour
    if state is not None:
        _enter(state)
    _colour = (r, g, b)
    if not available:
        return
    try:
        _np[0] = (r, g, b)
        _np.write()
    except Exception:  # noqa: BLE001
        pass


def status():
    """What the LED is doing, as plain data for the JSON API.

    `colour` is the last value written, as a CSS hex string so the browser can
    paint a dot in it without a conversion table. Scaled up from the MAX
    ceiling to full 0-255 range, or every colour would render near-black on
    screen - the ceiling exists for the eye looking at the board, not for a
    monitor.
    """
    r, g, b = _colour
    scale = 255 // MAX if MAX else 1
    return {
        "state": _state,
        "colour": "#{:02x}{:02x}{:02x}".format(
            min(255, r * scale), min(255, g * scale), min(255, b * scale)
        ),
        "since": _since,
        "changes": _changes,
        "requests": _requests,
        "available": available,
        "pin": NEOPIXEL_PIN,
        "history": [{"state": s, "at": t} for s, t in _history],
    }


def off():
    _show(0, 0, 0, "off")


def booting():
    _show(MAX, MAX, MAX, "booting")


def connecting(step):
    """Amber, pulsing. `step` is a counter from the connect loop.

    Driven by the caller rather than a timer because MicroPython has no
    threads here - the connect loop is already sleeping, so it may as well
    drive the animation while it waits.
    """
    level = MAX if step % 2 == 0 else MAX // 4
    _show(level, level // 2, 0, "connecting")


def serving(step):
    """Green, breathing. Called from the request loop's idle moments."""
    # A triangle wave over 20 steps: rises for 10, falls for 10. Cheaper than
    # a sine and indistinguishable at this size.
    phase = step % 20
    level = phase if phase < 10 else 20 - phase
    _show(0, max(2, level * MAX // 10), 0, "serving")


def request():
    """A brief blue flash, so you can see traffic hitting the board.

    Deliberately NOT a state transition. This fires on every request and
    returns to `serving` a moment later, so recording it would fill the
    history with blue/green churn and push out the transitions that matter.
    It is counted instead - the count is the useful signal, not the sequence.
    """
    global _requests
    _requests += 1
    _show(0, 0, MAX)


def error():
    _show(MAX, 0, 0, "error")
