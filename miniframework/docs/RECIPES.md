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

## Choose transports: HTTP, HTTPS, HTTP/2

Cargo features, so a board carries only the code it uses:

| Features | Serves | For |
|---|---|---|
| none (`default-features = false`) | HTTP | the smallest boards; no TLS server code linked |
| `tls` (in the defaults) | HTTP + HTTPS | boards with room for TLS (each session holds ~25 KiB) |
| `http2` (implies `tls`) | + HTTP/2 | boards where cold page loads hurt |

HTTP/2 is offered by ALPN in the TLS handshake; browsers pick it when they
can and fall back to HTTP/1.1. Its point is handshakes: a browser opens up
to six HTTP/1.1 connections at once and pays a full TLS handshake (about a
second on these boards) for each; over HTTP/2 the whole page shares one.
It does not raise throughput (the link is the limit). Streamed uploads
(`Service::streamed_body`) work over HTTP/2 too, given a `content-length`:
each may run at most 16 KiB ahead of the handler (the window the board
advertises). `Limits::h2_streams` bounds requests per connection.

At runtime, `limits.tls_clients = 0` turns HTTPS off in a build that has it.
Built with `http2`, the desktop also speaks cleartext HTTP/2 to clients with
prior knowledge, which is how it is tested. Measure a page either way:

```sh
cd bench && uv run pageload --url https://board.local --ca ../apps/housemetrics/certs/household-ca.crt
```

Apps expose the choice: `make firmware TRANSPORT=http|https|http2`
(housemetrics, Minicloud), `make firmware BOARD=s3 HTTP2=1` (NanaCoin, which
always keeps HTTPS).

Whatever the transport, the loop accepts a connection only when it has a
slot for it (or an idle one to replace); the rest wait in the listen backlog.
Accepting and then dropping showed up in browsers as failed loads and
reloads, and on HTTPS it wasted a handshake first.

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

### Safety regression tests and board ownership

The portable crate forbids unsafe Rust. The ESP-IDF module is the explicit
exception for C ABI calls. This does not mean
dependencies contain no unsafe code: use their safe ownership APIs wherever
they preserve the required behavior.

The board platform now consumes the HAL temperature token:
`board.platform(peripherals.temp_sensor)`. A failed sensor remains absent
from sysinfo and does not stop the web service. The driver is locked and
released by the HAL. A status light owns its output driver:
`Led::s2_mini(peripherals.pins.gpio15, "housemetrics")?`, or
`Led::new(pin, active_high, phrase)?`. An integer GPIO number cannot prove
ownership. These replace the previous no-argument platform and phrase-only
LED constructor; available consumers have been migrated.

`Reply::fill` initializes its entire slice to zero on each call, using the
existing allocation. It no longer treats allocation capacity/address as proof
of initialization. Every response body setter replaces all previous body and
compression state. Log cursors restart their epoch before u32 exhaustion;
a future cursor returns retained lines from the current epoch.

Hostile tests cover capacity/limit changes, body replacement, huge HTTP
lengths, stable TLS retry pointers/lengths, ticket setup failures, RTC corrupt
metadata and wrap at every byte position, decoder truncations and mutations,
HTTP/2 framing/window violations, and gzip flags/header CRC/body CRC/ISIZE,
expansion limits and trailing members. Browser tests also cover invalid UTF-8,
impossible lengths, invalid CBOR breaks/chunks, trailing values and prototype
keys. Run `make check` for the feature matrix and browser/tool checks.

For a targeted memory check, install nightly Miri and run from the crate:

```sh
cargo +nightly miri test --target x86_64-unknown-linux-gnu --lib fill_initializes_every_byte_across_capacity_and_limit_changes
cargo +nightly miri test --target x86_64-unknown-linux-gnu --lib tls_retry_keeps_pointer_and_length_when_output_grows
```

Both targeted tests passed in this audit. An isolated reproduction of the old
capacity/address-based buffer initialization failed under Miri: bytes beyond
the previous short fill were uninitialized when the next fill exposed them.
The ticket failure tests exercise the Rust fallback with a simulated SDK;
they do not inject failures into the real C implementation.

Remaining unsafe operations, all within `src/esp/`:

| Boundary | Why it remains; enforced contract |
|---|---|
| TLS context/init/handshake/read/write/delete and ALPN | svc 0.52.1 has no asynchronous server negotiation API. Blocking negotiation would defeat bounded concurrent handshakes. A private non-Clone `TlsSession` owns the raw context and drops before TCP; only that owner implements Send, never Sync. SDK init failures, timeouts and failed channel handoffs drop the owner. |
| TLS tickets | IDF 5.5.3 frees its ticket context without clearing the pointer on some initialization failures. `tls_config::optional_init` discards failed output before adding certs/ALPN; success retains the context for firmware lifetime. |
| C log callback/vsnprintf/registration | ESP-IDF supplies a C format and va_list; safe Rust cannot interpret that variadic ABI. The callback uses a local bounded buffer, safe UTF-8 conversion and fallible std::io console output; no second variadic printf is needed. |
| NVS custom partition initialization | svc's custom `take` may erase on NO_FREE_PAGES/NEW_VERSION_FOUND. Non-erasing initialization must succeed first. The pair is serialized and an owner is retained so returned clone drops cannot invalidate the preflight. There is no safe non-erasing custom constructor in the pinned svc. Default NVS still uses `take_with(false)`. |
| Wi-Fi power setting/AP client count | No corresponding safe method was found in the pinned svc; local initialized outputs and SDK error results are bounded by the radio lifecycle. Address/MAC/AP information queries use the owned radio's safe methods. |
| Heap/flash/version/stack queries | SDK-only query APIs: initialized bounded outputs; null selects the default flash/current task where documented; version is SDK-owned static NUL-terminated text. Flash query errors yield zero. |
| Partition iteration | SDK owns iterator/pointers. RAII releases even on early exit; entries are copied before advancing, labels decoded only within their fixed arrays, exhausted iterators replaced with null. |
| Core-dump check/summary/erase | SDK-only configured-partition API. Initialized summary, bounded task/backtrace decoding; erase only after reporting a readable summary, and report erase failures. Unread dumps are retained. |

Board memory: TLS retries retain at most 1024 bytes per TLS connection,
allocated only for TLS sessions (S2: at most three served plus one pending and
the bounded handoff queue). HTTP/2 retains one 12-byte ALPN pointer array per
server task on 32-bit targets so handed-off sessions cannot outlive a stack
array. NVS caches one small map entry/name/Arc per opened custom partition.
RTC recovery allocates one bounded byte vector, removing the former vector of
usize indices. Its 2048-byte retained array uses safely indexed AtomicU32
load/store operations (no read-modify-write instructions), with SeqCst word
ordering and a separate non-retained lock. RTC is RAM, not MMIO. The audited
S2 image places the array at 0x50000000 in a 2048-byte, four-byte-aligned
`.rtc_noinit` section. Its atomic shims use `l32i.n` and `s32i.n`, with memory
barriers and interrupt protection; the record lock lives in ordinary RAM.
Recheck placement and disassembly when changing chip/toolchain.
Gzip retains the existing bounded growing-output strategy and
approximately 11 KiB decoder state. Verify actual heap numbers on hardware
before deployment; desktop tests/Miri cannot execute ESP-IDF FFI or prove the
hardware memory model, SDK thread safety or real allocation-failure behavior.
