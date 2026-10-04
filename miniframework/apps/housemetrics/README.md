# housemetrics

A household metrics store and dashboard on an ESP32-S2 Mini, and the
reference app for [miniframework](../../README.md).

- Boards push readings as Influx lines (`temp,room=attic value=21.5`) or as
  a `WriteBatch` in any wire format, each with its own revocable token.
- The board can also scrape other boards: miniframework `/metrics`, or any
  JSON endpoint (NanaCoin's `/api/v1/diag`) whose numbers become series.
- It records its own health (heap, RSSI, requests, TLS handshakes) every
  10 s.
- Storage is RAM only: Gorilla-compressed raw points (about 2.7 bytes each
  on real sensor data) in a shared pool that evicts the oldest block, plus
  15-minute min/max/avg rollups for 3 days. Device tokens and scrape
  targets persist in NVS.
- The UI graphs any series over 15 minutes to 7 days (raw points or
  buckets with a min/max band), shows the board's health, manages devices
  and scrape targets, and has the **Format lab**.

[API.md](API.md) · [DEPLOY_S2.md](DEPLOY_S2.md) · [the format findings](../../docs/FORMATS.md)

## Run on a PC

```sh
cd miniframework
make run-bundle        # http://127.0.0.1:8080, admin password "admin" unless HOUSEMETRICS_ADMIN_PASSWORD is set
cd bench && uv run writer --url http://127.0.0.1:8080 --token admin --backfill 1d --live
```

## Code

| File | What |
|---|---|
| `src/messages.rs` | Every request/response message (`message!`) |
| `src/api.rs` | Routes (`Service` impl) |
| `src/views.rs` | Streamed responses: query results (rows/columns), benchmark rows |
| `src/tsdb.rs`, `src/gorilla.rs` | The store and its compression |
| `src/ingest.rs` | Influx lines, `WriteBatch`, JSON flattening |
| `src/auth.rs` | Admin password and device tokens |
| `src/scrape.rs` | Scrape targets |
| `src/lib.rs` | Size profiles (`Profile::s2()`, `Profile::desktop()`) |
| `src/bin/desktop.rs`, `src/bin/esp32.rs` | Wiring for each platform |
