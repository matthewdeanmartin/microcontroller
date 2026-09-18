"""Wall-clock time, via NTP.

An ESP32 has no battery-backed RTC, so it boots at 2000-01-01 00:00:00 every
single time. Until something tells it otherwise, every timestamp it produces
is fiction - which is why `synced` is reported alongside the time rather than
quietly handing out a plausible-looking wrong answer.

`time.time()` still counts seconds since boot regardless, so uptime works
without a clock. Only absolute time needs NTP.

Timezone handling is deliberately crude: MicroPython has no tzdata, so this
stores a fixed UTC offset and applies it arithmetically. That is correct for
display and wrong across a DST boundary, which is a trade worth making on a
device with no way to know the rules.
"""

import time

try:
    from config import UTC_OFFSET_HOURS
except ImportError:
    UTC_OFFSET_HOURS = 0

# MicroPython's epoch is 2000-01-01; Unix is 1970-01-01. Converting between
# them needs this constant, and getting it wrong shifts everything by 30 years.
EPOCH_OFFSET = 946684800

_synced = False
_sync_time = None      # ticks_ms when the sync landed
_last_error = None
_attempts = 0


def is_synced():
    """True once NTP has set the clock.

    The year check is the reliable test: an unsynced board reports 2000, and
    no plausible real time is in that year any more.
    """
    return time.localtime()[0] > 2000


def sync(timeout=5):
    """Fetch time from NTP. Returns True on success.

    Called once at boot and available on demand from the dashboard. Failure is
    not fatal - the board serves pages perfectly well with a wrong clock, so
    this reports and moves on.
    """
    global _synced, _sync_time, _last_error, _attempts
    _attempts += 1
    try:
        import ntptime

        ntptime.timeout = timeout
        ntptime.settime()
        _synced = True
        _sync_time = time.ticks_ms()
        _last_error = None
        return True
    except Exception as e:  # noqa: BLE001 - a bad clock must not stop serving
        _last_error = str(e) or type(e).__name__
        return False


def _local_tuple():
    """localtime() shifted by the configured UTC offset."""
    t = time.time() + int(UTC_OFFSET_HOURS * 3600)
    return time.localtime(t)


def iso():
    """Current local time as an ISO-8601 string, or None if unsynced."""
    if not is_synced():
        return None
    y, mo, d, h, mi, s = _local_tuple()[:6]
    return "{:04d}-{:02d}-{:02d}T{:02d}:{:02d}:{:02d}".format(y, mo, d, h, mi, s)


def unix():
    """Seconds since the Unix epoch, or None if unsynced.

    Converted from MicroPython's 2000-based epoch so the browser can hand the
    number straight to `new Date()`.
    """
    if not is_synced():
        return None
    return time.time() + EPOCH_OFFSET


def boot_time_iso():
    """When the board booted, in wall-clock terms.

    Only knowable after a sync: uptime is always available, but placing it on
    a calendar needs an absolute reference.
    """
    if not is_synced():
        return None
    import compat

    booted = time.time() - compat.uptime_seconds() + int(UTC_OFFSET_HOURS * 3600)
    y, mo, d, h, mi, s = time.localtime(booted)[:6]
    return "{:04d}-{:02d}-{:02d}T{:02d}:{:02d}:{:02d}".format(y, mo, d, h, mi, s)


def status():
    """Everything about the clock, for the JSON API."""
    synced = is_synced()
    return {
        "synced": synced,
        "iso": iso(),
        "unix": unix(),
        "boot_time": boot_time_iso(),
        "utc_offset_hours": UTC_OFFSET_HOURS,
        "attempts": _attempts,
        "last_error": _last_error,
        "age_s": (
            time.ticks_diff(time.ticks_ms(), _sync_time) // 1000
            if _sync_time is not None
            else None
        ),
    }
