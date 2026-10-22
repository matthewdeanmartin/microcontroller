# party

Clubs and parties for a household and the people around it: groups,
members, organizers, and (from Sprint 3) events and RSVPs. The canonical data
lives on one ESP32-S3 at `party.local`. People outside the house will reach
it through a static companion site and a Mastodon bot the board polls. The
board itself is never on the internet.

A [miniframework](../miniframework/) app. The design is in [spec/](spec/),
and the work plan is [spec/sprints.md](spec/sprints.md).

| Path | What |
|---|---|
| `api/` | Rust: the API, the record store, desktop and `party-s3` binaries |
| `ui/` | Angular workspace: `local` (served by the board), `companion` (GitHub Pages) and its `demo` build, `party-core` (shared) |
| `tools/` | `build-s3.sh` (compile only), `manifest.py`, `deploy-s3.py` (guarded USB flashing) |
| `spec/` | the specification, roadmap and sprints |

## Desktop

```sh
cd party
make test          # Rust (app + gate) and UI type checks + unit tests
make run           # API on http://127.0.0.1:8090, state in api/.local/state
make web-dev       # in another shell: the local app on http://localhost:4200
make run-bundle    # or: build the UI and serve everything from :8090
make check         # test + fmt + clippy + all three production UI builds
```

The first visitor sets the board up and becomes admin. Everyone else signs up
on the LAN with a name and a PIN (email optional).

## Outside the house

Email, the Mastodon bot and the companion site are optional and set up from
the admin page: see [INSTALL.md](INSTALL.md). The board only makes outbound
connections.

## Board

Read [DEPLOY.md](DEPLOY.md) first. `make firmware` only compiles. Flashing
needs a port and the board's MAC, and refuses any other board.
