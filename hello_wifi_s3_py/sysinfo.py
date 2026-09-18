"""Everything the board can say about itself.

Organised by how the numbers behave, because that is what decides how the
dashboard should treat them:

    static()    fixed for the life of the firmware. Chip, flash map, build,
                Python capabilities. Fetch once; polling it is waste.
    dynamic()   changes continuously. Heap, temperature, signal, clock.
    ondemand()  costly or disruptive, so only on an explicit click:
                a GC pass, a WiFi scan, a pin sweep.

The split matters on a board that answers one request at a time. The default
dashboard poll should carry only what actually moves, and the ~1.5KB of static
data should be fetched once per page load rather than 30 times a minute.

Everything here is defensive. Any individual probe may be missing on another
port or build, so each is wrapped and returns None rather than taking the
whole endpoint down. A system-info page that 500s is worse than one reporting
"n/a".
"""

import gc
import sys


def _try(fn, default=None):
    """Run a probe, swallowing anything it raises."""
    try:
        return fn()
    except Exception:  # noqa: BLE001 - one bad probe must not kill the page
        return default


# ---------------------------------------------------------------------------
# Static - fetch once
# ---------------------------------------------------------------------------


def _chip():
    def _uid():
        import binascii
        import machine

        return binascii.hexlify(machine.unique_id()).decode()

    def _freq():
        import machine

        return machine.freq()

    out = {"unique_id": _try(_uid), "freq_hz": _try(_freq)}

    # esptool reports the marketing name; the board itself only knows what
    # os.uname() was built with, so report that rather than inventing a model.
    def _uname():
        import os

        return os.uname()

    u = _try(_uname)
    if u:
        out["machine"] = u.machine
        out["sysname"] = u.sysname
        out["release"] = u.release
        out["version"] = u.version
    return out


def _python():
    """What this MicroPython can do - the interpreter's own data sheet."""
    out = {
        "implementation": "{} {}".format(
            sys.implementation.name,
            ".".join(str(p) for p in sys.implementation.version[:3]),
        ),
        "platform": _try(lambda: sys.platform),
        "byteorder": _try(lambda: sys.byteorder),
        "maxsize": _try(lambda: sys.maxsize),
        "path": _try(lambda: list(sys.path)),
        "mpy_version": _try(lambda: sys.implementation._mpy),
    }

    # platform.platform() encodes the toolchain and libc, which is the closest
    # thing to a build fingerprint the board can hand you.
    def _plat():
        import platform

        return {
            "platform": platform.platform(),
            "libc": "-".join(str(x) for x in platform.libc_ver()),
            "compiler": platform.python_compiler(),
        }

    p = _try(_plat)
    if p:
        out.update(p)

    out["opt_level"] = _try(lambda: __import__("micropython").opt_level())

    # Which optional modules this build actually shipped. More useful than a
    # version string: it tells you what you can import before you try.
    mods = (
        "bluetooth espnow ssl tls cryptolib hashlib deflate btree "
        "framebuf neopixel dht onewire asyncio requests umqtt.simple "
        "ntptime webrepl machine esp esp32 network socket json re "
        "array struct binascii random math cmath collections heapq io "
        "select time uctypes vfs errno"
    ).split()
    present = []
    for m in mods:
        try:
            __import__(m)
            present.append(m)
        except Exception:  # noqa: BLE001
            pass
    out["modules"] = present
    out["module_count"] = len(present)
    return out


def _partitions():
    """The flash map - where the 16MB actually went.

    This is the answer to "why does a 16MB chip show a 14MB filesystem", in
    the board's own words rather than arithmetic.
    """

    def go():
        import esp32

        rows = []
        for kind, label in (
            (esp32.Partition.TYPE_APP, "app"),
            (esp32.Partition.TYPE_DATA, "data"),
        ):
            for p in esp32.Partition.find(kind):
                t, st, addr, size, name, enc = p.info()
                rows.append(
                    {
                        "name": name,
                        "kind": label,
                        "subtype": st,
                        "offset": addr,
                        "size": size,
                        "encrypted": bool(enc),
                    }
                )
        rows.sort(key=lambda r: r["offset"])
        return rows

    return _try(go, [])


def _flash():
    def go():
        import esp

        return esp.flash_size()

    return _try(go)


def static():
    """Fixed for the life of the firmware. Safe to cache in the browser."""
    return {
        "chip": _chip(),
        "python": _python(),
        "partitions": _partitions(),
        "flash_size": _flash(),
        "boot": {
            "reset_cause": _try(
                lambda: __import__("machine").reset_cause()
            ),
            "reset_cause_name": _reset_cause_name(),
            "wake_reason": _try(
                lambda: __import__("machine").wake_reason()
            ),
        },
    }


def _reset_cause_name():
    """machine.reset_cause() as a word rather than an integer."""

    def go():
        import machine

        names = {
            getattr(machine, "PWRON_RESET", -1): "power-on",
            getattr(machine, "HARD_RESET", -2): "hard reset",
            getattr(machine, "WDT_RESET", -3): "watchdog",
            getattr(machine, "DEEPSLEEP_RESET", -4): "deep sleep",
            getattr(machine, "SOFT_RESET", -5): "soft reset",
        }
        return names.get(machine.reset_cause(), "unknown")

    return _try(go, "unknown")


# ---------------------------------------------------------------------------
# Dynamic - poll this
# ---------------------------------------------------------------------------


def _idf_heap():
    """Per-region heap from ESP-IDF, with high-water marks.

    gc.mem_free() is one number for MicroPython's own heap. This is the layer
    underneath: every region the IDF allocator manages, each with a `min_free`
    recording the worst moment since boot. That high-water mark is the figure
    that predicts an out-of-memory crash, and nothing else on the board
    exposes it.
    """

    def go():
        import esp32

        out = []
        for cap, label in (
            (esp32.HEAP_DATA, "data"),
            (esp32.HEAP_EXEC, "exec"),
        ):
            for total, free, largest, min_free in esp32.idf_heap_info(cap):
                # Regions of a few bytes are allocator bookkeeping and only
                # add noise to a chart.
                if total < 4096:
                    continue
                out.append(
                    {
                        "kind": label,
                        "total": total,
                        "free": free,
                        "largest": largest,
                        "min_free": min_free,
                        # Fragmentation: how much of what is free sits in the
                        # single biggest block. Low means a large allocation
                        # will fail despite plenty of total free space.
                        "contiguous_pct": round(100 * largest / free, 1)
                        if free
                        else 0.0,
                    }
                )
        return out

    return _try(go, [])


def _mem():
    gc.collect()
    free = _try(lambda: gc.mem_free())
    alloc = _try(lambda: gc.mem_alloc())
    out = {"free": free, "alloc": alloc}
    if free is not None and alloc is not None:
        out["total"] = free + alloc
        out["used_pct"] = round(100 * alloc / (free + alloc), 2)
    out["stack_use"] = _try(
        lambda: __import__("micropython").stack_use()
    )
    return out


def _clock():
    """Wall clock and monotonic counters.

    An unsynced ESP32 boots at 2000-01-01, so `synced` is a real signal: if
    the year is still 2000 the board has never reached an NTP server, and any
    timestamp it produces is meaningless.
    """

    def go():
        import time

        lt = time.localtime()
        return {
            "localtime": list(lt),
            "year": lt[0],
            "synced": lt[0] > 2000,
            "ticks_ms": time.ticks_ms(),
        }

    return _try(go, {})


def _wifi():
    def go():
        import network

        w = network.WLAN(network.STA_IF)
        out = {"connected": w.isconnected(), "active": w.active()}
        if w.isconnected():
            cfg = w.ifconfig()
            out.update({"ip": cfg[0], "netmask": cfg[1],
                        "gateway": cfg[2], "dns": cfg[3]})
        for key in ("ssid", "channel", "txpower", "hostname", "reconnects", "pm"):
            out[key] = _try(lambda k=key: w.config(k))
        out["rssi"] = _try(lambda: w.status("rssi"))
        out["status"] = _try(w.status)
        ap = _try(lambda: network.WLAN(network.AP_IF).active())
        out["ap_active"] = ap
        return out

    return _try(go, {})


def _temperature():
    def go():
        import esp32

        return round(esp32.mcu_temperature(), 1)

    return _try(go)


def _filesystem():
    def go():
        import os

        s = os.statvfs("/")
        total, free = s[0] * s[2], s[0] * s[3]
        return {
            "block_size": s[0],
            "total": total,
            "free": free,
            "used": total - free,
            "used_pct": round(100 * (total - free) / total, 2) if total else 0,
            "files": _try(lambda: sorted(os.listdir("/")), []),
        }

    return _try(go, {})


def dynamic():
    """Everything that moves. This is what the dashboard polls."""
    return {
        "mem": _mem(),
        "idf_heap": _idf_heap(),
        "temperature": _temperature(),
        "wifi": _wifi(),
        "clock": _clock(),
        "filesystem": _filesystem(),
    }


# ---------------------------------------------------------------------------
# On demand - explicit click only
# ---------------------------------------------------------------------------


def gc_probe():
    """Run a collection and report what it reclaimed.

    Deliberately not part of dynamic(): a forced collect is real work and
    pausing the interpreter on a timer to measure it would be measuring the
    measurement.
    """

    def go():
        import time

        before = gc.mem_free()
        t0 = time.ticks_us()
        gc.collect()
        elapsed = time.ticks_diff(time.ticks_us(), t0)
        after = gc.mem_free()
        return {
            "before": before,
            "after": after,
            "reclaimed": after - before,
            "micros": elapsed,
        }

    return _try(go, {})


def wifi_scan():
    """Survey the 2.4GHz band. SLOW AND DISRUPTIVE - see the warning.

    !! This drops the WiFi link while it runs. On this board a scan while
    associated wedged the radio for over two minutes and took the web server
    down with it - the page that triggered the scan could not be reloaded to
    see the result.

    So it is not wired to a dashboard button. It is kept here because the data
    is genuinely useful (channel congestion explains a marginal RSSI better
    than RSSI does), but it belongs in a REPL session or a maintenance window,
    not in a page anyone might click twice.

    Run it deliberately:

        mpremote connect COM8 exec "import sysinfo; print(sysinfo.wifi_scan())"
    """

    def go():
        import network

        w = network.WLAN(network.STA_IF)
        nets = []
        chan = {}
        for entry in w.scan():
            ssid, bssid, channel, rssi, sec, hidden = entry[:6]
            try:
                name = ssid.decode() or "(hidden)"
            except Exception:  # noqa: BLE001
                name = "(undecodable)"
            nets.append(
                {
                    "ssid": name,
                    "channel": channel,
                    "rssi": rssi,
                    "security": sec,
                    "hidden": bool(hidden),
                }
            )
            chan[channel] = chan.get(channel, 0) + 1
        nets.sort(key=lambda n: -n["rssi"])
        mine = _try(lambda: w.config("channel"))
        return {
            "count": len(nets),
            "networks": nets[:25],
            "by_channel": [
                {"channel": c, "count": chan[c]} for c in sorted(chan)
            ],
            "my_channel": mine,
            "my_channel_neighbours": chan.get(mine, 0),
        }

    return _try(go, {})


def pin_scan():
    """Read every GPIO that can be read.

    A live pin map, rather than a datasheet diagram: which pins exist on this
    chip, and what each is reading right now. Pins that are claimed by flash,
    PSRAM or USB raise on construction and are reported as unavailable, which
    is itself the useful information - it shows which pins are actually free.
    """

    def go():
        from machine import Pin

        rows = []
        for n in range(0, 49):
            try:
                rows.append({"pin": n, "value": Pin(n, Pin.IN).value(),
                             "ok": True})
            except Exception:  # noqa: BLE001
                rows.append({"pin": n, "value": None, "ok": False})
        return rows

    return _try(go, [])

def mounts():
    """Mounted volumes - the board's equivalent of "drives".

    There is normally exactly one: a LittleFS2 volume at "/" backed by the
    `vfs` flash partition. More can appear if an SD card is mounted, which is
    why this enumerates rather than assuming.

    LittleFS2 is worth knowing about: it is log-structured and power-fail safe,
    which is why pulling the plug mid-write does not corrupt the filesystem the
    way FAT would.
    """

    def go():
        import os
        import vfs

        out = []
        for obj, path in vfs.mount():
            row = {"path": path, "type": type(obj).__name__}
            try:
                st = os.statvfs(path)
                total, free = st[0] * st[2], st[0] * st[3]
                row.update(
                    {
                        "block_size": st[0],
                        "total": total,
                        "free": free,
                        "used": total - free,
                        "used_pct": round(100 * (total - free) / total, 2)
                        if total
                        else 0,
                        "name_max": st[9],
                    }
                )
            except Exception:  # noqa: BLE001
                pass
            out.append(row)
        return out

    def fallback():
        """CPython has no `vfs`. Report the working directory instead, so the
        dev server shows a real volume rather than an empty panel."""
        import os
        import shutil

        total, used, free = shutil.disk_usage(".")
        return [
            {
                "path": os.getcwd().replace("\\", "/"),
                "type": "host filesystem (dev server)",
                "total": total,
                "free": free,
                "used": used,
                "used_pct": round(100 * used / total, 2) if total else 0,
            }
        ]

    return _try(go) or _try(fallback, [])


# Directory listings are capped so a pathological tree cannot blow the heap or
# stall the single-threaded server. Neither limit is close to being hit by a
# normal MicroPython filesystem.
MAX_ENTRIES = 300
MAX_DEPTH = 4


def listdir(path=None, recursive=True):
    """Walk the filesystem and return every entry with type and size.

    `path` defaults to the filesystem root on the board, and to the project
    directory under CPython - walking "/" on a desktop would enumerate the
    whole drive, which is neither useful nor fast.

    Uses ilistdir() rather than listdir() + stat(): one call returns name,
    type and size together, which on a board doing this over HTTP is the
    difference between one pass and N+1 syscalls.

    The mode field from ilistdir is 0x4000 for a directory and 0x8000 for a
    regular file.
    """

    def go():
        import os

        root = path
        if root is None:
            root = "/" if hasattr(os, "ilistdir") else "."

        entries = []
        truncated = [False]

        def walk(d, depth):
            if depth > MAX_DEPTH or len(entries) >= MAX_ENTRIES:
                truncated[0] = True
                return
            try:
                lister = getattr(os, "ilistdir", None) or os.scandir
                items = sorted(
                    lister(d),
                    key=lambda e: e[0] if isinstance(e, tuple) else e.name,
                )
            except OSError:
                return
            for item in items:
                if len(entries) >= MAX_ENTRIES:
                    truncated[0] = True
                    return
                # MicroPython yields (name, mode, inode, size); CPython's
                # os.scandir yields DirEntry. Handle both so the dev server
                # shows a real tree.
                if isinstance(item, tuple):
                    name, mode = item[0], item[1]
                    size = item[3] if len(item) > 3 else 0
                    is_dir = bool(mode & 0x4000)
                else:
                    name = item.name
                    is_dir = item.is_dir()
                    size = 0 if is_dir else item.stat().st_size
                base = d.rstrip("/")
                full = (base + "/" + name) if base else "/" + name
                entries.append(
                    {
                        "name": name,
                        "path": full,
                        "dir": is_dir,
                        "size": 0 if is_dir else size,
                        "depth": depth,
                    }
                )
                if is_dir and recursive:
                    walk(full, depth + 1)

        walk(root, 0)

        files = [e for e in entries if not e["dir"]]
        return {
            "root": root,
            "entries": entries,
            "file_count": len(files),
            "dir_count": len(entries) - len(files),
            "total_bytes": sum(e["size"] for e in files),
            "truncated": truncated[0],
            "limits": {"max_entries": MAX_ENTRIES, "max_depth": MAX_DEPTH},
        }

    return _try(go, {})
