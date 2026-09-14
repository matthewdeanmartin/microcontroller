"""Papers over the differences between CPython and MicroPython.

app.py imports this and never has to care which one it is running under.

The differences that matter here:

  * free memory     - MicroPython has gc.mem_free(); CPython does not
  * uptime          - MicroPython's time has no time.monotonic() on all ports
  * implementation  - just for display on the page

Keeping every one of these behind a function means app.py stays identical in
both environments, which is the whole point of the layout.
"""

import gc
import sys
import time

# MicroPython reports "micropython" here; CPython reports "cpython".
IS_MICROPYTHON = sys.implementation.name == "micropython"

# Recorded at import, so uptime means "since the program started".
# On the board that is close enough to "since boot".
_START = time.time()


def implementation_name():
    """A short label for the page, e.g. 'MicroPython v1.29.0' or 'CPython 3.12.10'."""
    v = sys.implementation.version
    version = ".".join(str(p) for p in v[:3])
    if IS_MICROPYTHON:
        return f"MicroPython {version}"
    return f"CPython {version}"


def uptime_seconds():
    """Seconds since this program started."""
    return int(time.time() - _START)


def free_memory():
    """Free heap in bytes, or None where the question is meaningless.

    MicroPython can answer directly. CPython's heap grows on demand, so there
    is no fixed figure - returning None lets the page say so honestly instead
    of printing a fake number.
    """
    if IS_MICROPYTHON:
        gc.collect()  # so the figure reflects reclaimable memory, not garbage
        return gc.mem_free()
    return None


def free_memory_text():
    """free_memory() rendered for display."""
    free = free_memory()
    if free is None:
        return "n/a (only meaningful on the board)"
    return f"{free:,} bytes"


def rssi():
    """Raw RSSI in dBm, or None where unavailable. See signal_text()."""
    if not IS_MICROPYTHON:
        return None
    try:
        import network

        return network.WLAN(network.STA_IF).status("rssi")
    except Exception:  # noqa: BLE001
        return None


def signal_text():
    """WiFi signal strength as the board sees it.

    RSSI is in dBm, always negative, closer to zero is stronger:
        -30..-60 excellent   -60..-70 good
        -70..-80 marginal    below -80 unreliable

    This is the DOWNLINK only - how well the board hears the router. It says
    nothing about whether the board's replies get back, which is the weaker
    path for a small antenna far from the AP.
    """
    if not IS_MICROPYTHON:
        return "n/a (dev server)"

    try:
        import network

        wlan = network.WLAN(network.STA_IF)
        rssi = wlan.status("rssi")

        if rssi >= -60:
            quality = "excellent"
        elif rssi >= -70:
            quality = "good"
        elif rssi >= -80:
            quality = "marginal"
        else:
            quality = "unreliable"

        return f"{rssi} dBm ({quality})"
    except Exception as e:  # noqa: BLE001 - never let the status page 500
        return f"unknown ({e})"


def network_text():
    """IP and MAC, for display.

    Shown on the page so that once you have found the board, everything needed
    to find it again - the address now, and the MAC for a DHCP reservation -
    is visible without a serial cable.
    """
    if not IS_MICROPYTHON:
        return "localhost (dev server)"

    try:
        import network

        wlan = network.WLAN(network.STA_IF)
        ip = wlan.ifconfig()[0]
        mac = ":".join("{:02x}".format(b) for b in wlan.config("mac"))
        return f"{ip} &nbsp; (MAC {mac})"
    except Exception as e:  # noqa: BLE001 - never let the status page 500
        return f"unknown ({e})"


def format_response(status, content_type, body):
    """Build a raw HTTP/1.0 response.

    Shared so that the local server and the board produce byte-identical
    output - if a header is wrong, it is wrong in both places and shows up
    during local testing rather than after a flash.
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
