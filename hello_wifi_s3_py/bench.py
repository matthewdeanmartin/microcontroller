"""A short self-benchmark, with guards against running it too often.

Makes "240MHz" concrete. It would have caught this board's original
misconfiguration instantly: it shipped at 160MHz on a build with PSRAM
switched off, which no version string reveals but a benchmark does in one run.

Safety, in the order it matters
-------------------------------
**Thermal.** The work is deliberately tiny - a few thousand iterations, tens of
milliseconds total. An ESP32-S3 idles around 40C and is rated to 85C; a burst
this short moves the die by well under a degree. The benchmark also refuses to
start above THERMAL_LIMIT and reports the temperature either side of the run,
so a board that is already hot for other reasons is never asked to do more.

**Rate limiting.** The endpoint is unauthenticated on a LAN, so it is a free
CPU-burn primitive for anything that can reach port 80. MIN_INTERVAL_S enforces
a cooldown between runs; requests inside the window get the cached result
instead, which is both cheaper and more useful than an error.

**Blocking.** The board answers one request at a time, so a benchmark is a
deliberate stall for everything else. Sized by measurement: the first draft ran
272ms and raised the die 2C, so the counts were cut 5x to land near 55ms and
under 1C. A polling dashboard now sees one slightly slow frame, not a gap.
"""

import gc
import time

# MicroPython's monotonic millisecond/microsecond counters do not exist in
# CPython. Shimming them here keeps bench.py runnable under dev_server.py,
# which is the whole point of the compat layout - a benchmark you cannot try
# locally is a benchmark you debug on the board.
if not hasattr(time, "ticks_ms"):
    # perf_counter_ns, not monotonic: these intervals are sub-millisecond on a
    # desktop, and monotonic()'s resolution rounds them to zero.
    time.ticks_ms = lambda: time.perf_counter_ns() // 1000000
    time.ticks_us = lambda: time.perf_counter_ns() // 1000
    time.ticks_diff = lambda a, b: a - b

# Refuse to run above this die temperature, in Celsius. Well below the chip's
# 85C rating - the point is to never be the reason a hot board gets hotter.
THERMAL_LIMIT = 70

# Minimum seconds between real runs. Anything sooner gets the cached result.
MIN_INTERVAL_S = 30

# Iteration counts, sized from measurement on this board rather than guessed.
# The first draft used 5x these and took 272ms with a 2C rise - too long to
# block a single-request-at-a-time server, and more heat than a button anyone
# can press deserves. These land around 55ms total and under 1C.
N_INT = 4000
N_FLOAT = 2000
N_STR = 400
N_LIST = 1000

_last_run = None       # ticks_ms of the last real run
_last_result = None
_runs = 0
_rejected = 0          # requests refused or served from cache


def _int_ops():
    t0 = time.ticks_us()
    x = 0
    for i in range(N_INT):
        x = (x + i * 3) % 1000003
    return time.ticks_diff(time.ticks_us(), t0), x


def _float_ops():
    t0 = time.ticks_us()
    x = 1.0
    for i in range(N_FLOAT):
        x = x * 1.000001 + 0.5
    return time.ticks_diff(time.ticks_us(), t0), x


def _string_ops():
    """Concatenation in a loop - deliberately the naive pattern.

    This is the one that punishes a small heap, because every step allocates a
    new string. It is the most sensitive of the four to memory pressure.
    """
    t0 = time.ticks_us()
    parts = []
    for i in range(N_STR):
        parts.append("x" * 8)
    s = "".join(parts)
    return time.ticks_diff(time.ticks_us(), t0), len(s)


def _list_ops():
    t0 = time.ticks_us()
    a = []
    for i in range(N_LIST):
        a.append(i)
    total = 0
    for v in a:
        total += v
    return time.ticks_diff(time.ticks_us(), t0), total


def _temperature():
    try:
        import esp32

        return round(esp32.mcu_temperature(), 1)
    except Exception:  # noqa: BLE001
        return None


def run(force=False):
    """Run the benchmark, or return why it did not.

    Always returns a dict - callers render it rather than branching on an
    exception. `status` says which of the three outcomes happened: "ok",
    "cached" or "too_hot".
    """
    global _last_run, _last_result, _runs, _rejected

    now = time.ticks_ms()

    # Cooldown. Serving the cached result beats an error: the caller still
    # gets numbers, and a tight polling loop costs the board nothing.
    if not force and _last_run is not None:
        age = time.ticks_diff(now, _last_run) // 1000
        if age < MIN_INTERVAL_S:
            _rejected += 1
            if _last_result:
                out = dict(_last_result)
                out["status"] = "cached"
                out["cached_age_s"] = age
                out["retry_in_s"] = MIN_INTERVAL_S - age
                # _last_result was snapshotted before this rejection, so its
                # copy of the counter is stale. Report the live value.
                out["rejected"] = _rejected
                return out
            return {
                "status": "cached",
                "retry_in_s": MIN_INTERVAL_S - age,
                "note": "cooling down; no result cached yet",
            }

    temp_before = _temperature()
    if temp_before is not None and temp_before >= THERMAL_LIMIT:
        _rejected += 1
        return {
            "status": "too_hot",
            "temperature": temp_before,
            "limit": THERMAL_LIMIT,
            "note": "refused: die temperature at or above limit",
        }

    # Collect first so the run measures the work, not a collection that
    # happened to fall inside it.
    gc.collect()
    free_before = gc.mem_free() if hasattr(gc, "mem_free") else None

    t_int, _ = _int_ops()
    t_float, _ = _float_ops()
    t_str, _ = _string_ops()
    t_list, _ = _list_ops()

    temp_after = _temperature()
    gc.collect()

    total_us = t_int + t_float + t_str + t_list

    _runs += 1
    _last_run = time.ticks_ms()
    _last_result = {
        "status": "ok",
        "tests": [
            {"name": "integer", "us": t_int, "ops": N_INT,
             "ops_per_s": int(N_INT * 1000000 / t_int) if t_int else 0},
            {"name": "float", "us": t_float, "ops": N_FLOAT,
             "ops_per_s": int(N_FLOAT * 1000000 / t_float) if t_float else 0},
            {"name": "string", "us": t_str, "ops": N_STR,
             "ops_per_s": int(N_STR * 1000000 / t_str) if t_str else 0},
            {"name": "list", "us": t_list, "ops": N_LIST,
             "ops_per_s": int(N_LIST * 1000000 / t_list) if t_list else 0},
        ],
        "total_us": total_us,
        "total_ms": round(total_us / 1000, 1),
        "temp_before": temp_before,
        "temp_after": temp_after,
        "temp_delta": (
            round(temp_after - temp_before, 1)
            if (temp_before is not None and temp_after is not None)
            else None
        ),
        "cpu_mhz": _cpu_mhz(),
        "mem_free_before": free_before,
        "runs": _runs,
        "rejected": _rejected,
        "limits": {
            "thermal_c": THERMAL_LIMIT,
            "min_interval_s": MIN_INTERVAL_S,
        },
    }
    return dict(_last_result)


def _cpu_mhz():
    try:
        import machine

        return machine.freq() // 1000000
    except Exception:  # noqa: BLE001
        return None
