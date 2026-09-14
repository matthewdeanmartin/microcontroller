"""The Mastodon notifier - a JavaScript module, served as a static asset.

WHY THIS IS JAVASCRIPT AND NOT PYTHON

The first version of this had the board holding an API token and posting on
the user's behalf. That was wrong twice over.

Wrong in purpose: it posted the message text publicly, while carefully
encrypting the same text in RAM. If a message is worth encrypting it is not
worth tooting. What is actually wanted is a *notification* - a direct message
saying something is waiting, carrying a link that only resolves inside the
house. Mastodon is the transport, not the destination.

Wrong in mechanism: it needed a long-lived token in config.py, which meant a
credential on a device with no disk encryption, no TLS and a public REPL.

Doing it in the browser fixes both. mastodon.social - and every instance -
sends `Access-Control-Allow-Origin: *` on /api/v1/apps and /oauth/token, so
the browser can register a throwaway app and run the whole OAuth flow itself.
The board never sees a token. The PKCE logic here is a plain-JS port of
mawkingbird's ui/src/app/pkce.ts.

WHAT PKCE IS FOR

A browser cannot keep a secret, so an authorization code alone is not enough -
anyone who intercepted it could redeem it. PKCE binds the code to a one-time
`verifier` that never leaves this browser; only its SHA-256 hash travels. The
separate `state` value binds the callback to the flow this browser started.
"""

NOTIFY_JS = """
// Ported from mawkingbird ui/src/app/pkce.ts. Same reasoning, no framework.

var MASTO = {};

// Where the flow parks itself across the redirect out to the instance and
// back. sessionStorage, not localStorage: this is a one-shot secret that
// should not outlive the tab.
var OAUTH_KEY = "sm_oauth";
var TOKEN_KEY = "sm_masto_token";
var HOST_KEY = "sm_masto_host";

// Only the scope needed to send one DM. An instance mints the token against
// the app's registered scopes, so asking for less here genuinely limits what
// this token can ever do.
var SCOPES = "write:statuses";

function bytesToBase64Url(bytes) {
  var binary = "";
  for (var i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary).replace(/\\+/g, "-").replace(/\\//g, "_").replace(/=+$/, "");
}

function randomBase64Url(n) {
  return bytesToBase64Url(crypto.getRandomValues(new Uint8Array(n)));
}

function sha256Base64Url(value) {
  return crypto.subtle
    .digest("SHA-256", new TextEncoder().encode(value))
    .then(function (digest) { return bytesToBase64Url(new Uint8Array(digest)); });
}

// Never Math.random for either of these.
function createCodeVerifier() { return randomBase64Url(64); }
function createOAuthState() { return randomBase64Url(32); }

function statesMatch(expected, received) {
  if (!expected || !received || expected.length !== received.length) return false;
  var diff = 0;
  for (var i = 0; i < expected.length; i++) {
    diff |= expected.charCodeAt(i) ^ received.charCodeAt(i);
  }
  return diff === 0;
}

MASTO.token = function () {
  try { return sessionStorage.getItem(TOKEN_KEY); } catch (e) { return null; }
};

MASTO.host = function () {
  try { return sessionStorage.getItem(HOST_KEY); } catch (e) { return null; }
};

MASTO.connected = function () { return !!MASTO.token(); };

MASTO.disconnect = function () {
  try {
    sessionStorage.removeItem(TOKEN_KEY);
    sessionStorage.removeItem(HOST_KEY);
  } catch (e) {}
};

// The redirect target must be this exact page. The board serves the app at /,
// so strip any query string - a leftover ?m=7 would not match what we
// registered and the instance would refuse the callback.
function redirectUri() {
  return location.origin + location.pathname;
}

/**
 * Register a throwaway app on the instance, then hand off to its authorize
 * page. Returns a promise that never resolves on success, because the tab
 * navigates away.
 */
MASTO.connect = function (host) {
  host = (host || "").trim().replace(/^https?:\\/\\//, "").replace(/\\/$/, "");
  if (!host) return Promise.reject(new Error("Which instance?"));

  var redirect = redirectUri();

  // No client secret is pre-shared and none is needed: the app is registered
  // on the spot, for this browser, and discarded after the exchange.
  return fetch("https://" + host + "/api/v1/apps", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      client_name: "secret messages (home)",
      redirect_uris: redirect,
      scopes: SCOPES
    })
  })
    .then(function (r) {
      if (!r.ok) throw new Error("That instance refused the app registration.");
      return r.json();
    })
    .then(function (app) {
      var state = createOAuthState();
      var verifier = createCodeVerifier();

      return sha256Base64Url(verifier).then(function (challenge) {
        sessionStorage.setItem(OAUTH_KEY, JSON.stringify({
          host: host,
          clientId: app.client_id,
          clientSecret: app.client_secret,
          redirect: redirect,
          state: state,
          verifier: verifier
        }));

        var params = new URLSearchParams({
          client_id: app.client_id,
          redirect_uri: redirect,
          response_type: "code",
          scope: SCOPES,
          state: state,
          code_challenge: challenge,
          code_challenge_method: "S256"
        });

        // The instance handles this in the user's browser, so it works even
        // though secrets.local means nothing to the outside world.
        location.href = "https://" + host + "/oauth/authorize?" + params.toString();
      });
    });
};

/**
 * If this page load is an OAuth callback, finish the exchange.
 *
 * Returns a promise for a short status string, or null when there was no code
 * in the URL - which is every normal page load.
 */
MASTO.completeIfReturning = function () {
  var params = new URLSearchParams(location.search);
  var code = params.get("code");
  var state = params.get("state");
  var denied = params.get("error");

  if (!code && !denied) return Promise.resolve(null);

  var raw = null;
  try {
    raw = sessionStorage.getItem(OAUTH_KEY);
    sessionStorage.removeItem(OAUTH_KEY);
  } catch (e) {}

  // Clean the code out of the address bar either way, so a refresh does not
  // retry a spent code and so the URL is not left carrying it.
  function tidyUrl() {
    var keep = new URLSearchParams(location.search);
    keep.delete("code");
    keep.delete("state");
    keep.delete("error");
    keep.delete("error_description");
    var qs = keep.toString();
    history.replaceState({}, "", location.pathname + (qs ? "?" + qs : ""));
  }

  if (denied) { tidyUrl(); return Promise.resolve("Mastodon sign-in was cancelled."); }
  if (!raw) { tidyUrl(); return Promise.resolve("That sign-in did not start here."); }

  var flow = JSON.parse(raw);

  // The CSRF half of PKCE: proves this callback belongs to the flow this
  // browser started, not one an attacker minted.
  if (!statesMatch(flow.state, state)) {
    tidyUrl();
    return Promise.resolve("Mastodon sign-in failed a security check.");
  }

  var body = new URLSearchParams({
    grant_type: "authorization_code",
    client_id: flow.clientId,
    client_secret: flow.clientSecret,
    redirect_uri: flow.redirect,
    code: code,
    code_verifier: flow.verifier
  });

  return fetch("https://" + flow.host + "/oauth/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: body.toString()
  })
    .then(function (r) {
      if (!r.ok) throw new Error("The instance refused the token exchange.");
      return r.json();
    })
    .then(function (data) {
      sessionStorage.setItem(TOKEN_KEY, data.access_token);
      sessionStorage.setItem(HOST_KEY, flow.host);
      tidyUrl();
      return "Connected to " + flow.host + ".";
    })
    .catch(function (ex) {
      tidyUrl();
      return "Mastodon sign-in failed: " + ex.message;
    });
};

/**
 * Send the notification. A DIRECT message - never a public post.
 *
 * `handle` is the recipient's @user@instance. The mention is what actually
 * routes a direct status, so it has to be in the text itself; Mastodon has no
 * separate recipient field.
 */
MASTO.notify = function (handle, link, subject) {
  var token = MASTO.token();
  var host = MASTO.host();
  if (!token || !host) return Promise.reject(new Error("Not connected to Mastodon."));
  if (!handle) return Promise.reject(new Error("No Mastodon handle for that person."));

  var text =
    handle + " a message is waiting for you on the fridge: " +
    (subject ? '"' + subject + '" ' : "") + link +
    "\\n\\n(only opens from the home wifi)";

  var body = new URLSearchParams({
    status: text,
    visibility: "direct"
  });

  return fetch("https://" + host + "/api/v1/statuses", {
    method: "POST",
    headers: {
      "Authorization": "Bearer " + token,
      "Content-Type": "application/x-www-form-urlencoded"
    },
    body: body.toString()
  }).then(function (r) {
    if (r.status === 401) {
      // The token has been revoked or expired; forget it so the UI offers to
      // reconnect rather than failing the same way forever.
      MASTO.disconnect();
      throw new Error("Mastodon sign-in expired. Connect again.");
    }
    if (!r.ok) throw new Error("Mastodon refused the message (HTTP " + r.status + ").");
    return true;
  });
};
"""
