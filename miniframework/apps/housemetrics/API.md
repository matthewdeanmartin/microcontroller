# housemetrics API

Every response is in the format the client asks for: `?fmt=json|msgpack|cbor|cbor-int|protobuf`
or an `Accept` header (JSON when neither says). `?gz=1` gzips it. Errors are
`ErrorBody { error, message }` in the same format. Message definitions:
`GET /api/v1/schema` (or `/api/v1/schema.proto`); source: `src/messages.rs`.

| Method | Path | Auth | Returns |
|---|---|---|---|
| GET | `/api/v1/sys` | — | `SysInfo`: chip, memory, Wi-Fi, connections, TLS handshake times |
| GET | `/metrics` | — | The board's own numbers as Influx lines (for scraping) |
| GET | `/api/v1/series` | — | `SeriesList`: every series + `StoreStats` |
| GET | `/api/v1/store` | — | `StoreStats` |
| GET | `/api/v1/query` | — | `QueryResult` (below) |
| POST | `/api/v1/write` | device token | `WriteResult` |
| GET | `/api/v1/scrapes` | — | `TargetList` |
| POST | `/api/v1/scrapes` | admin | `Target` (body `NewTarget`) |
| DELETE | `/api/v1/scrapes/{id}` | admin | 204 |
| GET | `/api/v1/devices` | admin | `DeviceList` |
| POST | `/api/v1/devices` | admin | `DeviceToken` (body `NewDevice`); the token is shown once |
| DELETE | `/api/v1/devices/{id}` | admin | 204 |
| DELETE | `/api/v1/series/{id}` | admin | 204 |
| GET | `/api/v1/admin` | admin | 204 if the password is right |
| GET | `/api/v1/bench/rows` | — | `Rows` or `RowColumns`: synthetic data for format benchmarks |
| GET | `/ca`, `/trust` | — | The household CA certificate, and how to install it |

**Auth.** `Authorization: Bearer <secret>`. Device tokens (`hm_...`) can
only write. The admin password (set at build time,
`HOUSEMETRICS_ADMIN_PASSWORD`) can do everything, and on the board is only
accepted over HTTPS.

## Query

`GET /api/v1/query?ids=3,7&from=<ms>&to=<ms>&max=1000&shape=rows|columns`

- `ids`: up to 8 series on the board.
- `from`/`to`: Unix ms; default the last hour up to now.
- `max`: the most points per series the chart wants. If the raw points in
  range fit (and raw data still covers the range), you get `kind: "raw"`;
  otherwise `kind: "buckets"` with min/max/avg/count per bucket of `step`
  ms (a round number: 10 s, 1 min, 15 min...). Old ranges come from 15-minute
  rollups.
- `raw=1`: always raw, paged by `limit` (≤5000 on the board); `next` is the
  `from` of the next page.
- `shape=columns`: `t0` + `dt` (timestamp deltas) + value arrays instead of
  an array of objects. Usually 2–3× smaller in every format.

## Write

`POST /api/v1/write` with either

- **Influx line protocol** (any Content-Type that isn't one of the wire
  formats, e.g. `text/plain`):
  `temp,room=attic value=21.5,humidity=40i 1727900000` with
  `?precision=s|ms|us|ns` (default ns, as InfluxDB). No timestamp: now.
  Each numeric field is its own series; strings are rejected.
- **`WriteBatch { samples: [Sample { m, tags, f, t, v }] }`** in any wire
  format (`Content-Type: application/json`, `application/msgpack`,
  `application/cbor`, `application/x-protobuf`). `tags` is `k=v,k2=v2`,
  `f` defaults to `value`, `t` is Unix ms (0 = now).

`Content-Encoding: gzip` bodies are accepted. Points must be newer than the
series' last point; older ones are rejected (counted in `WriteResult`).
