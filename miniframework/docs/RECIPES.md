# Recipes

Each recipe is self-contained. Code is from `apps/housemetrics` unless
it says otherwise.

## Start a new app

1. Copy `apps/housemetrics` to `apps/<name>` and delete what you don't need
   (`tsdb.rs`, `gorilla.rs`, `ingest.rs`, `scrape.rs`, `views.rs` are
   housemetrics-specific). Keep `build.rs`, `sdkconfig.defaults`,
   `partitions.csv`, `src/mdns_bindings.h`, `src/bin/desktop.rs`,
   `src/bin/esp32.rs`.
2. Rename the package in `Cargo.toml`, the binaries, and the
   `HOUSEMETRICS_*` settings in `build.rs` and the bins.
3. `bash tools/certs.sh apps/<name> <name>` issues `<name>.local`'s
   certificate from the household CA.
4. Add an Angular project under `web/projects/<name>` (copy housemetrics'
   `angular.json` entry); import the browser library as `miniframework-ng`.
5. Add `make` targets (copy the housemetrics ones).

An app is three things: a `Service` (your routes), a `Config` (name, host,
limits, bundled assets, CA), and a platform (desktop or board). The two
binaries wire them together in about 50 lines each.

## Add a route

```rust
// messages.rs
message! {
    /// What the board knows about one plant.
    pub struct Plant {
        1 id: u32,
        2 name: String,
        3 moisture: f32,
        4 watered: Option<u64>,
    }
}

// api.rs, inside App::route
("GET", "/api/v1/plants") => reply.wire(req, &PlantList { plants: self.plants() }),
("POST", "/api/v1/plants") => {
    self.admin.check(req)?;                  // 401/403 as ErrorBody
    let new: NewPlant = req.decode()?;       // any format, gzip ok
    reply.wire_status(req, 201, &self.add(new)?);
}
```

- `req.param("name")`, `req.param_num::<u64>("limit")?`, `req.rest("/api/v1/plants")`
  (path segments after a prefix), `req.header("...")`, `req.secure`.
- `reply.wire` negotiates the format, applies `?gz=`, times the encode.
- Return `Err(ApiError::new(409, "duplicate", "..."))` from `route`; the
  `handle` wrapper sends it as an `ErrorBody` in the client's format.
- Add root messages to `Service::schemas()`.

## Add a field to a message

Append it with the next unused number. Old clients ignore it; old
messages decode with the field's default. Never renumber or reuse.

## Stream a large response

For pages of data, write directly from the source instead of building a
`Vec<Message>`. Implement `Encode` and make exactly the `Writer` calls the
declared message would (so clients decode it as that message):

```rust
pub struct PlantsView<'a>(pub &'a [PlantRecord]);

impl Encode for PlantsView<'_> {
    fn encode<W: Writer>(&self, w: &mut W) -> wire::Result<()> {
        w.begin(1)?;                       // PlantList has 1 field
        w.key(1, "plants")?;
        w.list(self.0.len(), false)?;      // false: elements are messages
        for p in self.0 {
            w.begin(2)?;                   // writing 2 of Plant's fields
            w.key(1, "id")?;   w.u64(p.id as u64)?;
            w.key(3, "moisture")?; w.f32(p.moisture)?;
            w.end()?;
        }
        w.end_list()?;
        w.end()
    }
}
```

Rules: `begin(n)` with the number of keys you will write; one value per
key; list lengths known up front (count first if you must — decoding is
cheap compared with holding everything). `list(len, true)` for numeric
lists (protobuf packs them). See `views.rs` for rows vs columns.

## Choose a format (or let the client)

Clients choose; the server supports all five. For your own UI: JSON for
small objects (debuggable, `JSON.parse` is fast), columns + CBOR int keys or
protobuf for big pages. Read [FORMATS.md](FORMATS.md).

## Persist small settings

```rust
let kv: Arc<dyn Kv> = Arc::new(board.kv("myapp")?);   // NVS namespace, ≤15 chars
kv.set("targets", &wire::to_vec(Format::Protobuf, &stored)?)?;
let stored: Stored = kv.get("targets").and_then(|b| wire::decode(Format::Protobuf, &b).ok()).unwrap_or_default();
```

Keys are ≤15 characters (an NVS limit, also enforced on desktop). Write
from the main task only: flash writes and PSRAM-stack tasks don't mix.

## Call another board

```rust
let fetch = board.fetch();               // esp-idf client; https trusts the household CA
let reply = fetch.get("http://nanacoin-s2.local/api/v1/diag", "application/json", 32 * 1024)?;
```

Desktop: `miniframework::fetch::PlainFetch` (http only). Run fetches on a
worker (`esp::spawn_worker(c"name", 16 * 1024, true, ...)`), never on the
serving task: they block for up to the timeout.

## Size things for the board

The S2 has 2 MiB of PSRAM shared by: Wi-Fi/lwIP buffers, TLS sessions
(~25 KiB each: a 16 KiB receive record), one request buffer per connection
(4 KiB headers + body limit), the response scratch buffer
(`response_limit`), unsent responses (`response_budget`), a gzip
compressor while one runs (~300 KiB), and your data. Check
`/api/v1/sys` → `heap.psram_min` after exercising the app; keep at least
~200 KiB of headroom.

## Run on an S3 (two cores) or an S2 (one)

Start from a profile and change what differs:

```rust
esp::init(); // first: patches, uptime, the served log
let peripherals = Peripherals::take()?;
// your own light or sensors may use peripherals.pins here
let mut config = BoardConfig::s3(SSID, PASSWORD, "myapp"); // or ::s2(...)
config.cert_pem = concat!(include_str!("../../certs/myapp.crt"), "\0").as_bytes();
config.key_pem = concat!(include_str!("../../certs/myapp.key"), "\0").as_bytes();
let board = esp::start(config, peripherals.modem)?;
let data = board.partition("mydata")?; // your own NVS partition, never erased
// ...
board.serve(site, |site| { /* runs on this task: may write flash */ })
```

`s3()` puts TLS handshakes on core 0 and the connection loop on its own task
on core 1, and runs `tick` on the calling (main) task; `s2()` runs both the
loop and `tick` on the calling task. Startup steps 1–7 are the framework's;
number your own from `status::FIRST_APP_STEP` with `SIGNALS.step(8, "...")`.
A failed startup (`esp::fail`) blinks the step and explains itself as text
on `http://<board>:8080/`.

The framework never erases NVS. A partition that is full or was written by
a newer ESP-IDF is reported as a startup error; erasing it is a deliberate
USB job.

## Port an app whose handlers write into a byte slice

```rust
fn handle(&self, req: &Request<'_>, reply: &mut Reply<'_>) {
    reply.fill("application/json", |out| my_api(req.method, req.uri, req.body, out));
    if is_public(req) {
        reply.revalidate(req, "public, no-cache"); // content ETag + 304
    }
    reply.header("Cache-Control", "private"); // replaces the site's default
}
```

`fill` hands over the whole response buffer without zeroing or copying it.
NanaCoin (`nanacoin_rs/src/server.rs`) is the worked example, including
`Config` CORS lists for its own headers and `Spa::Routes` so unknown paths
are 404s instead of the app shell.

## Require HTTPS

Return true from `Service::https_required` (it is asked per request, so a
setting can flip it at runtime). Plain HTTP then serves only `/trust`, `/ca`,
`/` as the trust page, and `/metrics`; everything else is 403.

## Keep your own incident history

```rust
miniframework::events::observe(|event| my_log.record(now(), event));
```

The observer sees every transport event (TLS failures, rejections,
timeouts, slow requests, Wi-Fi drops) and a heartbeat per loop turn, on the
reporting task: append to a fixed ring, never block.

## Update a board, and prove what it runs

`tools/boardsafe` (uv) checks before writing and probes after. Describe each
physical board once in the app's `boards.py` (see `apps/housemetrics`), and
embed its marker in the firmware:

```rust
#[used]
static BOARD_MARKER: &str = "MYAPP-BOARD:s2:myapp.local;";
```

```sh
make image-check                 # marker, chip, size against the smallest app slot
make update PORT=COMn            # chip, MAC, exact partition table, then the app slot only
make probe ADDRESS=192.168.1.x   # strict TLS, /api/v1/sys identity, every asset, /ca
```

`make flash` is the first install only (it writes the bootloader and table).

## Send metrics to housemetrics from another board

Influx line protocol needs no library:

```python
# MicroPython
import urequests
urequests.post("http://housemetrics.local/api/v1/write",
               data="temp,room=attic value=%.1f" % reading,
               headers={"Authorization": "Bearer hm_..."}).close()
```

```go
// TinyGo / Go
body := fmt.Sprintf("temp,room=attic value=%.1f", reading)
req, _ := http.NewRequest("POST", "http://housemetrics.local/api/v1/write", strings.NewReader(body))
req.Header.Set("Authorization", "Bearer hm_...")
```

No timestamp means "now" (the receiving board's clock). Add one in
seconds with `?precision=s`. Batches: many lines in one request. Tokens:
Devices page, or `POST /api/v1/devices`. Binary senders can post a
`WriteBatch` in any format instead (`bench/fmtbench/writer.py` shows all
of them, including protobuf by hand in 15 lines).

## Diagnose a board without a cable

Every miniframework board serves its own log: `GET /api/v1/log.txt` (or
`/api/v1/log?after=<seq>` in any format). It holds `log::` lines and
ESP-IDF's C log lines, a health line every minute, and, after a crash, the
previous boot's last lines from RTC memory. Log what you will need when it
breaks: `log::warn!` for anything surprising, with numbers.

## Test

- Pure logic: ordinary `#[test]`s.
- Routes: build a `Site` with your service and `DesktopPlatform`, feed it
  raw HTTP bytes through `http::parse` + `Site::respond`
  (`crates/miniframework/src/site/tests.rs` has helpers to copy).
- Browser decoding: `make test-web` checks the TypeScript decoders against
  bytes the Rust encoders wrote.
- Whole UI: `make run-bundle`, then `cd bench && uv run ui-smoke --url http://127.0.0.1:8080`.
