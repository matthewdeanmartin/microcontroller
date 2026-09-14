// Executes notify.js against a mock Mastodon instance, in Node.
//
//     node test_notify.mjs
//
// Separate from test_app.py because this is the half that does not run in
// Python: the OAuth client lives in the browser, and the only way to know the
// PKCE maths is right is to run it and have a server verify the challenge the
// way a real instance does.
//
// The Python tests can only check that notify.js *mentions* getRandomValues
// and S256. These check that SHA-256(verifier) actually equals the challenge
// that was sent, that a forged `state` is rejected, and that the direct
// message carries the link but never the message body.
//
// Needs Node 18+ (for built-in fetch and webcrypto). Nothing is installed and
// nothing reaches the network - the mock instance listens on localhost.

import { createServer } from "node:http";
import { webcrypto } from "node:crypto";
import vm from "node:vm";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname } from "node:path";

// Ask Python for the JavaScript, rather than parsing notify_js.py as text.
//
// This matters: NOTIFY_JS is a normal (non-raw) Python string, so a backslash
// written "\\/" in the source becomes "\/" by the time it is served. Reading
// the .py file directly would hand JavaScript the unescaped source form,
// whose regex literals do not parse. Going through Python guarantees this
// test sees exactly the bytes the board sends.
const here = dirname(fileURLToPath(import.meta.url));
const src = execFileSync(
  process.env.PYTHON || "python",
  ["-c", "import notify_js,sys; sys.stdout.write(notify_js.NOTIFY_JS)"],
  { cwd: here, encoding: "utf8" },
);

// --- a mock instance that enforces PKCE the way a real one does -----------

const state = { app: null, issuedCode: null, challenge: null, posted: null };

const server = createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    const send = (code, obj) => {
      res.writeHead(code, { "Content-Type": "application/json" });
      res.end(JSON.stringify(obj));
    };

    if (req.url === "/api/v1/apps") {
      const parsed = JSON.parse(body);
      state.app = parsed;
      return send(200, {
        client_id: "CID", client_secret: "CSECRET",
        redirect_uri: parsed.redirect_uris, scopes: [parsed.scopes],
      });
    }

    if (req.url === "/oauth/token") {
      const f = new URLSearchParams(body);
      // The whole point of PKCE: verify SHA256(verifier) === stored challenge.
      const verifier = f.get("code_verifier");
      const digest = webcrypto.subtle.digest(
        "SHA-256", new TextEncoder().encode(verifier));
      return digest.then((d) => {
        const b64 = Buffer.from(d).toString("base64")
          .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
        if (b64 !== state.challenge) {
          return send(400, { error: "PKCE verification failed" });
        }
        if (f.get("code") !== state.issuedCode) {
          return send(400, { error: "bad code" });
        }
        send(200, { access_token: "TOKEN123" });
      });
    }

    if (req.url === "/api/v1/statuses") {
      const f = new URLSearchParams(body);
      state.posted = {
        status: f.get("status"),
        visibility: f.get("visibility"),
        auth: req.headers.authorization,
      };
      return send(200, { id: "1" });
    }

    send(404, {});
  });
});

await new Promise((r) => server.listen(8901, r));

// --- a minimal browser ----------------------------------------------------

function makeWindow(search) {
  const store = new Map();
  const loc = {
    origin: "http://secrets.local",
    pathname: "/",
    search,
    href: "http://secrets.local/",
  };
  return {
    crypto: webcrypto,
    TextEncoder,
    btoa: (s) => Buffer.from(s, "binary").toString("base64"),
    location: loc,
    sessionStorage: {
      getItem: (k) => (store.has(k) ? store.get(k) : null),
      setItem: (k, v) => store.set(k, String(v)),
      removeItem: (k) => store.delete(k),
    },
    history: { replaceState: () => {} },
    URLSearchParams,
    Promise,
    // Rewrite https://host -> the local mock.
    fetch: (url, opts) =>
      fetch(String(url).replace("https://127.0.0.1:8901", "http://127.0.0.1:8901"), opts),
    console,
    _store: store,
  };
}

function run(ctx) {
  vm.createContext(ctx);
  vm.runInContext(src, ctx);
  return ctx.MASTO;
}

let failures = 0;
function check(label, cond) {
  console.log((cond ? "  ok    " : "  FAIL  ") + label);
  if (!cond) failures++;
}

console.log("\nPKCE flow (executed, not pattern-matched)");

// --- step 1: connect ------------------------------------------------------

const ctx = makeWindow("");
const MASTO = run(ctx);

check("starts disconnected", MASTO.connected() === false);

await MASTO.connect("127.0.0.1:8901").catch(() => {});

check("registered an app", state.app !== null);
check("asked only for write:statuses", state.app.scopes === "write:statuses");
check("registered this page as the redirect",
  state.app.redirect_uris === "http://secrets.local/");

const authorizeUrl = new URL(ctx.location.href);
const sentChallenge = authorizeUrl.searchParams.get("code_challenge");
const sentState = authorizeUrl.searchParams.get("state");

check("redirected to /oauth/authorize",
  authorizeUrl.pathname === "/oauth/authorize");
check("sent a code_challenge", !!sentChallenge);
check("declared S256",
  authorizeUrl.searchParams.get("code_challenge_method") === "S256");

const stashed = JSON.parse(ctx._store.get("sm_oauth"));
check("kept the verifier locally", !!stashed.verifier);
check("the verifier itself never left",
  !ctx.location.href.includes(stashed.verifier));

// Confirm the challenge really is SHA-256(verifier), base64url.
const d = await webcrypto.subtle.digest(
  "SHA-256", new TextEncoder().encode(stashed.verifier));
const expect = Buffer.from(d).toString("base64")
  .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
check("challenge == base64url(sha256(verifier))", sentChallenge === expect);

state.challenge = sentChallenge;
state.issuedCode = "AUTHCODE";

// --- step 2: the instance redirects back ---------------------------------

const ctx2 = makeWindow("?code=AUTHCODE&state=" + encodeURIComponent(sentState));
ctx2._store.set("sm_oauth", JSON.stringify(stashed));
const MASTO2 = run(ctx2);

const msg = await MASTO2.completeIfReturning();
check("exchange succeeded: " + msg, /Connected to/.test(msg || ""));
check("now connected", MASTO2.connected() === true);
check("token stored in sessionStorage",
  ctx2._store.get("sm_masto_token") === "TOKEN123");

// --- step 3: send the notification ---------------------------------------

await MASTO2.notify("@bob@example.social", "http://secrets.local/?m=7", "Bins");

check("sent a status", state.posted !== null);
check("as a DIRECT message", state.posted.visibility === "direct");
check("mentioned the recipient",
  state.posted.status.startsWith("@bob@example.social"));
check("carried the link", state.posted.status.includes("http://secrets.local/?m=7"));
check("carried the subject", state.posted.status.includes("Bins"));
check("did NOT carry the message body",
  !state.posted.status.includes("tomorrow"));
check("authorised with the token",
  state.posted.auth === "Bearer TOKEN123");

// --- step 4: a forged callback must fail ---------------------------------

const ctx3 = makeWindow("?code=AUTHCODE&state=WRONGSTATE");
ctx3._store.set("sm_oauth", JSON.stringify(stashed));
const MASTO3 = run(ctx3);
const bad = await MASTO3.completeIfReturning();
check("a mismatched state is rejected: " + bad, /security check/.test(bad));
check("and no token is kept", MASTO3.connected() === false);

// --- step 5: a callback with no flow in this browser ---------------------

const ctx4 = makeWindow("?code=AUTHCODE&state=" + encodeURIComponent(sentState));
const MASTO4 = run(ctx4);
const orphan = await MASTO4.completeIfReturning();
check("an unsolicited callback is refused: " + orphan,
  /did not start here/.test(orphan));

// --- step 6: a normal page load does nothing -----------------------------

const ctx5 = makeWindow("?m=7");
const MASTO5 = run(ctx5);
const none = await MASTO5.completeIfReturning();
check("a plain load is not an OAuth callback", none === null);

server.close();
console.log("\n" + (failures ? failures + " FAILED" : "all PKCE checks passed"));
process.exit(failures ? 1 : 0);
