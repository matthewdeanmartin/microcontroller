# miniframework

Small web apps on ESP32 boards: a Rust server that serves an Angular site
and an HTTP/HTTPS API from the board itself. Extracted from
[NanaCoin](../../nanacoin/nanacoin_rs) (the connection loop, HTTPS, static
files) and [mastomini](../../mastomini/mastomini_rs) (the household CA,
transport tuning), so a new app starts from a framework rather than a copy
of either.

The first app is **housemetrics**: boards around the house push readings
(or get scraped), the S2 stores them in RAM, and the browser graphs them.
It also hosts the experiment that started this: **is JSON a bad choice on
a board, or is serialization a rounding error next to network costs?**
See [docs/FORMATS.md](docs/FORMATS.md) for the answer so far.

## What the framework gives an app

- **One connection loop for HTTP, HTTPS and HTTP/2** (each a Cargo
  feature: plain HTTP only for the smallest boards, `tls`, `http2`),
  nonblocking, sized for a 2 MiB board: bounded connections, request buffers and unsent-response
  memory; TLS handshakes on their own task; keep-alive, `TCP_NODELAY` and
  TLS session tickets (a returning browser skips the ~1 s handshake).
- **Five wire formats from one declaration.** `message!` gives a struct,
  streaming encoders and decoders for JSON, MessagePack, CBOR, CBOR with
  integer keys, and protobuf, and a schema the server publishes at
  `/api/v1/schema` (and `/api/v1/schema.proto`). Clients choose with
  `?fmt=` or `Accept`; `?gz=1` adds gzip. Every response carries
  `Server-Timing` (handler, encode and gzip time).
- **Static files** from the firmware (gzip-compressed at build time, ETags,
  immutable caching for hashed names, SPA fallback).
- **HTTPS with the household CA** that mastomini already set up, plus
  `/trust` and `/ca` pages so devices can install it, and an optional
  HTTPS-only mode an app can switch at runtime.
- **Built-ins:** `/api/v1/sys` (chip, memory, Wi-Fi, connections, TLS
  handshake times), `/metrics` (the same as Influx lines, for scraping).
- **Board services:** Wi-Fi with retries, SNTP, mDNS, an NVS key-value
  store, named data partitions (never erased), an HTTP(S) client — and
  desktop equivalents, so the same app runs on a PC for development.
- **Two kinds of board:** `BoardConfig::s2()` (one core, 2 MiB PSRAM) and
  `::s3()` (handshakes on one core, serving on the other). A failed startup
  blinks its step and explains itself on port 8080.
- **Deploy safety and probes** (`tools/boardsafe`): an update writes only
  the app slot after checking the image marker, chip, MAC and exact
  partition table; a probe proves over strict TLS which build a board runs.
- **Browser side** (`miniframework-ng`): decoders for every format driven by
  the server's schema, and a fetch client that reports bytes, encode time,
  TTFB, connection setup and decode time for every call.

## Layout

```
crates/miniframework/     the crate (path or git dependency; not published)
apps/housemetrics/        reference app: metrics store + dashboard, desktop + ESP32-S2
web/                      Angular workspace: miniframework-ng library + housemetrics UI
tools/                    certs.sh, bundle-web.mjs, build-firmware.sh, flash.sh (first install)
tools/boardsafe/          uv: image checks, app-only updates, live probes (Python)
spec/                     designs not built yet (OTA.md)
bench/                    uv: fmtbench (connection modes x formats), writer, ui-smoke
docs/                     RECIPES.md, FORMATS.md
```

## Quick start (desktop)

```sh
cd miniframework
make test              # Rust + TypeScript tests
make run-bundle        # builds the UI, serves UI + API on http://127.0.0.1:8080
# in another shell: pretend to be some sensors (admin password works as a token on desktop)
cd bench && uv run writer --url http://127.0.0.1:8080 --token admin --backfill 1d --live
```

Open http://127.0.0.1:8080: Dashboard, System, Format lab, Devices.
For UI work use `make run` + `make web-dev` (http://localhost:4200, hot reload).

## On the board

`make firmware` builds an ESP32-S2 Mini image (Wi-Fi settings from
`apps/housemetrics/.env` or the file named by `MINIFRAMEWORK_ENV`). Read
[apps/housemetrics/DEPLOY_S2.md](apps/housemetrics/DEPLOY_S2.md) before flashing.

## Using the crate from another app

```toml
[dependencies]
miniframework = { path = "../miniframework/crates/miniframework" }
# or: miniframework = { git = "https://github.com/...", branch = "main" }
```

Then follow [docs/RECIPES.md](docs/RECIPES.md). Bots: start with
[AGENTS.md](AGENTS.md).

## Status

The October 3 observability update is prepared and tested on desktop; hardware
deployment is pending. See the deployment runbook for board identity and access.

## Household scrape configuration

Edit `apps/housemetrics/config/scrapes.json` to define the household's scrape
destinations. The default file contains HTTP `/metrics` targets for
`mastomini.local`, `nanacoin.local`, `mastomini-bots.local`, and
`minicloud.local`, every 30 seconds. Minicloud lives in the sibling
`mastomini/minicloud_rs` project. These URLs read machine health only.

`make firmware` validates and embeds the file; it is applied before the scrape
worker starts. To select a different non-secret file, set
`HOUSEMETRICS_CONFIG` to its absolute path at build time. Invalid JSON,
duplicate names/URLs, credentials in URLs, more than 16 entries, and intervals
outside 5–3600 seconds stop the build. An empty `targets` list is valid.

Configured targets are reconciled with NVS on boot. Matching URLs reuse
previously entered targets and their IDs; file changes update their intervals
and URLs, and removing file entries removes only those managed targets.
Unrelated manual targets and device tokens survive. UI changes to a managed
target apply until the next boot; change the file for a permanent edit.
Unchanged configuration does not rewrite NVS. Keep admin passwords and bot/API
secrets out of this file; broader configuration reuse is not implemented.

The Dashboard's **Board** selector filters collected series by configured
source name, falling back to host/app tags for existing or pushed data. Series
are grouped by board, and up to eight can be graphed. **System** describes the
collector itself. **Devices** reports scrape errors for peers without data.
The S2 permits 64 series with a smaller raw-history pool: 691,200 bytes at
capacity, below the previous 706,560-byte budget. Retention depends on load.

For an isolated desktop preview, after `make web`:

```sh
cd apps/housemetrics
HOUSEMETRICS_ADDR=127.0.0.1:18089 HOUSEMETRICS_DATA=.local/observability-preview \
  cargo run --features bundled-web
```

The preview uses its development admin password unless configured. Do not run
`ui-smoke` on a live board: it seeds test metrics and creates a device. See
`apps/housemetrics/DEPLOY_S2.md` for hardware deployment.

## Earlier validation

- Desktop: complete and tested (Rust unit and pipeline tests, TypeScript
  decoder tests against Rust-written fixtures, all five ingest formats from
  independent Python encoders, headless browser smoke test).
- ESP32-S2 firmware: builds (1.49 MB of a 3.9 MB app partition, UI
  included). **Not yet run on hardware**; board measurements are pending.
