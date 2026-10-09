# Agent notes: miniframework

Read this before changing anything here. It is the short version of
`README.md` and `docs/RECIPES.md`, written for a coding agent.

## What this is

A Rust + Angular framework for small web apps served from ESP32 boards,
extracted from NanaCoin and mastomini so new apps start here instead of
from a copy of one of them.

| Path | What |
|---|---|
| `crates/miniframework/` | The importable crate: HTTP/HTTPS loop, wire formats, static files, sysinfo, KV store, HTTP client, desktop + ESP-IDF runners |
| `apps/housemetrics/` | Reference app: household metrics store + dashboard (ESP32-S2) |
| `web/projects/miniframework-ng/` | Browser library: decoders for every format, measuring fetch client |
| `web/projects/housemetrics/` | The app's Angular UI |
| `tools/` | Generic scripts: certificates, web bundling, firmware build, first-install flashing |
| `tools/boardsafe/` | uv: image/board checks before a write, app-only updates, live probes |
| `spec/` | Designs not built yet (`OTA.md`) |
| `bench/` | uv project: connection/format benchmark, simulated sensor writer, headless UI smoke test |
| `docs/` | `RECIPES.md` (how to do things), `FORMATS.md` (the serialization findings) |

## Rules

- **Shell:** Git Bash, `make` targets. Python via `uv run` only. Firmware
  builds work from Git Bash (`tools/build-firmware.sh` sets up ESP-IDF).
- **Never flash, erase, or reset a board without asking the user first.**
  There are (at least) two S2 Minis: `80:65:99:f0:7b:68` runs housemetrics,
  `80:65:99:f0:1c:9c` is NanaCoin's second bank. `tools/flash.sh` prints the
  MAC and refuses to overwrite a NanaCoin ledger without
  `--replace-nanacoin`; that flag needs the user's explicit OK.
- **Never create a certificate authority.** HTTPS certificates are signed by
  mastomini's household CA (`tools/certs.sh`); a new CA means every device
  must trust another certificate. Never copy the CA key into this repo.
- **Never commit** unless asked. No co-author or generated-by trailers.
- **Message tags are forever.** In `message!`, the number before each field
  is its protobuf field number and CBOR integer key. Add new fields with new
  numbers; never renumber or reuse one.
- **Stream, don't build.** Large responses implement `Encode` and write
  straight from the data (see `apps/housemetrics/src/views.rs`). Don't
  collect thousands of items into a `Vec` just to encode them: on a 2 MiB
  board, memory held per request is the scarce resource.
- **Respect the board profile.** Sizes live in `Profile::s2()`
  (`apps/housemetrics/src/lib.rs`) and `Limits::small_board()`
  (`crates/miniframework/src/mux.rs`). If you add a buffer, say how big and
  check `/api/v1/sys` heap numbers on the board.
- **Never erase NVS** in framework code (`take_with(false)`, and
  `Board::partition` for app partitions): it may hold someone's data.
- **No app UI in the framework.** Each app owns its Angular client. Shared
  system pages are startup-failure text on port 8080, `/trust`, and the
  explicitly optional `wifi-setup` vanilla-JS provisioning portal. Its
  compiled optional code is a local setup gate, not app authentication.
  See `docs/WIFI_SETUP.md` for integration and the 12 KiB internal worker
  stack; setup workers write NVS and must never use PSRAM stacks.
- **Four apps use this crate**: NanaCoin (`../../nanacoin/nanacoin_rs`,
  features `tls`), Minicloud (`../../mastomini/minicloud_rs`, plain HTTP on
  an ESP32-C6 without PSRAM, streamed blob uploads/downloads), mastomini
  (`../../mastomini/mastomini_rs`: its own router owns every path,
  `Cors::Public` API, runtime Wi-Fi with a setup network, desktop HTTPS)
  and housemetrics. A change to `Site`, `Mux`, `http`, `esp` or `web` must
  keep their tests green: NanaCoin `cargo test` + `python scripts/smoke.py`,
  Minicloud `cargo test` + `uv run --with paho-mqtt==2.1.0 python
  scripts/smoke.py`, mastomini `make test smoke client-test conformance`
  (use a separate `CARGO_TARGET_DIR` if the owner has a dev server
  running).
- **Transports are features**: none (HTTP), `tls`, `http2`. Test all three
  (`make check` does).
- **Done means `make check` passes** (Rust tests with and without `gzip`,
  clippy `-D warnings`, fmt, boardsafe tests, TypeScript decoder tests,
  Angular build). If you touched the UI, also run
  `cd bench && uv run ui-smoke --url http://127.0.0.1:8080` against `make
  run-bundle`.

## Adding an API route (the common task)

1. Declare the response (and request) messages in the app's
   `messages.rs` with `message!`.
2. Add a match arm in the app's `Service::handle` (housemetrics:
   `api.rs`, `App::route`): `("GET", "/api/v1/thing") => reply.wire(req, &thing)`.
   Errors: return `ApiError` and let `handle` call `reply.fail`.
3. List new root messages in `Service::schemas` so browsers can decode
   them in binary formats.
4. Add a TypeScript interface with the same field names
   (`web/projects/housemetrics/src/app/messages.ts`) and call
   `api.get<Thing>('/api/v1/thing', 'Thing')`.
5. Test: a Rust unit test for logic, and the route through
   `Site::respond` if it is subtle (see `crates/miniframework/src/site/tests.rs`).

## Commands

```sh
make test          # Rust + TypeScript tests
make check         # test + clippy + fmt + Angular build
make run           # API on :8080 (desktop)     make web-dev   # UI on :4200
make run-bundle    # UI + API on :8080
make firmware      # board image (no flashing)
```
