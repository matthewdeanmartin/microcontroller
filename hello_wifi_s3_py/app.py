"""The web app. THIS is the file you edit.

Runs unchanged in two places:

  * locally, under CPython, via `python dev_server.py`
  * on the board, under MicroPython, via main.py

The split that matters
----------------------
The board serves *data*; the browser does the *work*. `/api/stats` returns a
few hundred bytes of JSON and nothing else. All the charting, history and
formatting happens in JavaScript on a device with a vastly larger budget for
it.

So the board keeps no time series. The browser accumulates one in a ring
buffer as it polls, which means history costs the board nothing and survives
exactly as long as the tab is open - the right trade for a machine whose
entire job is to answer one request at a time.
"""

import compat

# ---------------------------------------------------------------------------
# Web server self-monitoring
#
# The board times its own routing and keeps the result in memory. This is the
# one latency figure the browser cannot work out for itself: the browser's
# timing includes WiFi round-trip and its own scheduling, so a slow page there
# could equally be a slow board or a weak link. Timing inside route() separates
# the two.
#
# Fixed-size structures only. Per-path counters over a handful of known routes,
# and a ring buffer of the last few requests - both bounded, so a board serving
# for a month uses exactly as much memory as one that booted a minute ago.
# ---------------------------------------------------------------------------

RECENT = 15

_req_count = 0
_req_bytes = 0
_req_micros = 0
_req_by_path = {}      # path -> [count, total_micros, max_micros]
_req_recent = []       # ring of the last RECENT requests


def record_request(path, status, micros, size):
    """Called by main.py after each route() call."""
    global _req_count, _req_bytes, _req_micros
    _req_count += 1
    _req_bytes += size
    _req_micros += micros

    row = _req_by_path.get(path)
    if row is None:
        # Cap the key space. An unbounded dict keyed on request path is a
        # memory leak with a URL as its trigger - anyone hitting /a, /b, /c...
        # would grow it forever.
        if len(_req_by_path) < 12:
            _req_by_path[path] = [1, micros, micros]
    else:
        row[0] += 1
        row[1] += micros
        if micros > row[2]:
            row[2] = micros

    _req_recent.append({"path": path, "status": status,
                        "micros": micros, "size": size})
    if len(_req_recent) > RECENT:
        _req_recent.pop(0)


def server_stats():
    """Web server performance, as plain data."""
    paths = []
    for p, (n, total, mx) in _req_by_path.items():
        paths.append({"path": p, "count": n,
                      "avg_us": total // n if n else 0, "max_us": mx})
    paths.sort(key=lambda r: -r["count"])
    return {
        "requests": _req_count,
        "bytes_served": _req_bytes,
        "total_micros": _req_micros,
        "avg_us": _req_micros // _req_count if _req_count else 0,
        "by_path": paths,
        "recent": list(reversed(_req_recent)),
    }

# The page is one big string constant. MicroPython has no template engine and
# pulling one in would break the "same file runs in both places" rule that
# makes the dev server useful.
#
# Doubled braces are escapes - this goes through .format() for the heading.
PAGE = """<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>ESP32-S3</title>
<style>
  :root {{
    --bg:#0e1116; --panel:#171b22; --line:#262c36; --text:#e6edf3;
    --dim:#8b949e; --accent:#3fb950; --warn:#d29922; --bad:#f85149;
    --blue:#58a6ff;
  }}
  * {{ box-sizing:border-box; }}
  body {{ margin:0; background:var(--bg); color:var(--text);
    font-family:system-ui,-apple-system,sans-serif; line-height:1.5; }}
  .wrap {{ max-width:60rem; margin:0 auto; padding:1.5rem 1rem 3rem; }}
  header {{ display:flex; align-items:baseline; gap:.75rem; flex-wrap:wrap;
    margin-bottom:.25rem; }}
  h1 {{ font-size:1.4rem; margin:0; }}
  .env {{ font-size:.75rem; font-family:ui-monospace,monospace; color:var(--dim);
    border:1px solid var(--line); border-radius:.25rem; padding:.1rem .45rem; }}
  .sub {{ color:var(--dim); font-size:.9rem; margin:0 0 1.25rem; }}
  .grid {{ display:grid; gap:.75rem;
    grid-template-columns:repeat(auto-fit,minmax(13rem,1fr)); }}
  .card {{ background:var(--panel); border:1px solid var(--line);
    border-radius:.5rem; padding:.85rem 1rem; }}
  .card h2 {{ font-size:.7rem; text-transform:uppercase; letter-spacing:.06em;
    color:var(--dim); margin:0 0 .4rem; font-weight:600; }}
  .big {{ font-size:1.6rem; font-family:ui-monospace,monospace; line-height:1.2; }}
  .unit {{ font-size:.8rem; color:var(--dim); }}
  .row {{ display:flex; justify-content:space-between; font-size:.85rem;
    padding:.2rem 0; border-bottom:1px solid var(--line); }}
  .row:last-child {{ border-bottom:none; }}
  .row span:first-child {{ color:var(--dim); }}
  .row span:last-child {{ font-family:ui-monospace,monospace; }}
  .wide {{ grid-column:1/-1; }}
  svg {{ display:block; width:100%; height:auto; }}
  .bar {{ height:.5rem; background:var(--line); border-radius:.25rem;
    overflow:hidden; margin-top:.4rem; }}
  .bar i {{ display:block; height:100%; border-radius:.25rem; transition:width .3s; }}
  .dot {{ display:inline-block; width:.5rem; height:.5rem; border-radius:50%;
    background:var(--accent); margin-right:.35rem; vertical-align:middle; }}
  .stale .dot {{ background:var(--bad); }}
  footer {{ color:var(--dim); font-size:.8rem; margin-top:1.5rem;
    display:flex; gap:1rem; flex-wrap:wrap; }}
  a {{ color:var(--blue); }}
</style>
</head>
<body>
<div class="wrap">
  <header>
    <h1>{heading}</h1>
    <span class="env" id="impl">&hellip;</span>
  </header>
  <p class="sub" id="status"><span class="dot"></span><span id="statustext">connecting&hellip;</span></p>

  <div class="grid">
    <div class="card">
      <h2>Uptime</h2>
      <div class="big" id="uptime">&mdash;</div>
    </div>
    <div class="card">
      <h2>Free RAM (volatile)</h2>
      <div class="big"><span id="memfree">&mdash;</span> <span class="unit">MB</span></div>
      <div class="bar"><i id="membar" style="width:0;background:var(--accent)"></i></div>
    </div>
    <div class="card">
      <h2>Signal</h2>
      <div class="big"><span id="rssi">&mdash;</span> <span class="unit">dBm</span></div>
      <div class="bar"><i id="rssibar" style="width:0"></i></div>
    </div>
    <div class="card">
      <h2>Die temperature</h2>
      <div class="big"><span id="temp">&mdash;</span> <span class="unit">&deg;C</span></div>
    </div>

    <div class="card wide">
      <h2>Free memory &mdash; last 60 samples</h2>
      <svg id="memchart" viewBox="0 0 600 120" preserveAspectRatio="none"></svg>
    </div>
    <div class="card wide">
      <h2>Signal strength &mdash; last 60 samples</h2>
      <svg id="rssichart" viewBox="0 0 600 120" preserveAspectRatio="none"></svg>
    </div>

    <div class="card">
      <h2>Hardware</h2>
      <div class="row"><span>CPU</span><span id="cpu">&mdash;</span></div>
      <div class="row"><span>Flash chip</span><span id="flash">&mdash;</span></div>
      <div class="row"><span>Filesystem</span><span id="fs">&mdash;</span></div>
      <div class="row"><span>RAM in use</span><span id="memalloc">&mdash;</span></div>
    </div>
    <div class="card">
      <h2>Wall clock</h2>
      <div class="big" id="clocktime">&mdash;</div>
      <div class="row" style="margin-top:.5rem"><span>Source</span><span id="clocksrc">&mdash;</span></div>
      <div class="row"><span>Booted at</span><span id="clockboot">&mdash;</span></div>
      <button id="ntpbtn" style="margin-top:.6rem;width:100%;padding:.4rem;
        background:var(--line);color:var(--text);border:1px solid var(--line);
        border-radius:.25rem;cursor:pointer;font-size:.8rem">re-sync NTP</button>
    </div>

    <div class="card">
      <h2>Server performance</h2>
      <div class="big"><span id="srvavg">&mdash;</span> <span class="unit">ms avg</span></div>
      <div class="row" style="margin-top:.5rem"><span>Requests</span><span id="srvreqs">&mdash;</span></div>
      <div class="row"><span>Bytes served</span><span id="srvbytes">&mdash;</span></div>
      <div class="row"><span>Busy time</span><span id="srvbusy">&mdash;</span></div>
      <div id="srvpaths" style="margin-top:.5rem;font-size:.75rem;
        font-family:ui-monospace,monospace;color:var(--dim)"></div>
    </div>

    <div class="card wide">
      <h2>Signal vs. die temperature</h2>
      <svg id="scatter" viewBox="0 0 600 220"></svg>
      <p style="color:var(--dim);font-size:.75rem;margin:.5rem 0 0">
        Each dot is one sample; newer dots are brighter. Both series are already
        in the browser, so this costs the board nothing. A vertical smear means
        signal moves while temperature holds &mdash; the usual, and the sign that
        RSSI drift is the environment, not the board heating up.
      </p>
    </div>

    <div class="card wide">
      <h2>Filesystem</h2>
      <div id="mounts" style="margin-bottom:.6rem"></div>
      <div id="filetree" style="font-family:ui-monospace,monospace;font-size:.78rem"></div>
    </div>

    <div class="card wide">
      <h2>Boot timeline</h2>
      <div id="boottl"></div>
    </div>

    <div class="card wide">
      <h2>CPU benchmark</h2>
      <div style="display:flex;gap:.75rem;align-items:center;flex-wrap:wrap">
        <button id="benchbtn" style="padding:.45rem .9rem;background:var(--line);
          color:var(--text);border:1px solid var(--line);border-radius:.25rem;
          cursor:pointer;font-size:.85rem">run benchmark</button>
        <span id="benchnote" style="color:var(--dim);font-size:.78rem"></span>
      </div>
      <div id="benchout" style="margin-top:.6rem"></div>
    </div>

    <div class="card wide">
      <h2>Memory regions &mdash; ESP-IDF allocator</h2>
      <div id="heaptable" style="font-size:.78rem;font-family:ui-monospace,monospace"></div>
      <p style="color:var(--dim);font-size:.75rem;margin:.6rem 0 0">
        <strong>min free</strong> is the worst moment since boot &mdash; the figure that
        predicts an out-of-memory crash. <strong>contig</strong> is how much of the free
        space sits in one block; low means a big allocation fails despite free bytes.
      </p>
    </div>

    <div class="card">
      <h2>Status LED</h2>
      <div class="big" style="display:flex;align-items:center;gap:.5rem">
        <span id="leddot" style="width:1rem;height:1rem;border-radius:50%;
          background:#333;box-shadow:0 0 .6rem #000;flex:none"></span>
        <span id="ledstate" style="font-size:1.1rem">&mdash;</span>
      </div>
      <div class="row" style="margin-top:.5rem"><span>In state for</span><span id="ledsince">&mdash;</span></div>
      <div class="row"><span>Transitions</span><span id="ledchanges">&mdash;</span></div>
      <div class="row"><span>Requests served</span><span id="ledreqs">&mdash;</span></div>
      <div class="row"><span>GPIO</span><span id="ledpin">&mdash;</span></div>
      <div id="ledhist" style="margin-top:.5rem;font-size:.75rem;
        font-family:ui-monospace,monospace;color:var(--dim)"></div>
    </div>
    <div class="card">
      <h2>Network</h2>
      <div class="row"><span>SSID</span><span id="ssid">&mdash;</span></div>
      <div class="row"><span>Address</span><span id="ip">&mdash;</span></div>
      <div class="row"><span>MAC</span><span id="mac">&mdash;</span></div>
      <div class="row"><span>Quality</span><span id="quality">&mdash;</span></div>
    </div>
  </div>

  <div class="card wide" style="margin-top:.75rem">
    <h2>Data sheet &mdash; queried from the device</h2>
    <p style="color:var(--dim);font-size:.8rem;margin:0 0 .6rem">
      Fixed for the life of the firmware, so this is fetched once per page load
      rather than polled. Everything below was asked of the board, not looked up.
    </p>
    <div id="staticinfo" style="font-size:.78rem"></div>
    <div style="margin-top:.75rem;display:flex;gap:.5rem;flex-wrap:wrap">
      <button class="probe" data-ep="/api/pins" style="padding:.4rem .7rem;
        background:var(--line);color:var(--text);border:1px solid var(--line);
        border-radius:.25rem;cursor:pointer;font-size:.8rem">scan GPIO pins</button>
      <button class="probe" data-ep="/api/gc" style="padding:.4rem .7rem;
        background:var(--line);color:var(--text);border:1px solid var(--line);
        border-radius:.25rem;cursor:pointer;font-size:.8rem">force GC</button>
    </div>
    <div id="probeout" style="margin-top:.6rem;font-size:.75rem;
      font-family:ui-monospace,monospace;color:var(--dim)"></div>
  </div>

  <footer>
    <span><a href="/api/stats">/api/stats</a></span>
    <span><a href="/api/static">/api/static</a></span>
    <span><a href="/api/deep">/api/deep</a></span>
    <span><a href="/health">/health</a></span>
    <span id="pollnote">polling every 2s &middot; paused when tab hidden</span>
  </footer>
</div>

<script>
// ---------------------------------------------------------------------------
// All the intelligence lives here, not on the board. The device sends a small
// flat JSON object; everything below - history, scaling, drawing, formatting -
// is the browser's problem, which has the cycles to spare.
// ---------------------------------------------------------------------------

var KEEP = 60;                 // ring buffer depth, ~2 minutes at 2s
var hist = {{ mem: [], rssi: [], pairs: [] }};
var PAIRS = 120;   // rssi/temp samples, kept for the scatter
var misses = 0;

function push(key, value) {{
  if (value === null || value === undefined) return;
  var a = hist[key];
  a.push(value);
  if (a.length > KEEP) a.shift();
}}

function fmtUptime(s) {{
  if (s === null) return '--';
  var d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600);
  var m = Math.floor(s % 3600 / 60), sec = s % 60;
  if (d) return d + 'd ' + h + 'h';
  if (h) return h + 'h ' + m + 'm';
  if (m) return m + 'm ' + sec + 's';
  return sec + 's';
}}

function fmtBytes(b) {{
  if (b === null || b === undefined) return 'n/a';
  if (b >= 1048576) return (b / 1048576).toFixed(1) + ' MB';
  if (b >= 1024) return (b / 1024).toFixed(0) + ' KB';
  return b + ' B';
}}

// Draw a filled sparkline. Scales to the data's own range rather than zero,
// because the interesting part of both these series is the wobble, not the
// distance from the origin.
function spark(id, data, colour, lo, hi) {{
  var svg = document.getElementById(id);
  if (!data.length) {{ svg.innerHTML = ''; return; }}
  var W = 600, H = 120, pad = 4;
  var min = (lo !== undefined) ? lo : Math.min.apply(null, data);
  var max = (hi !== undefined) ? hi : Math.max.apply(null, data);
  if (max === min) {{ max = min + 1; }}
  var span = max - min;
  var step = data.length > 1 ? W / (data.length - 1) : W;

  var pts = data.map(function (v, i) {{
    var x = i * step;
    var y = pad + (H - 2 * pad) * (1 - (v - min) / span);
    return x.toFixed(1) + ',' + y.toFixed(1);
  }});

  var line = pts.join(' ');
  var area = '0,' + H + ' ' + line + ' ' + W + ',' + H;

  svg.innerHTML =
    '<polygon points="' + area + '" fill="' + colour + '" opacity="0.14"/>' +
    '<polyline points="' + line + '" fill="none" stroke="' + colour +
    '" stroke-width="2" stroke-linejoin="round"/>' +
    '<text x="6" y="14" fill="#8b949e" font-size="11" ' +
    'font-family="ui-monospace,monospace">' + fmtLabel(id, max) + '</text>' +
    '<text x="6" y="' + (H - 5) + '" fill="#8b949e" font-size="11" ' +
    'font-family="ui-monospace,monospace">' + fmtLabel(id, min) + '</text>';
}}

function fmtLabel(id, v) {{
  return id === 'memchart' ? fmtBytes(v) : Math.round(v) + ' dBm';
}}

function rssiColour(q) {{
  if (q === 'excellent' || q === 'good') return '#3fb950';
  if (q === 'marginal') return '#d29922';
  if (q === 'unreliable') return '#f85149';
  return '#8b949e';
}}

function set(id, text) {{ document.getElementById(id).textContent = text; }}

// The LED panel. Worth being clear about what this is: the board cannot read
// its own WS2812 - the protocol has no return path - so these figures are
// led.py's record of what it last *set*. Since led.py is also the thing that
// decides, that is the authoritative answer to "what state is the board in",
// which is the question actually worth asking.
function renderLed(led, uptime) {{
  var dot = document.getElementById('leddot');
  if (!led) {{
    set('ledstate', 'n/a');
    dot.style.background = '#333';
    dot.style.boxShadow = 'none';
    return;
  }}

  set('ledstate', led.state);
  set('ledsince', fmtUptime(Math.max(0, uptime - led.since)));
  set('ledchanges', led.changes);
  set('ledreqs', led.requests);
  set('ledpin', led.available ? 'GPIO' + led.pin : 'disabled');

  // Paint the dot in the colour actually written, and glow it to read as a
  // light rather than a swatch.
  dot.style.background = led.colour;
  dot.style.boxShadow = led.available ? '0 0 .7rem ' + led.colour : 'none';

  // Transitions, newest first - the sequence that tells you what happened
  // while you were not watching the board itself.
  var h = led.history.slice().reverse().map(function (e) {{
    return e.state + '<span style="opacity:.6"> @ ' + fmtUptime(e.at) + '</span>';
  }});
  document.getElementById('ledhist').innerHTML =
    h.length ? h.join('<br>') : '';
}}

function render(d) {{
  set('impl', d.implementation);
  set('uptime', fmtUptime(d.uptime));
  set('cpu', d.cpu_mhz ? d.cpu_mhz + ' MHz' : 'n/a');
  set('flash', fmtBytes(d.flash_size));
  // "13.9 MB free of 14.0 MB" reads as broken when the app is 48KB against a
  // 14MB partition - both halves round to the same number. Lead with what is
  // actually used instead; that is the figure that moves.
  if (d.fs_total) {{
    var used = d.fs_total - d.fs_free;
    set('fs', fmtBytes(used) + ' used of ' + fmtBytes(d.fs_total) +
              ' (' + (100 * used / d.fs_total).toFixed(1) + '%)');
  }} else {{
    set('fs', 'n/a');
  }}
  set('memalloc', fmtBytes(d.mem_alloc));
  set('ssid', d.ssid || 'n/a');
  set('ip', d.ip || 'n/a');
  set('mac', d.mac || 'n/a');
  set('quality', d.quality);
  set('temp', d.temperature === null ? 'n/a' : d.temperature);

  // Memory, shown against total heap so the bar means "how much is left".
  if (d.mem_free !== null) {{
    set('memfree', (d.mem_free / 1048576).toFixed(2));
    var total = d.mem_free + (d.mem_alloc || 0);
    document.getElementById('membar').style.width =
      Math.round(100 * d.mem_free / total) + '%';
  }} else {{
    set('memfree', 'n/a');
  }}

  // RSSI, mapped over -90..-30 - the ends of the useful scale in practice.
  if (d.rssi !== null) {{
    set('rssi', d.rssi);
    var pct = Math.max(0, Math.min(100, (d.rssi + 90) / 60 * 100));
    var bar = document.getElementById('rssibar');
    bar.style.width = pct + '%';
    bar.style.background = rssiColour(d.quality);
  }} else {{
    set('rssi', 'n/a');
  }}

  renderLed(d.led, d.uptime);
  renderClock(d.clock);
  renderServer(d.server);

  // Paired sample for the scatter. Both numbers are already here, so the
  // correlation view costs the board nothing extra.
  if (d.rssi !== null && d.temperature !== null) {{
    hist.pairs.push({{ rssi: d.rssi, temp: d.temperature }});
    if (hist.pairs.length > PAIRS) hist.pairs.shift();
    scatter(hist.pairs);
  }}

  push('mem', d.mem_free);
  push('rssi', d.rssi);
  spark('memchart', hist.mem, '#58a6ff');
  spark('rssichart', hist.rssi, rssiColour(d.quality), -90, -30);
}}

function renderClock(c) {{
  if (!c) {{ set('clocktime', 'n/a'); return; }}
  // An unsynced ESP32 reports the year 2000. Saying so beats displaying a
  // confident, wrong timestamp.
  if (!c.synced) {{
    set('clocktime', 'not synced');
    set('clocksrc', c.last_error ? 'NTP failed: ' + c.last_error : 'no NTP yet');
    set('clockboot', 'unknown without a clock');
    return;
  }}
  set('clocktime', c.iso.replace('T', ' '));
  set('clocksrc', 'NTP, UTC' + (c.utc_offset_hours >= 0 ? '+' : '') +
      c.utc_offset_hours + (c.age_s !== null ? ' · ' + fmtUptime(c.age_s) + ' ago' : ''));
  set('clockboot', c.boot_time ? c.boot_time.replace('T', ' ') : 'n/a');
}}

function renderServer(s) {{
  if (!s) return;
  set('srvavg', (s.avg_us / 1000).toFixed(2));
  set('srvreqs', s.requests.toLocaleString());
  set('srvbytes', fmtBytes(s.bytes_served));
  // What fraction of its life the board has spent answering, as opposed to
  // idling in accept(). The honest measure of how loaded it is.
  set('srvbusy', (s.total_micros / 1000).toFixed(0) + ' ms total');

  var rows = s.by_path.map(function (p) {{
    return p.path + '<span style="opacity:.6"> ×' + p.count + ' · ' +
      (p.avg_us / 1000).toFixed(1) + 'ms avg · ' +
      (p.max_us / 1000).toFixed(1) + 'ms max</span>';
  }});
  document.getElementById('srvpaths').innerHTML = rows.join('<br>');
}}

// Fetched once, not polled - see /api/deep on the board.
function loadDeep() {{
  fetch('/api/deep', {{ cache: 'no-store' }})
    .then(function (r) {{ return r.json(); }})
    .then(function (d) {{ renderHeap(d.idf_heap); }})
    .catch(function () {{}});
}}

function renderHeap(regions) {{
  if (!regions || !regions.length) return;
  var head = '<div style="display:grid;grid-template-columns:3rem 1fr 1fr 1fr 3.5rem;' +
    'gap:.4rem;color:#8b949e;border-bottom:1px solid #262c36;padding-bottom:.3rem">' +
    '<span>kind</span><span>total</span><span>free</span><span>min free</span>' +
    '<span>contig</span></div>';
  var body = regions.map(function (r) {{
    // Colour by how close the worst moment came to exhausting the region.
    var head_pct = r.total ? 100 * r.min_free / r.total : 0;
    var colour = head_pct > 40 ? '#3fb950' : head_pct > 15 ? '#d29922' : '#f85149';
    return '<div style="display:grid;grid-template-columns:3rem 1fr 1fr 1fr 3.5rem;' +
      'gap:.4rem;padding:.25rem 0;border-bottom:1px solid #1d222a">' +
      '<span style="color:#8b949e">' + r.kind + '</span>' +
      '<span>' + fmtBytes(r.total) + '</span>' +
      '<span>' + fmtBytes(r.free) + '</span>' +
      '<span style="color:' + colour + '">' + fmtBytes(r.min_free) + '</span>' +
      '<span>' + r.contiguous_pct + '%</span></div>';
  }}).join('');
  document.getElementById('heaptable').innerHTML = head + body;
}}

// The data sheet. Fetched once per page load.
function loadStatic() {{
  fetch('/api/static', {{ cache: 'no-store' }})
    .then(function (r) {{ return r.json(); }})
    .then(renderStatic)
    .catch(function () {{
      document.getElementById('staticinfo').textContent = 'unavailable';
    }});
}}

function kv(label, value) {{
  if (value === null || value === undefined || value === '') return '';
  return '<div class="row"><span>' + label + '</span><span>' + value + '</span></div>';
}}

function renderStatic(s) {{
  var c = s.chip || {{}}, p = s.python || {{}}, b = s.boot || {{}};
  var html = '<div style="display:grid;gap:1rem;' +
    'grid-template-columns:repeat(auto-fit,minmax(16rem,1fr))">';

  html += '<div>' +
    kv('Chip ID', c.unique_id) +
    kv('Machine', c.machine) +
    kv('CPU', c.freq_hz ? (c.freq_hz / 1e6) + ' MHz' : null) +
    kv('Flash', fmtBytes(s.flash_size)) +
    kv('Boot cause', b.reset_cause_name) +
    '</div>';

  html += '<div>' +
    kv('MicroPython', p.implementation) +
    kv('Build', p.platform) +
    kv('Compiler', p.compiler) +
    kv('libc', p.libc) +
    kv('Byte order', p.byteorder) +
    kv('Max int', p.maxsize ? p.maxsize.toLocaleString() : null) +
    kv('.mpy format', p.mpy_version) +
    kv('Import path', p.path ? p.path.join(' : ') : null) +
    '</div>';

  html += '</div>';

  // The flash map, drawn to scale. This is the visual answer to "where did
  // the other 2MB go" - the firmware partition is simply large.
  if (s.partitions && s.partitions.length) {{
    var total = s.flash_size || 1;
    html += '<h2 style="margin-top:1rem">Flash map</h2><div style="display:flex;' +
      'height:1.6rem;border-radius:.25rem;overflow:hidden;border:1px solid #262c36">';
    var colours = {{ factory: '#58a6ff', vfs: '#3fb950', nvs: '#d29922',
                    phy_init: '#8b949e' }};
    s.partitions.forEach(function (pt) {{
      var w = 100 * pt.size / total;
      html += '<div title="' + pt.name + ' @ 0x' + pt.offset.toString(16) +
        ' (' + fmtBytes(pt.size) + ')" style="width:' + w + '%;background:' +
        (colours[pt.name] || '#444') + ';display:flex;align-items:center;' +
        'justify-content:center;font-size:.65rem;color:#0e1116;overflow:hidden">' +
        (w > 8 ? pt.name : '') + '</div>';
    }});
    html += '</div><div style="font-family:ui-monospace,monospace;font-size:.72rem;' +
      'color:#8b949e;margin-top:.4rem">';
    s.partitions.forEach(function (pt) {{
      html += '0x' + pt.offset.toString(16).padStart(6, '0') + '  ' +
        pt.name.padEnd(10) + ' ' + fmtBytes(pt.size) + '<br>';
    }});
    html += '</div>';
  }}

  // Which optional modules this firmware shipped - more useful than a version
  // string, since it says what you can import before you try.
  if (p.modules) {{
    html += '<h2 style="margin-top:1rem">Modules present (' + p.module_count + ')</h2>' +
      '<div style="display:flex;flex-wrap:wrap;gap:.3rem">';
    p.modules.forEach(function (m) {{
      html += '<span style="background:#1d222a;border:1px solid #262c36;' +
        'border-radius:.2rem;padding:.1rem .4rem;font-size:.7rem;' +
        'font-family:ui-monospace,monospace">' + m + '</span>';
    }});
    html += '</div>';
  }}

  document.getElementById('staticinfo').innerHTML = html;
}}

// On-demand probes. Deliberately a click, not a poll - each does real work.
document.addEventListener('click', function (ev) {{
  var btn = ev.target.closest ? ev.target.closest('.probe') : null;
  if (btn) {{
    var out = document.getElementById('probeout');
    out.textContent = 'running ' + btn.dataset.ep + ' ...';
    fetch(btn.dataset.ep, {{ cache: 'no-store' }})
      .then(function (r) {{ return r.json(); }})
      .then(function (d) {{ out.innerHTML = formatProbe(btn.dataset.ep, d); }})
      .catch(function (e) {{ out.textContent = 'failed: ' + e; }});
  }}
  if (ev.target.id === 'benchbtn') {{
    var bb = ev.target;
    bb.textContent = 'running ...';
    bb.disabled = true;
    fetch('/api/bench', {{ cache: 'no-store' }})
      .then(function (r) {{ return r.json(); }})
      .then(renderBench)
      .catch(function (e) {{
        document.getElementById('benchout').textContent = 'failed: ' + e;
      }})
      .then(function () {{
        bb.textContent = 'run benchmark';
        bb.disabled = false;
      }});
  }}
  if (ev.target.id === 'ntpbtn') {{
    var b = ev.target;
    b.textContent = 'syncing ...';
    fetch('/api/ntp', {{ cache: 'no-store' }})
      .then(function (r) {{ return r.json(); }})
      .then(function (d) {{
        b.textContent = d.ok ? 'synced' : 'failed';
        if (d.clock) renderClock(d.clock);
        setTimeout(function () {{ b.textContent = 're-sync NTP'; }}, 2500);
      }})
      .catch(function () {{ b.textContent = 're-sync NTP'; }});
  }}
}});

function formatProbe(ep, d) {{
  if (ep === '/api/gc') {{
    return 'reclaimed ' + fmtBytes(d.reclaimed) + ' in ' + d.micros +
      ' us &middot; free now ' + fmtBytes(d.after);
  }}
  if (ep === '/api/pins') {{
    // Pins that refuse construction are claimed by flash, PSRAM or USB. That
    // is the useful half: it shows which pins are actually free.
    var free = d.filter(function (r) {{ return r.ok; }});
    var busy = d.filter(function (r) {{ return !r.ok; }});
    var s = free.length + ' of ' + d.length + ' GPIO readable &middot; ' +
      busy.length + ' claimed by flash/PSRAM/USB<br><br>';
    s += free.map(function (r) {{
      return '<span style="color:' + (r.value ? '#3fb950' : '#8b949e') + '">' +
        String(r.pin).padStart(2) + ':' + r.value + '</span>';
    }}).join('  ');
    if (busy.length) {{
      s += '<br><br><span style="opacity:.6">claimed: ' +
        busy.map(function (r) {{ return r.pin; }}).join(', ') + '</span>';
    }}
    return s;
  }}
  return JSON.stringify(d);
}}

// Scatter of RSSI against die temperature. Axes are padded to the data's own
// range: both series move over narrow bands, so a zero-based axis would show a
// single flat clump.
function scatter(pairs) {{
  var svg = document.getElementById('scatter');
  if (!pairs || pairs.length < 2) {{ svg.innerHTML = ''; return; }}
  var W = 600, H = 220, L = 46, B = 30, T = 10, R = 10;

  var xs = pairs.map(function (p) {{ return p.temp; }});
  var ys = pairs.map(function (p) {{ return p.rssi; }});
  var x0 = Math.min.apply(null, xs), x1 = Math.max.apply(null, xs);
  var y0 = Math.min.apply(null, ys), y1 = Math.max.apply(null, ys);
  if (x1 - x0 < 1) {{ x0 -= 0.5; x1 += 0.5; }}
  if (y1 - y0 < 2) {{ y0 -= 1; y1 += 1; }}

  var px = function (v) {{ return L + (W - L - R) * (v - x0) / (x1 - x0); }};
  var py = function (v) {{ return T + (H - T - B) * (1 - (v - y0) / (y1 - y0)); }};

  var g = '<rect x="' + L + '" y="' + T + '" width="' + (W - L - R) +
    '" height="' + (H - T - B) + '" fill="none" stroke="#262c36"/>';

  // Three ticks per axis: enough to read the scale without clutter.
  for (var i = 0; i <= 2; i++) {{
    var tv = x0 + (x1 - x0) * i / 2, yv = y0 + (y1 - y0) * i / 2;
    g += '<text x="' + px(tv).toFixed(0) + '" y="' + (H - 12) +
      '" fill="#8b949e" font-size="10" text-anchor="middle" ' +
      'font-family="ui-monospace,monospace">' + tv.toFixed(1) + '</text>';
    g += '<text x="' + (L - 6) + '" y="' + (py(yv) + 3).toFixed(0) +
      '" fill="#8b949e" font-size="10" text-anchor="end" ' +
      'font-family="ui-monospace,monospace">' + yv.toFixed(0) + '</text>';
  }}

  g += '<text x="' + (W / 2) + '" y="' + (H - 1) + '" fill="#8b949e" ' +
    'font-size="10" text-anchor="middle">die temperature (C)</text>';
  g += '<text x="10" y="' + (H / 2) + '" fill="#8b949e" font-size="10" ' +
    'text-anchor="middle" transform="rotate(-90 10 ' + (H / 2) + ')">RSSI (dBm)</text>';

  // Older samples fade, so direction of travel is visible.
  pairs.forEach(function (p, i) {{
    var age = i / Math.max(1, pairs.length - 1);
    g += '<circle cx="' + px(p.temp).toFixed(1) + '" cy="' + py(p.rssi).toFixed(1) +
      '" r="' + (2 + 2 * age).toFixed(1) + '" fill="#58a6ff" opacity="' +
      (0.18 + 0.72 * age).toFixed(2) + '"/>';
  }});

  // Pearson's r, in the browser. Reported only with enough samples to mean
  // something - a correlation over five points is noise with a number on it.
  if (pairs.length >= 10) {{
    var n = pairs.length, sx = 0, sy = 0, sxy = 0, sxx = 0, syy = 0;
    pairs.forEach(function (p) {{
      sx += p.temp; sy += p.rssi; sxy += p.temp * p.rssi;
      sxx += p.temp * p.temp; syy += p.rssi * p.rssi;
    }});
    var num = n * sxy - sx * sy;
    var den = Math.sqrt((n * sxx - sx * sx) * (n * syy - sy * sy));
    var r = den ? num / den : 0;
    var strength = Math.abs(r) < 0.3 ? 'no meaningful correlation'
      : Math.abs(r) < 0.6 ? 'weak' : 'strong';
    g += '<text x="' + (W - R - 4) + '" y="' + (T + 14) + '" fill="#8b949e" ' +
      'font-size="11" text-anchor="end" font-family="ui-monospace,monospace">r = ' +
      r.toFixed(2) + ' \\u00b7 ' + strength + ' \\u00b7 n=' + n + '</text>';
  }}

  svg.innerHTML = g;
}}

// Boot timeline. Static after startup, so fetched once.
function loadBoot() {{
  fetch('/api/boot', {{ cache: 'no-store' }})
    .then(function (r) {{ return r.json(); }})
    .then(function (b) {{
      if (!b.phases || !b.phases.length) return;
      var total = b.total_ms || 1;
      var colours = ['#58a6ff', '#3fb950', '#d29922', '#a371f7'];
      var bar = '<div style="display:flex;height:1.8rem;border-radius:.25rem;' +
        'overflow:hidden;border:1px solid #262c36">';
      b.phases.forEach(function (p, i) {{
        var w = 100 * p.duration_ms / total;
        bar += '<div title="' + p.name + ': ' + p.duration_ms + ' ms" ' +
          'style="width:' + w + '%;background:' + colours[i % 4] +
          ';display:flex;align-items:center;justify-content:center;' +
          'font-size:.68rem;color:#0e1116;overflow:hidden;white-space:nowrap">' +
          (w > 12 ? p.name : '') + '</div>';
      }});
      bar += '</div><div style="font-family:ui-monospace,monospace;font-size:.75rem;' +
        'margin-top:.5rem">';
      b.phases.forEach(function (p, i) {{
        bar += '<div class="row"><span style="color:' + colours[i % 4] + '">' +
          p.name + '</span><span>' + p.duration_ms + ' ms</span></div>';
      }});
      bar += '<div class="row"><span><strong>reset to serving</strong></span>' +
        '<span><strong>' + total + ' ms</strong></span></div></div>';
      document.getElementById('boottl').innerHTML = bar;
    }})
    .catch(function () {{}});
}}

function renderBench(d) {{
  var out = document.getElementById('benchout');
  var note = document.getElementById('benchnote');

  if (d.status === 'too_hot') {{
    note.textContent = '';
    out.innerHTML = '<span style="color:#f85149">refused: die at ' +
      d.temperature + 'C, limit ' + d.limit + 'C</span>';
    return;
  }}
  if (d.status === 'cached' && !d.tests) {{
    note.textContent = 'cooling down \\u00b7 retry in ' + d.retry_in_s + 's';
    return;
  }}

  note.textContent = d.status === 'cached'
    ? 'cached result from ' + d.cached_age_s + 's ago \\u00b7 retry in ' + d.retry_in_s + 's'
    : 'run ' + d.runs + ' \\u00b7 ' + d.total_ms + ' ms of work';

  var rows = d.tests.map(function (t) {{
    var ops = t.ops_per_s > 1000000
      ? (t.ops_per_s / 1000000).toFixed(2) + 'M'
      : (t.ops_per_s / 1000).toFixed(0) + 'k';
    return '<div class="row"><span>' + t.name + '</span><span>' + ops +
      ' ops/s <span style="opacity:.55">\\u00b7 ' + (t.us / 1000).toFixed(1) +
      ' ms</span></span></div>';
  }}).join('');

  var thermal = '';
  if (d.temp_before !== null && d.temp_after !== null) {{
    var sign = d.temp_delta > 0 ? '+' : '';
    thermal = '<div class="row"><span>Die temperature</span><span>' +
      d.temp_before + 'C \\u2192 ' + d.temp_after + 'C <span style="opacity:.55">(' +
      sign + d.temp_delta + ')</span></span></div>';
  }}

  out.innerHTML = rows + thermal +
    '<div class="row"><span>CPU</span><span>' + d.cpu_mhz + ' MHz</span></div>' +
    '<p style="color:#8b949e;font-size:.72rem;margin:.5rem 0 0">Guards: refuses above ' +
    d.limits.thermal_c + 'C, one real run per ' + d.limits.min_interval_s +
    's (' + d.rejected + ' requests served from cache or refused).</p>';
}}

// Filesystem. Fetched once per page load - files change when you deploy, not
// while you watch.
function loadFiles() {{
  fetch('/api/files', {{ cache: 'no-store' }})
    .then(function (r) {{ return r.json(); }})
    .then(renderFiles)
    .catch(function () {{}});
}}

function renderFiles(d) {{
  // Volumes first - the board's equivalent of "drives". Normally exactly one.
  var m = (d.mounts || []).map(function (v) {{
    var pct = v.used_pct || 0;
    // Colour by fullness, not by absolute free space: 200KB free is fine on a
    // 14MB volume and alarming on a 256KB one.
    var col = pct > 90 ? '#f85149' : pct > 70 ? '#d29922' : '#3fb950';
    return '<div style="margin-bottom:.5rem">' +
      '<div class="row"><span>' + v.path + '  <span style="opacity:.6">' +
      v.type + '</span></span><span>' + fmtBytes(v.used) + ' of ' +
      fmtBytes(v.total) + ' (' + pct + '%)</span></div>' +
      '<div class="bar"><i style="width:' + Math.max(0.5, pct) +
      '%;background:' + col + '"></i></div></div>';
  }}).join('');
  document.getElementById('mounts').innerHTML = m;

  var L = d.listing || {{}};
  var entries = L.entries || [];
  if (!entries.length) {{
    document.getElementById('filetree').textContent = 'empty';
    return;
  }}

  // Largest file, so the size bars have a sensible reference.
  var maxSize = 1;
  entries.forEach(function (e) {{ if (e.size > maxSize) maxSize = e.size; }});

  var rows = entries.map(function (e) {{
    var indent = '&nbsp;&nbsp;'.repeat(e.depth * 2);
    if (e.dir) {{
      return '<div style="padding:.15rem 0"><span style="color:#d29922">' +
        indent + e.name + '/</span></div>';
    }}
    var w = Math.max(1, Math.round(100 * e.size / maxSize));
    return '<div style="display:flex;align-items:center;gap:.5rem;padding:.15rem 0;' +
      'border-bottom:1px solid #1d222a">' +
      '<span style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;' +
      'white-space:nowrap">' + indent + e.name + '</span>' +
      '<span style="width:5rem;height:.4rem;background:#1d222a;border-radius:.2rem;' +
      'overflow:hidden;flex:none"><span style="display:block;height:100%;width:' +
      w + '%;background:#58a6ff"></span></span>' +
      '<span style="width:5rem;text-align:right;flex:none;color:#8b949e">' +
      fmtBytes(e.size) + '</span></div>';
  }}).join('');

  var summary = '<div style="color:#8b949e;margin-top:.5rem">' +
    L.file_count + ' files \\u00b7 ' + L.dir_count + ' directories \\u00b7 ' +
    fmtBytes(L.total_bytes) + ' total' +
    (L.truncated ? ' \\u00b7 <span style="color:#d29922">listing truncated at ' +
      L.limits.max_entries + ' entries</span>' : '') + '</div>';

  document.getElementById('filetree').innerHTML = rows + summary;
}}

function poll() {{
  if (document.hidden) return;          // a hidden tab must not tax the board
  fetch('/api/stats', {{ cache: 'no-store' }})
    .then(function (r) {{ return r.json(); }})
    .then(function (d) {{
      misses = 0;
      document.getElementById('status').classList.remove('stale');
      set('statustext', 'live · ' + hist.mem.length + ' samples');
      render(d);
    }})
    .catch(function () {{
      misses++;
      document.getElementById('status').classList.add('stale');
      set('statustext', 'no response (' + misses + ')');
    }});
}}

poll();
loadStatic();
loadDeep();
loadBoot();
loadFiles();
setInterval(poll, 2000);
// The IDF heap regions move slowly and cost more to gather, so they refresh
// on their own slower cadence rather than riding the 2s poll.
setInterval(function () {{ if (!document.hidden) loadDeep(); }}, 15000);
document.addEventListener('visibilitychange', function () {{
  if (!document.hidden) poll();         // refresh immediately on return
}});
</script>
</body>
</html>
"""


def page_index():
    """The dashboard shell. Numbers arrive later, via /api/stats."""
    if compat.IS_MICROPYTHON:
        heading = "ESP32-S3"
    else:
        heading = "ESP32-S3 (dev server)"
    return PAGE.format(heading=heading)


def route(path):
    """Map a URL path to (status, content_type, body)."""
    if path == "/":
        return 200, "text/html", page_index()

    if path == "/api/stats":
        # The hot path: only what changes, kept small because it is fetched
        # every two seconds for as long as a tab is open.
        d = compat.diagnostics()
        d["server"] = server_stats()
        try:
            import clock

            d["clock"] = clock.status()
        except Exception:  # noqa: BLE001
            d["clock"] = None
        return 200, "application/json", compat.json_dumps(d)

    if path == "/api/static":
        # Fixed for the life of the firmware, so the browser fetches it once
        # per page load rather than 30 times a minute.
        try:
            import sysinfo

            return 200, "application/json", compat.json_dumps(sysinfo.static())
        except Exception as e:  # noqa: BLE001
            return 500, "application/json", compat.json_dumps({"error": str(e)})

    if path == "/api/deep":
        # Everything sysinfo can report that is not in the hot path: per-region
        # IDF heap, full wifi config, filesystem detail.
        try:
            import sysinfo

            return 200, "application/json", compat.json_dumps(sysinfo.dynamic())
        except Exception as e:  # noqa: BLE001
            return 500, "application/json", compat.json_dumps({"error": str(e)})

    if path == "/api/gc":
        # Explicit click only: forcing a collection is real work, and running
        # it on a timer would be measuring the measurement.
        try:
            import sysinfo

            return 200, "application/json", compat.json_dumps(sysinfo.gc_probe())
        except Exception as e:  # noqa: BLE001
            return 500, "application/json", compat.json_dumps({"error": str(e)})

    if path == "/api/pins":
        # A live pin map. Pins that raise on construction are claimed by
        # flash, PSRAM or USB - which is the useful half of the answer.
        try:
            import sysinfo

            return 200, "application/json", compat.json_dumps(sysinfo.pin_scan())
        except Exception as e:  # noqa: BLE001
            return 500, "application/json", compat.json_dumps({"error": str(e)})

    if path == "/api/files":
        # Filesystem listing plus mounted volumes. Bounded inside sysinfo -
        # see MAX_ENTRIES / MAX_DEPTH - so a pathological tree cannot stall
        # the single-threaded server.
        try:
            import sysinfo

            return 200, "application/json", compat.json_dumps(
                {"mounts": sysinfo.mounts(), "listing": sysinfo.listdir()}
            )
        except Exception as e:  # noqa: BLE001
            return 500, "application/json", compat.json_dumps({"error": str(e)})

    if path == "/api/bench":
        # Rate-limited and thermally guarded inside bench.run(); see bench.py.
        # Unauthenticated on a LAN means this is a free CPU-burn primitive for
        # anything that can reach port 80, so the guards are not optional.
        try:
            import bench

            return 200, "application/json", compat.json_dumps(bench.run())
        except Exception as e:  # noqa: BLE001
            return 500, "application/json", compat.json_dumps({"error": str(e)})

    if path == "/api/boot":
        # The boot timeline. Static after startup, so fetched once.
        try:
            import boottime

            return 200, "application/json", compat.json_dumps(boottime.status())
        except Exception as e:  # noqa: BLE001
            return 500, "application/json", compat.json_dumps({"error": str(e)})

    if path == "/api/ntp":
        # Re-sync the clock on demand. Cheap (one UDP round trip) unlike the
        # wifi scan, so it is safe to expose as a button.
        try:
            import clock

            ok = clock.sync()
            return 200, "application/json", compat.json_dumps(
                {"ok": ok, "clock": clock.status()}
            )
        except Exception as e:  # noqa: BLE001
            return 500, "application/json", compat.json_dumps({"error": str(e)})

    if path == "/health":
        # Plain text and unauthenticated, so hello_wifi_py/watch.ps1 works
        # against this board unchanged.
        return 200, "text/plain", "ok rssi={}\n".format(compat.rssi())

    if path == "/favicon.ico":
        return 204, "text/plain", ""

    return 404, "text/plain", "not found\n"
