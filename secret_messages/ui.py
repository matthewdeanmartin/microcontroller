"""The page: HTML, CSS and JS, served from the board.

WHY IT IS SPLIT OUT
Kept separate from app.py so that routing logic stays readable, and so the
eventual move to a public static host is a matter of not serving these three
routes rather than unpicking an f-string. app.py already answers the API in
JSON and already sends CORS headers, so that move is a deployment change.

WHY THE ASSETS ARE SEPARATE ROUTES
/style.css and /app.js are their own routes rather than inlined in the page,
for two reasons. The browser caches them, so a phone reloading the bookmark
fetches only the small HTML shell - which matters on a board that serves one
request at a time. And it mirrors the layout a static host would want.

The page is a single-page app talking to the JSON API. That keeps the board
doing the one thing it is good at - answering small requests - and leaves
rendering to the device with the fast processor.
"""

PAGE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="dark light">
<title>Secret Messages</title>
<link rel="stylesheet" href="/style.css">
</head>
<body>

<main id="app">

  <section id="login-view">
    <div class="card login-card">
      <h1>Secret<span class="dot">.</span>Messages</h1>
      <p class="sub">Sign in to read what was written for you.</p>

      <form id="login-form" autocomplete="off">
        <label for="u">Who are you</label>
        <input id="u" name="username" autocapitalize="none" autocorrect="off"
               spellcheck="false" required>

        <label for="p">Password</label>
        <input id="p" name="password" type="password" required>

        <button type="submit" id="login-btn">Unlock</button>
        <p id="login-error" class="error" hidden></p>
      </form>

      <p class="hint">
        Seeded accounts: <code>alice</code> / <code>wonderland</code>
        and <code>bob</code> / <code>builder</code>.
      </p>
    </div>
  </section>

  <section id="main-view" hidden>
    <header class="bar">
      <div>
        <h1>Secret<span class="dot">.</span>Messages</h1>
        <p class="sub" id="whoami"></p>
      </div>
      <button id="logout-btn" class="ghost">Sign out</button>
    </header>

    <nav class="tabs">
      <button class="tab active" data-tab="read">Messages</button>
      <button class="tab" data-tab="write">Write</button>
      <button class="tab" data-tab="board">Board</button>
    </nav>

    <div id="tab-read">
      <div id="focus-banner" class="banner" hidden>
        <span id="focus-text"></span>
        <button id="show-all" class="linkish">show all</button>
      </div>
      <p id="empty" class="empty" hidden>Nothing here you can read.</p>
      <ul id="messages"></ul>
    </div>

    <div id="tab-write" hidden>
      <form id="post-form" class="card">
        <label for="subject">Subject</label>
        <input id="subject" maxlength="80" required>

        <label for="body">Message</label>
        <textarea id="body" rows="6" maxlength="1000" required></textarea>
        <p class="counter"><span id="count">0</span> / 1000</p>

        <label>Who can read it</label>
        <div class="choices" id="vis-choices">
          <label class="choice">
            <input type="radio" name="vis" value="public" checked>
            <span><strong>Everyone</strong><em>Any signed-in user</em></span>
          </label>
          <label class="choice">
            <input type="radio" name="vis" value="restricted">
            <span><strong>Only chosen people</strong><em>Encrypted to them</em></span>
          </label>
        </div>

        <div id="recipients-wrap" hidden>
          <label>Recipients</label>
          <div id="recipients" class="people"></div>
        </div>

        <label class="choice notify">
          <input type="checkbox" id="notify">
          <span><strong>Tell them on Mastodon</strong>
          <em id="notify-note">Sends a direct message with a link. The
          message itself stays here.</em></span>
        </label>

        <div id="masto-connect" class="connect" hidden>
          <p class="connect-why">
            Sends the notification as you, from your own account. Nothing is
            stored on the board — the sign-in lives in this tab only.
          </p>
          <div class="row">
            <input id="masto-host" placeholder="mastodon.social"
                   autocapitalize="none" autocorrect="off" spellcheck="false">
            <button type="button" id="masto-connect-btn" class="small">Connect</button>
          </div>
        </div>

        <p id="masto-status" class="muted" hidden></p>

        <button type="submit" id="post-btn">Send</button>
        <p id="post-error" class="error" hidden></p>
        <p id="post-ok" class="ok" hidden></p>
      </form>
    </div>

    <div id="tab-board" hidden>
      <div class="card">
        <h2>The board</h2>
        <p class="sub">What the hardware is doing right now.</p>

        <dl class="diag" id="diag"></dl>

        <div class="meter-wrap" id="signal-wrap" hidden>
          <div class="meter"><div class="meter-fill" id="signal-bar"></div></div>
          <p class="muted" id="signal-note"></p>
        </div>

        <p class="muted" id="diag-note"></p>
      </div>
    </div>

    <footer id="stats"></footer>
  </section>

</main>

<script src="/notify.js"></script>
<script src="/app.js"></script>
</body>
</html>
"""

# Hand-written and inlined rather than a framework from a CDN: the board has
# no public address, so a phone on this network may have no route to a CDN at
# all. Everything needed to render the page has to come from the board.
STYLE = """
:root {
  --bg: #0f1117;
  --panel: #171a23;
  --panel-2: #1e2230;
  --line: #2a2f40;
  --text: #e6e8ef;
  --dim: #8b91a6;
  --accent: #7c9cff;
  --accent-soft: #7c9cff22;
  --danger: #ff7b72;
  --ok: #5ddc9a;
  --radius: 14px;
}

/* The board serves one request at a time, so the page is designed to look
   finished with no images and no web fonts - nothing that costs a round trip. */
* { box-sizing: border-box; }

body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font: 15px/1.55 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
  padding: 0 16px;
  padding-block: 24px;
}

#app { max-width: 44rem; margin: 0 auto; }

h1 {
  font-size: 1.35rem;
  margin: 0;
  letter-spacing: -0.02em;
}
.dot { color: var(--accent); }

.sub { color: var(--dim); font-size: .85rem; margin: .25rem 0 0; }

.card {
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: var(--radius);
  padding: 20px;
}

.login-card { margin-top: 12vh; }
.login-card h1 { font-size: 1.6rem; }

label {
  display: block;
  font-size: .8rem;
  color: var(--dim);
  margin: 16px 0 6px;
  font-weight: 600;
  letter-spacing: .02em;
}

input[type=text], input[type=password], input:not([type]), textarea {
  width: 100%;
  padding: 11px 13px;
  background: var(--panel-2);
  border: 1px solid var(--line);
  border-radius: 10px;
  color: var(--text);
  font: inherit;
  transition: border-color .15s;
}
input:focus, textarea:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-soft);
}
textarea { resize: vertical; font: inherit; }

button {
  margin-top: 18px;
  width: 100%;
  padding: 12px;
  background: var(--accent);
  color: #0b1020;
  border: 0;
  border-radius: 10px;
  font: 600 15px/1 system-ui, sans-serif;
  cursor: pointer;
}
button:active { transform: translateY(1px); }
button:disabled { opacity: .55; cursor: default; }

button.ghost {
  width: auto;
  margin: 0;
  background: transparent;
  color: var(--dim);
  border: 1px solid var(--line);
  padding: 8px 14px;
  font-size: .85rem;
}

.hint {
  margin: 20px 0 0;
  padding-top: 16px;
  border-top: 1px solid var(--line);
  color: var(--dim);
  font-size: .8rem;
}
code {
  background: var(--panel-2);
  padding: 1px 5px;
  border-radius: 5px;
  font-family: ui-monospace, monospace;
  font-size: .85em;
}

.error { color: var(--danger); font-size: .85rem; margin: 12px 0 0; }
.ok { color: var(--ok); font-size: .85rem; margin: 12px 0 0; }

.bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 20px;
}

.tabs {
  display: flex;
  gap: 6px;
  margin-bottom: 18px;
  border-bottom: 1px solid var(--line);
}
.tab {
  width: auto;
  margin: 0;
  background: none;
  color: var(--dim);
  border-radius: 0;
  border-bottom: 2px solid transparent;
  padding: 10px 14px;
  font-size: .9rem;
}
.tab.active { color: var(--text); border-bottom-color: var(--accent); }

ul#messages { list-style: none; margin: 0; padding: 0; }

li.msg {
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: var(--radius);
  padding: 16px 18px;
  margin-bottom: 12px;
}
li.msg.restricted { border-left: 3px solid var(--accent); }

.msg-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 8px;
}
.msg-subject { font-weight: 600; }
.msg-from { color: var(--dim); font-size: .8rem; white-space: nowrap; }

/* pre-wrap so a pasted message keeps its line breaks, which is most of the
   point of a pastebin. */
.msg-body {
  margin: 0;
  white-space: pre-wrap;
  word-wrap: break-word;
  font: inherit;
}

.tag {
  display: inline-block;
  font-size: .7rem;
  padding: 2px 8px;
  border-radius: 999px;
  background: var(--accent-soft);
  color: var(--accent);
  margin-top: 10px;
  font-weight: 600;
}

.empty { color: var(--dim); text-align: center; padding: 40px 0; }

.choices { display: grid; gap: 8px; }
.choice {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin: 0;
  padding: 12px;
  background: var(--panel-2);
  border: 1px solid var(--line);
  border-radius: 10px;
  cursor: pointer;
  color: var(--text);
  font-size: .9rem;
  text-transform: none;
  letter-spacing: 0;
}
.choice input { margin-top: 2px; accent-color: var(--accent); }
.choice span { display: flex; flex-direction: column; }
.choice em { color: var(--dim); font-style: normal; font-size: .78rem; }

.people { display: flex; flex-wrap: wrap; gap: 8px; }
.person {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 0;
  padding: 8px 12px;
  background: var(--panel-2);
  border: 1px solid var(--line);
  border-radius: 999px;
  font-size: .85rem;
  cursor: pointer;
  color: var(--text);
  letter-spacing: 0;
  text-transform: none;
}
.person input { accent-color: var(--accent); margin: 0; }
.person .handle {
  color: var(--dim);
  font-size: .75rem;
  font-family: ui-monospace, monospace;
  margin-left: 2px;
}

.counter { color: var(--dim); font-size: .75rem; margin: 6px 0 0; text-align: right; }

.choice.notify { margin-top: 16px; }

.muted { color: var(--dim); font-size: .8rem; margin: 10px 0 0; }

button.small, button.linkish {
  width: auto;
  margin: 0;
  padding: 10px 16px;
  font-size: .85rem;
}
button.linkish {
  background: none;
  color: var(--accent);
  padding: 0;
  font-weight: 600;
}

.connect {
  margin-top: 12px;
  padding: 14px;
  background: var(--panel-2);
  border: 1px solid var(--line);
  border-radius: 10px;
}
.connect-why { margin: 0 0 10px; color: var(--dim); font-size: .8rem; }
.row { display: flex; gap: 8px; }
.row input { flex: 1; }

.banner {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 12px 16px;
  margin-bottom: 14px;
  background: var(--accent-soft);
  border: 1px solid var(--accent);
  border-radius: 10px;
  font-size: .85rem;
}

h2 { font-size: 1.05rem; margin: 0; }

/* Definition list as a two-column grid: label left, value right, aligned
   across rows without a table. */
dl.diag {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 10px 18px;
  margin: 18px 0 0;
  font-size: .9rem;
}
dl.diag dt { color: var(--dim); margin: 0; font-size: .8rem; font-weight: 600; }
dl.diag dd {
  margin: 0;
  font-family: ui-monospace, monospace;
  word-break: break-all;
}

.meter-wrap { margin-top: 20px; }
.meter {
  height: 8px;
  background: var(--panel-2);
  border-radius: 999px;
  overflow: hidden;
}
.meter-fill {
  height: 100%;
  width: 0;
  border-radius: 999px;
  background: var(--ok);
  transition: width .4s ease, background-color .4s ease;
}
.meter-fill.good { background: var(--ok); }
.meter-fill.marginal { background: #e8c15d; }
.meter-fill.unreliable { background: var(--danger); }

footer#stats {
  margin-top: 28px;
  padding-top: 16px;
  border-top: 1px solid var(--line);
  color: var(--dim);
  font-size: .75rem;
  font-family: ui-monospace, monospace;
}

@media (max-width: 420px) {
  .login-card { margin-top: 6vh; }
}
"""

# Plain ES5-ish JavaScript with no build step and no framework. It is served
# by a microcontroller; a bundler would be a strange thing to introduce.
APP_JS = """
// The session token lives in memory only, not localStorage. Closing the tab
// ends the session, which suits a shared house better than a token that
// lingers on a phone someone else might pick up.
var token = null;
var me = null;
var users = [];

// When arriving via a notification link (/?m=7), show just that message until
// the reader asks for the rest. The id is read once at load, because the
// address bar is tidied after sign-in.
var focusId = (function () {
  var m = new URLSearchParams(location.search).get("m");
  return m ? parseInt(m, 10) : null;
})();

function api(path, body) {
  var opts = { method: body ? "POST" : "GET", headers: {} };
  if (body) {
    opts.headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(body);
  }
  if (token) opts.headers["X-Session"] = token;

  return fetch(path, opts).then(function (r) {
    return r.json().then(function (data) {
      if (!r.ok) throw new Error(data.error || "request failed");
      return data;
    });
  });
}

function show(id, on) {
  var el = document.getElementById(id);
  if (el) el.hidden = !on;
}

function esc(s) {
  var d = document.createElement("div");
  d.textContent = s;
  return d.innerHTML;
}

// ---------------------------------------------------------------- login

document.getElementById("login-form").addEventListener("submit", function (e) {
  e.preventDefault();
  var btn = document.getElementById("login-btn");
  var err = document.getElementById("login-error");
  err.hidden = true;

  // The board deliberately takes a moment over the key derivation, so say so
  // rather than letting a tap feel ignored.
  btn.disabled = true;
  btn.textContent = "Unlocking...";

  api("/api/login", {
    username: document.getElementById("u").value.trim(),
    password: document.getElementById("p").value
  }).then(function (data) {
    token = data.token;
    me = data.user;
    users = data.users;

    document.getElementById("p").value = "";
    document.getElementById("whoami").textContent = "Signed in as " + data.display;
    show("login-view", false);
    show("main-view", true);
    buildRecipients();
    refresh();
  }).catch(function (ex) {
    err.textContent = ex.message;
    err.hidden = false;
  }).then(function () {
    btn.disabled = false;
    btn.textContent = "Unlock";
  });
});

document.getElementById("logout-btn").addEventListener("click", function () {
  api("/api/logout", {}).catch(function () {});
  token = null;
  me = null;
  document.getElementById("messages").innerHTML = "";
  show("main-view", false);
  show("login-view", true);
});

// ----------------------------------------------------------------- tabs

var tabs = document.querySelectorAll(".tab");
for (var i = 0; i < tabs.length; i++) {
  tabs[i].addEventListener("click", function () {
    for (var j = 0; j < tabs.length; j++) tabs[j].classList.remove("active");
    this.classList.add("active");
    var which = this.getAttribute("data-tab");
    show("tab-read", which === "read");
    show("tab-write", which === "write");
    show("tab-board", which === "board");

    // Fetched on demand rather than polled: every request costs the board a
    // round trip it cannot overlap with anything else.
    if (which === "board") loadDiagnostics();
  });
}

// -------------------------------------------------------------- reading

document.getElementById("show-all").addEventListener("click", function () {
  focusId = null;
  refresh();
});

function refresh() {
  api("/api/messages").then(function (data) {
    var ul = document.getElementById("messages");
    ul.innerHTML = "";

    var list = data.messages;

    // A notification link names one message. If it is readable, show only it;
    // if it is not - wrong account, or aged out of the ring - say so plainly
    // rather than silently showing everything.
    if (focusId !== null) {
      var only = list.filter(function (m) { return m.id === focusId; });
      show("focus-banner", true);
      document.getElementById("focus-text").textContent = only.length
        ? "Showing the message you were sent."
        : "That message is not available to you.";
      list = only;
    } else {
      show("focus-banner", false);
    }

    show("empty", list.length === 0 && focusId === null);

    list.forEach(function (m) {
      var li = document.createElement("li");
      li.className = "msg" + (m.visibility === "restricted" ? " restricted" : "");

      var who = m.sender === me ? "you" : m.sender;
      var html =
        '<div class="msg-head">' +
          '<span class="msg-subject">' + esc(m.subject) + "</span>" +
          '<span class="msg-from">' + esc(who) + "</span>" +
        "</div>" +
        '<pre class="msg-body">' + esc(m.body) + "</pre>";

      if (m.visibility === "restricted") {
        html += '<span class="tag">only ' + esc(m.recipients.join(", ")) + "</span>";
      }
      li.innerHTML = html;
      ul.appendChild(li);
    });

    var s = data.stats;
    document.getElementById("stats").textContent =
      s.messages + "/" + s.capacity + " messages held in RAM  -  " +
      s.users + " users  -  free heap " + data.free;
  }).catch(function (ex) {
    // A dead session should land the user back at the login form rather than
    // showing a stale inbox they can no longer refresh.
    if (/session/i.test(ex.message)) document.getElementById("logout-btn").click();
  });
}

// ---------------------------------------------------------- diagnostics

function loadDiagnostics() {
  api("/api/diagnostics").then(function (d) {
    var rows = [
      ["Running on", d.implementation],
      ["Uptime", formatUptime(d.uptime)],
      ["Free memory", d.free_memory_text],
      ["Network", d.ssid || "n/a"],
      ["Address", d.ip || "n/a"],
      ["MAC", d.mac || "n/a"],
      ["Signal", d.signal_text]
    ];

    var dl = document.getElementById("diag");
    dl.innerHTML = "";
    rows.forEach(function (row) {
      var dt = document.createElement("dt");
      dt.textContent = row[0];
      var dd = document.createElement("dd");
      dd.textContent = row[1];
      dl.appendChild(dt);
      dl.appendChild(dd);
    });

    // RSSI is negative dBm; map the useful range (-90 weak .. -30 strong)
    // onto a bar, because "-67" means nothing without a scale.
    if (d.rssi !== null && d.rssi !== undefined) {
      show("signal-wrap", true);
      var pct = Math.max(0, Math.min(100, ((d.rssi + 90) / 60) * 100));
      var bar = document.getElementById("signal-bar");
      bar.style.width = pct.toFixed(0) + "%";
      bar.className = "meter-fill " +
        (d.signal_quality === "excellent" ? "good" : d.signal_quality);
      document.getElementById("signal-note").textContent =
        "How well the board hears the router. It says nothing about whether " +
        "its replies get back \\u2014 the weaker direction for a small antenna.";
    } else {
      show("signal-wrap", false);
    }

    document.getElementById("diag-note").textContent = d.on_board
      ? ""
      : "Most of these only mean something on the board. This is the dev server.";
  }).catch(function () {});
}

function formatUptime(seconds) {
  if (seconds < 60) return seconds + "s";
  var m = Math.floor(seconds / 60), h = Math.floor(m / 60), dys = Math.floor(h / 24);
  if (dys > 0) return dys + "d " + (h % 24) + "h";
  if (h > 0) return h + "h " + (m % 60) + "m";
  return m + "m " + (seconds % 60) + "s";
}

// -------------------------------------------------------------- writing

function buildRecipients() {
  var wrap = document.getElementById("recipients");
  wrap.innerHTML = "";
  users.forEach(function (u) {
    if (u.name === me) return;  // you always get a copy; no need to pick yourself
    var l = document.createElement("label");
    l.className = "person";

    // Show the handle a notification would reach. These are real accounts on
    // real instances, so seeing where the DM lands before sending one is
    // worth the extra line.
    var who = esc(u.display);
    if (u.handle) who += '<span class="handle">' + esc(u.handle) + "</span>";

    l.innerHTML = '<input type="checkbox" value="' + esc(u.name) + '">' + who;
    wrap.appendChild(l);
  });
}

var visChoices = document.getElementsByName("vis");
for (var v = 0; v < visChoices.length; v++) {
  visChoices[v].addEventListener("change", function () {
    show("recipients-wrap", this.value === "restricted" && this.checked);
  });
}

var bodyEl = document.getElementById("body");
bodyEl.addEventListener("input", function () {
  document.getElementById("count").textContent = this.value.length;
});

// ------------------------------------------------------------- mastodon

// Prefill the instance box from the signed-in user's own handle. You almost
// always connect as yourself, and the two seeded accounts live on different
// instances - so a fixed default would be wrong half the time.
function suggestHost() {
  var mine = users.filter(function (u) { return u.name === me; })[0];
  if (!mine || !mine.handle) return "";
  var parts = mine.handle.split("@");
  return parts.length === 3 ? parts[2] : "";
}

function refreshMastoUi() {
  var wanted = document.getElementById("notify").checked;
  var connected = MASTO.connected();

  var hostBox = document.getElementById("masto-host");
  if (wanted && !connected && !hostBox.value) {
    hostBox.value = suggestHost();
  }

  // The connect box appears only when the user has asked to notify and is not
  // already signed in - no point showing an OAuth prompt otherwise.
  show("masto-connect", wanted && !connected);

  var status = document.getElementById("masto-status");
  if (wanted && connected) {
    status.textContent = "Will notify via " + MASTO.host() + ".";
    status.hidden = false;
  } else {
    status.hidden = true;
  }
}

document.getElementById("notify").addEventListener("change", refreshMastoUi);

document.getElementById("masto-connect-btn").addEventListener("click", function () {
  // Fall back to the signed-in user's own instance, not a fixed one - the
  // seeded accounts are on different servers.
  var host = document.getElementById("masto-host").value ||
             suggestHost() || "mastodon.social";
  var status = document.getElementById("masto-status");
  status.textContent = "Opening " + host + "...";
  status.hidden = false;

  MASTO.connect(host).catch(function (ex) {
    status.textContent = ex.message;
  });
});

// Finish an OAuth round trip, if this load is one. Runs before sign-in, so a
// returning user sees the result whether or not they are still logged in.
MASTO.completeIfReturning().then(function (message) {
  if (!message) return;
  var status = document.getElementById("masto-status");
  status.textContent = message;
  status.hidden = false;
  // The user was mid-compose when they left; put them back on that tab.
  if (MASTO.connected()) {
    document.getElementById("notify").checked = true;
    refreshMastoUi();
  }
});

document.getElementById("post-form").addEventListener("submit", function (e) {
  e.preventDefault();
  var err = document.getElementById("post-error");
  var ok = document.getElementById("post-ok");
  var btn = document.getElementById("post-btn");
  err.hidden = true;
  ok.hidden = true;

  var vis = document.querySelector('input[name=vis]:checked').value;
  var picked = [];
  var boxes = document.querySelectorAll("#recipients input:checked");
  for (var i = 0; i < boxes.length; i++) picked.push(boxes[i].value);

  if (vis === "restricted" && picked.length === 0) {
    err.textContent = "Choose at least one person, or make it public.";
    err.hidden = false;
    return;
  }

  btn.disabled = true;
  btn.textContent = "Sending...";

  var subjectEl = document.getElementById("subject");
  var subject = subjectEl.value;
  var wantNotify = document.getElementById("notify").checked;

  api("/api/post", {
    subject: subject,
    body: bodyEl.value,
    visibility: vis,
    recipients: picked
  }).then(function (data) {
    subjectEl.value = "";
    bodyEl.value = "";
    document.getElementById("count").textContent = "0";

    ok.textContent = "Sent.";
    ok.hidden = false;
    refresh();

    // The message is already safe on the board. Notifying is a separate,
    // best-effort step - a Mastodon failure must never read as a lost message.
    if (!wantNotify) return;
    if (!MASTO.connected()) {
      ok.textContent = "Sent. Not signed in to Mastodon, so nobody was told.";
      return;
    }

    // data.link is built by the board from the address you reached it on, so
    // it is a link that actually works from here.
    var targets = (vis === "restricted") ? picked : users.map(function (u) { return u.name; });
    targets = targets.filter(function (n) { return n !== me; });

    var sends = targets.map(function (name) {
      var user = users.filter(function (u) { return u.name === name; })[0];
      if (!user || !user.handle) return Promise.resolve(name + " (no handle)");
      return MASTO.notify(user.handle, data.link, subject)
        .then(function () { return null; })
        .catch(function (ex) { return name + " (" + ex.message + ")"; });
    });

    return Promise.all(sends).then(function (results) {
      var failed = results.filter(function (r) { return r !== null; });
      if (failed.length === 0) {
        ok.textContent = "Sent, and told " + targets.length +
          (targets.length === 1 ? " person" : " people") + " on Mastodon.";
      } else {
        ok.textContent = "Sent. Could not notify: " + failed.join(", ");
      }
    });
  }).catch(function (ex) {
    err.textContent = ex.message;
    err.hidden = false;
  }).then(function () {
    btn.disabled = false;
    btn.textContent = "Send";
  });
});
"""
