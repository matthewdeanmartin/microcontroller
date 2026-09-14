# Publishing The Front End

The plan: serve the HTML, CSS and JavaScript from a public website, and leave
only the JSON API on the board. The pretty part then gets edited and deployed
like any other static site, instead of being shipped over USB.

The code is already arranged for it. This page covers what is in place, and
the one browser rule that makes the whole idea harder than it looks.

## What is already done

**Static files are separate routes.** `/`, `/style.css` and `/app.js` are
three distinct routes rather than one big string. Stop serving them and the
board is an API.

**The API is JSON-only.** No route mixes HTML and data. Everything under
`/api/` takes and returns JSON, with the session in an `X-Session` header
rather than a cookie — cookies would drag in same-origin rules and a
`SameSite` argument that a cross-origin setup does not need.

**CORS headers go out on every response**, from `compat.format_response`:

```text
Access-Control-Allow-Origin: *
Access-Control-Allow-Headers: Content-Type, X-Session
Access-Control-Allow-Methods: GET, POST, OPTIONS
```

**Preflight is answered.** A browser sends `OPTIONS` before a cross-origin
POST that carries a custom header, and `app.route` returns 204 for it. This is
the piece most often missed, because it costs nothing today and everything on
the day of the move.

So the change is: stop serving three routes, and point `app.js` at the board's
address instead of using relative URLs.

## The thing that makes it hard

**A page served over `https://` may not call an API over `http://`.**

Browsers call this mixed content and block it outright. It is not a CORS
problem and no header on the board can permit it — the rule exists precisely
so that a secure page cannot be undermined by an insecure request.

And the board cannot offer HTTPS. A certificate requires a publicly resolvable
DNS name and a way for a certificate authority to verify it. The board has
neither — that is the entire premise of this project.

So the public page must be served over **plain `http://`**, which means:

- No GitHub Pages, Netlify, Cloudflare Pages or similar. They all redirect to
  HTTPS, most with HSTS, which makes the redirect non-negotiable.
- A plain-HTTP host of your own, and browsers are steadily more hostile to
  those — "Not secure" warnings now, HTTPS-First mode soon.

## The options, honestly

**Serve everything from the board.** What it does today. Nothing to configure,
works with the internet down, and the cost is that a CSS tweak needs
`deploy.ps1` — about two seconds. This is the one that keeps working.

**Static files on a plain-HTTP host.** Achievable, and increasingly fragile.
Worth it only if the front end grows past what is comfortable to serve from
flash.

**A reverse proxy on the network.** A Raspberry Pi or an always-on machine
terminating TLS with a real certificate and proxying to the board. This
actually solves it — the browser talks HTTPS to the proxy, the proxy talks
HTTP to the board over the LAN. The cost is another always-on machine, which
rather undercuts the appeal of a five-dollar board.

**A tunnel** (Cloudflare Tunnel, Tailscale Funnel). Gives a real HTTPS name
without a public IP. But it puts the site on the internet, and "you cannot
load this without being on the home network" was a feature — it is most of the
threat model, given that passwords cross the wire in the clear.

## The recommendation

Leave it on the board. The page is about 15KB in total — 3KB of HTML, 6KB of
JavaScript, 5KB of CSS — and the browser caches the CSS and JS after the first
load, so a phone opening the bookmark fetches only the small HTML shell.

That is well within what the board can serve comfortably, and it keeps the
property that makes this thing pleasant: it works when nothing else does.

The groundwork is laid regardless, because it cost nothing to lay. If the
front end ever outgrows the board, the move is a deployment change and not a
rewrite.
