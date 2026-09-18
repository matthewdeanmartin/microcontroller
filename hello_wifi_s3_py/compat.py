"""Papers over the differences between CPython and MicroPython.

app.py imports this and never has to care which one it is running under.
Same role as the S2 project's compat.py, with one addition: `diagnostics()`
returns structured data rather than display strings, because this project
feeds a JSON API and a browser does the formatting.

Returning None for "cannot know" is deliberate and load-bearing. Under the
dev server there is no radio and no heap ceiling, and a plausible-looking
fake number on a diagnostics page is worse than a blank - you might believe
it. The browser renders None as "n/a".
"""

import gc
import sys
import time

IS_MICROPYTHON = sys.implementation.name == "micropython"

_START = time.time()

# RSSI thresholds in dBm, shared by the label and the browser's gauge so the
# two cannot disagree. Closer to zero is stronger; these are the conventional
# breakpoints and match the S2 project's compat.py.
RSSI_EXCELLENT = -60
RSSI_GOOD = -70
RSSI_MARGINAL = -80


def implementation_name():
    v = sys.implementation.version
    version = ".".join(str(p) for p in v[:3])
    return f"{'MicroPython' if IS_MICROPYTHON else 'CPython'} {version}"


def uptime_seconds():
    return int(time.time() - _START)


def _wlan():
    """The station interface, or None off the board."""
    if not IS_MICROPYTHON:
        return None
    try:
        import network

        return network.WLAN(network.STA_IF)
    except Exception:  # noqa: BLE001
        return None


def free_memory():
    """Free heap in bytes, or None where the question is meaningless."""
    if not IS_MICROPYTHON:
        return None
    gc.collect()
    return gc.mem_free()


def allocated_memory():
    if not IS_MICROPYTHON:
        return None
    return gc.mem_alloc()


def rssi():
    w = _wlan()
    if w is None:
        return None
    try:
        return w.status("rssi")
    except Exception:  # noqa: BLE001
        return None


def signal_quality(value):
    """Map an RSSI to a label. None -> 'unknown', never a guess."""
    if value is None:
        return "unknown"
    if value >= RSSI_EXCELLENT:
        return "excellent"
    if value >= RSSI_GOOD:
        return "good"
    if value >= RSSI_MARGINAL:
        return "marginal"
    return "unreliable"


def _flash_size():
    if not IS_MICROPYTHON:
        return None
    try:
        import esp

        return esp.flash_size()
    except Exception:  # noqa: BLE001
        return None


def _filesystem():
    """(total, free) bytes of the on-board filesystem."""
    if not IS_MICROPYTHON:
        return None, None
    try:
        import os

        s = os.statvfs("/")
        return s[0] * s[2], s[0] * s[3]
    except Exception:  # noqa: BLE001
        return None, None


def _cpu_mhz():
    if not IS_MICROPYTHON:
        return None
    try:
        import machine

        return machine.freq() // 1_000_000
    except Exception:  # noqa: BLE001
        return None


def _temperature():
    """Internal die temperature in Celsius, if the port exposes it.

    Present on the S3 and absent on plenty of other ports, hence the guard.
    This is the SoC die, not the room - it reads well above ambient.
    """
    if not IS_MICROPYTHON:
        return None
    try:
        import esp32

        return round(esp32.mcu_temperature(), 1)
    except Exception:  # noqa: BLE001
        return None


def _network_info():
    w = _wlan()
    if w is None:
        return None, None, None
    try:
        ip = w.ifconfig()[0] if w.isconnected() else None
        mac = ":".join("{:02x}".format(b) for b in w.config("mac"))
        try:
            ssid = w.config("essid") or None
        except Exception:  # noqa: BLE001
            ssid = None
        return ip, mac, ssid
    except Exception:  # noqa: BLE001
        return None, None, None


def diagnostics():
    """Everything the dashboard shows, as plain data.

    One function so the HTML page and the JSON API cannot drift apart - the
    lesson from secret_messages, where two code paths reporting the same
    figures was a bug waiting to happen.
    """
    fs_total, fs_free = _filesystem()
    ip, mac, ssid = _network_info()
    signal = rssi()

    # The LED cannot be read back - WS2812 is write-only - so led.py reports
    # what it last set rather than what the hardware shows. Imported here
    # rather than at module level so compat stays usable on its own.
    try:
        import led

        led_status = led.status()
    except Exception:  # noqa: BLE001
        led_status = None

    return {
        "led": led_status,
        "implementation": implementation_name(),
        "is_board": IS_MICROPYTHON,
        "uptime": uptime_seconds(),
        "mem_free": free_memory(),
        "mem_alloc": allocated_memory(),
        "fs_total": fs_total,
        "fs_free": fs_free,
        "flash_size": _flash_size(),
        "cpu_mhz": _cpu_mhz(),
        "temperature": _temperature(),
        "rssi": signal,
        "quality": signal_quality(signal),
        "ip": ip,
        "mac": mac,
        "ssid": ssid,
    }


def json_dumps(obj):
    """Serialise to JSON.

    MicroPython ships `json`, so this is a thin alias - but routing it through
    compat keeps app.py free of conditional imports.
    """
    import json

    return json.dumps(obj)


def format_response(status, content_type, body):
    """Build a raw HTTP/1.0 response.

    Shared so the dev server and the board emit byte-identical output: a wrong
    header is then wrong in both places and surfaces locally, not after a flash.
    """
    reason = {
        200: "OK",
        204: "No Content",
        404: "Not Found",
        500: "Internal Server Error",
    }.get(status, "OK")
    body_bytes = body.encode("utf-8") if isinstance(body, str) else body

    head = (
        f"HTTP/1.0 {status} {reason}\r\n"
        f"Content-Type: {content_type}\r\n"
        f"Content-Length: {len(body_bytes)}\r\n"
        "Connection: close\r\n"
        "\r\n"
    )
    return head.encode("utf-8") + body_bytes
