"""Papers over CPython/MicroPython differences, as in hello_wifi_py.

Same rule as that project: app.py imports this and never asks which runtime it
is on. This version adds the primitives crypto.py needs - hashing, AES-CTR and
randomness - because those are exactly the places the two runtimes diverge
most sharply.

The AES split is worth calling out. MicroPython has `cryptolib` (AES in C);
CPython's standard library has no AES at all. So the board uses the fast
native path and the dev server falls back to aes_soft.py. CTR mode itself is
written once, here, so a bug in the mode cannot appear on one side only.
"""

import gc
import sys
import time

IS_MICROPYTHON = sys.implementation.name == "micropython"

_START = time.time()

# ---------------------------------------------------------------- hashing

if IS_MICROPYTHON:
    import hashlib as _hashlib

    def sha256(data):
        return _hashlib.sha256(data).digest()
else:
    import hashlib as _hashlib

    def sha256(data):
        return _hashlib.sha256(data).digest()


# ------------------------------------------------------------ randomness

if IS_MICROPYTHON:
    import os as _os

    def random_bytes(n):
        """Hardware RNG. The ESP32's urandom is backed by its RNG peripheral."""
        return _os.urandom(n)
else:
    import os as _os

    def random_bytes(n):
        return _os.urandom(n)


# ------------------------------------------------------------------- AES

if IS_MICROPYTHON:
    import cryptolib as _cryptolib

    def _aes_ecb_encrypt_block(key, block):
        # Mode 1 is ECB. We only ever encrypt the counter with it, never user
        # data - ECB on real data would leak repeated blocks.
        return _cryptolib.aes(key, 1).encrypt(block)
else:
    from aes_soft import AES128

    def _aes_ecb_encrypt_block(key, block):
        return AES128(key).encrypt_block(block)


def aes_ctr(key, nonce, data):
    """AES-128 in counter mode. Encryption and decryption are the same call.

    CTR turns the block cipher into a stream cipher: encrypt a counter, XOR
    the result over the data. Chosen over CBC because it needs no padding -
    which matters when messages are short and the store is in RAM.

    SAFETY: a (key, nonce) pair must never encrypt two different messages.
    Here every message gets a fresh random body key AND a fresh random nonce,
    so the pair is unique by construction.
    """
    if len(key) != 16:
        raise ValueError("aes_ctr needs a 16-byte key")
    if len(nonce) != 8:
        raise ValueError("aes_ctr needs an 8-byte nonce")

    out = bytearray(len(data))
    counter = 0
    for offset in range(0, len(data), 16):
        # Counter block: 8-byte nonce then an 8-byte big-endian block index.
        block = nonce + counter.to_bytes(8, "big")
        stream = _aes_ecb_encrypt_block(key, block)

        chunk = min(16, len(data) - offset)
        for i in range(chunk):
            out[offset + i] = data[offset + i] ^ stream[i]
        counter += 1

    return bytes(out)


# ------------------------------------------------------------ base64-ish

# Messages are held as bytes but must survive a trip through JSON, which is
# text. MicroPython has binascii but not always base64, so hex is used: twice
# the size, but available everywhere and impossible to get subtly wrong.

def to_hex(data):
    return "".join("{:02x}".format(b) for b in data)


def from_hex(text):
    return bytes(int(text[i:i + 2], 16) for i in range(0, len(text), 2))


# ------------------------------------------------------------ board facts

def implementation_name():
    v = sys.implementation.version
    version = ".".join(str(p) for p in v[:3])
    return f"{'MicroPython' if IS_MICROPYTHON else 'CPython'} {version}"


def uptime_seconds():
    return int(time.time() - _START)


def free_memory():
    """Free heap in bytes, or None under CPython where the figure is meaningless."""
    if IS_MICROPYTHON:
        gc.collect()
        return gc.mem_free()
    return None


def free_memory_text():
    free = free_memory()
    if free is None:
        return "n/a"
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


# The thresholds the quality labels use, shared so the page and the API cannot
# disagree about what "good" means.
RSSI_EXCELLENT = -60
RSSI_GOOD = -70
RSSI_MARGINAL = -80


def signal_quality(value):
    """A one-word label for an RSSI figure."""
    if value is None:
        return "unknown"
    if value >= RSSI_EXCELLENT:
        return "excellent"
    if value >= RSSI_GOOD:
        return "good"
    if value >= RSSI_MARGINAL:
        return "marginal"
    return "unreliable"


def signal_text():
    """WiFi signal strength as the board sees it.

    RSSI is in dBm, always negative, closer to zero is stronger:
        -30..-60 excellent   -60..-70 good
        -70..-80 marginal    below -80 unreliable

    This is the DOWNLINK only - how well the board hears the router. It says
    nothing about whether the board's replies get back, which is the weaker
    path for a small antenna far from the AP. See
    docs/micropython/signal_and_placement.md.
    """
    if not IS_MICROPYTHON:
        return "n/a (dev server)"

    value = rssi()
    if value is None:
        return "unknown"
    return "{} dBm ({})".format(value, signal_quality(value))


def ip_address():
    """The board's IP, or None off-board."""
    if not IS_MICROPYTHON:
        return None
    try:
        import network

        return network.WLAN(network.STA_IF).ifconfig()[0]
    except Exception:  # noqa: BLE001
        return None


def mac_address():
    """The board's MAC as aa:bb:cc:dd:ee:ff - what a DHCP reservation needs."""
    if not IS_MICROPYTHON:
        return None
    try:
        import network

        mac = network.WLAN(network.STA_IF).config("mac")
        return ":".join("{:02x}".format(b) for b in mac)
    except Exception:  # noqa: BLE001
        return None


def ssid():
    """The network the board actually joined."""
    if not IS_MICROPYTHON:
        return None
    try:
        import network

        return network.WLAN(network.STA_IF).config("essid")
    except Exception:  # noqa: BLE001
        return None


def network_text():
    if not IS_MICROPYTHON:
        return "localhost (dev server)"
    ip = ip_address()
    return ip if ip else "unknown"


def diagnostics():
    """Everything the diagnostics page shows, as plain data.

    Gathered in one place and returned as a dict so the page, the JSON API and
    any future logging all report identical figures. Every value is optional:
    off-board most are None, and the page says so rather than inventing them.
    """
    value = rssi()
    free = free_memory()

    return {
        "implementation": implementation_name(),
        "on_board": IS_MICROPYTHON,
        "uptime": uptime_seconds(),
        "free_memory": free,
        "free_memory_text": free_memory_text(),
        "rssi": value,
        "signal_quality": signal_quality(value),
        "signal_text": signal_text(),
        "ip": ip_address(),
        "mac": mac_address(),
        "ssid": ssid(),
    }


def ticks_ms():
    """A monotonic millisecond counter, for session expiry.

    time.time() is used elsewhere for display, but it can jump if the clock is
    set from the network. Session expiry must not be affected by that.
    """
    if IS_MICROPYTHON:
        return time.ticks_ms()
    return int(time.monotonic() * 1000)


def ticks_diff(new, old):
    """Difference in ms, correct across MicroPython's counter wraparound."""
    if IS_MICROPYTHON:
        return time.ticks_diff(new, old)
    return new - old


def format_response(status, content_type, body, extra_headers=None):
    """Build a raw HTTP/1.0 response.

    Shared by the dev server and the board so both emit identical bytes - a
    wrong header shows up locally rather than after a flash.
    """
    reason = {
        200: "OK",
        204: "No Content",
        400: "Bad Request",
        401: "Unauthorized",
        403: "Forbidden",
        404: "Not Found",
        405: "Method Not Allowed",
        500: "Internal Server Error",
    }.get(status, "OK")

    body_bytes = body.encode("utf-8") if isinstance(body, str) else body

    head = (
        f"HTTP/1.0 {status} {reason}\r\n"
        f"Content-Type: {content_type}\r\n"
        f"Content-Length: {len(body_bytes)}\r\n"
    )

    # CORS is on from day one, even though everything is same-origin today.
    # The stated plan is to move the static files to a public host and leave
    # the API here; when that happens this header is already correct and the
    # move is a deploy, not a debugging session.
    head += "Access-Control-Allow-Origin: *\r\n"
    head += "Access-Control-Allow-Headers: Content-Type, X-Session\r\n"
    head += "Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n"

    for key, value in (extra_headers or {}).items():
        head += f"{key}: {value}\r\n"

    head += "Connection: close\r\n\r\n"
    return head.encode("utf-8") + body_bytes
